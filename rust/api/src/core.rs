//! What every route shares: settings, the two database pools, Flask-shaped JSON, the response
//! envelope of `utils/common/handler.py`, and the Bearer login of `user_apis/routes.py`.

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::sync::OnceCell;

pub struct Ctx {
    /// The app database (`DB_SQLALCHEMY_URI`), Flask-SQLAlchemy's `db.session`.
    pub app: PgPool,
    /// The GIS database (`GIS_DB_SQLALCHEMY_URI`), `GeoUtils.get_db`.
    pub gis: PgPool,
    pub secret: String,
    /// `ENV` of the config class: `development`, `uat` or `production`.
    pub env: &'static str,
}

static CTX: OnceCell<Ctx> = OnceCell::const_new();

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn pool(uri: &str, max: u32, statement_timeout_ms: Option<u32>) -> PgPool {
    // SQLAlchemy names the driver in the scheme; sqlx does not know `+psycopg2`.
    let uri = uri.replacen("postgresql+psycopg2://", "postgresql://", 1);
    let mut options = PgPoolOptions::new().max_connections(max);
    if let Some(ms) = statement_timeout_ms {
        options = options.after_connect(move |conn, _| {
            Box::pin(async move {
                // a bare &str goes over the simple query protocol (no prepared statement)
                sqlx::Executor::execute(conn, format!("SET statement_timeout = {ms}").as_str())
                    .await
                    .map(|_| ())
            })
        });
    }
    // INFALLIBLE: connect_lazy only fails on a URI that cannot be parsed, which the original
    // also cannot start with.
    options.connect_lazy(&uri).expect("a database URI")
}

pub async fn ctx() -> &'static Ctx {
    CTX.get_or_init(|| async {
        // config.py loads `.env` beside it without overriding the process environment; the
        // server runs in that folder (rust/app's main enters NBSTOOL_HOME first)
        load_dotenv(".env");
        let env = match var("CONFIGURATION_SETUP").as_str() {
            "config.DevelopmentConfig" => "development",
            "config.UatConfig" => "uat",
            _ => "production",
        };
        Ctx {
            // pool sizes as config.py and GeoUtils set them
            app: pool(&var("DB_SQLALCHEMY_URI"), 15, None),
            gis: pool(&var("GIS_DB_SQLALCHEMY_URI"), 4, Some(30_000)),
            secret: var("SECRET_KEY"),
            env,
        }
    })
    .await
}

/// python-dotenv 1.x `dotenv_values` for a `.env` file: `[export ]KEY=VALUE` bindings, `#`
/// comments, single-quoted values literal (only `\'` and `\\` decoded), double-quoted values with
/// python-dotenv's escapes only (`\\ \' \" \a \b \f \n \r \t \v`, so a Windows path's `\U` stays),
/// unquoted values cut at ` #` and trimmed. A binding it cannot read is skipped and the next one
/// read, as python-dotenv does (dotenvy stops at the first).
// shortcut: no `${VAR}` interpolation (python-dotenv expands it in unquoted and double-quoted
// values); the .env files in use have none.
pub fn dotenv_parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return out;
        }
        let line_end = rest.find('\n').unwrap_or(rest.len());
        if rest.starts_with('#') {
            rest = &rest[line_end..];
            continue;
        }
        let s = rest.strip_prefix("export ").map_or(rest, str::trim_start);
        let key_len = s
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(s.len());
        let key = &s[..key_len];
        let after_key = s[key_len..].trim_start_matches([' ', '\t']);
        let Some(v) = after_key.strip_prefix('=').filter(|_| !key.is_empty()) else {
            rest = &rest[line_end..];
            continue;
        };
        let v = v.trim_start_matches([' ', '\t']);
        let parsed = match v.chars().next() {
            Some('\'') => dotenv_quoted(v, '\'', "\\'"),
            Some('"') => dotenv_quoted(v, '"', "\\'\"abfnrtv"),
            _ => {
                let end = v.find(['\r', '\n']).unwrap_or(v.len());
                let raw = &v[..end];
                let raw = raw
                    .find(" #")
                    .or_else(|| raw.find("\t#"))
                    .map_or(raw, |i| &raw[..i]);
                Some((raw.trim_end().to_string(), end))
            }
        };
        match parsed {
            Some((value, used)) => {
                out.push((key.to_string(), value));
                let tail = &v[used..];
                rest = &tail[tail.find('\n').unwrap_or(tail.len())..];
            }
            None => rest = &rest[line_end..],
        }
    }
}

