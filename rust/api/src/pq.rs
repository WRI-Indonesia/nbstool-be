//! `sqlx::query`/`query_as`/`query_scalar` look-alikes that never prepare a statement: the `$n`
//! binds are inlined as typed literals (`'5'::int4`, `'x'::text`, the type sqlx would have
//! declared in its Parse) and the text goes over the simple query protocol, as psycopg2 sends
//! everything. A connection pooler in transaction mode (the GIS database answers 26000
//! "prepared statement does not exist" to sqlx's named statements under concurrency) can then
//! hand each statement to any server connection.

use futures_util::future::BoxFuture;
use sqlx::postgres::{PgConnection, PgPool, PgQueryResult, PgRow};
use sqlx::{Decode, Executor, FromRow, Postgres, Row, Type};

/// A value as sqlx would bind it, written as a typed SQL literal.
pub trait Arg {
    /// The Postgres type sqlx declares for it (`NULL::<type>` keeps it for a `None`).
    const TYPE: &'static str;
    fn sql(&self) -> Result<String, String>;
}

fn quote(s: &str) -> Result<String, String> {
    if s.contains('\0') {
        // what Postgres answers when the bound text holds NUL
        return Err("invalid byte sequence for encoding \"UTF8\": 0x00".into());
    }
    Ok(format!("'{}'", s.replace('\'', "''")))
}

fn typed(text: &str, ty: &str) -> Result<String, String> {
    Ok(format!("{}::{ty}", quote(text)?))
}

impl<T: Arg + ?Sized> Arg for &T {
    const TYPE: &'static str = T::TYPE;
    fn sql(&self) -> Result<String, String> {
        (**self).sql()
    }
}

impl<T: Arg> Arg for Option<T> {
    const TYPE: &'static str = T::TYPE;
    fn sql(&self) -> Result<String, String> {
        match self {
            Some(v) => v.sql(),
            None => Ok(format!("NULL::{}", T::TYPE)),
        }
    }
}

macro_rules! display_arg {
    ($($t:ty => $pg:literal),* $(,)?) => {$(
        impl Arg for $t {
            const TYPE: &'static str = $pg;
            fn sql(&self) -> Result<String, String> {
                typed(&self.to_string(), $pg)
            }
        }
    )*};
}
display_arg!(i16 => "int2", i32 => "int4", i64 => "int8", str => "text", String => "text",
             chrono::NaiveDate => "date");

/// A string inlined as psycopg2 does: quoted, no cast (Postgres resolves it from the context).
pub struct Untyped<'s>(pub &'s str);

impl Arg for Untyped<'_> {
    const TYPE: &'static str = "unknown";
    fn sql(&self) -> Result<String, String> {
        quote(self.0)
    }
}

impl Arg for bool {
    const TYPE: &'static str = "bool";
    fn sql(&self) -> Result<String, String> {
        Ok(if *self { "true" } else { "false" }.into())
    }
}

fn float(f: f64, ty: &str) -> Result<String, String> {
    let text = if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else {
        // shortest round-trip digits: the float Postgres parses is the one sqlx would send
        crate::core::py_float_repr(f)
    };
    typed(&text, ty)
}

impl Arg for f64 {
    const TYPE: &'static str = "float8";
    fn sql(&self) -> Result<String, String> {
        float(*self, "float8")
    }
}

impl Arg for f32 {
    const TYPE: &'static str = "float4";
    fn sql(&self) -> Result<String, String> {
        // f32's own shortest digits (the f64 widening would add digits float4 cannot keep)
        if self.is_finite() {
            typed(&format!("{self:?}"), "float4")
        } else {
            float(*self as f64, "float4")
        }
    }
}

impl Arg for chrono::NaiveDateTime {
    const TYPE: &'static str = "timestamp";
    fn sql(&self) -> Result<String, String> {
        // sqlx sends whole microseconds
        typed(
            &self.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
            "timestamp",
        )
    }
}

impl Arg for chrono::DateTime<chrono::Utc> {
    const TYPE: &'static str = "timestamptz";
    fn sql(&self) -> Result<String, String> {
        typed(
            &self.format("%Y-%m-%d %H:%M:%S%.6f+00").to_string(),
            "timestamptz",
        )
    }
}

