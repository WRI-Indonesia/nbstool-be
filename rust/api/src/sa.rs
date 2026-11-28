//! Statements sent the way SQLAlchemy 2.0 + psycopg2 2.9 send them: the compiled SQL with
//! `%(name)s` placeholders, the values inlined client-side as psycopg2 literals (so Postgres sees
//! the same text, picks the same types and fails at the same position), and, on failure, the
//! message SQLAlchemy builds (`(psycopg2.errors.X) <libpq error>\n[SQL: ...]\n[parameters: ...]`),
//! which routes show in development.

use serde_json::Value;
use sqlx::postgres::{PgDatabaseError, PgErrorPosition, PgRow};

use crate::sa_codes::SQLSTATES;

/// A Python value as psycopg2 adapts it.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Lit>),
    Dict(Vec<(String, Lit)>),
    /// SQL text inlined as is (a timestamp psycopg2 renders as `'...'::timestamp`, ...), with its
    /// Python repr for the `[parameters: ...]` line.
    Raw(String, String),
}

impl Lit {
    /// The Python value `json.loads` gives for a JSON value.
    pub fn from_json(v: &Value) -> Lit {
        match v {
            Value::Null => Lit::Null,
            Value::Bool(b) => Lit::Bool(*b),
            Value::Number(n) => match n.as_i64() {
                Some(i) if !n.is_f64() => Lit::Int(i),
                _ => Lit::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            Value::String(s) => Lit::Str(s.clone()),
            Value::Array(items) => Lit::List(items.iter().map(Lit::from_json).collect()),
            Value::Object(map) => Lit::Dict(
                map.iter()
                    .map(|(k, v)| (k.clone(), Lit::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// A naive `datetime`: psycopg2 sends `'<isoformat>'::timestamp`.
    pub fn timestamp(d: chrono::NaiveDateTime) -> Lit {
        use chrono::{Datelike, Timelike};
        let us = d.and_utc().timestamp_subsec_micros();
        let mut repr = format!(
            "datetime.datetime({}, {}, {}, {}, {}",
            d.year(),
            d.month(),
            d.day(),
            d.hour(),
            d.minute()
        );
        if d.second() != 0 || us != 0 {
            repr.push_str(&format!(", {}", d.second()));
        }
        if us != 0 {
            repr.push_str(&format!(", {us}"));
        }
        repr.push(')');
        Lit::Raw(
            format!("'{}'::timestamp", crate::core::py_isoformat(d)),
            repr,
        )
    }

    /// psycopg2's `adapt(value).getquoted()`; a NUL in a string is psycopg2's ValueError.
    pub fn sql(&self) -> Result<String, String> {
        Ok(match self {
            Lit::Null => "NULL".into(),
            Lit::Bool(b) => if *b { "true" } else { "false" }.into(),
            // negative numbers get a leading space, so `- -1` never reads as a comment
            Lit::Int(i) if *i < 0 => format!(" {i}"),
            Lit::Int(i) => i.to_string(),
            Lit::Float(f) if f.is_nan() => "'NaN'::float".into(),
            Lit::Float(f) if f.is_infinite() => if *f > 0.0 {
                "'Infinity'::float"
            } else {
                "'-Infinity'::float"
            }
            .into(),
            Lit::Float(f) => {
                let r = crate::core::py_float_repr(*f);
                if *f < 0.0 { format!(" {r}") } else { r }
            }
            Lit::Str(s) => {
                if s.contains('\0') {
                    return Err("A string literal cannot contain NUL (0x00) characters.".into());
                }
                format!("'{}'", s.replace('\'', "''"))
            }
            Lit::List(items) if items.is_empty() => "'{}'".into(),
            Lit::List(items) => format!(
                "ARRAY[{}]",
                items
                    .iter()
                    .map(Lit::sql)
                    .collect::<Result<Vec<_>, _>>()?
                    .join(",")
            ),
            Lit::Dict(pairs) if pairs.is_empty() => "''::hstore".into(),
            Lit::Dict(pairs) => {
                let keys = Lit::List(pairs.iter().map(|(k, _)| Lit::Str(k.clone())).collect());
                let values = Lit::List(pairs.iter().map(|(_, v)| v.clone()).collect());
                format!("hstore({}, {})", keys.sql()?, values.sql()?)
            }
            Lit::Raw(sql, _) => sql.clone(),
        })
    }

    /// Python's `repr`.
    pub fn repr(&self) -> String {
        match self {
            Lit::Null => "None".into(),
            Lit::Bool(b) => if *b { "True" } else { "False" }.into(),
            Lit::Int(i) => i.to_string(),
            Lit::Float(f) if f.is_nan() => "nan".into(),
            Lit::Float(f) if f.is_infinite() => if *f > 0.0 { "inf" } else { "-inf" }.into(),
            Lit::Float(f) => crate::core::py_float_repr(*f),
            Lit::Str(s) => py_repr_str(s),
            Lit::List(items) => {
                format!(
                    "[{}]",
                    items.iter().map(Lit::repr).collect::<Vec<_>>().join(", ")
                )
            }
            Lit::Dict(pairs) => format!(
                "{{{}}}",
                pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", py_repr_str(k), v.repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Lit::Raw(_, repr) => repr.clone(),
        }
    }
}

/// Python's `repr(str)`. shortcut: non-printable non-ASCII is kept as is (Python escapes it).
pub fn py_repr_str(s: &str) -> String {
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

/// One compiled statement and its parameters, in the order SQLAlchemy lists them.
pub struct Stmt {
    pub sql: String,
    pub params: Vec<(String, Lit)>,
}

impl Stmt {
    pub fn new(sql: impl Into<String>) -> Stmt {
        Stmt {
            sql: sql.into(),
            params: Vec::new(),
        }
    }

    pub fn param(mut self, name: &str, value: Lit) -> Stmt {
        self.params.push((name.to_string(), value));
        self
    }

    /// `cursor.execute(sql, params)`: psycopg2's client-side interpolation.
    pub fn literal(&self) -> Result<String, String> {
        let mut out = String::with_capacity(self.sql.len());
        let mut rest = self.sql.as_str();
        while let Some(i) = rest.find('%') {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            if let Some(t) = tail.strip_prefix("%%") {
                out.push('%');
                rest = t;
            } else if let Some(t) = tail.strip_prefix("%(") {
                let end = t
                    .find(")s")
                    .ok_or_else(|| "incomplete placeholder".to_string())?;
                let name = &t[..end];
                let value = self
                    .params
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v)
                    .ok_or_else(|| format!("'{name}'"))?;
                out.push_str(&value.sql()?);
                rest = &t[end + 2..];
            } else {
                out.push('%');
                rest = &tail[1..];
            }
        }
        out.push_str(rest);
        Ok(out)
    }

    /// `[parameters: ...]`: `_repr_params` of a dict, each value cut to 300 characters.
    fn params_repr(&self) -> String {
        let trunc = |rep: String| {
            let n = rep.chars().count();
            if n > 300 {
                let head: String = rep.chars().take(150).collect();
                let tail: String = rep.chars().skip(n - 150).collect();
                format!("{head} ... ({} characters truncated) ... {tail}", n - 300)
            } else {
                rep
            }
        };
        format!(
            "{{{}}}",
            self.params
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), trunc(v.repr())))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// The text of the exception the route's `except Exception` prints for `e`.
    pub fn error_text(&self, literal: &str, e: &sqlx::Error) -> String {
        let Some(pg) = e
            .as_database_error()
            .and_then(|d| d.try_downcast_ref::<PgDatabaseError>())
        else {
            return e.to_string();
        };
        let (class, code) = SQLSTATES
            .iter()
            .find(|(s, _, _)| *s == pg.code())
            .map(|(_, c, k)| (*c, *k))
            .unwrap_or(("DatabaseError", "4xp6"));
        let mut text = format!("(psycopg2.errors.{class}) {}\n", pg.message());
        match pg.position() {
            Some(PgErrorPosition::Original(p)) => error_position(&mut text, literal, p),
            Some(PgErrorPosition::Internal { position, query }) => {
                error_position(&mut text, query, position)
            }
            None => {}
        }
        for (label, value) in [("DETAIL", pg.detail()), ("HINT", pg.hint())] {
            if let Some(v) = value {
                text.push_str(&format!("{label}:  {v}\n"));
            }
        }
        if let Some(PgErrorPosition::Internal { query, .. }) = pg.position() {
            text.push_str(&format!("QUERY:  {query}\n"));
        }
        if let Some(v) = pg.r#where() {
            text.push_str(&format!("CONTEXT:  {v}\n"));
        }
        text.push_str(&format!("\n[SQL: {}]", self.sql));
        if !self.params.is_empty() {
            text.push_str(&format!("\n[parameters: {}]", self.params_repr()));
        }
        text.push_str(&format!(
            "\n(Background on this error at: https://sqlalche.me/e/20/{code})"
        ));
        text
    }

    pub async fn fetch_all(&self, conn: &mut sqlx::PgConnection) -> Result<Vec<PgRow>, String> {
        let literal = self.literal()?;
        // the simple query protocol, as psycopg2: no prepared statement a pooler could lose
        // (a bare &str: sqlx::raw_sql's generic future fails the handlers' Send check)
        sqlx::Executor::fetch_all(&mut *conn, literal.as_str())
            .await
            .map_err(|e| self.error_text(&literal, &e))
    }

    /// Rows affected.
    pub async fn execute(&self, conn: &mut sqlx::PgConnection) -> Result<u64, String> {
        let literal = self.literal()?;
        // the simple query protocol, as psycopg2: no prepared statement a pooler could lose
        sqlx::Executor::execute(&mut *conn, literal.as_str())
            .await
            .map(|r| r.rows_affected())
            .map_err(|e| self.error_text(&literal, &e))
    }
}

/// libpq's `reportErrorPosition` (fe-protocol3.c): the `LINE n:` excerpt, cut to 60 screen
/// columns around the 1-based character position, and the caret line.
fn error_position(msg: &mut String, query: &str, loc: usize) {
    const DISPLAY_SIZE: usize = 60;
    const MIN_RIGHT_CUT: usize = 10;
    if loc == 0 {
        return;
    }
    let loc = loc - 1;
    let chars: Vec<char> = query
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    let mut scridx: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    let (mut scroffset, mut loc_line, mut ibeg) = (0usize, 1usize, 0usize);
    let mut iend: Option<usize> = None;
    for (cno, &ch) in chars.iter().enumerate() {
        scridx.push(scroffset);
        if ch == '\r' || ch == '\n' {
            if cno < loc {
                if ch == '\r' || cno == 0 || chars[cno - 1] != '\r' {
                    loc_line += 1;
                }
                ibeg = cno + 1;
            } else {
                iend = Some(cno);
                break;
            }
        }
        scroffset += display_width(ch);
    }
    let mut iend = match iend {
        Some(i) => i,
        None => {
            scridx.push(scroffset);
            chars.len()
        }
    };
    if loc > iend {
        return; // libpq prints nothing for a position past the query
    }
    let (mut beg_trunc, mut end_trunc) = (false, false);
    if scridx[iend] - scridx[ibeg] > DISPLAY_SIZE {
        if scridx[ibeg] + DISPLAY_SIZE >= scridx[loc] + MIN_RIGHT_CUT {
            while scridx[iend] - scridx[ibeg] > DISPLAY_SIZE {
                iend -= 1;
            }
            end_trunc = true;
        } else {
            while scridx[loc] + MIN_RIGHT_CUT < scridx[iend] {
                iend -= 1;
                end_trunc = true;
            }
            while scridx[iend] - scridx[ibeg] > DISPLAY_SIZE {
                ibeg += 1;
                beg_trunc = true;
            }
        }
    }
    let mut prefix = format!("LINE {loc_line}: ");
    if beg_trunc {
        prefix.push_str("...");
    }
    let caret = prefix.chars().count() + scridx[loc] - scridx[ibeg];
    msg.push_str(&prefix);
    msg.extend(&chars[ibeg..iend]);
    if end_trunc {
        msg.push_str("...");
    }
    msg.push('\n');
    msg.push_str(&" ".repeat(caret));
    msg.push_str("^\n");
}

/// `pg_encoding_dsplen` for UTF-8: East Asian wide characters take two columns, every other
/// character one (control and combining characters count as one, as libpq treats `w <= 0`).
fn display_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x2FFFD | 0x30000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_match_psycopg2() {
        let s = Stmt::new("x = %(a)s AND y LIKE %(b)s AND z = '%%'")
            .param("a", Lit::Int(-1))
            .param("b", Lit::Str("it's".into()));
        assert_eq!(
            s.literal().unwrap(),
            "x =  -1 AND y LIKE 'it''s' AND z = '%'"
        );
        assert_eq!(
            Lit::from_json(&serde_json::json!({"a": 1})).sql().unwrap(),
            "hstore(ARRAY['a'], ARRAY[1])"
        );
        assert_eq!(
            Lit::from_json(&serde_json::json!(["a"])).sql().unwrap(),
            "ARRAY['a']"
        );
        assert_eq!(Lit::Float(13.0).sql().unwrap(), "13.0");
    }

    #[test]
    fn error_position_matches_libpq() {
        let q = "SELECT a\nFROM t\nWHERE tbl_organization_types.id = hstore(ARRAY['a'], ARRAY[1]) \n LIMIT 1";
        let mut m = String::new();
        error_position(&mut m, q, 51);
        assert_eq!(
            m,
            "LINE 3: WHERE tbl_organization_types.id = hstore(ARRAY['a'], ARRAY[1...\n                                          ^\n"
        );
    }
}