/// A quoted value starting at `v[0]`: (decoded value, bytes used including both quotes).
fn dotenv_quoted(v: &str, q: char, escapes: &str) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut chars = v.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            let (_, e) = chars.next()?;
            if escapes.contains(e) {
                value.push(match e {
                    'a' => '\u{7}',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'v' => '\u{b}',
                    e => e,
                });
            } else {
                value.push('\\');
                value.push(e);
            }
        } else if c == q {
            return Some((value, i + 1));
        } else {
            value.push(c);
        }
    }
    None
}

/// `load_dotenv(path)` (no override): dotenvy sets the variables, fed only lines it reads the
/// same way (each value re-quoted with dotenvy's own escapes).
pub fn load_dotenv(path: &str) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut requoted = String::new();
    for (key, value) in dotenv_parse(&text) {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        requoted.push_str(&format!("{key}=\"{escaped}\"\n"));
    }
    let _ = dotenvy::from_read(requoted.as_bytes());
}

/// A request read in full: its head and its body. Handlers hold it across awaits, which the
/// streaming `Request` does not allow (its body is not `Sync`).
pub struct Req {
    pub parts: Parts,
    pub body: Bytes,
}

impl Req {
    #[expect(
        clippy::result_large_err,
        reason = "the error is the finished axum Response, as in every handler"
    )]
    pub async fn read(req: Request) -> Result<Req, Response> {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, usize::MAX).await {
            Ok(body) => Ok(Req { parts, body }),
            Err(_) => Err(respond(
                400,
                "text/html; charset=utf-8",
                "unreadable request body",
            )),
        }
    }
}

/// `request.args.get(name)`: the first value of the query parameter.
pub fn arg(req: &Req, name: &str) -> Option<String> {
    form_urlencoded::parse(req.parts.uri.query().unwrap_or("").as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

// ---- JSON as Flask's `jsonify` writes it: sorted keys, compact, ASCII only, Python floats ----

/// Python's `repr(float)`: shortest digits, exponent form below 1e-4 and from 1e16.
fn py_float(f: f64, out: &mut String) {
    let sci = format!("{f:e}"); // shortest round-trip digits, e.g. `1.5e-7`
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

/// Rust's shortest digits round an exact tie up (27.5026397705078125 -> `...813`); Python's dtoa
/// takes the even neighbour (`...812`) when both round-trip.
fn tie_to_even(f: f64, digits: String, exp: i32) -> String {
    let n = digits.len();
    let last = digits.as_bytes()[n - 1] - b'0';
    if last.is_multiple_of(2) {
        return digits;
    }
    // cheap filter first, then the exact expansion: is f exactly halfway at digit n+1?
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
    // the tie sits between `lower` and `lower + 1`; Python keeps the even one if it round-trips
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
                // ensure_ascii: UTF-16 units as \uXXXX
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

/// `sort`: Flask's `sort_keys=True` (compact separators); otherwise Python's `json.dumps(v)`
/// defaults: insertion order, `", "` and `": "` separators.
fn write_json_as(v: &Value, out: &mut String, sort: bool) {
    let (item_sep, key_sep) = if sort { (",", ":") } else { (", ", ": ") };
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
                    out.push_str(item_sep);
                }
                write_json_as(item, out, sort);
            }
            out.push(']');
        }
        // Flask's `sort_keys=True`: sorted by code point (= UTF-8 byte order), whatever order the
        // map keeps (serde_json `preserve_order` is on once rust/geo is linked).
        Value::Object(map) => {
            out.push('{');
            let mut entries: Vec<_> = map.iter().collect();
            if sort {
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            }
            for (i, (k, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                py_str(k, out);
                out.push_str(key_sep);
                write_json_as(item, out, sort);
            }
            out.push('}');
        }
    }
}

/// Python's `repr(float)` for a finite float.
pub fn py_float_repr(f: f64) -> String {
    let mut out = String::new();
    py_float(f, &mut out);
    out
}

/// Python 3.12's `sum(values)` (start 0) over ints, bools and floats: exact ints until the first
/// float joins (`int + float`), then Neumaier-compensated float adds (ints added plainly), the
/// compensation applied at the end. `None` when an item is not a number (the caller raises).
pub fn py_sum_numbers<'a>(values: impl IntoIterator<Item = &'a Value>) -> Option<Value> {
    let (mut int, mut float, mut c): (i64, Option<f64>, f64) = (0, None, 0.0);
    for v in values {
        let i = match v {
            Value::Bool(b) => Some(*b as i64),
            Value::Number(n) if !n.is_f64() => Some(n.as_i64()?),
            Value::Number(_) => None,
            _ => return None,
        };
        match (i, float) {
            (Some(i), None) => int = int.checked_add(i)?,
            (Some(i), Some(f)) => float = Some(f + i as f64),
            (None, None) => float = Some(int as f64 + v.as_f64()?),
            (None, Some(f)) => {
                let x = v.as_f64()?;
                let t = f + x;
                c += if f.abs() >= x.abs() {
                    (f - t) + x
                } else {
                    (x - t) + f
                };
                float = Some(t);
            }
        }
    }
    Some(match float {
        Some(f) if c != 0.0 && c.is_finite() => json!(f + c),
        Some(f) => json!(f),
        None => json!(int),
    })
}

