//! `v3/pipeline.py`: the NDJSON envelope and failure contract every v3 module shares.
//!
//! One line per step: `{"process", "data", "error_status", "w", "a", "next"}`, opened by the
//! `preparation` line carrying the plan and closed by the `end` line with empty data and a null
//! `next`. `pack` writes each line exactly as Python's `json.dumps(data) + "\n"` does: keys in
//! insertion order (this crate enables serde_json `preserve_order`), `", "` and `": "` separators,
//! ASCII only, Python float repr.
//!
//! Integration (rust/api): call a module runner to get a `Stream`, then write each item of the
//! iterator to the response body as it arrives (a chunked `Body::from_stream`), with the
//! NDJSON content type the Flask route sets. The api's own Flask-JSON writer relies on
//! serde_json's map being SORTED (core.rs, `write_json`); once this crate is linked, cargo's
//! feature unification turns `preserve_order` on for the api too, so that writer must sort keys
//! itself (collect and sort the map entries) before the two are linked.

use serde_json::{Value, json};

use crate::Error;
use crate::py::fsum_py;

/// One entry of a module's `processes` list.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub name: String,
    pub w: f64,
}

impl Process {
    pub fn new(name: &str, w: f64) -> Self {
        Self {
            name: name.to_string(),
            w,
        }
    }

    fn to_json(&self) -> Value {
        json!({"name": self.name, "w": self.w})
    }
}

/// `pack(data)`: one NDJSON line, as `json.dumps(data) + "\n"`.
pub fn pack(data: &Value) -> String {
    let mut out = String::new();
    write_py_json(data, &mut out);
    out.push('\n');
    out
}

/// `safe(stage, fn, *args)`: a component error becomes `({'failed': True, 'flags': [...]}, {})`,
/// carrying only the exception TYPE, as the Python does (the text can hold a URL or a DSN).
pub fn safe(stage: &str, r: crate::Result<(Value, Value)>) -> (Value, Value) {
    match r {
        Ok(pair) => pair,
        Err(e) => (
            json!({"failed": true, "flags": [format!("{stage} could not be computed ({}).", e.kind)]}),
            json!({}),
        ),
    }
}

/// `after(stage, fn, aoi, *futures)` once the dependencies have landed: `safe`, then
/// `retryable=True` when any dependency's results carry `failed`.
pub fn after(stage: &str, r: crate::Result<(Value, Value)>, deps: &[&Value]) -> (Value, Value) {
    let (mut results, view) = safe(stage, r);
    if deps.iter().any(|d| truthy(d.get("failed")))
        && let Value::Object(m) = &mut results
    {
        m.insert("retryable".into(), Value::Bool(true));
    }
    (results, view)
}

/// Python truthiness of an optional JSON value.
fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::py::truthy)
}

/// `error_status(results)`: None when the component ran clean.
pub fn error_status(results: &Value) -> Option<Value> {
    let list = |k: &str| {
        results
            .get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (flags, missing) = (list("flags"), list("missing"));
    if !(truthy(results.get("failed")) || !flags.is_empty() || !missing.is_empty()) {
        return None;
    }
    let retryable = truthy(results.get("failed")) || truthy(results.get("retryable"));
    let messages: Vec<Value> = flags.into_iter().chain(missing).collect();
    Some(json!({
        "state": if retryable { "partial" } else { "failed" },
        "retryable": retryable,
        "retry_url": null,
        "messages": messages,
    }))
}

/// `stream(processes, views, retry_url)`: an iterator of NDJSON lines. `views` yields one
/// `(view_results, error_status)` per entry between `preparation` and `end`, in order; it is
/// pulled lazily, so a line is ready as soon as its component is.
pub struct Stream<I> {
    processes: Vec<Process>,
    views: I,
    retry_url: Option<RetryUrl>,
    total_w: f64,
    step: usize,
    done: bool,
}

/// Builds the retry URL of a failed component from its name.
pub type RetryUrl = Box<dyn Fn(&str) -> String + Send>;

pub fn stream<I>(
    processes: Vec<Process>,
    views: I,
    retry_url: Option<RetryUrl>,
) -> crate::Result<Stream<I>>
where
    I: Iterator<Item = (Value, Option<Value>)>,
{
    if processes.len() < 2 {
        return Err(Error::new("IndexError", "list index out of range"));
    }
    // round(sum(p['w'] for p in processes), 2)
    let total_w = crate::py::round_ndigits(fsum_py(processes.iter().map(|p| p.w)), 2);
    Ok(Stream {
        processes,
        views,
        retry_url,
        total_w,
        step: 0,
        done: false,
    })
}

impl<I> Stream<I> {
    fn forge(&self, step: usize, data: Value, status: Option<Value>) -> String {
        let last = self.processes.len() - 1;
        let status = match (status, &self.retry_url) {
            (Some(mut s), Some(url)) if truthy(s.get("retryable")) => {
                if let Value::Object(m) = &mut s {
                    m.insert("retry_url".into(), json!(url(&self.processes[step].name)));
                }
                Some(s)
            }
            (s, _) => s,
        };
        pack(&json!({
            "process": self.processes[step].name,
            "data": data,
            "error_status": status,
            "w": self.total_w,
            "a": self.processes[step].w,
            "next": if step < last { json!(self.processes[step + 1].name) } else { Value::Null },
        }))
    }
}

impl<I: Iterator<Item = (Value, Option<Value>)>> Iterator for Stream<I> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        if self.done {
            return None;
        }
        let line = if self.step == 0 {
            let plan: Vec<Value> = self.processes.iter().map(Process::to_json).collect();
            self.forge(0, json!({"processes": plan}), None)
        } else {
            match self.views.next() {
                Some((view, status)) => self.forge(self.step, view, status),
                None => {
                    self.done = true;
                    self.forge(self.processes.len() - 1, json!({}), None)
                }
            }
        };
        self.step += 1;
        Some(line)
    }
}