impl<T: serde::Serialize> Arg for sqlx::types::Json<T> {
    const TYPE: &'static str = "jsonb";
    fn sql(&self) -> Result<String, String> {
        typed(
            &serde_json::to_string(&self.0).map_err(|e| e.to_string())?,
            "jsonb",
        )
    }
}

impl Arg for serde_json::Value {
    const TYPE: &'static str = "jsonb";
    fn sql(&self) -> Result<String, String> {
        typed(
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
            "jsonb",
        )
    }
}

impl Arg for [u8] {
    const TYPE: &'static str = "bytea";
    fn sql(&self) -> Result<String, String> {
        let hex: String = self.iter().map(|b| format!("{b:02x}")).collect();
        Ok(format!("'\\x{hex}'::bytea"))
    }
}

impl Arg for Vec<u8> {
    const TYPE: &'static str = "bytea";
    fn sql(&self) -> Result<String, String> {
        self.as_slice().sql()
    }
}

/// Element types of the array binds (`= any($1)`).
pub trait Elem: Arg {}
impl Elem for String {}
impl Elem for &str {}
impl Elem for i32 {}
impl Elem for i64 {}

impl<T: Elem> Arg for [T] {
    const TYPE: &'static str = "unknown[]"; // replaced below: the element type decides
    fn sql(&self) -> Result<String, String> {
        let ty = format!("{}[]", T::TYPE);
        if self.is_empty() {
            return Ok(format!("'{{}}'::{ty}"));
        }
        let items: Vec<String> = self.iter().map(Arg::sql).collect::<Result<_, _>>()?;
        Ok(format!("ARRAY[{}]::{ty}", items.join(",")))
    }
}

impl<T: Elem> Arg for Vec<T> {
    const TYPE: &'static str = "unknown[]";
    fn sql(&self) -> Result<String, String> {
        self.as_slice().sql()
    }
}

/// `$n` replaced by the n-th literal; quoted text, quoted identifiers and comments are copied.
fn render(sql: &str, args: &[Result<String, String>]) -> Result<String, String> {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 16 * args.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1; // a doubled quote is two quoted runs back to back
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            b'$' if b.get(i + 1).is_some_and(u8::is_ascii_digit)
                && !(i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let end = (i + 1..b.len())
                    .find(|&j| !b[j].is_ascii_digit())
                    .unwrap_or(b.len());
                let n: usize = sql[i + 1..end]
                    .parse()
                    .map_err(|_| "bad placeholder".to_string())?;
                let lit = args
                    .get(n.wrapping_sub(1))
                    .ok_or_else(|| format!("no value bound for ${n}"))?
                    .clone()?;
                out.push_str(&sql[copied..i]);
                out.push_str(&lit);
                i = end;
                copied = end;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[copied.min(b.len())..]);
    Ok(out)
}

/// A statement and its binds; `fetch_*`/`execute` render it and run it unprepared on a pool or a
/// connection (`&mut *tx`): the rendered text lives inside the future, so it is only lent to a
/// connection borrowed there.
pub struct Query {
    sql: String,
    args: Vec<Result<String, String>>,
}

pub fn query(sql: &str) -> Query {
    Query {
        sql: sql.to_string(),
        args: Vec::new(),
    }
}

type Fut<'a, T> = BoxFuture<'a, Result<T, sqlx::Error>>;

/// Where a statement runs: the pool or a connection (`&mut *tx`). A concrete type, not
/// `Executor`/`Acquire`: their free lifetimes make the handlers' futures fail the `Send` check.
pub enum On<'a> {
    Pool(&'a PgPool),
    Conn(&'a mut PgConnection),
}

impl<'a> From<&'a PgPool> for On<'a> {
    fn from(pool: &'a PgPool) -> Self {
        On::Pool(pool)
    }
}

impl<'a> From<&'a mut PgConnection> for On<'a> {
    fn from(conn: &'a mut PgConnection) -> Self {
        On::Conn(conn)
    }
}

macro_rules! run {
    ($self:ident, $on:ident, $method:ident) => {{
        let text = render(&$self.sql, &$self.args).map_err(|e| sqlx::Error::Encode(e.into()));
        let on: On<'a> = $on.into();
        Box::pin(async move {
            let text = text?;
            match on {
                On::Pool(pool) => {
                    let mut conn = pool.acquire().await?;
                    (&mut *conn).$method(text.as_str()).await
                }
                On::Conn(conn) => (&mut *conn).$method(text.as_str()).await,
            }
        })
    }};
}

