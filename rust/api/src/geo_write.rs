//! `application/apis/geo_apis`: the POST routes of `polygon/routes.py` and `map/routes.py`, with
//! `GeoUtils.construct_gdf` / `get_db`, `GeoLogic.calculate_project_area_db` /
//! `form_project_area_result` and `UserLogic.add_user_sessions_for_log`.
//!
//! Not wired (ADR-003): nothing here has run against a database.
//!
//! The request body is parsed by a port of Python's `json.loads` (below), not serde_json:
//! serde_json's default float parsing is not correctly rounded for 17-digit coordinates (a client
//! sending `104.63025787812345` could be stored one ULP off), and it refuses the `NaN` /
//! `Infinity` tokens Python accepts.

use axum::response::Response;
use chrono::{NaiveDateTime, Timelike};
use serde_json::{Map, Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::core::{self, Req, ctx};
use crate::sa::{Lit, Stmt};

// ---- how the routes' `except` clauses answer ----

pub(crate) enum Fail {
    /// `AppMessageException`: 400, the message is shown.
    App(String),
    /// Any other exception: 500, the message is shown in development only.
    Py(String),
    /// A database error: 500 through `core::db_error`.
    Db(sqlx::Error),
    /// A failed flush (INSERT/UPDATE): 500 like `Py`; it leaves the request's session expired
    /// and pending rollback, so the after_request log cannot read a logged-in `current_user`.
    Flush(String),
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

pub(crate) type PyResult<T> = Result<T, Fail>;

pub(crate) fn py_err(message: impl Into<String>) -> Fail {
    Fail::Py(message.into())
}

pub(crate) async fn fail_response(fail: Fail) -> Response {
    match fail {
        Fail::App(m) => core::json(400, &core::app_error(&m, None)),
        Fail::Py(m) => core::json(500, &core::other_error(&m, None).await),
        Fail::Db(e) => core::db_error(e).await,
        Fail::Flush(m) => core::json(500, &core::other_error(&m, None).await),
    }
}

// ---- Python values, as json.loads and psycopg2 hand them to the routes ----

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Py {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// `decimal.Decimal` from a `numeric` column, kept as its `str()`.
    Decimal(String),
    Str(String),
    List(Vec<Py>),
    /// Insertion-ordered, as a Python dict.
    Dict(Vec<(String, Py)>),
}

impl Py {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Py::None => "NoneType",
            Py::Bool(_) => "bool",
            Py::Int(_) => "int",
            Py::Float(_) => "float",
            Py::Decimal(_) => "decimal.Decimal",
            Py::Str(_) => "str",
            Py::List(_) => "list",
            Py::Dict(_) => "dict",
        }
    }

    pub(crate) fn truthy(&self) -> bool {
        match self {
            Py::None => false,
            Py::Bool(b) => *b,
            Py::Int(i) => *i != 0,
            Py::Float(f) => *f != 0.0, // NaN is truthy
            Py::Decimal(s) => s.parse::<f64>() != Ok(0.0),
            Py::Str(s) => !s.is_empty(),
            Py::List(v) => !v.is_empty(),
            Py::Dict(v) => !v.is_empty(),
        }
    }

    /// `obj.get(key)`: only a dict has it.
    pub(crate) fn get(&self, key: &str) -> PyResult<Py> {
        match self {
            Py::Dict(items) => Ok(items
                .iter()
                .find(|(k, _)| k == key)
                .map_or(Py::None, |(_, v)| v.clone())),
            other => Err(py_err(format!(
                "'{}' object has no attribute 'get'",
                other.type_name()
            ))),
        }
    }

    /// `dict[key]` on a dict known to be one.
    fn item(&self, key: &str) -> PyResult<Py> {
        match self {
            Py::Dict(items) => items
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| py_err(py_repr(&Py::Str(key.into())))), // KeyError
            other => Err(py_err(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }

    /// `obj[0]`
    fn first(&self) -> PyResult<Py> {
        match self {
            Py::List(v) => v
                .first()
                .cloned()
                .ok_or_else(|| py_err("list index out of range")),
            Py::Str(s) => s
                .chars()
                .next()
                .map(|c| Py::Str(c.to_string()))
                .ok_or_else(|| py_err("string index out of range")),
            Py::Dict(_) => Err(py_err("0")), // KeyError(0); JSON keys are str
            other => Err(py_err(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }

    /// `for x in obj`
    pub(crate) fn iter(&self) -> PyResult<Vec<Py>> {
        match self {
            Py::List(v) => Ok(v.clone()),
            Py::Str(s) => Ok(s.chars().map(|c| Py::Str(c.to_string())).collect()),
            Py::Dict(items) => Ok(items.iter().map(|(k, _)| Py::Str(k.clone())).collect()),
            other => Err(py_err(format!(
                "'{}' object is not iterable",
                other.type_name()
            ))),
        }
    }

    fn lower(&self) -> PyResult<String> {
        match self {
            Py::Str(s) => Ok(s.to_lowercase()),
            other => Err(py_err(format!(
                "'{}' object has no attribute 'lower'",
                other.type_name()
            ))),
        }
    }
}

/// `repr(float)` (`str()` is the same).
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        core::to_flask_json(&json!(f)).trim_end().to_string()
    }
}

/// `repr(obj)`. Strings: Python's quote choice and escapes; non-ASCII is printed as is
/// (Python escapes non-printable code points too, which is not reproduced).
pub(crate) fn py_repr(v: &Py) -> String {
    match v {
        Py::Str(s) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::from(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                        out.push_str(&format!("\\x{:02x}", c as u32))
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        Py::Decimal(s) => format!("Decimal('{s}')"),
        other => py_str(other),
    }
}

/// `str(obj)`
pub(crate) fn py_str(v: &Py) -> String {
    match v {
        Py::None => "None".into(),
        Py::Bool(b) => if *b { "True" } else { "False" }.into(),
        Py::Int(i) => i.to_string(),
        Py::Float(f) => float_repr(*f),
        Py::Decimal(s) | Py::Str(s) => s.clone(),
        Py::List(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Py::Dict(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Py::Str(k.clone())), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str.strip()`: Python's whitespace is Unicode White_Space plus U+001C..U+001F.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// Digits with single underscores between them, as `int()` / `float()` accept; None when an
/// underscore is misplaced.
fn without_underscores(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'_'
            && !(i > 0
                && b[i - 1].is_ascii_digit()
                && b.get(i + 1).is_some_and(|n| n.is_ascii_digit()))
        {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// `int(s)` for a str. shortcut: values beyond i64 (Python has no bound) and non-ASCII digits
/// are refused; upgrade if MAX_DRAW_AREA ever holds one.
pub(crate) fn py_int(s: &str) -> Option<i64> {
    without_underscores(py_strip(s))?.parse().ok()
}

/// `float(obj)`
pub(crate) fn py_float(v: &Py) -> PyResult<f64> {
    match v {
        Py::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Py::Int(i) => Ok(*i as f64),
        Py::Float(f) => Ok(*f),
        Py::Decimal(s) => s
            .parse()
            .map_err(|_| py_err(format!("could not convert {s}"))),
        Py::Str(s) => without_underscores(py_strip(s))
            .and_then(|t| t.parse::<f64>().ok())
            .ok_or_else(|| {
                py_err(format!(
                    "could not convert string to float: {}",
                    py_repr(&Py::Str(s.clone()))
                ))
            }),
        other => Err(py_err(format!(
            "float() argument must be a string or a real number, not '{}'",
            other.type_name()
        ))),
    }
}

/// What `jsonify` writes for a value. shortcut: NaN / Infinity, which Python writes as bare
/// tokens, become null; only a database function returning NaN can reach this.
pub(crate) fn to_json(v: &Py) -> Value {
    match v {
        Py::None => Value::Null,
        Py::Bool(b) => json!(b),
        Py::Int(i) => json!(i),
        Py::Float(f) if f.is_finite() => json!(f),
        Py::Float(_) => Value::Null,
        Py::Decimal(s) | Py::Str(s) => json!(s), // Flask writes Decimal as str()
        Py::List(items) => Value::Array(items.iter().map(to_json).collect()),
        Py::Dict(items) => Value::Object(
            items
                .iter()
                .map(|(k, v)| (k.clone(), to_json(v)))
                .collect::<Map<_, _>>(),
        ),
    }
}

/// `PickleType` bind: None is NULL, anything else `pickle.dumps(obj)`.
/// serde-pickle writes protocol 3 where SQLAlchemy writes `HIGHEST_PROTOCOL` (5); both unpickle to
/// the same value. Dict keys come out sorted (dict equality ignores order).
fn pickle_column(v: &Py) -> PyResult<Option<Vec<u8>>> {
    fn convert(v: &Py) -> serde_pickle::Value {
        use serde_pickle::{HashableValue as H, Value as P};
        match v {
            Py::None => P::None,
            Py::Bool(b) => P::Bool(*b),
            Py::Int(i) => P::I64(*i),
            Py::Float(f) => P::F64(*f),
            // a request body never holds a Decimal
            Py::Decimal(s) | Py::Str(s) => P::String(s.clone()),
            Py::List(items) => P::List(items.iter().map(convert).collect()),
            Py::Dict(items) => P::Dict(
                items
                    .iter()
                    .map(|(k, v)| (H::String(k.clone()), convert(v)))
                    .collect(),
            ),
        }
    }
    if *v == Py::None {
        return Ok(None);
    }
    serde_pickle::value_to_vec(&convert(v), serde_pickle::SerOptions::new())
        .map(Some)
        .map_err(|e| py_err(e.to_string()))
}

// ---- json.loads (CPython 3.12's C scanner, strict) ----

/// CPython's default recursion limit, less the frames Flask itself holds.
const MAX_DEPTH: usize = 950;

struct JsonParser<'a> {
    s: &'a str,
    i: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    /// `JSONDecodeError(msg, doc, pos)`: positions count characters, not bytes.
    fn err(&self, msg: &str, at: usize) -> String {
        let before = &self.s[..at];
        let pos = before.chars().count();
        let line = before.matches('\n').count() + 1;
        let col = match before.rfind('\n') {
            Some(nl) => pos - self.s[..nl].chars().count(),
            None => pos + 1,
        };
        format!("{msg}: line {line} column {col} (char {pos})")
    }

    fn value(&mut self) -> Result<Py, String> {
        let rest = &self.s[self.i..];
        match self.peek() {
            Some(b'"') => {
                let start = self.i;
                self.i += 1;
                self.string(start).map(Py::Str)
            }
            Some(b'{') => {
                self.i += 1;
                self.nested(Self::object, "object")
            }
            Some(b'[') => {
                self.i += 1;
                self.nested(Self::array, "array")
            }
            _ if rest.starts_with("null") => {
                self.i += 4;
                Ok(Py::None)
            }
            _ if rest.starts_with("true") => {
                self.i += 4;
                Ok(Py::Bool(true))
            }
            _ if rest.starts_with("false") => {
                self.i += 5;
                Ok(Py::Bool(false))
            }
            _ => {
                if let Some(n) = self.number() {
                    return Ok(n);
                }
                for (token, f) in [
                    ("NaN", f64::NAN),
                    ("Infinity", f64::INFINITY),
                    ("-Infinity", f64::NEG_INFINITY),
                ] {
                    if rest.starts_with(token) {
                        self.i += token.len();
                        return Ok(Py::Float(f));
                    }
                }
                Err(self.err("Expecting value", self.i))
            }
        }
    }

    /// Python raises RecursionError (not a ValueError, so not Flask's BadRequest) past its
    /// recursion limit; this also keeps a hostile body from overflowing the stack.
    fn nested(
        &mut self,
        parse: fn(&mut Self) -> Result<Py, String>,
        what: &str,
    ) -> Result<Py, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!(
                "maximum recursion depth exceeded while decoding a JSON {what} from a unicode string"
            ));
        }
        let v = parse(self);
        self.depth -= 1;
        v
    }

    /// `-?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?`, ASCII digits only; int when there is neither
    /// fraction nor exponent.
    fn number(&mut self) -> Option<Py> {
        let b = self.s.as_bytes();
        let digit = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
        let start = self.i;
        let mut j = start;
        if b.get(j) == Some(&b'-') {
            j += 1;
        }
        match b.get(j) {
            Some(b'0') => j += 1,
            Some(b'1'..=b'9') => {
                while digit(j) {
                    j += 1;
                }
            }
            _ => return None,
        }
        let mut is_float = false;
        if b.get(j) == Some(&b'.') && digit(j + 1) {
            j += 1;
            while digit(j) {
                j += 1;
            }
            is_float = true;
        }
        if matches!(b.get(j), Some(b'e' | b'E')) {
            let mut k = j + 1;
            if matches!(b.get(k), Some(b'+' | b'-')) {
                k += 1;
            }
            if digit(k) {
                while digit(k) {
                    k += 1;
                }
                j = k;
                is_float = true;
            }
        }
        let text = &self.s[start..j];
        self.i = j;
        // Rust's str -> f64 is correctly rounded, as Python's float() is.
        let float = || Py::Float(text.parse().unwrap_or(f64::NAN));
        Some(if is_float {
            float()
        } else {
            // shortcut: an int beyond i64 becomes a float (Python keeps an exact int); upgrade if
            // a client ever sends one.
            text.parse().map_or_else(|_| float(), Py::Int)
        })
    }

    fn object(&mut self) -> Result<Py, String> {
        let mut items: Vec<(String, Py)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Py::Dict(items));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.err("Expecting property name enclosed in double quotes", self.i));
            }
            let start = self.i;
            self.i += 1;
            let key = self.string(start)?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("Expecting ':' delimiter", self.i));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            // a repeated key keeps its first position and takes the last value
            match items.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = v,
                None => items.push((key, v)),
            }
            self.ws();
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Py::Dict(items));
                }
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Py, String> {
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Py::List(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b']') => {
                    self.i += 1;
                    return Ok(Py::List(items));
                }
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter", self.i)),
            }
        }
    }

    fn hex4(&mut self, u_at: usize) -> Result<u32, String> {
        let hex = self
            .s
            .get(self.i..self.i + 4)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .ok_or_else(|| self.err("Invalid \\uXXXX escape", u_at))?;
        self.i += 4;
        Ok(hex)
    }

    /// The string body after its opening quote at `start`.
    fn string(&mut self, start: usize) -> Result<String, String> {
        let mut out = String::new();
        loop {
            let Some(c) = self.s[self.i..].chars().next() else {
                return Err(self.err("Unterminated string starting at", start));
            };
            match c {
                '"' => {
                    self.i += 1;
                    return Ok(out);
                }
                '\\' => {
                    let esc_at = self.i;
                    self.i += 1;
                    let Some(e) = self.s[self.i..].chars().next() else {
                        return Err(self.err("Unterminated string starting at", start));
                    };
                    self.i += e.len_utf8();
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut code = self.hex4(esc_at + 1)?;
                            if (0xd800..0xdc00).contains(&code)
                                && self.s[self.i..].starts_with("\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let low = self.hex4(self.i - 1)?;
                                if (0xdc00..0xe000).contains(&low) {
                                    code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                                } else {
                                    self.i = save;
                                }
                            }
                            // shortcut: a lone surrogate, which a Python str can hold, becomes
                            // U+FFFD; upgrade if a client ever sends one.
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.err("Invalid \\escape", esc_at)),
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(self.err("Invalid control character at", self.i));
                }
                c => {
                    out.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }
}

pub(crate) fn json_loads(s: &str) -> Result<Py, String> {
    let mut p = JsonParser { s, i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != s.len() {
        return Err(p.err("Extra data", p.i));
    }
    Ok(v)
}

// ---- the request body, as Flask 3.0 / Werkzeug 3.0 read it ----

/// `request.is_json`: the mimetype is `application/json` or `application/*+json`.
pub(crate) fn is_json(req: &Req) -> bool {
    let mimetype = req
        .parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"))
}

/// `request.get_json()` after `is_json` held; Err is the `str()` of what it raises. Development
/// (the only config that shows the message) runs with DEBUG=True, where Flask re-raises
/// werkzeug's `BadRequest(f"Failed to decode JSON object: {e}")`.
/// shortcut: UTF-16/32 bodies (json.loads detects them) are refused; upgrade if a client sends one.
pub(crate) fn body_json(req: &Req) -> Result<Py, String> {
    // `flask run` is not in debug mode: werkzeug answers its generic BadRequest text
    let bad = |_e: String| core::BAD_REQUEST_TEXT.to_string();
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    let text = std::str::from_utf8(body).map_err(|e| {
        let at = e.valid_up_to();
        bad(format!(
            "'utf-8' codec can't decode byte 0x{:02x} in position {at}: invalid start byte",
            body.get(at).copied().unwrap_or(0)
        ))
    })?;
    json_loads(text).map_err(|e| {
        if e.starts_with("maximum recursion depth") {
            e
        } else {
            bad(e)
        }
    })
}

// ---- parameters as psycopg2 renders them ----

/// A value compared with a `String(100)` column (`filter_by(session_id=v)`): None matches no row
/// (`IS NULL` on a non-null column); any non-string makes Postgres or psycopg2 raise.
pub(crate) fn str_param(v: &Py) -> PyResult<Option<String>> {
    let pg_type = match v {
        Py::None => return Ok(None),
        Py::Str(s) => return Ok(Some(s.clone())),
        Py::Dict(_) => {
            return Err(py_err(
                "(psycopg2.ProgrammingError) can't adapt type 'dict'",
            ));
        }
        Py::Bool(_) => "boolean",
        Py::Int(_) => "integer",
        Py::Float(_) | Py::Decimal(_) => "numeric",
        Py::List(_) => "text[]",
    };
    Err(py_err(format!(
        "(psycopg2.errors.UndefinedFunction) operator does not exist: character varying = {pg_type}"
    )))
}

/// `get_date()`: naive UTC now, microsecond precision as Python's datetime.
pub(crate) fn get_date() -> NaiveDateTime {
    let now = chrono::Utc::now().naive_utc();
    now.with_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

/// `str(uuid.uuid4())` (OS RNG).
fn uuid4() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---- GeoUtils.get_db on the GIS database ----

/// One retry after a failure, as the original refreshes its connection and runs the query again.
async fn gis_fetch(sql: &str, param: &str) -> Result<Vec<PgRow>, sqlx::Error> {
    let gis = &ctx().await.gis;
    // the value inlined as psycopg2 inlines it (an untyped literal), sent unprepared: the GIS
    // database sits behind a pooler that loses prepared statements (26000) under concurrency
    let run = || {
        crate::pq::query(sql)
            .bind(crate::pq::Untyped(param))
            .fetch_all(gis)
    };
    match run().await {
        Ok(rows) => Ok(rows),
        Err(_) => run().await,
    }
}

/// A column read as `x::text` with `pg_typeof(x)::text`, turned into what psycopg2 returns.
/// shortcut: date/time types come back as str (psycopg2 gives datetime); none of these
/// functions/views is known to return one.
fn typed(text: Option<String>, pg_type: Option<String>) -> Py {
    let Some(text) = text else { return Py::None };
    match pg_type.as_deref().unwrap_or("") {
        "double precision" | "real" => Py::Float(text.parse().unwrap_or(f64::NAN)),
        "integer" | "bigint" | "smallint" => text.parse().map_or(Py::Str(text), Py::Int),
        "numeric" => Py::Decimal(text),
        "boolean" => Py::Bool(text == "true"),
        "json" | "jsonb" => json_loads(&text).unwrap_or(Py::Str(text)),
        _ => Py::Str(text), // text, varchar, uuid (str(UUID) is its text), geometry (hex EWKB)
    }
}

fn row_typed(row: &PgRow, column: &str) -> Result<Py, sqlx::Error> {
    Ok(typed(
        row.try_get(column)?,
        row.try_get(format!("{column}_type").as_str())?,
    ))
}

/// `row.get('within') == 0` on the JSON text of the column: 0, 0.0 and False equal 0.
fn json_is_zero(text: &str) -> bool {
    text == "false" || text.parse::<f64>().is_ok_and(|f| f == 0.0)
}

// ---- shapely: Polygon / MultiPolygon from GeoJSON coordinates, and str(geom) ----

/// `LinearRing(coords)` (shapely 2.0.5): None when empty, else the closed ring, every position
/// with the same 2 or 3 ordinates.
fn linear_ring(v: &Py) -> PyResult<Option<Vec<Vec<f64>>>> {
    if *v == Py::None {
        return Ok(None);
    }
    let positions = v.iter()?;
    let mut coords = Vec::with_capacity(positions.len() + 1);
    for o in &positions {
        coords.push(
            o.iter()?
                .iter()
                .map(py_float)
                .collect::<PyResult<Vec<f64>>>()?,
        );
    }
    let Some(first) = coords.first().cloned() else {
        return Ok(None);
    };
    let dim = first.len();
    if coords.iter().any(|c| c.len() != dim) {
        return Err(py_err(format!(
            "setting an array element with a sequence. The requested array has an inhomogeneous \
             shape after 1 dimensions. The detected shape was ({},) + inhomogeneous part.",
            coords.len()
        )));
    }
    if dim != 2 && dim != 3 {
        return Err(py_err(format!(
            "The ordinate (last) dimension should be 2 or 3, got {dim}"
        )));
    }
    // shapely closes an open ring (`!=` on every ordinate, so NaN is never closed) and always
    // repeats the first position of a 3-position ring
    if coords.len() == 3 || coords.last().is_some_and(|last| *last != first) {
        coords.push(first.clone());
    }
    if coords.len() < 4 {
        return Err(py_err("A linearring requires at least 4 coordinates."));
    }
    // GEOS checks closure in 2D with ==, which NaN fails
    if first[0].is_nan() || first[1].is_nan() {
        return Err(py_err(
            "IllegalArgumentException: Points of LinearRing do not form a closed linestring",
        ));
    }
    Ok(Some(coords))
}

/// A number as GEOS 3.11's WKTWriter (trim, 16 decimals) prints it: the shortest round-trip
/// digits, rounded half-even to at most 16 decimals, fixed notation, no trailing zeros.
fn geos_num(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x == 0.0 {
        return "0".into();
    }
    let sci = format!("{:e}", x.abs()); // shortest digits, e.g. `1.2345e-7`
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let mut digits: Vec<u8> = mantissa
        .bytes()
        .filter(|b| *b != b'.')
        .map(|b| b - b'0')
        .collect();
    // decimals = digits after the point; the first digit sits at 10^exp
    let mut scale = digits.len() as i64 - 1 - exp;
    if scale > 16 {
        let keep = digits.len() as i64 - (scale - 16);
        if keep < 0 {
            digits = vec![0];
        } else {
            let keep = keep as usize;
            let next = digits[keep];
            let rest_nonzero = digits[keep + 1..].iter().any(|d| *d != 0);
            let last_odd = keep > 0 && digits[keep - 1] % 2 == 1;
            digits.truncate(keep);
            if next > 5 || (next == 5 && (rest_nonzero || last_odd)) {
                let mut i = digits.len();
                loop {
                    if i == 0 {
                        digits.insert(0, 1);
                        break;
                    }
                    i -= 1;
                    if digits[i] == 9 {
                        digits[i] = 0;
                    } else {
                        digits[i] += 1;
                        break;
                    }
                }
            }
            if digits.is_empty() {
                digits.push(0);
            }
        }
        scale = 16;
    }
    while scale > 0 && digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
        scale -= 1;
    }
    if digits.iter().all(|d| *d == 0) {
        return "0".into();
    }
    let text: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    let body = if scale <= 0 {
        format!("{text}{}", "0".repeat((-scale) as usize))
    } else if text.len() as i64 > scale {
        let point = text.len() - scale as usize;
        format!("{}.{}", &text[..point], &text[point..])
    } else {
        format!("0.{}{text}", "0".repeat(scale as usize - text.len()))
    };
    if x < 0.0 { format!("-{body}") } else { body }
}

fn ring_wkt(ring: &[Vec<f64>], with_z: bool) -> String {
    let coords: Vec<String> = ring
        .iter()
        .map(|c| {
            let mut s = format!("{} {}", geos_num(c[0]), geos_num(c[1]));
            if with_z {
                // GEOS writes a missing (NaN) z as 0
                let z = c.get(2).copied().unwrap_or(f64::NAN);
                s.push(' ');
                s.push_str(&if z.is_nan() { "0".into() } else { geos_num(z) });
            }
            s
        })
        .collect();
    format!("({})", coords.join(", "))
}

/// `str(Polygon(coordinates[0]))`: the shell only; holes are dropped.
fn polygon_wkt(shell: &Py) -> PyResult<String> {
    Ok(match linear_ring(shell)? {
        None => "POLYGON EMPTY".into(),
        Some(ring) if ring[0].len() == 3 => format!("POLYGON Z ({})", ring_wkt(&ring, true)),
        Some(ring) => format!("POLYGON ({})", ring_wkt(&ring, false)),
    })
}

/// `str(MultiPolygon([Polygon(cord) for polygon in coordinates for cord in polygon]))`: every
/// ring, holes included, becomes its own filled polygon; empty ones are dropped.
fn multipolygon_wkt(coordinates: &Py) -> PyResult<String> {
    let mut rings = Vec::new();
    for polygon in coordinates.iter()? {
        for cord in polygon.iter()? {
            rings.push(linear_ring(&cord)?);
        }
    }
    let rings: Vec<Vec<Vec<f64>>> = rings.into_iter().flatten().collect();
    if rings.is_empty() {
        return Ok("MULTIPOLYGON EMPTY".into());
    }
    let with_z = rings.iter().any(|r| r[0].len() == 3);
    let parts: Vec<String> = rings
        .iter()
        .map(|r| format!("({})", ring_wkt(r, with_z)))
        .collect();
    Ok(format!(
        "MULTIPOLYGON{} ({})",
        if with_z { " Z" } else { "" },
        parts.join(", ")
    ))
}

/// shapely's `_is_coordinates_empty`
fn coordinates_empty(v: &Py) -> bool {
    match v {
        Py::None => true,
        Py::List(items) => items.iter().all(coordinates_empty),
        _ => false,
    }
}

/// `GeoUtils.construct_gdf(data)`: only its failures matter (the GeoDataFrame, the CEA area and
/// the print are discarded). `geometry` is a dict whose `type` is a str.
fn construct_gdf(geometry: &Py) -> PyResult<()> {
    let geom_type = geometry.item("type")?.lower()?;
    if geom_type == "polygon" {
        // shape({"type": ..., "coordinates": [coordinates[0]]})
        let shell = geometry.item("coordinates")?.first()?;
        if !coordinates_empty(&shell) {
            linear_ring(&shell)?;
        }
        return Ok(());
    }
    // shape(data['geometry'])
    if let Ok(coordinates) = geometry.item("coordinates")
        && coordinates_empty(&coordinates)
    {
        return Ok(());
    }
    match geom_type.as_str() {
        "multipolygon" => {
            // MultiPolygon([[c[0], c[1:]] for c in coordinates]): every subscript first, then the
            // polygons in order
            let mut polygons = Vec::new();
            for c in geometry.item("coordinates")?.iter()? {
                let shell = c.first()?;
                let holes: Vec<Py> = match &c {
                    Py::List(v) => v[1..].to_vec(),
                    Py::Str(s) => s
                        .chars()
                        .skip(1)
                        .map(|ch| Py::Str(ch.to_string()))
                        .collect(),
                    _ => Vec::new(),
                };
                polygons.push((shell, holes));
            }
            // Polygon(shell, holes): None is empty at once, else the shell ring, then each hole
            for (shell, holes) in polygons {
                if shell == Py::None {
                    continue;
                }
                linear_ring(&shell)?;
                for hole in &holes {
                    linear_ring(hole)?;
                }
            }
            Ok(())
        }
        // shape() builds these; the route then fails on `geom` (shortcut: a malformed one fails
        // inside shape() first in Python, with another 500 message)
        "point" | "linestring" | "linearring" | "multipoint" | "multilinestring"
        | "geometrycollection" | "feature" => Ok(()),
        other => Err(py_err(format!(
            "Unknown geometry type: {}",
            py_repr(&Py::Str(other.into()))
        ))),
    }
}

// ---- POST /geos/polygon ----

/// `GeoLogic.form_project_area_result(area_size, user_id)`: (size, limit, exceed). The user
/// lookup is the authenticated `current_user` (Python re-reads the same row by id; user_id 0 or
/// None finds none).
async fn form_project_area_result(
    area_size: Py,
    user: Option<&core::User>,
) -> PyResult<(Py, Py, bool)> {
    let area_size = if area_size.truthy() {
        area_size
    } else {
        Py::Int(0)
    };
    let value: Option<String> =
        crate::pq::query_scalar("select value from tbl_master_settings where name = $1 limit 1")
            .bind("MAX_DRAW_AREA")
            .fetch_optional(&ctx().await.app)
            .await?;
    let setting = value
        .as_deref()
        .and_then(py_int)
        .ok_or_else(|| py_err("max draw area settings invalid or not found"))?;
    let limit = match user.and_then(|u| u.size_limit) {
        Some(s) if s != 0.0 => Py::Float(s),
        _ => Py::Int(setting),
    };
    let exceed = py_float(&area_size)? > py_float(&limit)?;
    Ok((area_size, limit, exceed))
}

/// Python's `round(x, 2)` for a float: correctly rounded, ties to even.
fn round2_float(f: f64) -> f64 {
    format!("{f:.2}").parse().unwrap_or(f)
}

/// `round(Decimal, 2)`: quantize to two places, ties to even, on the decimal text.
fn round2_decimal(s: &str) -> String {
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", s),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) || int.is_empty() {
        return s.into(); // NaN, Infinity
    }
    let mut digits: Vec<u8> = int
        .bytes()
        .chain(frac.bytes().chain("00".bytes()).take(2))
        .map(|b| b - b'0')
        .collect();
    let rest = frac.as_bytes().get(2..).unwrap_or(&[]);
    let up = match rest.first() {
        Some(d) if *d > b'5' => true,
        Some(b'5') => {
            rest[1..].iter().any(|d| *d != b'0') || digits.last().is_some_and(|d| d % 2 == 1)
        }
        _ => false,
    };
    if up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let text: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    let point = text.len() - 2;
    let int = text[..point].trim_start_matches('0');
    format!(
        "{sign}{}.{}",
        if int.is_empty() { "0" } else { int },
        &text[point..]
    )
}

/// `round(row.get('project_area'), 2)`
fn round2(v: Py) -> PyResult<Py> {
    match v {
        Py::Float(f) => Ok(Py::Float(round2_float(f))),
        Py::Int(i) => Ok(Py::Int(i)),
        Py::Bool(b) => Ok(Py::Int(b as i64)),
        Py::Decimal(s) => Ok(Py::Decimal(round2_decimal(&s))),
        other => Err(py_err(format!(
            "type {} doesn't define __round__ method",
            other.type_name()
        ))),
    }
}

/// The text a rounded area is written to `numeric(12,2)` with (`$n::numeric`).
fn numeric_text(v: &Py) -> Option<String> {
    match v {
        Py::Int(i) => Some(i.to_string()),
        Py::Float(f) if f.is_nan() => Some("NaN".into()),
        Py::Float(f) if f.is_infinite() => {
            Some(if *f > 0.0 { "Infinity" } else { "-Infinity" }.into())
        }
        Py::Float(f) => Some(f.to_string()),
        Py::Decimal(s) => Some(s.clone()),
        _ => None,
    }
}

/// `POST /geos/polygon`
pub async fn polygon_post(req: Req) -> Response {
    // g.__request_data__ = request.get_json() runs before the try; it stays unset when that raises
    if !is_json(&req) {
        // log: none, __session_id__ is None
        return core::json(400, &core::app_error("please provide json data", None));
    }
    let data = match body_json(&req) {
        Ok(data) => data,
        Err(message) => {
            // log: none, __session_id__ is None
            return core::json(500, &core::other_error(&message, None).await);
        }
    };
    // `current_user` is read next; Flask-Login touching the session adds Vary: Cookie from here
    let user = match core::current_user(&req).await {
        Ok(user) => user,
        // log: none, __session_id__ is None
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let mut log_session: Option<String> = None;
    let (response, description) =
        match polygon_post_run(&data, user.as_ref(), &mut log_session).await {
            Ok(body) => {
                let area = body["result"]["area_size"].clone();
                (core::json(200, &body), json!({ "project_area": area }))
            }
            // log_func reads current_user.is_authenticated off the expired user: it raises, no row
            Err(fail @ Fail::Flush(_)) if user.is_some() => {
                return core::vary_cookie(fail_response(fail).await);
            }
            Err(fail) => (fail_response(fail).await, json!({})),
        };
    let entry = crate::logging::LogEntry {
        log_it: true,
        api_name: Some("geo_post_polygon".into()),
        session_id: log_session,
        request_data: to_json(&data),
        description_data: description,
        user_id: user.map(|u| u.id),
    };
    crate::logging::log_response(&entry, core::vary_cookie(response)).await
}

async fn polygon_post_run(
    data: &Py,
    user: Option<&core::User>,
    log_session: &mut Option<String>,
) -> PyResult<Value> {
    let session_id = data.get("session_id")?;
    let geometry = data.get("geometry")?;
    let geom_type = geometry.get("type")?.lower()?;
    if data.get("is_debug")?.truthy() {
        return Err(Fail::App("This is a general error for debugging".into()));
    }

    construct_gdf(&geometry)?;
    let geom = match geom_type.as_str() {
        "polygon" => polygon_wkt(&geometry.get("coordinates")?.first()?)?,
        "multipolygon" => multipolygon_wkt(&geometry.get("coordinates")?)?,
        _ => {
            return Err(py_err(
                "cannot access local variable 'geom' where it is not associated with a value",
            ));
        }
    };

    // `from sea.gadm_bbox('{aio_geom}')`: the WKT inlined as a quoted literal (a quote in it is
    // doubled, where the original's str.format would break the statement). `select *` then
    // `row.get('within')`: to_jsonb keeps "no such column" as NULL.
    let rows = gis_fetch(
        "select (to_jsonb(t) -> 'within')::text as within from sea.gadm_bbox($1) t",
        &geom,
    )
    .await?;
    for row in &rows {
        let within: Option<String> = row.try_get("within")?;
        if within.as_deref().is_some_and(json_is_zero) {
            return Err(Fail::App("Area is not in our coverage analysis".into()));
        }
    }

    let (status_code, message) = (201u16, "successfully");
    let sid = if session_id.truthy() {
        py_strip(&py_str(&session_id)).to_string()
    } else {
        String::new()
    };
    let new_session = sid.is_empty();
    let sid = if new_session { uuid4() } else { sid };

    let app = &ctx().await.app;
    let mut tx = app.begin().await?;
    if new_session {
        // UserLogic.add_user_sessions_for_log: pending until the autoflush of the next query
        // created_at: get_date(), naive UTC; column assumed `timestamp without time zone`
        crate::pq::query(
            // the ORM INSERT lists every mapped column without a Python default as NULL
            "insert into tbl_user_sessions (user_id, session_id, is_active, created_at, \
             project_name, project_description, is_project, privacy_level, analyzer_version, \
             deletion_reminder_sent_at, updated_at, updated_by) \
             values ($1, $2, 1, $3, NULL, NULL, false, 1, NULL, NULL, NULL, 0)",
        )
        .bind(user.map(|u| u.id))
        .bind(&sid)
        .bind(get_date())
        .execute(&mut *tx)
        .await?;
        crate::pq::query(r#"insert into "DataAnalyzer" (session_id) values ($1)"#)
            .bind(&sid)
            .execute(&mut *tx)
            .await?;
    }
    let known_session: Option<i32> =
        crate::pq::query_scalar("select id from tbl_sessions where session_id = $1 limit 1")
            .bind(&sid)
            .fetch_optional(&mut *tx)
            .await?;
    if known_session.is_none() {
        // tbl_sessions.created_at is sa.DateTime() (migration c4cd51ca5fb7): timestamp without tz
        // the ORM INSERT lists every mapped column without a Python default as NULL
        crate::pq::query("insert into tbl_sessions (session_id, created_at, aoi_total_area, aoi, baseline_id, intervention_id, intervention_parameter_value, benefit_id, is_active) values ($1, $2, NULL, NULL, NULL, NULL, NULL, NULL, 1)")
            .bind(&sid)
            .bind(get_date())
            .execute(&mut *tx)
            .await?;
    }
    *log_session = Some(sid.clone());

    let known_polygon: Option<i32> =
        crate::pq::query_scalar("select id from polygons where session_id = $1 limit 1")
            .bind(&sid)
            .fetch_optional(&mut *tx)
            .await?;
    // `geom = str(geom)`: geoalchemy2 binds a plain str as is, inside ST_GeomFromEWKT (SRID 0,
    // which the geometry(GEOMETRY,4326) typmod turns into 4326)
    let (status_code, message, polygon_id) = match known_polygon {
        Some(id) => {
            Stmt::new("UPDATE polygons SET geom=ST_GeomFromEWKT(%(geom)s) WHERE polygons.id = %(polygons_id)s")
                .param("geom", Lit::Str(geom.clone()))
                .param("polygons_id", Lit::Int(id.into()))
                .execute(&mut tx)
                .await
                .map_err(Fail::Flush)?;
            (200, "updated, Geometry is updated successfully", id)
        }
        None => {
            // the ORM INSERT lists every mapped column without a Python default as NULL
            let rows = Stmt::new(
                "INSERT INTO polygons (geom, session_id, project_area_size, country, province, district) VALUES (ST_GeomFromEWKT(%(geom)s), %(session_id)s, %(project_area_size)s, %(country)s, %(province)s, %(district)s) RETURNING polygons.id",
            )
            .param("geom", Lit::Str(geom.clone()))
            .param("session_id", Lit::Str(sid.clone()))
            .param("project_area_size", Lit::Null)
            .param("country", Lit::Null)
            .param("province", Lit::Null)
            .param("district", Lit::Null)
            .fetch_all(&mut tx)
            .await
            .map_err(Fail::Flush)?;
            let id: i32 = match rows.first() {
                Some(row) => row.try_get(0)?,
                None => return Err(py_err("INSERT returned no row")),
            };
            (status_code, message, id)
        }
    };
    tx.commit().await?;

    // db.session.refresh(known_polygons), then calculate_project_area_db in the same transaction.
    // `geom.desc`: the lowercase hex of ST_AsEWKB, as geoalchemy2 loads it.
    let mut tx = app.begin().await?;
    let hex: Option<String> = crate::pq::query_scalar(
        "select encode(ST_AsEWKB(geom), 'hex') from polygons where id = $1",
    )
    .bind(polygon_id)
    .fetch_one(&mut *tx)
    .await?;
    let hex = hex.ok_or_else(|| py_err("'NoneType' object has no attribute 'desc'"))?;
    let rows = gis_fetch(
        "select project_area::text as project_area, pg_typeof(project_area)::text as project_area_type, \
         district::text as district, province::text as province, country::text as country, \
         iso_3::text as iso_3, pg_typeof(iso_3)::text as iso_3_type \
         from sea.v3_adm($1)",
        &hex,
    )
    .await?;
    let mut area_size = Py::Int(0);
    // the last row wins: (country, province, district, iso_3)
    #[expect(
        clippy::type_complexity,
        reason = "the four columns the Python reads from the last row"
    )]
    let mut adm: Option<(Option<String>, Option<String>, Option<String>, Py)> = None;
    for row in &rows {
        area_size = round2(row_typed(row, "project_area")?)?;
        adm = Some((
            row.try_get("country")?,
            row.try_get("province")?,
            row.try_get("district")?,
            row_typed(row, "iso_3")?,
        ));
    }
    if let Some((country, province, district, _)) = &adm {
        crate::pq::query(
            "update polygons set project_area_size = $1::numeric, country = $2, province = $3, \
             district = $4 where id = $5",
        )
        .bind(numeric_text(&area_size))
        .bind(country)
        .bind(province)
        .bind(district)
        .bind(polygon_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let (size, limit, exceed) = form_project_area_result(area_size, user).await?;

    // `iso_3` is not a mapped column: the object only has it when v3_adm set it
    let Some((country, _, _, iso_3)) = adm else {
        return Err(py_err("'Polygons' object has no attribute 'iso_3'"));
    };
    Ok(core::success(
        json!({ "result": {
            "session_id": sid,
            "area_size": to_json(&size),
            "user_limit": to_json(&limit),
            "exceed_size": exceed,
            "country": country,
            "iso_3": to_json(&iso_3),
        }}),
        Some(status_code),
        Some(message),
    ))
}

// ---- POST /geos/map/explorer ----

/// `POST /geos/map/explorer`
pub async fn map_explorer_post(req: Req) -> Response {
    if !is_json(&req) {
        return core::json(400, &core::app_error("please provide json data", None));
    }
    let data = match body_json(&req) {
        Ok(data) => data,
        Err(message) => return core::json(500, &core::other_error(&message, None).await),
    };
    match map_explorer_run(&data).await {
        Ok(body) => core::json(200, &body),
        Err(fail) => fail_response(fail).await,
    }
}

/// A request value as psycopg2 adapts it (`sa::Lit`).
fn lit(v: &Py) -> Lit {
    match v {
        Py::None => Lit::Null,
        Py::Bool(b) => Lit::Bool(*b),
        Py::Int(i) => Lit::Int(*i),
        Py::Float(f) => Lit::Float(*f),
        Py::Decimal(d) => Lit::Raw(d.clone(), format!("Decimal('{d}')")),
        Py::Str(s) => Lit::Str(s.clone()),
        Py::List(items) => Lit::List(items.iter().map(lit).collect()),
        Py::Dict(pairs) => Lit::Dict(pairs.iter().map(|(k, v)| (k.clone(), lit(v))).collect()),
    }
}

/// `json.dumps(sanitize_for_jsonb(v))`, as SQLAlchemy's JSONB binds it (insertion order).
fn py_dumps(v: &Py) -> String {
    match v {
        Py::None => "null".into(),
        Py::Bool(b) => b.to_string(),
        Py::Int(i) => i.to_string(),
        Py::Float(f) if f.is_finite() => core::py_float_repr(*f),
        Py::Float(_) => "null".into(),
        Py::Decimal(d) => match d.parse::<f64>() {
            Ok(f) if f.is_finite() => core::py_float_repr(f),
            _ => "null".into(),
        },
        Py::Str(s) => core::to_flask_json(&json!(s)).trim_end().to_string(),
        Py::List(items) => format!(
            "[{}]",
            items.iter().map(py_dumps).collect::<Vec<_>>().join(", ")
        ),
        Py::Dict(pairs) => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!(
                    "{}: {}",
                    core::to_flask_json(&json!(k)).trim_end(),
                    py_dumps(v)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python `==` between a request value and a loaded one (1 == 1.0 == True; dicts by keys).
fn py_eq(a: &Py, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Bool(x) => Some(*x as i64 as f64),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match (a, b) {
        (Py::None, Value::Null) => true,
        (Py::Bool(x), _) => num(b) == Some(*x as i64 as f64),
        (Py::Int(x), _) => num(b) == Some(*x as f64),
        (Py::Float(x), _) => num(b) == Some(*x),
        (Py::Str(x), Value::String(y)) => x == y,
        (Py::List(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| py_eq(x, y))
        }
        (Py::Dict(xs), Value::Object(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .all(|(k, x)| ys.get(k).is_some_and(|y| py_eq(x, y)))
        }
        _ => false,
    }
}

/// A `PickleType` value as psycopg2 sends `Binary(pickle.dumps(v))`: a bytea literal, shown in
/// the error's parameters as the Binary object (its address is noise to the recording).
fn pickle_lit(v: &Py) -> PyResult<Lit> {
    Ok(match pickle_column(v)? {
        None => Lit::Null,
        Some(bytes) => {
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            Lit::Raw(
                format!("'\\x{hex}'::bytea"),
                "<psycopg2.extensions.Binary object at 0x0000000000000000>".into(),
            )
        }
    })
}

async fn map_explorer_run(data: &Py) -> PyResult<Value> {
    let session_id = data.get("session_id")?;
    let project_duration = data.get("project_duration")?;
    let estimated = data.get("estimated_unplanned_deforestation")?;
    let rest_target = data.get("rest_target")?;
    let intervention = data.get("intervention")?;

    // filter_by(session_id=None) is `IS NULL` on a NOT NULL column: no row, no query needed
    let Some(sid) = str_param(&session_id)? else {
        return Err(Fail::App(
            "session id not found, please input polygon with session id first".into(),
        ));
    };
    let mut tx = ctx().await.app.begin().await?;
    let polygon: Option<i32> =
        crate::pq::query_scalar("select id from polygons where session_id = $1 limit 1")
            .bind(&sid)
            .fetch_optional(&mut *tx)
            .await?;
    if polygon.is_none() {
        return Err(Fail::App(
            "session id not found, please input polygon with session id first".into(),
        ));
    }
    let existing = crate::pq::query(
        r#"select id, project_duration, estimated_unplanned_deforestation, rest_target_json,
           intervention_json from "MapExplorer" where session_id = $1 limit 1"#,
    )
    .bind(&sid)
    .fetch_optional(&mut *tx)
    .await?;

    // the flush, with the values as psycopg2 inlines them
    let values = [
        ("project_duration", lit(&project_duration)),
        ("estimated_unplanned_deforestation", lit(&estimated)),
        ("rest_target", pickle_lit(&rest_target)?),
        ("intervention", pickle_lit(&intervention)?),
        ("rest_target_json", Lit::Str(py_dumps(&rest_target))),
        ("intervention_json", Lit::Str(py_dumps(&intervention))),
    ];
    let (status_code, message) = match existing {
        Some(row) => {
            // SQLAlchemy SETs only what changed (Python ==; a pickle is compared through its
            // JSONB twin, which holds the same value)
            let loaded: [Value; 4] = [
                json!(row.try_get::<Option<i32>, _>(1)?),
                json!(row.try_get::<Option<f64>, _>(2)?),
                row.try_get::<Option<Value>, _>(3)?.unwrap_or(Value::Null),
                row.try_get::<Option<Value>, _>(4)?.unwrap_or(Value::Null),
            ];
            let new = [&project_duration, &estimated, &rest_target, &intervention];
            let changed: Vec<usize> = (0..4).filter(|&i| !py_eq(new[i], &loaded[i])).collect();
            let mut stmt = Stmt::new("");
            let mut sets = Vec::new();
            for (i, (col, value)) in values.into_iter().enumerate() {
                // the JSONB twins (4, 5) follow their pickles (2, 3)
                if changed.contains(&(if i < 4 { i } else { i - 2 })) {
                    sets.push(format!("{col}=%({col})s"));
                    stmt = stmt.param(col, value);
                }
            }
            if !sets.is_empty() {
                stmt.sql = format!(
                    r#"UPDATE "MapExplorer" SET {} WHERE "MapExplorer".id = %(MapExplorer_id)s"#,
                    sets.join(", ")
                );
                stmt.param("MapExplorer_id", Lit::Int(row.try_get::<i32, _>(0)?.into()))
                    .execute(&mut tx)
                    .await
                    .map_err(Fail::Flush)?;
            }
            (200u16, "updated, details is updated succesfully")
        }
        None => {
            let mut stmt = Stmt::new(
                r#"INSERT INTO "MapExplorer" (session_id, project_duration, estimated_unplanned_deforestation, rest_target, intervention, rest_target_json, intervention_json) VALUES (%(session_id)s, %(project_duration)s, %(estimated_unplanned_deforestation)s, %(rest_target)s, %(intervention)s, %(rest_target_json)s, %(intervention_json)s) RETURNING "MapExplorer".id"#,
            )
            .param("session_id", Lit::Str(sid.clone()));
            for (col, value) in values {
                stmt = stmt.param(col, value);
            }
            stmt.fetch_all(&mut tx).await.map_err(Fail::Flush)?;
            (201, "successfully")
        }
    };
    tx.commit().await?;

    Ok(core::success(json!({}), Some(status_code), Some(message)))
}

// ---- POST /geos/map/location and /geos/map/location-search ----

/// `str(value or '')`
fn or_empty_str(v: &Py) -> String {
    if v.truthy() { py_str(v) } else { String::new() }
}

/// `POST /geos/map/location`
pub async fn map_location_post(req: Req) -> Response {
    if !is_json(&req) {
        return core::json(400, &core::app_error("please provide json data", None));
    }
    let data = match body_json(&req) {
        Ok(data) => data,
        Err(message) => return core::json(500, &core::other_error(&message, None).await),
    };
    match map_location_run(&data).await {
        Ok(body) => core::json(200, &body),
        Err(fail) => fail_response(fail).await,
    }
}

async fn map_location_run(data: &Py) -> PyResult<Value> {
    let location_value = or_empty_str(&data.get("location_name")?);
    // psycopg2's untyped literal (gis_fetch inlines it)
    let rows = gis_fetch(
        r#"select lat::text as lat, pg_typeof(lat)::text as lat_type,
            lng::text as lng, pg_typeof(lng)::text as lng_type
        from public."mvwGetLocation"
        where location_value = $1"#,
        &location_value,
    )
    .await?;
    let mut results = Map::new();
    for row in &rows {
        results.insert("lat".into(), to_json(&row_typed(row, "lat")?));
        results.insert("lng".into(), to_json(&row_typed(row, "lng")?));
    }
    Ok(core::success(
        json!({ "selected_location": results }),
        None,
        Some("Location has been successfully identify"),
    ))
}

/// `POST /geos/map/location-search`
pub async fn map_location_search_post(req: Req) -> Response {
    if !is_json(&req) {
        return core::json(400, &core::app_error("please provide json data", None));
    }
    let data = match body_json(&req) {
        Ok(data) => data,
        Err(message) => return core::json(500, &core::other_error(&message, None).await),
    };
    match map_location_search_run(&data).await {
        Ok(body) => core::json(200, &body),
        Err(fail) => fail_response(fail).await,
    }
}

async fn map_location_search_run(data: &Py) -> PyResult<Value> {
    let location_id = or_empty_str(&data.get("location_value")?);
    // psycopg2's untyped literal (gis_fetch inlines it), coerced to the function's parameter type
    let rows = gis_fetch(
        r#"select loc_geom::text as loc_geom, pg_typeof(loc_geom)::text as loc_geom_type
        from public."v3_get_location"($1)"#,
        &location_id,
    )
    .await?;
    let mut results = Map::new();
    for row in &rows {
        results.insert("geom".into(), to_json(&row_typed(row, "loc_geom")?));
    }
    Ok(core::success(
        json!({ "selected_location": results }),
        None,
        Some("Location has been successfully identify"),
    ))
}

// ---- POST /geos/map/upload ----

/// `str(uuid.UUID(text))`: `urn:`/`uuid:` and braces/hyphens dropped, then 32 characters that
/// `int(x, 16)` takes (whitespace, a sign, `0x`, single `_` between digits) and that fit 128 bits.
fn py_uuid(text: &str) -> Option<String> {
    let h = text.replace("urn:", "").replace("uuid:", "");
    let h = h.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if h.chars().count() != 32 {
        return None;
    }
    let t = h.trim_matches(|c: char| c.is_whitespace());
    let (negative, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let t = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(rest) => rest.strip_prefix('_').unwrap_or(rest),
        None => t,
    };
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let digits = t.replace('_', "");
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u128::from_str_radix(&digits, 16).ok()?;
    if negative && value != 0 {
        return None; // UUID(int=...) is out of range
    }
    Some(uuid::Uuid::from_u128(value).to_string())
}

/// The upload folder paths in GDAL messages as `os.path.join` writes them on this platform
/// (nbstool-vector writes POSIX, as staging).
fn os_paths(message: &str) -> String {
    if !cfg!(windows) {
        return message.to_string();
    }
    let mut out = String::new();
    let mut rest = message;
    while let Some(i) = rest.find("Uploaded-File/") {
        out.push_str(&rest[..i]);
        let end = rest[i..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |e| i + e);
        out.push_str(&rest[i..end].replace('/', "\\"));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `POST /geos/map/upload` (`geo_map_upload`): form `session_id`, file `file` (zip, kml, kmz),
/// parsed by `nbstool_vector::parse_upload`; area from `sea.v1_get_area_size` on the GIS DB.
pub async fn map_upload_post(req: Req) -> Response {
    let content_type = req
        .parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let form = crate::users_write::parse_form(content_type, &req.body).await;
    // request.form.to_dict(): the first value of each field
    let mut request_data = Map::new();
    for (k, v) in &form.fields {
        request_data.entry(k.clone()).or_insert_with(|| json!(v));
    }
    let user = match core::current_user(&req).await {
        Ok(user) => user,
        // log: none, __session_id__ is None
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let mut log_session: Option<String> = None;
    let mut description = Map::new();
    let response =
        match map_upload_run(&form, user.as_ref(), &mut log_session, &mut description).await {
            Ok(body) => core::json(200, &body),
            Err(fail @ Fail::Flush(_)) if user.is_some() => {
                return core::vary_cookie(fail_response(fail).await);
            }
            Err(fail) => fail_response(fail).await,
        };
    let entry = crate::logging::LogEntry {
        log_it: true,
        api_name: Some("geo_map_upload".into()),
        session_id: log_session,
        request_data: Value::Object(request_data),
        description_data: Value::Object(description),
        user_id: user.map(|u| u.id),
    };
    crate::logging::log_response(&entry, core::vary_cookie(response)).await
}

async fn map_upload_run(
    form: &crate::users_write::Form,
    user: Option<&core::User>,
    log_session: &mut Option<String>,
    description: &mut Map<String, Value>,
) -> PyResult<Value> {
    let session_id = form.field("session_id").unwrap_or("");
    // `if not file`: a FileStorage without a filename is falsy
    let Some(file) = form.file("file").filter(|f| !f.filename.is_empty()) else {
        return Err(Fail::App("No selected file".into()));
    };
    if !nbstool_vector::allowed_file(&file.filename) {
        return Err(Fail::App(
            "Invalid file format, only ZIP, KML or KMZ files are allowed".into(),
        ));
    }
    let (sid, new_session) = if session_id.is_empty() {
        (uuid4(), true)
    } else {
        let sid = py_uuid(session_id).ok_or_else(|| Fail::App("Invalid session_id".into()))?;
        (sid, false)
    };
    *log_session = Some(sid.clone());
    let extension = file
        .filename
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_lowercase());
    description.insert("file_type".into(), json!(extension));

    // GDAL/GEOS work runs off the async threads
    let (filename, data, parse_sid) = (file.filename.clone(), file.data.clone(), sid.clone());
    let parsed = tokio::task::spawn_blocking(move || {
        nbstool_vector::parse_upload(&filename, &data, &parse_sid)
    })
    .await
    .map_err(|e| py_err(e.to_string()))?;
    let mut results = match parsed {
        Ok(v) => v,
        Err(e) if e.status == 400 => return Err(Fail::App(os_paths(&e.message))),
        Err(e) => return Err(py_err(os_paths(&e.message))),
    };

    // calculate_project_area_geom: `str(features[0]["geometry"])` into sea.v1_get_area_size
    // (bound as a parameter: PostGIS reads the same doubles from any round-trip float text)
    let geometry = results
        .get("features")
        .and_then(Value::as_array)
        .and_then(|f| f.first())
        .ok_or_else(|| py_err("list index out of range"))?
        .get("geometry")
        .cloned()
        .unwrap_or(Value::Null);
    let geom = if geometry.is_null() {
        "None".to_string()
    } else {
        geometry.to_string()
    };
    let rows = gis_fetch(
        "select area_size::text as area_size, pg_typeof(area_size)::text as area_size_type \
         from sea.v1_get_area_size($1)",
        &geom,
    )
    .await?;
    let mut area_size = Py::Int(0);
    for row in &rows {
        area_size = row_typed(row, "area_size")?;
    }
    let (size, limit, exceed) = form_project_area_result(area_size, user).await?;

    if new_session {
        // UserLogic.add_user_sessions_for_log: flushed by form_project_area_result's first query
        // and committed by the route's db.session.commit(); nothing is written when that fails
        let mut tx = ctx().await.app.begin().await?;
        crate::pq::query(
            // the ORM INSERT lists every mapped column without a Python default as NULL
            "insert into tbl_user_sessions (user_id, session_id, is_active, created_at, \
             project_name, project_description, is_project, privacy_level, analyzer_version, \
             deletion_reminder_sent_at, updated_at, updated_by) \
             values ($1, $2, 1, $3, NULL, NULL, false, 1, NULL, NULL, NULL, 0)",
        )
        .bind(user.map(|u| u.id))
        .bind(&sid)
        .bind(get_date())
        .execute(&mut *tx)
        .await?;
        crate::pq::query(r#"insert into "DataAnalyzer" (session_id) values ($1)"#)
            .bind(&sid)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    let size = to_json(&size);
    description.insert("project_area".into(), size.clone());
    if let Value::Object(map) = &mut results {
        map.insert("session_id".into(), json!(sid));
        map.insert("area_size".into(), size);
        map.insert("user_limit".into(), to_json(&limit));
        map.insert("exceed_size".into(), json!(exceed));
    }
    Ok(core::success(
        json!({ "selected_polygon": results }),
        None,
        Some("File successfully uploaded"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(v: PyResult<String>) -> String {
        match v {
            Ok(s) => s,
            Err(Fail::Py(m)) | Err(Fail::App(m)) | Err(Fail::Flush(m)) => panic!("error: {m}"),
            Err(Fail::Db(e)) => panic!("db: {e}"),
        }
    }

    fn err(v: PyResult<String>) -> String {
        match v {
            Err(Fail::Py(m)) => m,
            _ => panic!("expected a Python error"),
        }
    }

    fn parse(s: &str) -> Py {
        json_loads(s).unwrap()
    }

    #[test]
    fn numbers_print_as_geos_trimmed_wkt() {
        // every expected value is shapely 2.0.5 / GEOS 3.11.4 output (venv, 2026-11-27)
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (2.5, "2.5"),
            (104.63025787812344, "104.63025787812344"),
            (-1.977075121, "-1.977075121"),
            (0.12345678901234568, "0.1234567890123457"),
            (1e-20, "0"),
            (-1e-17, "0"),
            (123456789012345678.0, "123456789012345680"),
            (1e17, "100000000000000000"),
            (0.1 + 0.2, "0.3"),
            (1e-5, "0.00001"),
            (1e-7, "0.0000001"),
            (1e16, "10000000000000000"),
            (123_456.789_012_345_67, "123456.78901234567"),
            (0.000_123_456_789_012_345_67, "0.0001234567890123"),
            (1e20, "100000000000000000000"),
            (5e-17, "0"),
            (1.5e-16, "0.0000000000000002"),
            (2.5e-16, "0.0000000000000002"),
            (3.5e-16, "0.0000000000000004"),
            (0.123_456_789_012_345_65, "0.1234567890123456"),
            (1.0000000000000002, "1.0000000000000002"),
            (99999999999999.99, "99999999999999.98"),
            (1e16 + 2.0, "10000000000000002"),
            (2f64.powi(-17), "0.0000076293945312"),
            (3.0 * 2f64.powi(-17), "0.0000228881835938"),
            (5.0 * 2f64.powi(-17), "0.0000381469726562"),
            (0.04453871940548015, "0.0445387194054802"),
            (0.05875806061435595, "0.058758060614356"),
            (0.09524673882682695, "0.095246738826827"),
            (1e22, "10000000000000000000000"),
            (123_456_789.123_456_79, "123456789.12345679"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
        ];
        for (x, want) in cases {
            assert_eq!(geos_num(x), want, "{x:e}");
        }
    }

    #[test]
    fn polygon_wkt_matches_shapely() {
        let wkt = |s: &str| polygon_wkt(&parse(s));
        assert_eq!(
            ok(wkt("[[0,0],[1,0],[1,1]]")),
            "POLYGON ((0 0, 1 0, 1 1, 0 0))"
        );
        assert_eq!(
            ok(wkt("[[0,0],[1,1],[2,2],[0,0]]")),
            "POLYGON ((0 0, 1 1, 2 2, 0 0))"
        );
        assert_eq!(
            ok(wkt("[[0,0],[1,1],[0,0],[0,0]]")),
            "POLYGON ((0 0, 1 1, 0 0, 0 0))"
        );
        assert_eq!(
            ok(wkt("[[0,0],[0,0],[0,0]]")),
            "POLYGON ((0 0, 0 0, 0 0, 0 0))"
        );
        assert_eq!(
            ok(wkt("[[1,2],[3,4],[5,6],[1,2],[7,8]]")),
            "POLYGON ((1 2, 3 4, 5 6, 1 2, 7 8, 1 2))"
        );
        assert_eq!(
            ok(wkt("[[0,0,1],[1,1,1],[2,2,2],[0,0,2]]")),
            "POLYGON Z ((0 0 1, 1 1 1, 2 2 2, 0 0 2, 0 0 1))"
        );
        assert_eq!(
            ok(wkt("[[0,0,NaN],[1,1,1],[2,2,2],[0,0,NaN]]")),
            "POLYGON Z ((0 0 0, 1 1 1, 2 2 2, 0 0 0, 0 0 0))"
        );
        assert_eq!(
            ok(wkt("[[0.0,0],[1,1],[2,2],[-0.0,0]]")),
            "POLYGON ((0 0, 1 1, 2 2, 0 0))"
        );
        assert_eq!(
            ok(wkt(r#"[["inf"," 1.5 "],["1_0","-nan"],[2,2]]"#)),
            "POLYGON ((Infinity 1.5, 10 NaN, 2 2, Infinity 1.5))"
        );
        assert_eq!(
            ok(wkt(r#"[["1.5", true],[1,0],[1,1]]"#)),
            "POLYGON ((1.5 1, 1 0, 1 1, 1.5 1))"
        );
        assert_eq!(ok(wkt("[]")), "POLYGON EMPTY");
        assert_eq!(ok(wkt("null")), "POLYGON EMPTY");
        // a 17-digit coordinate keeps its exact double (serde_json's default parse may not)
        assert_eq!(
            ok(wkt("[[104.63025787812345,-1.977075121],[1,1],[2,2]]")),
            "POLYGON ((104.63025787812344 -1.977075121, 1 1, 2 2, 104.63025787812344 -1.977075121))"
        );
    }

    #[test]
    fn bad_rings_fail_as_shapely_does() {
        let wkt = |s: &str| polygon_wkt(&parse(s));
        assert_eq!(
            err(wkt("[[0,0]]")),
            "A linearring requires at least 4 coordinates."
        );
        assert_eq!(
            err(wkt("[[0,0],[0,0]]")),
            "A linearring requires at least 4 coordinates."
        );
        assert_eq!(
            err(wkt("[[0,0],[1,0]]")),
            "A linearring requires at least 4 coordinates."
        );
        assert_eq!(
            err(wkt("[[1,2],[3,4],[5,6,7]]")),
            "setting an array element with a sequence. The requested array has an inhomogeneous \
             shape after 1 dimensions. The detected shape was (3,) + inhomogeneous part."
        );
        assert_eq!(
            err(wkt("[[]]")),
            "The ordinate (last) dimension should be 2 or 3, got 0"
        );
        assert_eq!(
            err(wkt("\"1234\"")),
            "The ordinate (last) dimension should be 2 or 3, got 1"
        );
        assert_eq!(
            err(wkt("[[0,0,0,0],[1,1,1,1],[2,2,2,2]]")),
            "The ordinate (last) dimension should be 2 or 3, got 4"
        );
        assert_eq!(
            err(wkt("[null,[1,2]]")),
            "'NoneType' object is not iterable"
        );
        assert_eq!(
            err(wkt("[[0,null],[1,0],[1,1]]")),
            "float() argument must be a string or a real number, not 'NoneType'"
        );
        assert_eq!(
            err(wkt("[[NaN,0],[1,1],[2,2]]")),
            "IllegalArgumentException: Points of LinearRing do not form a closed linestring"
        );
        assert_eq!(
            err(wkt(r#"[["a",0],[1,1],[2,2]]"#)),
            "could not convert string to float: 'a'"
        );
    }

    #[test]
    fn multipolygon_makes_every_ring_a_polygon() {
        let wkt = |s: &str| multipolygon_wkt(&parse(s));
        assert_eq!(
            ok(wkt(
                "[[[[0,0],[4,0],[4,4]],[[1,1],[2,1],[2,2]]],[[[9,9],[8,9],[8,8]]]]"
            )),
            "MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)), ((1 1, 2 1, 2 2, 1 1)), ((9 9, 8 9, 8 8, 9 9)))"
        );
        assert_eq!(
            ok(wkt("[[[[0,0],[1,0],[1,1]]],[[[0,0,5],[1,0,5],[1,1,5]]]]")),
            "MULTIPOLYGON Z (((0 0 0, 1 0 0, 1 1 0, 0 0 0)), ((0 0 5, 1 0 5, 1 1 5, 0 0 5)))"
        );
        assert_eq!(ok(wkt("[]")), "MULTIPOLYGON EMPTY");
        assert_eq!(ok(wkt("[[[]]]")), "MULTIPOLYGON EMPTY");
        assert_eq!(ok(wkt("[[null]]")), "MULTIPOLYGON EMPTY");
        assert_eq!(err(wkt("null")), "'NoneType' object is not iterable");
    }

    #[test]
    fn construct_gdf_fails_where_shape_does() {
        let gdf = |s: &str| match construct_gdf(&parse(s)) {
            Ok(()) => "ok".to_string(),
            Err(Fail::Py(m)) => m,
            Err(_) => "other".into(),
        };
        assert_eq!(
            gdf(r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1]],[[0,0]]]}"#),
            "ok"
        );
        assert_eq!(
            gdf(r#"{"type":"Polygon","coordinates":[]}"#),
            "list index out of range"
        );
        assert_eq!(gdf(r#"{"type":"Polygon"}"#), "'coordinates'");
        assert_eq!(gdf(r#"{"type":"Polygon","coordinates":[[[]]]}"#), "ok"); // shape() short-cuts
        assert_eq!(
            gdf(r#"{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1]]],[]]}"#),
            "list index out of range"
        );
        assert_eq!(
            gdf(r#"{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1]],[[0,0],[1,0]]]]}"#),
            "A linearring requires at least 4 coordinates."
        );
        assert_eq!(gdf(r#"{"type":"MultiPolygon","coordinates":[[[]]]}"#), "ok");
        assert_eq!(gdf(r#"{"type":"MultiPolygon"}"#), "'coordinates'");
        assert_eq!(gdf(r#"{"type":"Point","coordinates":[1,2]}"#), "ok");
        assert_eq!(
            gdf(r#"{"type":"Blob","coordinates":[1]}"#),
            "Unknown geometry type: 'blob'"
        );
    }

    #[test]
    fn json_loads_as_python() {
        // a repeated key keeps its place and takes the last value; NaN / -Infinity are accepted
        let Py::Dict(items) = parse(r#" {"a": [1], "b": -Infinity, "a": NaN} "#) else {
            panic!("not a dict")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "a");
        assert!(matches!(items[0].1, Py::Float(f) if f.is_nan()));
        assert_eq!(items[1].1, Py::Float(f64::NEG_INFINITY));
        assert_eq!(
            parse("[1, 2.5, -0, 1e2, true, null, 1.0, 12345678901234567890]"),
            Py::List(vec![
                Py::Int(1),
                Py::Float(2.5),
                Py::Int(0),
                Py::Float(100.0),
                Py::Bool(true),
                Py::None,
                Py::Float(1.0),
                Py::Float(12345678901234567890.0),
            ])
        );
        assert_eq!(parse(r#""é😀""#), Py::Str("é😀".into()));
        let e = |s: &str| json_loads(s).unwrap_err();
        assert_eq!(e(""), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(
            e("{\"a\":1,}"),
            "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)"
        );
        assert_eq!(e("[1,]"), "Expecting value: line 1 column 4 (char 3)");
        assert_eq!(
            e("{\"a\" 1}"),
            "Expecting ':' delimiter: line 1 column 6 (char 5)"
        );
        assert_eq!(
            e("[1 2]"),
            "Expecting ',' delimiter: line 1 column 4 (char 3)"
        );
        assert_eq!(e("{}\n x"), "Extra data: line 2 column 2 (char 4)");
        assert_eq!(
            e("\"é"),
            "Unterminated string starting at: line 1 column 1 (char 0)"
        );
        assert_eq!(
            e("\"\t\""),
            "Invalid control character at: line 1 column 2 (char 1)"
        );
        assert!(e(&"[".repeat(5000)).starts_with("maximum recursion depth exceeded"));
    }

    #[test]
    fn python_str_and_truthiness() {
        assert_eq!(
            py_str(&parse(r#"[1, "a'b", {"k": null}, 2.0, true]"#)),
            "[1, \"a'b\", {'k': None}, 2.0, True]"
        );
        assert_eq!(py_str(&Py::Float(1e16)), "1e+16");
        assert_eq!(py_strip(" \u{1c}abc\u{3000} "), "abc");
        assert!(!Py::Float(0.0).truthy());
        assert!(Py::Float(f64::NAN).truthy());
        assert!(!Py::Decimal("0.00".into()).truthy());
        assert_eq!(or_empty_str(&Py::Int(0)), "");
        assert_eq!(or_empty_str(&Py::Int(7)), "7");
        assert_eq!(py_int(" 1_000 "), Some(1000));
        assert_eq!(py_int("1__0"), None);
    }

    #[test]
    fn rounding_matches_python_round() {
        assert_eq!(round2_float(0.125), 0.12);
        assert_eq!(round2_float(0.375), 0.38);
        assert_eq!(round2_float(2.675), 2.67); // 2.67499999...
        assert_eq!(round2_float(1234.5678), 1234.57);
        assert_eq!(round2_decimal("5"), "5.00");
        assert_eq!(round2_decimal("1.125"), "1.12");
        assert_eq!(round2_decimal("1.135"), "1.14");
        assert_eq!(round2_decimal("1.1251"), "1.13");
        assert_eq!(round2_decimal("-9.999"), "-10.00");
        assert_eq!(round2_decimal("NaN"), "NaN");
    }

    #[test]
    fn gis_values_decode_as_psycopg2_returns_them() {
        let t = |v: &str, ty: &str| typed(Some(v.into()), Some(ty.into()));
        assert_eq!(t("1.5", "double precision"), Py::Float(1.5));
        assert_eq!(t("12", "integer"), Py::Int(12));
        assert_eq!(t("1.50", "numeric"), Py::Decimal("1.50".into()));
        assert_eq!(to_json(&t("1.50", "numeric")), json!("1.50"));
        assert_eq!(t("true", "boolean"), Py::Bool(true));
        assert_eq!(typed(None, Some("text".into())), Py::None);
        assert!(json_is_zero("0") && json_is_zero("0.00") && json_is_zero("false"));
        assert!(!json_is_zero("\"0\"") && !json_is_zero("1") && !json_is_zero("true"));
    }

    #[test]
    fn pickle_and_jsonb_twins() {
        let v = parse(r#"{"b": [1, 2.5, "x", null, true], "a": NaN}"#);
        let bytes = pickle_column(&v).ok().flatten().unwrap();
        let back = serde_pickle::value_from_slice(&bytes, serde_pickle::DeOptions::new()).unwrap();
        use serde_pickle::{HashableValue as H, Value as P};
        let P::Dict(map) = back else {
            panic!("not a dict")
        };
        assert_eq!(
            map.get(&H::String("b".into())),
            Some(&P::List(vec![
                P::I64(1),
                P::F64(2.5),
                P::String("x".into()),
                P::None,
                P::Bool(true)
            ]))
        );
        assert!(matches!(map.get(&H::String("a".into())), Some(P::F64(f)) if f.is_nan()));
        assert_eq!(
            py_dumps(&v),
            r#"{"b": [1, 2.5, "x", null, true], "a": null}"#
        );
        assert!(pickle_column(&Py::None).ok().flatten().is_none());
        assert_eq!(py_dumps(&Py::None), "null");
    }

    #[test]
    fn literals_and_equality() {
        assert_eq!(lit(&Py::Float(12.5)), Lit::Float(12.5));
        assert!(py_eq(&Py::Int(1), &json!(1.0)));
        assert!(py_eq(&Py::Bool(true), &json!(1)));
        assert!(!py_eq(&Py::Str("12".into()), &json!(12)));
        assert!(matches!(str_param(&Py::None), Ok(None)));
        assert!(str_param(&Py::Int(1)).is_err());
    }

    #[test]
    fn uuid4_shape() {
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn py_uuid_is_uuid_uuid() {
        let canon = "12345678-1234-5678-1234-567812345678";
        assert_eq!(py_uuid(canon).as_deref(), Some(canon));
        assert_eq!(
            py_uuid("{12345678123456781234567812345678}").as_deref(),
            Some(canon)
        );
        assert_eq!(
            py_uuid("urn:uuid:12345678-1234-5678-1234-567812345678").as_deref(),
            Some(canon)
        );
        assert_eq!(
            py_uuid("1234-5678123456781234567812345678").as_deref(),
            Some(canon)
        );
        assert_eq!(py_uuid("1234567812345678123456781234567G"), None);
        assert_eq!(py_uuid("abc"), None);
        assert_eq!(py_uuid("-1234567812345678123456781234567"), None);
        assert_eq!(
            py_uuid("+1234567812345678123456781234567").as_deref(),
            Some("01234567-8123-4567-8123-456781234567")
        );
        assert_eq!(
            py_uuid("0x123456781234567812345678123456").as_deref(),
            Some("00123456-7812-3456-7812-345678123456")
        );
    }
}