/// `component_count(processes)`
pub fn component_count(processes: &[Process]) -> usize {
    processes.len().saturating_sub(2)
}

/// Run a blocking line producer (any runner's iterator) on its own thread, handing lines over a
/// ONE-slot channel: the producer stays a single line ahead, as the route's `_keepalive` queue
/// (`maxsize=1`). The route polls it with `recv_timeout(30 s)` and writes the heartbeat `" "` on
/// a timeout; `Disconnected` is the end of the stream. Dropping the receiver stops the producer
/// at its next line (a running component finishes first).
pub fn spawn_lines<I: Iterator<Item = String> + Send + 'static>(
    lines: I,
) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        for line in lines {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
}

// ---- the component pool every streaming runner shares (`_run_components`) ----

/// A component's `(results, view_results)`.
pub type Pair = (Value, Value);

/// One component of a run: its name (the `safe` stage), the indices of the jobs it needs (all
/// earlier in the list), and its body, called with those jobs' outputs in order.
pub struct Job {
    pub name: String,
    pub deps: Vec<usize>,
    #[expect(
        clippy::type_complexity,
        reason = "the component closure, used in this one field"
    )]
    pub run: Box<dyn Fn(&[&Pair]) -> crate::Result<Pair> + Send + Sync>,
}

/// `(cost, deps)` per name in declaration order -> the order `sorted(_UNORDERED, key=_finishes_at)`
/// gives: own cost plus the longest dependency chain, ties in declaration order (stable).
pub fn cheapest_first(table: &[(&str, f64, Vec<&str>)]) -> Vec<String> {
    fn finishes_at(table: &[(&str, f64, Vec<&str>)], name: &str) -> f64 {
        let Some((_, cost, deps)) = table.iter().find(|(n, _, _)| *n == name) else {
            return 0.0;
        };
        // max((...), default=0.0): the first maximal value
        let longest = deps
            .iter()
            .map(|d| finishes_at(table, d))
            .fold(0.0, |m: f64, x| if x > m { x } else { m });
        cost + longest
    }
    let mut names: Vec<(&str, f64)> = table
        .iter()
        .map(|(n, _, _)| (*n, finishes_at(table, n)))
        .collect();
    names.sort_by(|a, b| a.1.total_cmp(&b.1));
    names.into_iter().map(|(n, _)| n.to_string()).collect()
}

/// `resolve(wanted)`: the names to RUN (requested plus their dependencies, transitively), in
/// `order`. Unknown names are the route's 400, checked before this.
pub fn resolve(
    order: &[String],
    deps_of: impl Fn(&str) -> Vec<String>,
    wanted: Option<&[String]>,
) -> Vec<String> {
    let Some(wanted) = wanted else {
        return order.to_vec();
    };
    let mut needed: Vec<String> = Vec::new();
    let mut stack: Vec<String> = wanted.to_vec();
    while let Some(name) = stack.pop() {
        if needed.contains(&name) {
            continue;
        }
        stack.extend(deps_of(&name));
        needed.push(name);
    }
    order
        .iter()
        .filter(|n| needed.contains(n))
        .cloned()
        .collect()
}

/// `plan(wanted)`: the bookends plus the components being EMITTED.
pub fn plan(processes: &[Process], wanted: Option<&[String]>) -> Vec<Process> {
    let Some(wanted) = wanted else {
        return processes.to_vec();
    };
    let mut p = vec![processes[0].clone()];
    p.extend(
        processes[1..processes.len() - 1]
            .iter()
            .filter(|p| wanted.contains(&p.name))
            .cloned(),
    );
    p.push(processes[processes.len() - 1].clone());
    p
}