impl Query {
    pub fn bind<T: Arg>(mut self, value: T) -> Self {
        self.args.push(value.sql());
        self
    }

    pub fn fetch_all<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Vec<PgRow>> {
        run!(self, a, fetch_all)
    }

    pub fn fetch_optional<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Option<PgRow>> {
        run!(self, a, fetch_optional)
    }

    pub fn fetch_one<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, PgRow> {
        run!(self, a, fetch_one)
    }

    pub fn execute<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, PgQueryResult> {
        run!(self, a, execute)
    }
}

/// `sqlx::query_as`: each row through `FromRow`.
pub struct QueryAs<O>(Query, std::marker::PhantomData<fn() -> O>);

pub fn query_as<O>(sql: &str) -> QueryAs<O> {
    QueryAs(query(sql), std::marker::PhantomData)
}

impl<O: for<'r> FromRow<'r, PgRow> + Send + 'static> QueryAs<O> {
    pub fn bind<T: Arg>(self, value: T) -> Self {
        QueryAs(self.0.bind(value), self.1)
    }

    pub fn fetch_all<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Vec<O>> {
        let rows = self.0.fetch_all(a);
        Box::pin(async move { rows.await?.iter().map(O::from_row).collect() })
    }

    pub fn fetch_optional<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Option<O>> {
        let row = self.0.fetch_optional(a);
        Box::pin(async move { row.await?.as_ref().map(O::from_row).transpose() })
    }
}

/// `sqlx::query_scalar`: the first column of each row.
pub struct QueryScalar<O>(Query, std::marker::PhantomData<fn() -> O>);

pub fn query_scalar<O>(sql: &str) -> QueryScalar<O> {
    QueryScalar(query(sql), std::marker::PhantomData)
}

impl<O: for<'r> Decode<'r, Postgres> + Type<Postgres> + Send + 'static> QueryScalar<O> {
    pub fn bind<T: Arg>(self, value: T) -> Self {
        QueryScalar(self.0.bind(value), self.1)
    }

    pub fn fetch_all<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Vec<O>> {
        let rows = self.0.fetch_all(a);
        Box::pin(async move { rows.await?.iter().map(|r| r.try_get(0)).collect() })
    }

    pub fn fetch_optional<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, Option<O>> {
        let row = self.0.fetch_optional(a);
        Box::pin(async move { row.await?.as_ref().map(|r| r.try_get(0)).transpose() })
    }

    #[allow(dead_code)] // the tests use it
    pub fn fetch_one<'a>(self, a: impl Into<On<'a>>) -> Fut<'a, O> {
        let row = self.0.fetch_one(a);
        Box::pin(async move { row.await?.try_get(0) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(sql: &str, args: Vec<Result<String, String>>) -> String {
        render(sql, &args).unwrap()
    }

    #[test]
    fn binds_become_typed_literals() {
        assert_eq!(
            r(
                "select '$1', \"a$1\", x from t where a = $1 and b = $2 -- $3\n and c=$10",
                vec![
                    Ok(5i32.sql().unwrap()),
                    "it's".sql(),
                    Ok("x".into()),
                    Ok("".into()),
                    Ok("".into()),
                    Ok("".into()),
                    Ok("".into()),
                    Ok("".into()),
                    Ok("".into()),
                    Ok("true".into())
                ]
            ),
            "select '$1', \"a$1\", x from t where a = '5'::int4 and b = 'it''s'::text -- $3\n and c=true"
        );
        assert_eq!(None::<i32>.sql().unwrap(), "NULL::int4");
        assert_eq!(
            vec!["a".to_string()].sql().unwrap(),
            "ARRAY['a'::text]::text[]"
        );
        assert_eq!(Vec::<i64>::new().sql().unwrap(), "'{}'::int8[]");
        assert_eq!(vec![1u8, 255].sql().unwrap(), "'\\x01ff'::bytea");
        assert_eq!(0.1f64.sql().unwrap(), "'0.1'::float8");
        assert!(render("x = $2", &[Ok("1".into())]).is_err());
    }
}