/// werkzeug's `BadRequest` text when `get_json` fails outside debug mode (`flask run`).
pub const BAD_REQUEST_TEXT: &str =
    "400 Bad Request: The browser (or proxy) sent a request that this server could not understand.";

/// `utils.common.get_date()`: naive UTC now, microsecond precision.
pub fn get_date() -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros())
        .map(|d| d.naive_utc())
        .unwrap_or_default()
}

fn write_json(v: &Value, out: &mut String) {
    write_json_as(v, out, true)
}

/// Python's `json.dumps(v)` with its defaults (what client libraries send as a request body).
pub fn py_json_dumps(v: &Value) -> String {
    let mut out = String::new();
    write_json_as(v, &mut out, false);
    out
}

pub fn to_flask_json(v: &Value) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out.push('\n');
    out
}

/// Python's `datetime.isoformat()`: microseconds only when they are not zero.
pub fn py_isoformat(d: chrono::NaiveDateTime) -> String {
    if d.and_utc().timestamp_subsec_micros() == 0 {
        d.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        d.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

// ---- responses ----

/// `uuid.uuid4().hex[:12]`, the `X-Request-Id` every response carries.
fn request_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// A response with the headers Flask adds to every answer (flask-cors, `after_request`).
pub fn respond(status: u16, content_type: &'static str, body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if let Ok(id) = HeaderValue::from_str(&request_id()) {
        headers.insert("x-request-id", id);
    }
    response
}

/// werkzeug's NotFound page.
pub const NOT_FOUND_PAGE: &str = "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n\
<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL \
manually please check your spelling and try again.</p>\n";

/// `make_response(jsonify(value), status)`.
pub fn json(status: u16, value: &Value) -> Response {
    respond(status, "application/json", to_flask_json(value))
}

/// Sets one more header on a response (`Cache-Control`, `Retry-After`, ...).
pub fn with_header(mut response: Response, name: &'static str, value: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
    response
}

/// Flask-Login touches the session when a route reads `current_user`, which adds this header.
/// `settle_vary` takes it off again when the request loader found a user row.
pub fn vary_cookie(response: Response) -> Response {
    with_header(response, "vary", "Cookie")
}

/// Same as `vary_cookie` (kept for the call sites that name the rule).
pub async fn vary_if_anonymous(_req: &Req, response: Response) -> Response {
    vary_cookie(response)
}

/// Set on the request by the dispatcher; `current_user` marks it when the Bearer token loads a
/// `tbl_users` row (active or not).
#[derive(Clone, Default)]
pub struct UserLoaded(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

/// run.py's `CustomSessionInterface`: once the request loader returned a user row (active or
/// not), `g.login_via_header` skips `save_session`, so the answer carries neither `Vary: Cookie`
/// nor a session cookie.
pub fn settle_vary(mut response: Response, loaded: &UserLoaded) -> Response {
    if loaded.0.load(std::sync::atomic::Ordering::Relaxed) {
        response.headers_mut().remove(header::VARY);
        response.headers_mut().remove(header::SET_COOKIE);
    }
    response
}

/// flask-cors (`CORS(app)`, origins `*`, not send_wildcard): a request with an `Origin` gets it
/// echoed in `Access-Control-Allow-Origin` and `Origin` first in `Vary` (flask-cors runs before
/// the session adds `Cookie`).
pub fn cors(mut response: Response, origin: Option<&HeaderValue>) -> Response {
    let Some(origin) = origin else {
        return response;
    };
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    let vary = match headers.get(header::VARY).and_then(|v| v.to_str().ok()) {
        Some(existing) if !existing.is_empty() => format!("Origin, {existing}"),
        _ => "Origin".to_string(),
    };
    if let Ok(v) = HeaderValue::from_str(&vary) {
        headers.insert(header::VARY, v);
    }
    response
}

/// `success_handler(results, status_code, message)`.
pub fn success(results: Value, status_code: Option<u16>, message: Option<&str>) -> Value {
    let mut context = match results {
        Value::Object(map) if !map.is_empty() => Value::Object(map),
        Value::Array(items) if !items.is_empty() => Value::Array(items),
        // falsy results: the original starts from an empty dict
        _ => Value::Object(Map::new()),
    };
    if let Value::Object(map) = &mut context {
        if let Some(message) = message {
            map.insert("message".into(), json!(message));
        }
        if let Some(code) = status_code {
            map.insert("status_code".into(), json!(code));
        }
    }
    context
}

/// `app_exception_handler(AppMessageException(message), status_code)`: the message is shown.
pub fn app_error(message: &str, status_code: Option<u16>) -> Value {
    let mut context = json!({ "message": message, "success": false });
    if let Some(code) = status_code {
        context["status_code"] = json!(code);
    }
    context
}

/// `app_exception_handler(e, status_code)` for anything that is not an `AppMessageException`
/// (a plain string such as `'not logged in'` too): shown in development, redacted elsewhere.
pub async fn other_error(message: &str, status_code: Option<u16>) -> Value {
    if ctx().await.env == "development" {
        app_error(message, status_code)
    } else {
        app_error("-- prod redacted, please contact admin --", status_code)
    }
}

/// The 401 every login-only route answers: `app_exception_handler('not logged in', 401)`.
pub async fn not_logged_in() -> Response {
    vary_cookie(json(401, &other_error("not logged in", Some(401)).await))
}

/// A database failure inside a route's `except Exception`: 500 with the envelope.
pub async fn db_error(e: sqlx::Error) -> Response {
    json(500, &other_error(&e.to_string(), None).await)
}

// ---- login ----

/// The columns of `tbl_users` routes read from `current_user`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i32,
    pub email: String,
    pub name: String,
    pub organization_type_id: Option<i32>,
    pub organization_name: String,
    pub is_active: Option<bool>,
    pub permission_policy: Option<i32>,
    pub size_limit: Option<f64>,
    pub assistance_eligible: bool,
}

/// `current_user` when `current_user.is_authenticated`: `load_user_from_request`, then
/// Flask-Login's rule that an inactive user is not authenticated.
pub async fn current_user(req: &Req) -> Result<Option<User>, sqlx::Error> {
    let Some(value) = req
        .parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let token = value.strip_prefix("Bearer ").unwrap_or(value);
    let ctx = ctx().await;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.leeway = 0;
    validation.required_spec_claims.clear();
    let key = jsonwebtoken::DecodingKey::from_secret(ctx.secret.as_bytes());
    let Ok(data) = jsonwebtoken::decode::<Value>(token, &key, &validation) else {
        return Ok(None);
    };
    let Some(user_id) = data.claims.get("user_id").and_then(Value::as_i64) else {
        return Ok(None);
    };
    let user: Option<User> = crate::pq::query_as(
        "select id, email, name, organization_type_id, organization_name, is_active, \
         permission_policy, size_limit, assistance_eligible from tbl_users where id = $1",
    )
    .bind(user_id as i32)
    .fetch_optional(&ctx.app)
    .await?;
    if user.is_some()
        && let Some(loaded) = req.parts.extensions.get::<UserLoaded>()
    {
        loaded.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(user.filter(|u| u.is_active == Some(true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f64) -> String {
        let mut s = String::new();
        py_float(x, &mut s);
        s
    }

    #[test]
    fn floats_print_as_python_repr() {
        assert_eq!(f(817216.98), "817216.98");
        assert_eq!(f(1.0), "1.0");
        assert_eq!(f(0.1), "0.1");
        assert_eq!(f(-2.5), "-2.5");
        assert_eq!(f(0.0001), "0.0001");
        assert_eq!(f(0.00001), "1e-05");
        assert_eq!(f(1.5e-7), "1.5e-07");
        assert_eq!(f(1e16), "1e+16");
        assert_eq!(f(1.25e22), "1.25e+22");
        assert_eq!(f(123456789012345.0), "123456789012345.0");
        assert_eq!(f(0.0), "0.0");
        // exact tie (f32 value): Python's repr takes the even neighbour
        assert_eq!(f(27.502639770507812), "27.502639770507812");
        assert_eq!(f(-27.502639770507812), "-27.502639770507812");
    }

    #[test]
    fn sum_is_python_312_sum() {
        // sum() of these in CPython 3.12.3 (a plain left fold gives ...8749868665)
        let v = json!([
            80602.69569622053,
            77307.48287358867,
            77307.48287358867,
            77307.48287358867,
            4880.802364945064,
            978.6193333432166,
            5935.222864330654,
            85783.08610726098
        ]);
        let items = v.as_array().unwrap();
        assert_eq!(
            py_float_repr(py_sum_numbers(items).unwrap().as_f64().unwrap()),
            "410102.87498686643"
        );
        assert_eq!(py_sum_numbers(&[json!(1), json!(true)]), Some(json!(2)));
        assert_eq!(py_sum_numbers(&[json!("a")]), None);
    }

    #[test]
    fn json_is_sorted_compact_and_ascii() {
        let v = json!({"b": [1, 2.0, null, true], "a": "é😀\"\n"});
        assert_eq!(
            to_flask_json(&v),
            "{\"a\":\"\\u00e9\\ud83d\\ude00\\\"\\n\",\"b\":[1,2.0,null,true]}\n"
        );
    }

    #[test]
    fn json_keys_sort_whatever_the_map_order() {
        let mut inner = Map::new();
        inner.insert("z".into(), json!(1));
        inner.insert("é".into(), json!(2));
        inner.insert("A".into(), json!(3));
        let mut outer = Map::new();
        outer.insert("b".into(), Value::Array(vec![Value::Object(inner.clone())]));
        outer.insert("a".into(), Value::Object(inner));
        assert_eq!(
            to_flask_json(&Value::Object(outer)),
            r#"{"a":{"A":3,"z":1,"\u00e9":2},"b":[{"A":3,"z":1,"\u00e9":2}]}"#.to_string() + "\n"
        );
    }

    #[test]
    fn dotenv_reads_as_python_dotenv() {
        let text = "# c\nA=\"C:\\Users\\x\\n\"\n\nexport B = 'it\\'s \\n' # c\nC=plain value # note\nbad line\nD=\"q\\\"x\"\nE=\n=nokey\nF=\"unterminated\nG=last";
        assert_eq!(
            dotenv_parse(text),
            vec![
                ("A".to_string(), "C:\\Users\\x\n".to_string()),
                ("B".to_string(), "it's \\n".to_string()),
                ("C".to_string(), "plain value".to_string()),
                ("D".to_string(), "q\"x".to_string()),
                ("E".to_string(), String::new()),
                ("G".to_string(), "last".to_string()),
            ]
        );
    }

    #[test]
    fn envelopes_match_handler_py() {
        assert_eq!(
            success(json!({}), Some(200), None),
            json!({"status_code": 200})
        );
        assert_eq!(success(json!([1]), Some(200), Some("m")), json!([1]));
        assert_eq!(
            app_error("x", Some(401)),
            json!({"message": "x", "success": false, "status_code": 401})
        );
    }
}