#[derive(Default)]
struct PoolState {
    done: Vec<Option<std::sync::Arc<Pair>>>,
    next: usize,
    cancelled: bool,
}

type Shared = std::sync::Arc<(std::sync::Mutex<PoolState>, std::sync::Condvar)>;

/// The views of a pool run, in emission order; dropping it stops the pool taking new jobs (a
/// running component finishes on its own thread, as a ThreadPoolExecutor's would).
pub struct PoolViews {
    shared: Shared,
    emit: std::vec::IntoIter<usize>,
}

impl Iterator for PoolViews {
    type Item = (Value, Option<Value>);
    fn next(&mut self) -> Option<Self::Item> {
        let i = self.emit.next()?;
        let (lock, cv) = &*self.shared;
        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(Some(pair)) = g.done.get(i) {
                let pair = pair.clone();
                drop(g);
                return Some((pair.1.clone(), error_status(&pair.0)));
            }
            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for PoolViews {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).cancelled = true;
        cv.notify_all();
    }
}

/// `_run_components`: `jobs` (topologically ordered) on `min(len, workers)` threads taking jobs
/// in submission order (ThreadPoolExecutor's FIFO queue), each `safe`-wrapped, a dependent waiting
/// for its dependencies on its worker and then `after`-wrapped. Yields `(view, error_status)` for
/// the jobs in `emit`, in that order, each as soon as it and every earlier one has landed.
pub fn run_pool(jobs: Vec<Job>, emit: Vec<usize>, workers: usize) -> PoolViews {
    let n = jobs.len();
    let shared: Shared = std::sync::Arc::new((
        std::sync::Mutex::new(PoolState {
            done: vec![None; n],
            ..Default::default()
        }),
        std::sync::Condvar::new(),
    ));
    let jobs = std::sync::Arc::new(jobs);
    for _ in 0..workers.min(n).max(1) {
        let (shared, jobs) = (shared.clone(), jobs.clone());
        std::thread::spawn(move || {
            let (lock, cv) = &*shared;
            loop {
                let (i, deps) = {
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if g.cancelled || g.next >= jobs.len() {
                        return;
                    }
                    let i = g.next;
                    g.next += 1;
                    // wait on the worker for every dependency's result
                    loop {
                        if g.cancelled {
                            return;
                        }
                        let ready: Option<Vec<std::sync::Arc<Pair>>> =
                            jobs[i].deps.iter().map(|d| g.done[*d].clone()).collect();
                        if let Some(r) = ready {
                            break (i, r);
                        }
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let job = &jobs[i];
                let dep_refs: Vec<&Pair> = deps.iter().map(|d| d.as_ref()).collect();
                let r =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.run)(&dep_refs)))
                        .unwrap_or_else(|_| {
                            Err(Error::new("PanicException", "component panicked"))
                        });
                let out = if job.deps.is_empty() {
                    safe(&job.name, r)
                } else {
                    let dep_results: Vec<&Value> = deps.iter().map(|d| &d.0).collect();
                    after(&job.name, r, &dep_results)
                };
                lock.lock().unwrap_or_else(|e| e.into_inner()).done[i] =
                    Some(std::sync::Arc::new(out));
                cv.notify_all();
            }
        });
    }
    PoolViews {
        shared,
        emit: emit.into_iter(),
    }
}

// ---- json.dumps(data) with the default arguments ----

/// Python's `repr(float)`: shortest digits, exponent form below 1e-4 and from 1e16, and
/// `NaN` / `Infinity` as json.dumps writes them (serde_json cannot hold those, so they only reach
/// here through `py_float` callers).
// shortcut: a copy of rust/api core.rs `py_float` (same algorithm, proven there); merge the two
// into one shared crate when the api links this one.
pub fn py_float(f: f64, out: &mut String) {
    if f.is_nan() {
        out.push_str("NaN");
        return;
    }
    if f.is_infinite() {
        out.push_str(if f > 0.0 { "Infinity" } else { "-Infinity" });
        return;
    }
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits = tie_to_even(f, mantissa.chars().filter(|c| *c != '.').collect(), exp);
    out.push_str(sign);
    if !(-4..16).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        ));
    } else if exp < 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-exp - 1) as usize));
        out.push_str(&digits);
    } else {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            out.push_str(&"0".repeat(int_len - digits.len()));
            out.push_str(".0");
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    }
}

