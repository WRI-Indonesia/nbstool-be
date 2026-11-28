//! `pickle.loads` for the plain data the app pickles (dicts, lists, tuples, str, int, float,
//! bool, None, bytes), protocols 2-5. serde-pickle 1.2 miscounts memo references when a
//! memoized string is fetched (`BINGET`) and memoized again (`MEMOIZE`), which protocol 5 does for
//! every repeated dict key, and then fails with "recursive structure found". Objects live in an
//! arena so a dict memoized empty and filled later is the one every `BINGET` sees, as in Python.

use std::collections::{BTreeMap, HashMap};

use serde_pickle::{HashableValue as H, Value as P};

enum Node {
    Leaf(P),
    List(Vec<usize>),
    Tuple(Vec<usize>),
    Dict(Vec<(usize, usize)>),
}

enum Slot {
    Obj(usize),
    Mark,
}

struct Vm<'a> {
    data: &'a [u8],
    pos: usize,
    arena: Vec<Node>,
    stack: Vec<Slot>,
    memo: HashMap<u32, usize>,
}

impl<'a> Vm<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or("pickle data was truncated")?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn uint(&mut self, n: usize) -> Result<u64, String> {
        let b = self.take(n)?;
        Ok(b.iter()
            .rev()
            .fold(0u64, |acc, x| (acc << 8) | u64::from(*x)))
    }

    fn push(&mut self, node: Node) {
        self.arena.push(node);
        self.stack.push(Slot::Obj(self.arena.len() - 1));
    }

    fn pop(&mut self) -> Result<usize, String> {
        match self.stack.pop() {
            Some(Slot::Obj(i)) => Ok(i),
            _ => Err("unpickling stack underflow".into()),
        }
    }

    fn top(&self) -> Result<usize, String> {
        match self.stack.last() {
            Some(Slot::Obj(i)) => Ok(*i),
            _ => Err("unpickling stack underflow".into()),
        }
    }

    /// Everything above the topmost MARK, which is removed.
    fn pop_mark(&mut self) -> Result<Vec<usize>, String> {
        let at = self
            .stack
            .iter()
            .rposition(|s| matches!(s, Slot::Mark))
            .ok_or("could not find MARK")?;
        let items = self.stack.split_off(at + 1);
        self.stack.pop();
        Ok(items
            .into_iter()
            .filter_map(|s| if let Slot::Obj(i) = s { Some(i) } else { None })
            .collect())
    }

    fn text(&mut self, n: usize) -> Result<(), String> {
        let b = self.take(n)?;
        let s = std::str::from_utf8(b)
            .map_err(|e| e.to_string())?
            .to_string();
        self.push(Node::Leaf(P::String(s)));
        Ok(())
    }

    fn extend_list(&mut self, list: usize, items: Vec<usize>) -> Result<(), String> {
        match &mut self.arena[list] {
            Node::List(v) => {
                v.extend(items);
                Ok(())
            }
            _ => Err("APPEND to a non-list".into()),
        }
    }

    fn set_items(&mut self, dict: usize, items: Vec<usize>) -> Result<(), String> {
        let Node::Dict(pairs) = &mut self.arena[dict] else {
            return Err("SETITEM on a non-dict".into());
        };
        for kv in items.chunks(2) {
            if let [k, v] = kv {
                pairs.push((*k, *v));
            }
        }
        Ok(())
    }

    fn run(mut self) -> Result<P, String> {
        loop {
            let op = self.take(1)?[0];
            match op {
                0x80 => {
                    self.take(1)?;
                } // PROTO
                0x95 => {
                    self.take(8)?;
                } // FRAME
                b'.' => {
                    let top = self.pop()?;
                    return self.build(top, 0);
                }
                b'(' => self.stack.push(Slot::Mark),
                b'}' => self.push(Node::Dict(Vec::new())),
                b']' => self.push(Node::List(Vec::new())),
                b')' => self.push(Node::Tuple(Vec::new())),
                b'N' => self.push(Node::Leaf(P::None)),
                0x88 => self.push(Node::Leaf(P::Bool(true))),
                0x89 => self.push(Node::Leaf(P::Bool(false))),
                b'J' => {
                    let v = self.uint(4)? as u32 as i32;
                    self.push(Node::Leaf(P::I64(v.into())));
                }
                b'K' => {
                    let v = self.uint(1)?;
                    self.push(Node::Leaf(P::I64(v as i64)));
                }
                b'M' => {
                    let v = self.uint(2)?;
                    self.push(Node::Leaf(P::I64(v as i64)));
                }
                0x8a => {
                    // LONG1: little-endian two's complement
                    let n = self.uint(1)? as usize;
                    let b = self.take(n)?;
                    if n > 8 {
                        return Err("integer out of range".into());
                    }
                    let mut v: i64 = 0;
                    for (i, x) in b.iter().enumerate() {
                        v |= i64::from(*x) << (8 * i);
                    }
                    if n > 0 && n < 8 && b[n - 1] & 0x80 != 0 {
                        v -= 1i64 << (8 * n);
                    }
                    self.push(Node::Leaf(P::I64(v)));
                }
                b'G' => {
                    let b = self.take(8)?;
                    let mut a = [0u8; 8];
                    a.copy_from_slice(b);
                    self.push(Node::Leaf(P::F64(f64::from_be_bytes(a))));
                }
                0x8c => {
                    let n = self.uint(1)? as usize;
                    self.text(n)?;
                }
                b'X' => {
                    let n = self.uint(4)? as usize;
                    self.text(n)?;
                }
                0x8d => {
                    let n = self.uint(8)? as usize;
                    self.text(n)?;
                }
                b'C' | b'B' | 0x8e => {
                    let n = match op {
                        b'C' => self.uint(1)?,
                        b'B' => self.uint(4)?,
                        _ => self.uint(8)?,
                    } as usize;
                    let b = self.take(n)?.to_vec();
                    self.push(Node::Leaf(P::Bytes(b)));
                }
                0x94 => {
                    let top = self.top()?;
                    let id = self.memo.len() as u32;
                    self.memo.insert(id, top);
                }
                b'q' | b'r' => {
                    let id = if op == b'q' {
                        self.uint(1)?
                    } else {
                        self.uint(4)?
                    } as u32;
                    let top = self.top()?;
                    self.memo.insert(id, top);
                }
                b'h' | b'j' => {
                    let id = if op == b'h' {
                        self.uint(1)?
                    } else {
                        self.uint(4)?
                    } as u32;
                    let at = *self
                        .memo
                        .get(&id)
                        .ok_or_else(|| format!("Memo value not found at index {id}"))?;
                    self.stack.push(Slot::Obj(at));
                }
                b'a' => {
                    let item = self.pop()?;
                    let list = self.top()?;
                    self.extend_list(list, vec![item])?;
                }
                b'e' => {
                    let items = self.pop_mark()?;
                    let list = self.top()?;
                    self.extend_list(list, items)?;
                }
                b's' => {
                    let v = self.pop()?;
                    let k = self.pop()?;
                    let dict = self.top()?;
                    self.set_items(dict, vec![k, v])?;
                }
                b'u' => {
                    let items = self.pop_mark()?;
                    let dict = self.top()?;
                    self.set_items(dict, items)?;
                }
                b't' => {
                    let items = self.pop_mark()?;
                    self.push(Node::Tuple(items));
                }
                0x85..=0x87 => {
                    let n = usize::from(op - 0x84);
                    let mut items = Vec::new();
                    for _ in 0..n {
                        items.push(self.pop()?);
                    }
                    items.reverse();
                    self.push(Node::Tuple(items));
                }
                other => return Err(format!("unsupported pickle opcode 0x{other:02x}")),
            }
        }
    }

    fn build(&self, i: usize, depth: usize) -> Result<P, String> {
        // a self-referencing container would recurse forever: Python's json fails on it too
        if depth > 500 {
            return Err("Circular reference detected".into());
        }
        Ok(match &self.arena[i] {
            Node::Leaf(v) => v.clone(),
            Node::List(items) => P::List(
                items
                    .iter()
                    .map(|j| self.build(*j, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Node::Tuple(items) => P::Tuple(
                items
                    .iter()
                    .map(|j| self.build(*j, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Node::Dict(pairs) => {
                let mut map = BTreeMap::new();
                for (k, v) in pairs {
                    let key = match self.build(*k, depth + 1)? {
                        P::None => H::None,
                        P::Bool(b) => H::Bool(b),
                        P::I64(n) => H::I64(n),
                        P::F64(f) => H::F64(f),
                        P::String(s) => H::String(s),
                        P::Bytes(b) => H::Bytes(b),
                        _ => return Err("unhashable type".into()),
                    };
                    map.insert(key, self.build(*v, depth + 1)?);
                }
                P::Dict(map)
            }
        })
    }
}

pub fn loads(data: &[u8]) -> Result<P, String> {
    Vm {
        data,
        pos: 0,
        arena: Vec::new(),
        stack: Vec::new(),
        memo: HashMap::new(),
    }
    .run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_keys_and_shared_dicts() {
        // {"k": 1, "j": <BINGET of the memoized "k">}, and LONG1 -300
        let p = super::loads(&[
            0x80, 0x05, b'}', 0x94, b'(', 0x8c, 1, b'k', 0x94, b'K', 1, 0x8c, 1, b'j', b'h', 1,
            b'u', b'.',
        ]);
        let mut want = std::collections::BTreeMap::new();
        want.insert(
            serde_pickle::HashableValue::String("k".into()),
            serde_pickle::Value::I64(1),
        );
        want.insert(
            serde_pickle::HashableValue::String("j".into()),
            serde_pickle::Value::String("k".into()),
        );
        assert_eq!(p, Ok(serde_pickle::Value::Dict(want)));
        assert_eq!(
            super::loads(&[0x80, 0x05, 0x8a, 2, 0xd4, 0xfe, b'.']),
            Ok(serde_pickle::Value::I64(-300))
        );
    }
}
