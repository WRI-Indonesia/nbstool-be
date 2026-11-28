//! `zlib.compress(data)` byte for byte: a port of zlib 1.3.1's `deflate_slow` (level 6,
//! windowBits 15, memLevel 8, default strategy) and `trees.c`, for the one-block inputs Flask's
//! session cookie compresses. Checked against CPython's zlib on 305 inputs
//! (`rust/scratch-db/zlib_vectors.txt`, test `matches_python_zlib`).
//! shortcut: inputs of 16000 bytes or more return None (they would need zlib's window slide and
//! block splitting); upgrade if a caller compresses more.

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const W_SIZE: usize = 1 << 15;
const MAX_DIST: usize = W_SIZE - MIN_LOOKAHEAD;
const HASH_BITS: u32 = 15;
const HASH_MASK: usize = (1 << HASH_BITS) - 1;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(MIN_MATCH as u32);
const TOO_FAR: usize = 4096;
// level 6
const GOOD_LENGTH: usize = 8;
const MAX_LAZY: usize = 16;
const NICE_LENGTH: usize = 128;
const MAX_CHAIN: usize = 128;

const LITERALS: usize = 256;
const LENGTH_CODES: usize = 29;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const EXTRA_LBITS: [u32; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DBITS: [u32; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [u32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// `tr_static_init`: length/distance code tables.
struct Tables {
    length_code: [u8; 256],
    dist_code: [u8; 512],
    base_length: [u32; LENGTH_CODES],
    base_dist: [u32; D_CODES],
    static_ltree: Vec<Node>,
    static_dtree: Vec<Node>,
}

#[derive(Clone, Copy, Default)]
struct Node {
    freq: u32, // shares storage with `code` in zlib; kept apart, never read after gen_codes
    code: u32,
    len: u32, // shares storage with `dad` in zlib: gen_bitlen overwrites dad with len
    dad: u32,
}

fn bi_reverse(mut code: u32, len: u32) -> u32 {
    let mut res = 0;
    for _ in 0..len {
        res = (res << 1) | (code & 1);
        code >>= 1;
    }
    res
}

fn gen_codes(tree: &mut [Node], max_code: isize, bl_count: &[u32; MAX_BITS + 1]) {
    let mut next_code = [0u32; MAX_BITS + 1];
    let mut code = 0u32;
    for bits in 1..=MAX_BITS {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    for n in 0..=max_code.max(-1) {
        if n < 0 {
            break;
        }
        let n = n as usize;
        let len = tree[n].len;
        if len == 0 {
            continue;
        }
        tree[n].code = bi_reverse(next_code[len as usize], len);
        next_code[len as usize] += 1;
    }
}

impl Tables {
    fn new() -> Tables {
        let mut t = Tables {
            length_code: [0; 256],
            dist_code: [0; 512],
            base_length: [0; LENGTH_CODES],
            base_dist: [0; D_CODES],
            static_ltree: vec![Node::default(); L_CODES + 2],
            static_dtree: vec![Node::default(); D_CODES],
        };
        let mut length = 0usize;
        let mut code = 0usize;
        while code < LENGTH_CODES - 1 {
            t.base_length[code] = length as u32;
            for _ in 0..(1usize << EXTRA_LBITS[code]) {
                t.length_code[length] = code as u8;
                length += 1;
            }
            code += 1;
        }
        t.length_code[length - 1] = code as u8;
        let mut dist = 0usize;
        code = 0;
        while code < 16 {
            t.base_dist[code] = dist as u32;
            for _ in 0..(1usize << EXTRA_DBITS[code]) {
                t.dist_code[dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        dist >>= 7;
        while code < D_CODES {
            t.base_dist[code] = (dist << 7) as u32;
            for _ in 0..(1usize << (EXTRA_DBITS[code] - 7)) {
                t.dist_code[256 + dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        let mut bl_count = [0u32; MAX_BITS + 1];
        for (range, len) in [(0..144, 8), (144..256, 9), (256..280, 7), (280..288, 8)] {
            for n in range {
                t.static_ltree[n].len = len;
                bl_count[len as usize] += 1;
            }
        }
        gen_codes(&mut t.static_ltree, (L_CODES + 1) as isize, &bl_count);
        #[expect(
            clippy::needless_range_loop,
            reason = "zlib trees.c tr_static_init, line for line"
        )]
        for n in 0..D_CODES {
            t.static_dtree[n].len = 5;
            t.static_dtree[n].code = bi_reverse(n as u32, 5);
        }
        t
    }

    fn d_code(&self, dist: usize) -> usize {
        if dist < 256 {
            self.dist_code[dist] as usize
        } else {
            self.dist_code[256 + (dist >> 7)] as usize
        }
    }
}

struct Desc {
    elems: usize,
    extra: &'static [u32],
    extra_base: usize,
    max_length: u32,
    max_code: isize,
}

struct Bits {
    out: Vec<u8>,
    buf: u64,
    n: u32,
}

impl Bits {
    fn send(&mut self, value: u32, length: u32) {
        self.buf |= (value as u64) << self.n;
        self.n += length;
        while self.n >= 8 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.n -= 8;
        }
    }
    fn windup(&mut self) {
        if self.n > 0 {
            self.out.push(self.buf as u8);
        }
        self.buf = 0;
        self.n = 0;
    }
}

struct State {
    heap: [usize; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; HEAP_SIZE],
    bl_count: [u32; MAX_BITS + 1],
    opt_len: u64,
    static_len: u64,
}

fn smaller(tree: &[Node], n: usize, m: usize, depth: &[u8]) -> bool {
    tree[n].freq < tree[m].freq || (tree[n].freq == tree[m].freq && depth[n] <= depth[m])
}

impl State {
    fn pqdownheap(&mut self, tree: &[Node], mut k: usize) {
        let v = self.heap[k];
        let mut j = k << 1;
        while j <= self.heap_len {
            if j < self.heap_len && smaller(tree, self.heap[j + 1], self.heap[j], &self.depth) {
                j += 1;
            }
            if smaller(tree, v, self.heap[j], &self.depth) {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = v;
    }

    fn gen_bitlen(&mut self, tree: &mut [Node], desc: &Desc, stree: Option<&[Node]>) {
        let max_code = desc.max_code;
        self.bl_count = [0; MAX_BITS + 1];
        tree[self.heap[self.heap_max]].len = 0;
        let mut overflow = 0i32;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h];
            let mut bits = tree[tree[n].dad as usize].len + 1;
            if bits > desc.max_length {
                bits = desc.max_length;
                overflow += 1;
            }
            tree[n].len = bits;
            h += 1;
            if n as isize > max_code {
                continue;
            }
            self.bl_count[bits as usize] += 1;
            let xbits = if n >= desc.extra_base {
                desc.extra[n - desc.extra_base]
            } else {
                0
            };
            let f = tree[n].freq as u64;
            self.opt_len = self.opt_len.wrapping_add(f * (bits + xbits) as u64);
            if let Some(stree) = stree {
                self.static_len = self
                    .static_len
                    .wrapping_add(f * (stree[n].len + xbits) as u64);
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = desc.max_length as usize - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[desc.max_length as usize] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE;
        for bits in (1..=desc.max_length).rev() {
            let mut n = self.bl_count[bits as usize];
            while n != 0 {
                h -= 1;
                let m = self.heap[h];
                if m as isize > max_code {
                    continue;
                }
                if tree[m].len != bits {
                    self.opt_len = (self.opt_len as i64
                        + (bits as i64 - tree[m].len as i64) * tree[m].freq as i64)
                        as u64;
                    tree[m].len = bits;
                }
                n -= 1;
            }
        }
    }

    fn build_tree(&mut self, tree: &mut [Node], desc: &mut Desc, stree: Option<&[Node]>) {
        let elems = desc.elems;
        let mut max_code: isize = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if tree[n].freq != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n;
                max_code = n as isize;
                self.depth[n] = 0;
            } else {
                tree[n].len = 0;
                tree[n].dad = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code as usize
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            tree[node].freq = 1;
            self.depth[node] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            if let Some(stree) = stree {
                self.static_len = self.static_len.wrapping_sub(stree[node].len as u64);
            }
        }
        desc.max_code = max_code;
        for n in (1..=self.heap_len / 2).rev() {
            self.pqdownheap(tree, n);
        }
        let mut node = elems;
        loop {
            // pqremove
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(tree, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            tree[node].freq = tree[n].freq + tree[m].freq;
            self.depth[node] = self.depth[n].max(self.depth[m]) + 1;
            tree[n].dad = node as u32;
            tree[m].dad = node as u32;
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(tree, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(tree, desc, stree);
        gen_codes(tree, max_code, &self.bl_count);
    }
}

/// `scan_tree` (bl_tree frequencies) or `send_tree` (emit), which walk the lengths alike.
fn walk_tree(tree: &mut [Node], max_code: isize, bl: &mut [Node], mut out: Option<&mut Bits>) {
    let mut prevlen: i64 = -1;
    let mut nextlen = tree[0].len as i64;
    let mut count = 0;
    let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
    if out.is_none() {
        tree[(max_code + 1) as usize].len = 0xffff; // guard
    }
    for n in 0..=max_code {
        let curlen = nextlen;
        nextlen = tree[(n + 1) as usize].len as i64;
        count += 1;
        if count < max_count && curlen == nextlen {
            continue;
        }
        match out.as_deref_mut() {
            None => {
                if count < min_count {
                    bl[curlen as usize].freq += count as u32;
                } else if curlen != 0 {
                    if curlen != prevlen {
                        bl[curlen as usize].freq += 1;
                    }
                    bl[REP_3_6].freq += 1;
                } else if count <= 10 {
                    bl[REPZ_3_10].freq += 1;
                } else {
                    bl[REPZ_11_138].freq += 1;
                }
            }
            Some(o) => {
                let send = |o: &mut Bits, c: usize| o.send(bl[c].code, bl[c].len);
                if count < min_count {
                    for _ in 0..count {
                        send(o, curlen as usize);
                    }
                } else if curlen != 0 {
                    if curlen != prevlen {
                        send(o, curlen as usize);
                        count -= 1;
                    }
                    send(o, REP_3_6);
                    o.send((count - 3) as u32, 2);
                } else if count <= 10 {
                    send(o, REPZ_3_10);
                    o.send((count - 3) as u32, 3);
                } else {
                    send(o, REPZ_11_138);
                    o.send((count - 11) as u32, 7);
                }
            }
        }
        count = 0;
        prevlen = curlen;
        (max_count, min_count) = if nextlen == 0 {
            (138, 3)
        } else if curlen == nextlen {
            (6, 3)
        } else {
            (7, 4)
        };
    }
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// `zlib.compress(data)` (level 6). None past the one-block size this port covers.
pub fn compress(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() >= 16000 {
        return None;
    }
    let t = Tables::new();
    // the window, zero past the input as zlib's high-water initialisation leaves it
    let mut window = data.to_vec();
    window.resize(data.len() + MAX_MATCH + MIN_MATCH + 8, 0);
    let mut head = vec![0usize; HASH_MASK + 1];
    let mut prev = vec![0usize; W_SIZE];
    // (dist, lc): dist 0 is a literal
    let mut syms: Vec<(usize, usize)> = Vec::new();
    let mut ltree = vec![Node::default(); HEAP_SIZE];
    let mut dtree = vec![Node::default(); 2 * D_CODES + 1];
    let mut bltree = vec![Node::default(); 2 * BL_CODES + 1];
    ltree[END_BLOCK].freq = 1;

    let mut strstart = 0usize;
    let mut lookahead = data.len();
    let mut ins_h = 0usize;
    if lookahead >= MIN_MATCH {
        ins_h = window[0] as usize;
        ins_h = ((ins_h << HASH_SHIFT) ^ window[1] as usize) & HASH_MASK;
    }
    let mut match_length = MIN_MATCH - 1;
    let mut match_start = 0usize;
    let mut prev_length;
    let mut prev_match;
    let mut match_available = false;

    let insert = |ins_h: &mut usize, head: &mut [usize], prev: &mut [usize], s: usize| -> usize {
        *ins_h = ((*ins_h << HASH_SHIFT) ^ window[s + MIN_MATCH - 1] as usize) & HASH_MASK;
        let h = head[*ins_h];
        prev[s & (W_SIZE - 1)] = h;
        head[*ins_h] = s;
        h
    };
    let tally_lit = |syms: &mut Vec<(usize, usize)>, ltree: &mut [Node], c: u8| {
        syms.push((0, c as usize));
        ltree[c as usize].freq += 1;
    };

    loop {
        if lookahead == 0 {
            break;
        }
        let mut hash_head = 0usize;
        if lookahead >= MIN_MATCH {
            hash_head = insert(&mut ins_h, &mut head, &mut prev, strstart);
        }
        prev_length = match_length;
        prev_match = match_start;
        match_length = MIN_MATCH - 1;
        if hash_head != 0 && prev_length < MAX_LAZY && strstart - hash_head <= MAX_DIST {
            // longest_match
            let mut chain_length = MAX_CHAIN;
            let mut best_len = prev_length;
            let mut nice_match = NICE_LENGTH;
            let limit = strstart.saturating_sub(MAX_DIST);
            if prev_length >= GOOD_LENGTH {
                chain_length >>= 2;
            }
            if nice_match > lookahead {
                nice_match = lookahead;
            }
            let mut cur_match = hash_head;
            loop {
                let m = cur_match;
                let s = strstart;
                if window[m + best_len] == window[s + best_len]
                    && window[m + best_len - 1] == window[s + best_len - 1]
                    && window[m] == window[s]
                    && window[m + 1] == window[s + 1]
                {
                    let mut len = 2;
                    while len < MAX_MATCH && window[s + len] == window[m + len] {
                        len += 1;
                    }
                    if len > best_len {
                        match_start = cur_match;
                        best_len = len;
                        if len >= nice_match {
                            break;
                        }
                    }
                }
                cur_match = prev[cur_match & (W_SIZE - 1)];
                chain_length -= 1;
                if !(cur_match > limit && chain_length != 0) {
                    break;
                }
            }
            match_length = best_len.min(lookahead);
            if match_length <= 5 && match_length == MIN_MATCH && strstart - match_start > TOO_FAR {
                match_length = MIN_MATCH - 1;
            }
        }
        if prev_length >= MIN_MATCH && match_length <= prev_length {
            let max_insert = strstart + lookahead - MIN_MATCH;
            let dist = strstart - 1 - prev_match;
            let len = prev_length - MIN_MATCH;
            syms.push((dist, len));
            ltree[t.length_code[len] as usize + LITERALS + 1].freq += 1;
            dtree[t.d_code(dist - 1)].freq += 1;
            lookahead -= prev_length - 1;
            let mut pl = prev_length - 2;
            loop {
                strstart += 1;
                if strstart <= max_insert {
                    insert(&mut ins_h, &mut head, &mut prev, strstart);
                }
                pl -= 1;
                if pl == 0 {
                    break;
                }
            }
            match_available = false;
            match_length = MIN_MATCH - 1;
            strstart += 1;
        } else if match_available {
            tally_lit(&mut syms, &mut ltree, window[strstart - 1]);
            strstart += 1;
            lookahead -= 1;
        } else {
            match_available = true;
            strstart += 1;
            lookahead -= 1;
        }
    }
    if match_available {
        tally_lit(&mut syms, &mut ltree, window[strstart - 1]);
    }

    // _tr_flush_block(last = 1)
    let mut st = State {
        heap: [0; HEAP_SIZE],
        heap_len: 0,
        heap_max: 0,
        depth: [0; HEAP_SIZE],
        bl_count: [0; MAX_BITS + 1],
        opt_len: 0,
        static_len: 0,
    };
    let mut l_desc = Desc {
        elems: L_CODES,
        extra: &EXTRA_LBITS,
        extra_base: LITERALS + 1,
        max_length: 15,
        max_code: 0,
    };
    let mut d_desc = Desc {
        elems: D_CODES,
        extra: &EXTRA_DBITS,
        extra_base: 0,
        max_length: 15,
        max_code: 0,
    };
    let mut bl_desc = Desc {
        elems: BL_CODES,
        extra: &EXTRA_BLBITS,
        extra_base: 0,
        max_length: 7,
        max_code: 0,
    };
    st.build_tree(&mut ltree, &mut l_desc, Some(&t.static_ltree));
    st.build_tree(&mut dtree, &mut d_desc, Some(&t.static_dtree));
    walk_tree(&mut ltree, l_desc.max_code, &mut bltree, None);
    walk_tree(&mut dtree, d_desc.max_code, &mut bltree, None);
    st.build_tree(&mut bltree, &mut bl_desc, None);
    let mut max_blindex = BL_CODES - 1;
    while max_blindex >= 3 && bltree[BL_ORDER[max_blindex]].len == 0 {
        max_blindex -= 1;
    }
    st.opt_len = st
        .opt_len
        .wrapping_add(3 * (max_blindex as u64 + 1) + 5 + 5 + 4);
    let mut opt_lenb = (st.opt_len.wrapping_add(3 + 7)) >> 3;
    let static_lenb = (st.static_len.wrapping_add(3 + 7)) >> 3;
    if static_lenb <= opt_lenb {
        opt_lenb = static_lenb;
    }

    let mut bits = Bits {
        out: vec![0x78, 0x9c],
        buf: 0,
        n: 0,
    };
    let stored_len = data.len() as u64;
    if stored_len + 4 <= opt_lenb {
        // _tr_stored_block, last: header bits, align, LEN, NLEN, the bytes
        bits.send(1, 3);
        bits.windup();
        let len = data.len() as u16;
        bits.out.extend_from_slice(&len.to_le_bytes());
        bits.out.extend_from_slice(&(!len).to_le_bytes());
        bits.out.extend_from_slice(data);
    } else {
        let (lt, dt): (&[Node], &[Node]) = if static_lenb == opt_lenb {
            bits.send(2 + 1, 3);
            (&t.static_ltree, &t.static_dtree)
        } else {
            bits.send(4 + 1, 3);
            let (lcodes, dcodes, blcodes) =
                (l_desc.max_code + 1, d_desc.max_code + 1, max_blindex + 1);
            bits.send((lcodes - 257) as u32, 5);
            bits.send((dcodes - 1) as u32, 5);
            bits.send((blcodes - 4) as u32, 4);
            for &rank in BL_ORDER.iter().take(blcodes) {
                bits.send(bltree[rank].len, 3);
            }
            walk_tree(&mut ltree, lcodes - 1, &mut bltree, Some(&mut bits));
            walk_tree(&mut dtree, dcodes - 1, &mut bltree, Some(&mut bits));
            (&ltree, &dtree)
        };
        for &(dist, lc) in &syms {
            if dist == 0 {
                bits.send(lt[lc].code, lt[lc].len);
            } else {
                let code = t.length_code[lc] as usize;
                bits.send(lt[code + LITERALS + 1].code, lt[code + LITERALS + 1].len);
                let extra = EXTRA_LBITS[code];
                if extra != 0 {
                    bits.send(lc as u32 - t.base_length[code], extra);
                }
                let d = dist - 1;
                let code = t.d_code(d);
                bits.send(dt[code].code, dt[code].len);
                let extra = EXTRA_DBITS[code];
                if extra != 0 {
                    bits.send(d as u32 - t.base_dist[code], extra);
                }
            }
        }
        bits.send(lt[END_BLOCK].code, lt[END_BLOCK].len);
        bits.windup();
    }
    let mut out = bits.out;
    out.extend_from_slice(&adler32(data).to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn small_inputs() {
        assert_eq!(compress(b"").unwrap(), unhex("789c030000000001"));
        assert_eq!(compress(b"a").unwrap(), unhex("789c4b040000620062"));
    }

    /// Every vector `zlib.compress` produced in CPython 3.12 (zlib 1.3.1), when the file exists.
    #[test]
    fn matches_python_zlib() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratch-db/zlib_vectors.txt"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut n = 0;
        for line in text.lines() {
            let (input, want) = line.split_once(' ').unwrap();
            let input = unhex(input);
            if input.len() >= 16000 {
                continue;
            }
            assert_eq!(
                compress(&input).unwrap(),
                unhex(want),
                "input of {} bytes",
                input.len()
            );
            n += 1;
        }
        assert!(n > 100);
    }
}