/// Rust's shortest digits round an exact tie up; Python's dtoa takes the even neighbour when both
/// round-trip.
fn tie_to_even(f: f64, digits: String, exp: i32) -> String {
    let n = digits.len();
    let last = digits.as_bytes()[n - 1] - b'0';
    if last.is_multiple_of(2) {
        return digits;
    }
    let near = format!("{:.*e}", n, f.abs());
    let near: String = near
        .split('e')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| *c != '.')
        .collect();
    if !near.ends_with('5') {
        return digits;
    }
    let exact = format!("{:.800e}", f.abs());
    let (exact, exact_exp) = exact.split_once('e').unwrap_or((&exact, ""));
    let exact: String = exact.chars().filter(|c| *c != '.').collect();
    let exact = exact.trim_end_matches('0');
    if exact.len() != n + 1 || !exact.ends_with('5') || exact_exp != exp.to_string() {
        return digits;
    }
    let lower = &exact[..n];
    let even = if (lower.as_bytes()[n - 1] - b'0').is_multiple_of(2) {
        lower.to_string()
    } else {
        match lower.parse::<u128>() {
            Ok(v) if (v + 1).to_string().len() == n => (v + 1).to_string(),
            _ => return digits,
        }
    };
    let parsed: Option<f64> = format!("{}.{}e{exp}", &even[..1], &even[1..]).parse().ok();
    if parsed == Some(f.abs()) {
        even
    } else {
        digits
    }
}

fn py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `json.dumps(v)`: insertion-ordered keys, `", "` / `": "`, ASCII only, Python floats.
pub fn write_py_json(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => py_float(f, out),
            _ => out.push_str(&n.to_string()),
        },
        Value::String(s) => py_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_str(k, out);
                out.push_str(": ");
                write_py_json(item, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_like_python() {
        // json.dumps({"b": 1, "a": [1.0, 1e16, 0.0001, 1e-05, "é\n"], "c": None, "d": {}})
        let v = json!({"b": 1, "a": [1.0, 1e16, 0.0001, 1e-05, "\u{e9}\n"], "c": null, "d": {}});
        assert_eq!(
            pack(&v),
            "{\"b\": 1, \"a\": [1.0, 1e+16, 0.0001, 1e-05, \"\\u00e9\\n\"], \"c\": null, \"d\": {}}\n"
        );
    }

    #[test]
    fn envelope_and_status() {
        let processes = vec![
            Process::new("preparation", 0.5),
            Process::new("x", 1.25),
            Process::new("end", 0.0),
        ];
        let failed = safe("x", Err(Error::new("OSError", "boom")));
        let status = error_status(&failed.0);
        // a runner's stream can be handed to `spawn_lines`
        fn send<T: Send + 'static>() {}
        send::<Stream<PoolViews>>();
        let pool = run_pool(
            vec![
                Job {
                    name: "a".into(),
                    deps: vec![],
                    run: Box::new(|_| Err(Error::new("OSError", "x"))),
                },
                Job {
                    name: "b".into(),
                    deps: vec![0],
                    run: Box::new(|d| Ok((json!({"seen": d[0].0["failed"]}), json!({"v": 1})))),
                },
            ],
            vec![1, 0],
            1,
        );
        let got: Vec<_> = pool.collect();
        assert_eq!(got[0].0, json!({"v": 1}));
        assert_eq!(got[0].1, None); // `after` sets retryable, but no flags: clean
        assert_eq!(
            got[1].1.as_ref().map(|s| s["state"].clone()),
            Some(json!("partial"))
        );
        assert_eq!(
            cheapest_first(&[
                ("x", 2.0, vec![]),
                ("y", 0.5, vec!["x"]),
                ("z", 1.0, vec![]),
                ("w", 2.5, vec![])
            ]),
            ["z", "x", "y", "w"]
        );
        let lines: Vec<String> = stream(
            processes,
            vec![(failed.1, status)].into_iter(),
            Some(Box::new(|n: &str| format!("/retry?process={n}"))),
        )
        .unwrap()
        .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "{\"process\": \"preparation\", \"data\": {\"processes\": [{\"name\": \"preparation\", \"w\": 0.5}, {\"name\": \"x\", \"w\": 1.25}, {\"name\": \"end\", \"w\": 0.0}]}, \"error_status\": null, \"w\": 1.75, \"a\": 0.5, \"next\": \"x\"}\n"
        );
        assert_eq!(
            lines[1],
            "{\"process\": \"x\", \"data\": {}, \"error_status\": {\"state\": \"partial\", \"retryable\": true, \"retry_url\": \"/retry?process=x\", \"messages\": [\"x could not be computed (OSError).\"]}, \"w\": 1.75, \"a\": 1.25, \"next\": \"end\"}\n"
        );
        assert_eq!(
            lines[2],
            "{\"process\": \"end\", \"data\": {}, \"error_status\": null, \"w\": 1.75, \"a\": 0.0, \"next\": null}\n"
        );
    }
}
