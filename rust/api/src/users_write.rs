//! The write routes of `user_apis/routes.py`: profile save, register, verification (resend and
//! token), login, forgot/reset/change password and the broader-area request. Not wired: the
//! original still answers these routes until they are recorded against a scratch database.
//!
//! Transactions follow Flask-SQLAlchemy's session: one transaction opens at the first statement
//! and commits where the Python commits; every early return drops it (rollback, as the request
//! teardown does). Statements the Python issues after its commit (mail settings, token settings)
//! run on the pool, as the session's next implicit transaction which is never committed.

use std::collections::HashMap;
use std::time::Duration;

use axum::http::header;
use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use crate::core::{self, Req, ctx};

// ---- failures: the three `except` arms of every route ----

enum Fail {
    /// `AppMessageException` -> 400, message shown.
    App(String),
    /// any other exception -> 500, message shown in development only.
    Other(String),
    Db(sqlx::Error),
    /// The view returned `None`: Flask's own 500 page, outside the route's try.
    Html500,
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

fn app(message: impl Into<String>) -> Fail {
    Fail::App(message.into())
}

fn other(message: impl Into<String>) -> Fail {
    Fail::Other(message.into())
}

/// werkzeug's `InternalServerError` body (production; debug configs propagate to gunicorn).
const FLASK_500: &str = "<!doctype html>\n<html lang=en>\n<title>500 Internal Server Error</title>\n\
     <h1>Internal Server Error</h1>\n<p>The server encountered an internal error and was \
     unable to complete your request. Either the server is overloaded or there is an error \
     in the application.</p>\n";

async fn finish(result: Result<Response, Fail>) -> Response {
    match result {
        Ok(response) => response,
        Err(Fail::App(m)) => core::json(400, &core::app_error(&m, None)),
        Err(Fail::Other(m)) => core::json(500, &core::other_error(&m, None).await),
        Err(Fail::Db(e)) => core::db_error(e).await,
        Err(Fail::Html500) => core::respond(500, "text/html; charset=utf-8", FLASK_500),
    }
}

/// `g.__request_data__ = request.get_json()` before the route's try: unset (None) when it raises.
fn pre_request_data(req: &Req) -> Value {
    if !is_json(req) {
        return Value::Null;
    }
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// The after_request activity log of a `__log_it__` route (no description data).
async fn logged(
    api: &str,
    session_id: &str,
    request_data: Value,
    user_id: Option<i32>,
    response: Response,
) -> Response {
    let entry = crate::logging::LogEntry {
        log_it: true,
        api_name: Some(api.into()),
        session_id: Some(session_id.into()),
        request_data,
        description_data: Value::Null,
        user_id,
    };
    // log_func reads current_user, so Flask-Login touches the session: Vary: Cookie
    crate::logging::log_response(&entry, core::vary_cookie(response)).await
}

fn ok(results: Value, status_code: Option<u16>, message: &str) -> Response {
    core::json(200, &core::success(results, status_code, Some(message)))
}

// ---- request.get_json() and Python value semantics ----

/// `request.is_json`: `application/json` or `application/*+json`, parameters ignored.
fn is_json(req: &Req) -> bool {
    let mimetype = content_type(req)
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"))
}

fn content_type(req: &Req) -> &str {
    req.parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// `data = request.get_json()` followed by `data.get(...)`. Not JSON content: werkzeug's 415,
/// caught by `except Exception` (500). Bad JSON: `BadRequest` (500); the text after the colon is
/// serde's, not Python's (development only). A non-dict body fails at the first `data.get`.
fn json_body(req: &Req) -> Result<Map<String, Value>, Fail> {
    if !is_json(req) {
        return Err(other(
            "415 Unsupported Media Type: Did not attempt to load JSON data because the request \
             Content-Type was not 'application/json'.",
        ));
    }
    // json.loads(bytes) accepts a UTF-8 BOM
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    // shortcut: Python's json also takes NaN/Infinity and UTF-16/32 bodies; serde rejects them
    match serde_json::from_slice(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(v) => Err(other(format!(
            "'{}' object has no attribute 'get'",
            py_type(&v)
        ))),
        Err(_) => Err(other(core::BAD_REQUEST_TEXT)), // not debug mode: generic text
    }
}

static NULL: Value = Value::Null;

/// `data.get(key)`
fn get<'a>(map: &'a Map<String, Value>, key: &str) -> &'a Value {
    map.get(key).unwrap_or(&NULL)
}

/// Python's `bool(value)` for a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `data.get(key) or ''`
fn or_empty(v: &Value) -> Value {
    if truthy(v) { v.clone() } else { json!("") }
}

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python's `repr(str)`: single quotes unless the text has `'` and no `"`.
fn py_repr_str(s: &str) -> String {
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

/// Python's `repr` of a JSON value as `json.loads` builds it.
fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => py_repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // dict keys come out sorted here (serde_json map), insertion-ordered in Python
        other => py_str(other),
    }
}

/// Python's `str(value)`.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(_) => core::to_flask_json(v).trim_end().to_string(), // float repr
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `str.title()`: a cased letter after a cased letter is lowered, any other is uppered.
/// shortcut: uses upper case where Python uses title case (only differs for digraphs like `ǆ`).
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    out
}

/// Python's `int(str)` for the plain cases: surrounding whitespace, a sign, digits with single
/// underscores between them. `Ok(None)`: valid but past i64 (a Python big int).
fn py_int(s: &str) -> Result<Option<i64>, ()> {
    let t = s.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return Err(());
    }
    Ok(t.replace('_', "").parse().ok())
}

/// `int(value)` of a JSON value. `Err(())` is the ValueError/TypeError (bare `except:` in the
/// route); `Ok(None)` an int too big for i64, which matches no row in Python either.
fn py_int_value(v: &Value) -> Result<Option<i64>, ()> {
    match v {
        Value::Bool(b) => Ok(Some(*b as i64)),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Ok(Some(i)),
            (None, Some(f)) if f.is_finite() => {
                let t = f.trunc();
                Ok((t.abs() < 9.2e18).then_some(t as i64))
            }
            _ => Err(()), // inf: OverflowError, caught by the bare except as well
        },
        Value::String(s) => py_int(s),
        _ => Err(()),
    }
}

/// The text psycopg2 inlines for a number, as Postgres reads it into `numeric`.
fn num_text(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e16 => format!("{f}.0"),
        Some(f) if n.is_f64() => format!("{f}"),
        _ => n.to_string(),
    }
}

/// How psycopg2 sends a scalar: a quoted string takes the column's type (`None` here), a number
/// is `numeric`, a bool `boolean`. Lists and dicts fail in the original too (ARRAY / can't adapt).
fn pg_literal(v: &Value) -> Result<(String, Option<&'static str>), Fail> {
    match v {
        Value::String(s) => Ok((s.clone(), None)),
        Value::Number(n) => Ok((num_text(n), Some("numeric"))),
        Value::Bool(b) => Ok((b.to_string(), Some("boolean"))),
        Value::Null => Ok((String::new(), Some("text"))), // callers pass truthy values
        other => Err(Fail::Other(format!(
            "can't adapt type '{}'",
            py_type(other)
        ))),
    }
}

/// `re.match(r'[^@]+@[^@]+\.[^@]+', email)`: anchored at the start only.
fn email_ok(s: &str) -> bool {
    let Some(at) = s.find('@') else { return false };
    if at == 0 {
        return false;
    }
    let rest = &s[at + 1..];
    let domain = rest.split('@').next().unwrap_or("");
    // some '.' with at least one character on each side, inside the run without '@'
    domain
        .char_indices()
        .any(|(i, c)| c == '.' && i > 0 && i + 1 < domain.len())
}

fn check_email(v: &Value) -> Result<(), Fail> {
    match v {
        Value::String(s) if email_ok(s) => Ok(()),
        Value::String(_) => Err(app("invalid input format: email")),
        v => Err(other(format!(
            "expected string or bytes-like object, got '{}'",
            py_type(v)
        ))),
    }
}

fn now_naive() -> chrono::NaiveDateTime {
    // get_date(): naive UTC. Columns are sa.DateTime() -> `timestamp without time zone` per the
    // migrations (not checked against the live tables).
    chrono::Utc::now().naive_utc()
}

// ---- randomness, uuid4 ----

/// `str(uuid.uuid4())` (OS RNG, as Python's os.urandom).
fn uuid4() -> Result<String, Fail> {
    Ok(uuid::Uuid::new_v4().to_string())
}

// ---- passwords: flask_bcrypt with BCRYPT_LOG_ROUNDS unset -> 12 rounds, prefix 2b ----

/// `User.encode_password`. bcrypt truncates past 72 bytes like Python bcrypt 4.2.0.
/// shortcut: bcrypt at cost 12 blocks this task's thread (~250 ms); tokio has only `sync` here.
fn hash_password(password: &Value) -> Result<String, Fail> {
    match password {
        Value::String(p) => bcrypt::hash(p, 12).map_err(|e| other(e.to_string())),
        // `_unicode_to_bytes` leaves a non-str as is and `bcrypt.hashpw` raises TypeError
        v => Err(other(format!(
            "argument 'password': '{}' object cannot be converted to 'PyBytes'",
            py_type(v)
        ))),
    }
}

/// `User.check_password`: `hashpw(password, hash) == hash`; a malformed hash raises ValueError.
fn check_password(hash: &str, password: &Value) -> Result<bool, Fail> {
    match password {
        Value::String(p) => bcrypt::verify(p, hash).map_err(|_| other("Invalid salt")),
        v => Err(other(format!(
            "argument 'password': '{}' object cannot be converted to 'PyBytes'",
            py_type(v)
        ))),
    }
}

// ---- JWT ----

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PyJWT's `base64url_decode` (strict: Python silently drops foreign characters).
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    // urlsafe_b64decode maps -_ to +/ first, so both alphabets decode; trailing bits are ignored
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    ENGINE
        .decode(s.trim_end_matches('=').replace('+', "-").replace('/', "_"))
        .ok()
}

/// `jwt.encode(payload, secret, "HS256")` with the payload already serialized the way PyJWT
/// writes it (compact, insertion order); PyJWT sorts the header keys.
fn sign_hs256(payload_json: &str, secret: &str) -> Result<String, Fail> {
    let message = format!(
        "{}.{}",
        b64url(br#"{"alg":"HS256","typ":"JWT"}"#),
        b64url(payload_json.as_bytes())
    );
    let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());
    jsonwebtoken::crypto::sign(message.as_bytes(), &key, jsonwebtoken::Algorithm::HS256)
        .map(|signature| format!("{message}.{signature}"))
        .map_err(|e| other(e.to_string()))
}

const NO_JWT_SECRET: &str =
    "JWT_SECRET_KEY or flask SECRET_KEY must be set when using symmetric algorithm \"HS256\"";

/// flask_jwt_extended 4.6.0 `create_access_token(identity=user_id, expires_delta=1 day)` with
/// the defaults this app runs (no JWT_* config): claims in `_encode_jwt`'s insertion order,
/// `nbf` on, no csrf/aud/iss, identity claim `sub`, key = SECRET_KEY.
fn activation_payload(now: i64, jti: &str, user_id: i32) -> String {
    format!(
        r#"{{"fresh":false,"iat":{now},"jti":"{jti}","type":"access","sub":{user_id},"nbf":{now},"exp":{}}}"#,
        now + 86_400
    )
}

fn create_access_token(user_id: i32, secret: &str) -> Result<String, Fail> {
    if secret.is_empty() {
        return Err(other(NO_JWT_SECRET));
    }
    let now = chrono::Utc::now().timestamp(); // timegm(utctimetuple()) drops the microseconds
    sign_hs256(&activation_payload(now, &uuid4()?, user_id), secret)
}

/// PyJWT's `int(claim)` inside `_validate_*`.
fn claim_int(v: &Value) -> Option<f64> {
    match py_int_value(v) {
        Ok(Some(i)) => Some(i as f64),
        Ok(None) => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())),
        Err(()) => None,
    }
}

/// flask_jwt_extended `decode_token(token, allow_expired=not verify_exp)` as PyJWT 2.9 checks
/// it. `Err` carries the exception text; `"Signature has expired"` is the one the route tests.
/// shortcut: texts of structural errors follow PyJWT's but not byte for byte (development only).
fn decode_token(
    token: &str,
    secret: &str,
    verify_exp: bool,
    now: f64,
) -> Result<Map<String, Value>, String> {
    if secret.is_empty() {
        return Err(NO_JWT_SECRET.into());
    }
    let (signing_input, crypto) = token.rsplit_once('.').ok_or("Not enough segments")?;
    let (header_seg, payload_seg) = signing_input.split_once('.').ok_or("Not enough segments")?;
    let header = b64url_decode(header_seg).ok_or("Invalid header padding")?;
    let header: Value =
        serde_json::from_slice(&header).map_err(|e| format!("Invalid header string: {e}"))?;
    let Value::Object(header) = header else {
        return Err("Invalid header string: must be a json object".into());
    };
    let payload = b64url_decode(payload_seg).ok_or("Invalid payload padding")?;
    b64url_decode(crypto).ok_or("Invalid crypto padding")?;
    // the unverified decode parses the payload before anything is verified
    let payload: Value =
        serde_json::from_slice(&payload).map_err(|e| format!("Invalid payload string: {e}"))?;
    let Value::Object(payload) = payload else {
        return Err("Invalid payload string: must be a json object".into());
    };
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return Err("The specified alg value is not allowed".into());
    }
    let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());
    let valid = jsonwebtoken::crypto::verify(
        crypto,
        signing_input.as_bytes(),
        &key,
        jsonwebtoken::Algorithm::HS256,
    )
    .unwrap_or(false);
    if !valid {
        return Err("Signature verification failed".into());
    }
    if let Some(iat) = payload.get("iat") {
        let iat = claim_int(iat).ok_or("Issued At claim (iat) must be an integer.")?;
        if iat > now {
            return Err("The token is not yet valid (iat)".into());
        }
    }
    if let Some(nbf) = payload.get("nbf") {
        let nbf = claim_int(nbf).ok_or("Not Before claim (nbf) must be an integer.")?;
        if nbf > now {
            return Err("The token is not yet valid (nbf)".into());
        }
    }
    if verify_exp && let Some(exp) = payload.get("exp") {
        let exp = claim_int(exp).ok_or("Expiration Time claim (exp) must be an integer.")?;
        if exp <= now {
            return Err("Signature has expired".into());
        }
    }
    if !payload.contains_key("sub") {
        return Err("Missing claim: sub".into());
    }
    Ok(payload)
}

// ---- tables ----

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i32,
    email: String,
    name: String,
    password: String,
    organization_name: String,
    organization_type_id: Option<i32>,
    is_active: Option<bool>,
    assistance_eligible: bool,
}

const USER_COLS: &str = "id, email, name, password, organization_name, organization_type_id, \
                         is_active, assistance_eligible";

/// `User.query.filter_by(email=email).first()`
async fn user_by_email(conn: &mut PgConnection, email: &Value) -> Result<Option<UserRow>, Fail> {
    let (text, ty) = pg_literal(email)?;
    let sql = format!(
        "select {USER_COLS} from tbl_users where email = $1{} limit 1",
        ty.map(|t| format!("::{t}")).unwrap_or_default()
    );
    Ok(crate::pq::query_as(&sql)
        .bind(text)
        .fetch_optional(conn)
        .await?)
}

/// `Settings.find_by_name(name)`, `None` for no row.
pub(crate) async fn setting<'e>(
    executor: impl Into<crate::pq::On<'e>>,
    name: &str,
) -> Result<Option<String>, sqlx::Error> {
    crate::pq::query_scalar("select value from tbl_master_settings where name = $1 limit 1")
        .bind(name)
        .fetch_optional(executor)
        .await
}

/// `UserLogic.add_user_sessions_for_log`: the session row and its empty DataAnalyzer, with the
/// model defaults SQLAlchemy fills in (`created_at`, `privacy_level`, `updated_by`).
/// `updated_by` exists in the model only (its migration is commented out); the Python insert
/// names it, so the live table must have it. No FK links the two tables, so their order inside
/// SQLAlchemy's flush does not matter; add order is kept.
async fn add_user_sessions_for_log(
    conn: &mut PgConnection,
    user_id: i32,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    crate::pq::query(
        // the ORM INSERT lists every mapped column without a Python default as NULL
        "insert into tbl_user_sessions (user_id, session_id, is_active, created_at, project_name, \
         project_description, is_project, privacy_level, analyzer_version, \
         deletion_reminder_sent_at, updated_at, updated_by) \
         values ($1, $2, 1, $3, NULL, NULL, false, 1, NULL, NULL, NULL, 0) returning id",
    )
    .bind(user_id)
    .bind(session_id)
    .bind(now_naive())
    .execute(&mut *conn)
    .await?;
    crate::pq::query(r#"insert into "DataAnalyzer" (session_id) values ($1) returning id"#)
        .bind(session_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// A dict built from JSON values as the pickle `PickleType` writes. serde-pickle writes
/// protocol 3 (SQLAlchemy uses 5); both unpickle to the same dict in Python.
fn to_pickle(v: &Value) -> Result<Vec<u8>, Fail> {
    use serde_pickle::{HashableValue as H, Value as P};
    fn conv(v: &Value) -> Result<P, Fail> {
        Ok(match v {
            Value::Null => P::None,
            Value::Bool(b) => P::Bool(*b),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => P::I64(i),
                // shortcut: a JSON int past i64 would need num-bigint, not a dependency here
                (None, Some(f)) if n.is_f64() => P::F64(f),
                _ => return Err(other(format!("integer {n} out of range for pickling"))),
            },
            Value::String(s) => P::String(s.clone()),
            Value::Array(items) => P::List(items.iter().map(conv).collect::<Result<_, _>>()?),
            Value::Object(map) => P::Dict(
                map.iter()
                    .map(|(k, v)| Ok((H::String(k.clone()), conv(v)?)))
                    .collect::<Result<_, Fail>>()?,
            ),
        })
    }
    serde_pickle::value_to_vec(&conv(v)?, serde_pickle::SerOptions::new())
        .map_err(|e| other(e.to_string()))
}

// `sanitize_for_jsonb` only maps NaN/inf floats (and Decimals) to None. Values here come from
// serde_json, which cannot hold NaN or inf, so the JSONB twin is the same dict.

// ---- reCAPTCHA Enterprise, GCS: google client libraries over Application Default Credentials ----

pub(crate) fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

/// A JSON answer, or `Err("<status> <message>")` as google-api-core words a failed call.
pub(crate) async fn read_json(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        let message = v
            .pointer("/error/message")
            .or_else(|| v.get("error_description"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(format!("{} {message}", status.as_u16()));
    }
    Ok(v)
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut s = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

/// An OAuth access token as google-auth's `default()` finds credentials:
/// `GOOGLE_APPLICATION_CREDENTIALS`, then gcloud's well-known file, then the GCE metadata
/// server. Service-account keys and gcloud user credentials are handled.
/// NOT PORTED: external-account (workload identity federation) and impersonated credentials,
/// the quota-project header, and token caching (google-auth reuses a token until expiry; this
/// fetches one per call).
pub(crate) async fn google_access_token(client: &reqwest::Client) -> Result<String, String> {
    google_access_token_for(client, None).await
}

/// The same with explicit scopes, as `google.auth.default(scopes=...)` (googleapiclient's
/// `build()` asks for the API's discovery scopes): the service-account claim and the metadata
/// server's `?scopes=` carry them.
pub(crate) async fn google_access_token_for(
    client: &reqwest::Client,
    scopes: Option<&[&str]>,
) -> Result<String, String> {
    let well_known = if cfg!(windows) {
        std::env::var("APPDATA")
            .map(|d| format!("{d}\\gcloud\\application_default_credentials.json"))
            .ok()
    } else {
        std::env::var("HOME")
            .map(|d| format!("{d}/.config/gcloud/application_default_credentials.json"))
            .ok()
    };
    let path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| well_known.filter(|p| std::path::Path::new(p).is_file()));
    let Some(path) = path else {
        let v = read_json(
            client
                .get(match scopes {
                    None => "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token".to_string(),
                    Some(scopes) => format!(
                        "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token?{}",
                        form_body(&[("scopes", &scopes.join(","))])
                    ),
                })
                .header("Metadata-Flavor", "Google")
                .send()
                .await
                .map_err(|e| e.to_string())?,
        )
        .await?;
        return v
            .get("access_token")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| "no access_token from the metadata server".into());
    };
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cred: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let s = |k: &str| {
        cred.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let token_uri = Some(s("token_uri"))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "https://oauth2.googleapis.com/token".into());
    let body = match s("type").as_str() {
        "service_account" => {
            let now = chrono::Utc::now().timestamp();
            let claims = json!({
                "iss": s("client_email"),
                "scope": scopes.map_or("https://www.googleapis.com/auth/cloud-platform".to_string(), |s| s.join(" ")),
                "aud": token_uri,
                "iat": now,
                "exp": now + 3600,
            });
            let mut head = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
            head.kid = Some(s("private_key_id"));
            let key = jsonwebtoken::EncodingKey::from_rsa_pem(s("private_key").as_bytes())
                .map_err(|e| e.to_string())?;
            let assertion =
                jsonwebtoken::encode(&head, &claims, &key).map_err(|e| e.to_string())?;
            form_body(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
        }
        "authorized_user" => form_body(&[
            ("grant_type", "refresh_token"),
            ("client_id", &s("client_id")),
            ("client_secret", &s("client_secret")),
            ("refresh_token", &s("refresh_token")),
        ]),
        other => return Err(format!("credential type '{other}' not ported")),
    };
    let v = read_json(
        client
            .post(&token_uri)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await?;
    v.get("access_token")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| "no access_token in the token response".into())
}

/// `RecaptchaEnterpriseServiceClient.create_assessment` over REST (the client library uses
/// gRPC; the REST method is the same RPC). The answer is proto3 JSON: unset fields are absent.
async fn create_assessment(project_id: &str, site_key: &str, token: &str) -> Result<Value, String> {
    let client = http()?;
    let access = google_access_token(&client).await?;
    let body = json!({ "event": { "siteKey": site_key, "token": token } });
    read_json(
        client
            .post(format!(
                "https://recaptchaenterprise.googleapis.com/v1/projects/{project_id}/assessments"
            ))
            .bearer_auth(access)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await
}

/// `CloudRecaptcha.verify(token, action)`
async fn recaptcha_verify(
    conn: &mut PgConnection,
    token: &Value,
    action: &str,
) -> Result<(), Fail> {
    let rows: Vec<(String, String)> = crate::pq::query_as(
        "select name, value from tbl_master_settings where name in ($1, $2, $3)",
    )
    .bind("RECAPTCHA_PROJECT_ID")
    .bind("RECAPTCHA_SITE_KEY")
    .bind("RECAPTCHA_SCORE_THRESHOLD")
    .fetch_all(conn)
    .await?;
    let settings: HashMap<String, String> = rows.into_iter().collect();
    let configured = |k: &str| settings.get(k).filter(|v| !v.is_empty());
    let (Some(project_id), Some(site_key)) = (
        configured("RECAPTCHA_PROJECT_ID"),
        configured("RECAPTCHA_SITE_KEY"),
    ) else {
        return Ok(()); // reCAPTCHA not configured for this environment
    };
    if !truthy(token) {
        return Err(app("please input: recaptcha_token (text mandatory)"));
    }
    let Value::String(token) = token else {
        // proto-plus refuses a non-str for a string field
        return Err(other(format!(
            "{} has type {}, but expected one of: bytes, unicode",
            py_repr(token),
            py_type(token)
        )));
    };
    let response = create_assessment(project_id, site_key, token)
        .await
        .map_err(Fail::Other)?;
    let valid = response
        .pointer("/tokenProperties/valid")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !valid {
        return Err(app("recaptcha verification failed."));
    }
    let got = response
        .pointer("/tokenProperties/action")
        .and_then(Value::as_str)
        .unwrap_or("");
    if got != action {
        return Err(app("recaptcha verification failed.."));
    }
    let score = response
        .pointer("/riskAnalysis/score")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let threshold = match settings.get("RECAPTCHA_SCORE_THRESHOLD") {
        None => 0.8,
        // float(str); shortcut: Python also takes digit underscores
        Some(s) => s.trim().parse::<f64>().map_err(|_| {
            other(format!(
                "could not convert string to float: {}",
                py_repr_str(s)
            ))
        })?,
    };
    if score < threshold {
        return Err(app("recaptcha verification failed..."));
    }
    Ok(())
}

/// `CloudStorage.upload(source)`: destination = source; every failure is only logged. As the
/// google-cloud-storage client does it: `STORAGE_EMULATOR_HOST` (anonymous) when set, else
/// storage.googleapis.com with a token; a multipart upload (JSON metadata, then the bytes).
pub(crate) async fn gcs_upload(path: &str, data: &[u8], ext: &str) {
    let result: Result<(), String> = async {
        let bucket = std::env::var("GCS_BUCKET_NAME").map_err(|e| e.to_string())?;
        let client = http()?;
        let emulator = std::env::var("STORAGE_EMULATOR_HOST").ok().filter(|h| !h.is_empty());
        let base = emulator.clone().unwrap_or_else(|| "https://storage.googleapis.com".into());
        // upload_from_filename guesses the type from the extension
        let mime = match ext {
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            // mimetypes on Windows reads the registry (Excel's type); Linux, staging: text/csv
            "csv" if cfg!(windows) => "application/vnd.ms-excel",
            "csv" => "text/csv",
            _ => "image/jpeg",
        };
        // email's boundary: 15 '=', 19 random digits, '=='
        let digits = uuid::Uuid::new_v4().as_u128() % 10u128.pow(19);
        let boundary = format!("==============={digits:019}==");
        let metadata = core::to_flask_json(&json!({ "name": path })).trim_end().replace("\":", "\": ");
        let mut body = format!(
            "--{boundary}\r\ncontent-type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\ncontent-type: {mime}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
        let mut request = client
            .post(format!("{base}/upload/storage/v1/b/{bucket}/o?uploadType=multipart"))
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary=\"{boundary}\""),
            )
            .body(body);
        if emulator.is_none() {
            request = request.bearer_auth(google_access_token(&client).await?);
        }
        read_json(request.send().await.map_err(|e| e.to_string())?)
            .await
            .map(|_| ())
    }
    .await;
    if let Err(e) = result {
        eprintln!("CloudStorage Upload FAILED: {path} to {path} | {e}");
    }
}

// ---- mail: BaseMail + Jinja templates ----

const TPL_REGISTER: &str =
    include_str!("../../../application/apis/mail_apis/templates/user_register.html");
const TPL_FORGOT: &str =
    include_str!("../../../application/apis/mail_apis/templates/forgot_password.html");
const TPL_AREA_REQUEST: &str =
    include_str!("../../../application/apis/mail_apis/templates/user_area_request.html");

const SUBJECT_REGISTER: &str =
    "Welcome to NbS Tool! Activate your account and unlock your potential";
const SUBJECT_FORGOT: &str = "NbS Tool Password Reset: Get Back on Track";
const SUBJECT_AREA_REQUEST: &str = "[NbS Tool] Request for Broader Area ANalysis";

/// markupsafe's `escape(str(value))`, what Flask's autoescape does to `{{ ... }}` in `.html`.
pub(crate) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A template value: Python's `str()` then markupsafe escaping. minijinja's own HTML escaping
/// also escapes `/`, which Flask does not, so values go in pre-escaped and unescaped.
fn h(v: &Value) -> Value {
    Value::String(escape_html(&py_str(v)))
}

/// `render_template(template, data=data)`
fn render(template: &str, data: Value) -> Result<String, Fail> {
    minijinja::Environment::new()
        .render_str(template, json!({ "data": data }))
        .map_err(|e| other(e.to_string()))
}

/// `BaseMail.__init__`: the FE URL every template gets as `data.fe_url`.
async fn fe_url() -> Result<String, Fail> {
    setting(&ctx().await.app, "FE_URL_V3")
        .await?
        .ok_or_else(|| other("utils common mail: fe url v3 invalid or not found"))
}

/// `BaseMail.send_mail`. What raises before its `try` (the From address, the port) fails the
/// route; everything inside the `try` (connect, TLS, login, send) is only logged.
async fn send_mail(to: &str, subject: &str, html: String) -> Result<bool, Fail> {
    use lettre::message::{Mailbox, header::ContentType};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let env = |k: &str| std::env::var(k).ok();
    let from_name = env("MAIL_SMTP_FROM_NAME").filter(|n| !n.is_empty());
    // formataddr: `address.encode('ascii')`
    let from_email = match env("MAIL_SMTP_USER") {
        None => return Err(other("'NoneType' object has no attribute 'encode'")),
        Some(e) if !e.is_ascii() => {
            return Err(other(
                "'ascii' codec can't encode characters in MAIL_SMTP_USER",
            ));
        }
        Some(e) => e,
    };
    let port = match env("MAIL_SMTP_PORT").filter(|p| !p.is_empty()) {
        None => 587,
        Some(p) => match py_int(&p) {
            Ok(Some(n)) => n,
            _ => {
                return Err(other(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr_str(&p)
                )));
            }
        },
    };

    let sent: Result<(), String> = async {
        let host = env("MAIL_SMTP_HOST").ok_or("no MAIL_SMTP_HOST")?;
        let password = env("MAIL_SMTP_PASSWORD").ok_or("no MAIL_SMTP_PASSWORD")?;
        let port = u16::try_from(port).map_err(|e| e.to_string())?;
        let from = Mailbox::new(
            from_name,
            from_email
                .parse()
                .map_err(|e: lettre::address::AddressError| e.to_string())?,
        );
        let mut builder = Message::builder().from(from).subject(subject);
        for address in to.split(';') {
            let mailbox: Mailbox = address
                .trim()
                .parse()
                .map_err(|e: lettre::address::AddressError| e.to_string())?;
            builder = builder.to(mailbox);
        }
        let message = builder
            .header(ContentType::TEXT_HTML)
            .body(html)
            .map_err(|e| e.to_string())?;
        // 465 = implicit TLS, anything else = STARTTLS
        let transport = if port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
        }
        .map_err(|e| e.to_string())?
        .port(port)
        .credentials(Credentials::new(from_email.clone(), password))
        .timeout(Some(Duration::from_secs(30)))
        .build();
        transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = &sent {
        eprintln!("Exception when sending smtp mail: {e}");
    }
    Ok(sent.is_ok())
}

/// The EMailUserRegister mail (register passes the email as `name`, resend the name).
async fn send_activation_mail(to: &str, name: &str, user_id: i32) -> Result<(), Fail> {
    let access_token = create_access_token(user_id, &ctx().await.secret)?;
    let fe = fe_url().await?;
    let html = render(
        TPL_REGISTER,
        json!({ "fe_url": escape_html(&fe), "name": escape_html(name),
                "access_token": escape_html(&access_token) }),
    )?;
    send_mail(to, SUBJECT_REGISTER, html).await.map(|_| ())
}

// ---- tokens of get_auth_success_response / encode_access_token ----

// datetime.min / datetime.max as epoch seconds: `now + timedelta` outside raises OverflowError.
const PY_DATETIME_MIN: i64 = -62_135_596_800;
const PY_DATETIME_MAX: i64 = 253_402_300_799;

/// `User.encode_access_token` / `encode_refresh_token` payload (PyJWT, insertion order).
fn user_payload(exp: i64, iat: i64, user_id: i32) -> String {
    format!(r#"{{"exp":{exp},"iat":{iat},"user_id":{user_id}}}"#)
}

/// `User.encode_access_token()`: `(token, expires_in_seconds)`.
async fn encode_access_token(conn: &mut PgConnection, user_id: i32) -> Result<(String, i64), Fail> {
    let now = chrono::Utc::now().timestamp();
    let hours = setting(&mut *conn, "TOKEN_EXPIRE_HOURS").await?;
    let minutes = setting(&mut *conn, "TOKEN_EXPIRE_MINUTES").await?;
    let parse = |v: Option<String>| v.and_then(|v| py_int(&v).ok().flatten());
    let (Some(h), Some(m)) = (parse(hours), parse(minutes)) else {
        return Err(other("encode access token settings invalid or not found"));
    };
    let expires_in = h
        .checked_mul(3600)
        .and_then(|h| m.checked_mul(60).and_then(|m| h.checked_add(m)))
        .ok_or_else(|| other("date value out of range"))?;
    let exp = now
        .checked_add(expires_in)
        .filter(|t| (PY_DATETIME_MIN..=PY_DATETIME_MAX).contains(t))
        .ok_or_else(|| other("date value out of range"))?;
    let token = sign_hs256(&user_payload(exp, now, user_id), &ctx().await.secret)?;
    Ok((token, expires_in))
}

/// `UserLogic.get_auth_success_response(known_user)`
/// Flask's `session` cookie after `login_user(user)`: Flask-Login's `{_fresh, _id, _user_id}`,
/// written by itsdangerous's URLSafeTimedSerializer (compact sorted JSON, zlib when shorter,
/// base64url, timestamp, HMAC-SHA1 under the key derived from SECRET_KEY and `cookie-session`).
fn session_cookie(req: &Req, user_id: i32, secret: &str) -> String {
    use base64::Engine as _;
    use hmac::Mac as _;
    use sha2::Digest as _;
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    // _create_identifier: sha512 of f"{remote_addr bytes}|{user_agent bytes or None}"
    let header = |name: &str| req.parts.headers.get(name).map(|v| v.as_bytes().to_vec());
    let addr = match header("x-forwarded-for") {
        Some(v) => v
            .split(|b| *b == b',')
            .next()
            .unwrap_or(&[])
            .trim_ascii()
            .to_vec(),
        None => req
            .parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string().into_bytes())
            .unwrap_or_default(),
    };
    let agent = header("user-agent").map_or("None".to_string(), |ua| py_bytes_repr(&ua));
    let id_text = [py_bytes_repr(&addr).as_bytes(), b"|", agent.as_bytes()].concat();
    let id: String = sha2::Sha512::digest(&id_text)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let json =
        core::to_flask_json(&json!({ "_fresh": true, "_id": id, "_user_id": user_id.to_string() }));
    let json = json.trim_end().as_bytes();
    let payload = match crate::deflate::compress(json) {
        Some(z) if z.len() < json.len() - 1 => format!(".{}", b64(&z)),
        _ => b64(json),
    };
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let ts_bytes = now.to_be_bytes();
    let first = ts_bytes
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(ts_bytes.len());
    let value = format!("{payload}.{}", b64(&ts_bytes[first..]));
    // derive_key ("hmac"): HMAC-SHA1(secret, salt); then HMAC-SHA1(key, value)
    let sign = |key: &[u8], msg: &[u8]| -> Vec<u8> {
        match hmac::Hmac::<sha1::Sha1>::new_from_slice(key) {
            Ok(mut mac) => {
                mac.update(msg);
                mac.finalize().into_bytes().to_vec()
            }
            Err(_) => Vec::new(), // INFALLIBLE: HMAC takes a key of any length
        }
    };
    let key = sign(secret.as_bytes(), b"cookie-session");
    let signature = b64(&sign(&key, value.as_bytes()));
    format!("session={value}.{signature}; HttpOnly; Path=/")
}

/// Python's `repr(bytes)`: `b'...'`.
fn py_bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c as char == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

async fn auth_success_response(user: &UserRow, message: &str) -> Result<Response, Fail> {
    let ctx = ctx().await;
    let mut conn = ctx.app.acquire().await?;
    let (access_token, expires_in) = encode_access_token(&mut conn, user.id).await?;
    let now = chrono::Utc::now().timestamp();
    let refresh_token = sign_hs256(&user_payload(now + 24 * 3600, now, user.id), &ctx.secret)?;
    let body = json!({
        "status": "success",
        "user_id": user.id,
        "message": message,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": "bearer",
        "expires_in": expires_in,
        "assistance_eligible": user.assistance_eligible,
    });
    let response = core::with_header(core::json(200, &body), "cache-control", "no-store");
    Ok(core::with_header(response, "pragma", "no-cache"))
}

// ---- request.form / request.files ----

#[derive(Debug, Default)]
pub(crate) struct Form {
    pub(crate) fields: Vec<(String, String)>,
    pub(crate) files: Vec<FilePart>,
}

#[derive(Debug)]
pub(crate) struct FilePart {
    pub(crate) name: String,
    pub(crate) filename: String,
    pub(crate) data: Vec<u8>,
}

impl Form {
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub(crate) fn file(&self, name: &str) -> Option<&FilePart> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// werkzeug's form parser: urlencoded or multipart; anything malformed gives an empty form
/// (its parser is `silent`). A part with a `filename` parameter is a file, even an empty one.
pub(crate) async fn parse_form(content_type: &str, body: &[u8]) -> Form {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "application/x-www-form-urlencoded" => Form {
            fields: form_urlencoded::parse(body).into_owned().collect(),
            files: Vec::new(),
        },
        "multipart/form-data" => parse_multipart(content_type, body)
            .await
            .unwrap_or_default(),
        _ => Form::default(),
    }
}

async fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Form, multer::Error> {
    let boundary = multer::parse_boundary(content_type)?;
    let body = axum::body::Bytes::copy_from_slice(body);
    let stream = futures_util::stream::once(async move { Ok::<_, std::convert::Infallible>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut form = Form::default();
    while let Some(field) = multipart.next_field().await? {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };
        let filename = field.file_name().map(str::to_string);
        let data = field.bytes().await?;
        match filename {
            Some(filename) => form.files.push(FilePart {
                name,
                filename,
                data: data.to_vec(),
            }),
            None => form
                .fields
                .push((name, String::from_utf8_lossy(&data).into_owned())),
        }
    }
    Ok(form)
}

/// `allowed_image_file(filename)`
fn allowed_image_file(filename: &str) -> bool {
    filename.rsplit_once('.').is_some_and(|(_, ext)| {
        ["jpg", "jpeg", "png", "svg"].contains(&ext.to_lowercase().as_str())
    })
}

// ---- POST /users/account ----

enum Bind {
    /// a psycopg2 literal and the cast it gets (`None`: the column's own type)
    Literal(String, &'static str),
    Bytes(Vec<u8>),
    Json(Value),
}

fn literal(v: &Value, column_type: &'static str) -> Result<Bind, Fail> {
    let (text, ty) = pg_literal(v)?;
    Ok(Bind::Literal(text, ty.unwrap_or(column_type)))
}

/// `POST /users/account` (`user_save_profile`), multipart: `data` (JSON), `section`, file `logo`.
pub async fn account_post(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    core::vary_cookie(finish(account_post_inner(&req, &user).await).await)
}

async fn account_post_inner(req: &Req, user: &core::User) -> Result<Response, Fail> {
    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    // part of current_user's row in the original
    let old_json: Option<Value> =
        crate::pq::query_scalar("select extended_data_json from tbl_users where id = $1")
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?
            .flatten();

    let form = parse_form(content_type(req), &req.body).await;
    let section = form.field("section");
    // json.loads(None) and bad JSON both become the AppMessageException
    // shortcut: Python's json.loads also takes NaN/Infinity, serde does not
    let Some(mut data) = form
        .field("data")
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    else {
        return Err(app("bad request, invalid data"));
    };

    let mut uploaded: Option<Vec<u8>> = None;
    // `if avatar:` is FileStorage's bool(filename)
    if let Some(file) = form.file("logo").filter(|f| !f.filename.is_empty())
        && allowed_image_file(&file.filename)
    {
        let ext = file
            .filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        // os.path.join: relative to the process cwd, with the platform's separator
        let dir = std::path::Path::new("Uploaded-File").join("avatar");
        std::fs::create_dir_all(&dir).map_err(|e| other(e.to_string()))?;
        // secure_filename leaves `avatar_<id>.<jpg|jpeg|png|svg>` unchanged
        let path = dir
            .join(format!("avatar_{}.{ext}", user.id))
            .to_string_lossy()
            .into_owned();
        std::fs::write(&path, &file.data).map_err(|e| other(e.to_string()))?;
        gcs_upload(&path, &file.data, &ext).await;
        match &mut data {
            Value::Object(map) => {
                map.insert("avatar".into(), Value::String(path));
            }
            Value::Array(_) => {
                return Err(other("list indices must be integers or slices, not str"));
            }
            v => {
                return Err(other(format!(
                    "'{}' object does not support item assignment",
                    py_type(v)
                )));
            }
        }
        uploaded = Some(file.data.clone());
    }

    let Value::Object(data) = data else {
        return Err(other(format!(
            "'{}' object has no attribute 'get'",
            py_type(&data)
        )));
    };
    // `dict(current_user.extended_data_json)`, `{}` when that raises.
    // shortcut: dict() of a JSON list of pairs would also succeed in Python; only dicts are
    // ever written to this column.
    let mut extended = match old_json {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let mut sets: Vec<(&str, Bind)> = Vec::new();
    match section {
        Some("profile") => {
            let name = get(&data, "name");
            if !truthy(name) {
                return Err(app("please input: fullname"));
            }
            sets.push(("name", literal(name, "varchar")?));
            // Only the uploaded bytes are used for the avatar; an `avatar` value sent in the
            // form data is ignored, by design.
            if let Some(bytes) = uploaded {
                sets.push(("avatar", Bind::Bytes(bytes)));
            }
            for key in ["role", "phone", "short_bio"] {
                extended.insert(key.into(), or_empty(get(&data, key)));
            }
        }
        Some("organization") => {
            let type_id = get(&data, "organization_type_id");
            if !truthy(type_id) {
                return Err(app("please input: organization type"));
            } else if !truthy(get(&data, "organization_name")) {
                return Err(app("please input: organization name"));
            } else if !truthy(get(&data, "country")) {
                return Err(app("please input: country (text mandatory)"));
            }
            let (id_text, id_type) = pg_literal(type_id)?;
            let sql = format!(
                "select id from tbl_organization_types where id = $1::{} limit 1",
                id_type.unwrap_or("int4")
            );
            let found: Option<i32> = crate::pq::query_scalar(&sql)
                .bind(id_text)
                .fetch_optional(&mut *tx)
                .await?;
            if found.is_none() {
                return Err(app("invalid input: organization type not found"));
            }
            sets.push(("organization_type_id", literal(type_id, "int4")?));
            sets.push((
                "organization_name",
                literal(get(&data, "organization_name"), "varchar")?,
            ));
            extended.insert(
                "country".into(),
                json!(py_title(&py_str(get(&data, "country")))),
            );
            extended.insert("province".into(), or_empty(get(&data, "province")));
            extended.insert("city".into(), or_empty(get(&data, "city")));
        }
        _ => {
            let policy = get(&data, "permissionPolicy");
            if truthy(policy) {
                sets.push(("permission_policy", literal(policy, "int4")?));
            }
        }
    }
    let is_org = section == Some("organization");
    let extended = Value::Object(extended);
    sets.push(("extended_data", Bind::Bytes(to_pickle(&extended)?)));
    // The Organization query autoflushes while extended_data_json still holds the unmutated
    // copy, which makes that copy the committed value; the mutations then land in it, so the
    // final value compares equal and SQLAlchemy writes no JSON change for this section.
    if !is_org {
        sets.push(("extended_data_json", Bind::Json(extended)));
    }

    // SQLAlchemy SETs only columns whose value changed; writing an unchanged value again has the
    // same effect (no triggers known on tbl_users).
    let assignments: Vec<String> = sets
        .iter()
        .enumerate()
        .map(|(i, (col, bind))| match bind {
            Bind::Literal(_, ty) => format!("{col} = ${}::{ty}", i + 1),
            _ => format!("{col} = ${}", i + 1),
        })
        .collect();
    let sql = format!(
        "update tbl_users set {} where id = ${}",
        assignments.join(", "),
        sets.len() + 1
    );
    let mut query = crate::pq::query(&sql);
    for (_, bind) in sets {
        query = match bind {
            Bind::Literal(text, _) => query.bind(text),
            Bind::Bytes(bytes) => query.bind(bytes),
            Bind::Json(v) => query.bind(sqlx::types::Json(v)),
        };
    }
    let updated = query.bind(user.id).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        // SQLAlchemy's StaleDataError
        return Err(other(format!(
            "UPDATE statement on table 'tbl_users' expected to update 1 row(s); {} were matched.",
            updated.rows_affected()
        )));
    }
    tx.commit().await?;
    Ok(ok(
        json!({}),
        Some(200),
        "Updated. Data is updated successfully",
    ))
}

// ---- POST /users/account/register ----

/// `POST /users/account/register`
pub async fn register(req: Req) -> Response {
    let session_id = match uuid4() {
        Ok(id) => id,
        Err(f) => return finish(Err(f)).await,
    };
    let response = finish(register_inner(&req, &session_id).await).await;
    logged(
        "user_account_register",
        &session_id,
        pre_request_data(&req),
        None,
        response,
    )
    .await
}

async fn register_inner(req: &Req, session_id: &str) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let email = get(&data, "email");
    let password = get(&data, "password");
    let name = get(&data, "name");
    let organization_type_id = get(&data, "organizationTypeId");
    let organization_name = get(&data, "organizationName");
    let country = get(&data, "country");

    if !truthy(email) {
        return Err(app("please input: email (text mandatory)"));
    }
    if !truthy(password) {
        return Err(app("please input: password (text mandatory)"));
    }
    if !truthy(name) {
        return Err(app("please input: name (text mandatory)"));
    }
    if !truthy(organization_type_id) {
        return Err(app("please input: organizationTypeId (int mandatory)"));
    }
    if !truthy(organization_name) {
        return Err(app("please input: organizationName (text mandatory)"));
    }
    if !truthy(country) {
        return Err(app("please input: country (text mandatory)"));
    }
    check_email(email)?;
    let organization_type_id = py_int_value(organization_type_id)
        .map_err(|_| app("invalid input format: organizationTypeId"))?;

    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    recaptcha_verify(&mut tx, get(&data, "recaptcha_token"), "signup").await?;

    // an int past i64 cannot be an id: not found, as in Python (which still queries)
    let organization_type_id = match organization_type_id {
        Some(id) => id,
        None => return Err(app("invalid input: organization type not found")),
    };
    let known: Option<i32> =
        crate::pq::query_scalar("select id from tbl_organization_types where id = $1 limit 1")
            .bind(organization_type_id)
            .fetch_optional(&mut *tx)
            .await?;
    if known.is_none() {
        return Err(app("invalid input: organization type not found"));
    }
    let email_text = email.as_str().unwrap_or_default(); // check_email passed: a str
    if let Some(user) = user_by_email(&mut tx, email).await? {
        return Err(app(if user.is_active == Some(true) {
            format!("{email_text} already registered and your account is active")
        } else {
            format!(
                "{email_text} is already registered and not active, please activate your account by checking your email"
            )
        }));
    }

    let extended = json!({ "country": py_title(&py_str(country)) });
    let pickle = to_pickle(&extended)?;
    let password_hash = hash_password(password)?;
    let (name_text, name_type) = pg_literal(name)?;
    let (org_text, org_type) = pg_literal(organization_name)?;
    let sql = format!(
        "insert into tbl_users (email, name, password, created_at, organization_type_id, \
         organization_name, is_active, permission_policy, extended_data, extended_data_json, \
         avatar, size_limit, assistance_eligible) \
         values ($1, $2::{}, $3, $4, $5, $6::{}, false, NULL, $7, $8, NULL, $9, false) \
         returning id",
        name_type.unwrap_or("varchar"),
        org_type.unwrap_or("varchar")
    );
    let user_id: i32 = crate::pq::query_scalar(&sql)
        .bind(email_text)
        .bind(name_text)
        .bind(password_hash)
        .bind(now_naive())
        .bind(organization_type_id) // int8 into the int4 column: assignment cast
        .bind(org_text)
        .bind(pickle)
        .bind(sqlx::types::Json(extended))
        .bind(100_000f64) // size_limit default (sa.Float: double precision)
        .fetch_one(&mut *tx)
        .await?;
    // db.session.refresh(known_user)
    crate::pq::query(
        "select id, email, name, password, created_at, organization_type_id, organization_name, \
         is_active, permission_policy, extended_data, extended_data_json, avatar, size_limit, \
         assistance_eligible from tbl_users where id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    add_user_sessions_for_log(&mut tx, user_id, session_id).await?;
    tx.commit().await?;

    send_activation_mail(email_text, email_text, user_id).await?;
    Ok(ok(
        json!({}),
        Some(201),
        "Register is success, Please check your email to activate account",
    ))
}

// ---- POST /users/account/verification/resend ----

/// `POST /users/account/verification/resend`
pub async fn verification_resend(req: Req) -> Response {
    finish(verification_resend_inner(&req).await).await
}

async fn verification_resend_inner(req: &Req) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let email = get(&data, "email");
    if !truthy(email) {
        return Err(app("please input: email (text mandatory)"));
    }
    check_email(email)?;
    let mut tx = ctx().await.app.begin().await?;
    let Some(user) = user_by_email(&mut tx, email).await? else {
        return Err(app("user email is not registered"));
    };
    if user.is_active == Some(true) {
        return Err(app(format!(
            "{} already registered and your account is active",
            email.as_str().unwrap_or_default()
        )));
    }
    drop(tx); // nothing written: the session rolls back at teardown
    send_activation_mail(&user.email, &user.name, user.id).await?;
    // render_template ran Flask-Login's context processor (current_user): Vary: Cookie
    Ok(core::vary_cookie(ok(
        json!({}),
        Some(201),
        "Your activation email has been sent. Please check your email to activate account",
    )))
}

// ---- GET /users/account/verification/<token> ----

/// Flask's path decoding of the `<token>` segment.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `GET /users/account/verification/{token}`
pub async fn verification(req: Req) -> Response {
    let token = percent_decode(req.parts.uri.path().rsplit('/').next().unwrap_or(""));
    let session_id = match uuid4() {
        Ok(id) => id,
        Err(f) => return finish(Err(f)).await,
    };
    let response = finish(verification_inner(&token, &session_id).await).await;
    logged(
        "user_account_verification_token",
        &session_id,
        json!({ "token": token }),
        None,
        response,
    )
    .await
}

fn now_float() -> f64 {
    chrono::Utc::now().timestamp_micros() as f64 / 1e6
}

async fn verification_inner(token: &str, session_id: &str) -> Result<Response, Fail> {
    let ctx = ctx().await;
    let claims = decode_token(token, &ctx.secret, false, now_float()).map_err(Fail::Other)?;
    let sub = get(&claims, "sub");

    let mut tx = ctx.app.begin().await?;
    // User.query.filter_by(id=sub): psycopg2's literal for whatever type `sub` has
    let (sql, text) = match sub {
        Value::Null => (
            format!("select {USER_COLS} from tbl_users where id is null limit 1"),
            None,
        ),
        v => {
            let (text, ty) = pg_literal(v)?;
            (
                format!(
                    "select {USER_COLS} from tbl_users where id = $1::{} limit 1",
                    ty.unwrap_or("int4")
                ),
                Some(text),
            )
        }
    };
    let mut query = crate::pq::query_as::<UserRow>(&sql);
    if let Some(text) = text {
        query = query.bind(text);
    }
    let Some(user) = query.fetch_optional(&mut *tx).await? else {
        return Err(app("Invalid activation code"));
    };
    if user.is_active == Some(true) {
        return Err(app(
            "Your account already registered and your account is active",
        ));
    }
    if let Err(e) = decode_token(token, &ctx.secret, true, now_float()) {
        let (data, message) = if e == "Signature has expired" {
            (
                json!({ "user_email": user.email, "status": "fail" }),
                "Your verification code is not valid anymore",
            )
        } else {
            (json!({ "status": "fail" }), "Invalid activation code")
        };
        return Ok(core::json(
            401,
            &core::success(data, Some(401), Some(message)),
        ));
    }
    crate::pq::query("update tbl_users set is_active = true, size_limit = $1 where id = $2")
        .bind(500_000f64)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    add_user_sessions_for_log(&mut tx, user.id, session_id).await?;
    tx.commit().await?;
    Ok(ok(json!({}), None, "Account activated successfully!"))
}

// ---- POST /users/account/login ----

/// `POST /users/account/login`
///
/// NOT PORTED: `login_user(known_user)` also writes Flask's signed `session` cookie
/// (itsdangerous, HMAC-SHA1, plus `_id` = sha512 of address and user agent); no SHA-1 is
/// available among the allowed crates. Clients here authenticate with the Bearer token in the
/// body; the response lacks only the `Set-Cookie`.
pub async fn login(req: Req) -> Response {
    let session_id = match uuid4() {
        Ok(id) => id,
        Err(f) => return finish(Err(f)).await,
    };
    // __request_data__ = dict(request.get_json()), its truthy `password` replaced by a bcrypt
    // hash of str(password); unset when that raises
    let mut request_data = pre_request_data(&req);
    if let Some(map) = request_data.as_object_mut() {
        if let Some(p) = map.get("password").filter(|p| truthy(p)).cloned() {
            let text = match p {
                Value::String(s) => s,
                other => py_str(&other),
            };
            match bcrypt::hash(text, 12) {
                Ok(hash) => {
                    map.insert("password".into(), json!(hash));
                }
                Err(_) => request_data = Value::Null,
            }
        }
    } else {
        request_data = Value::Null; // dict(list/str/number) raises
    }
    let (response, user_id) = match login_inner(&req, &session_id).await {
        // login_user(known_user): the after_request log sees the user as current_user
        Ok((response, user_id)) => (response, Some(user_id)),
        Err(fail) => (finish(Err(fail)).await, None),
    };
    logged(
        "user_account_login",
        &session_id,
        request_data,
        user_id,
        response,
    )
    .await
}

/// The response and the id of the user logged in.
async fn login_inner(req: &Req, session_id: &str) -> Result<(Response, i32), Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let email = get(&data, "email");
    let password = get(&data, "password");
    if !truthy(email) {
        return Err(app("please input: email (text mandatory)"));
    }
    check_email(email)?;
    if !truthy(password) {
        return Err(app("please input: password (text mandatory)"));
    }
    let mut tx = ctx().await.app.begin().await?;
    recaptcha_verify(&mut tx, get(&data, "recaptcha_token"), "login").await?;
    let user = user_by_email(&mut tx, email).await?;
    let user = match user {
        Some(user) if check_password(&user.password, password)? => user,
        _ => return Err(app("email or password does not match")),
    };
    if user.is_active != Some(true) {
        return Err(app("user email has not been verified"));
    }
    add_user_sessions_for_log(&mut tx, user.id, session_id).await?;
    tx.commit().await?;
    // login_user wrote the session, which adds `Vary: Cookie`
    let response = auth_success_response(&user, "successfully logged in").await?;
    let cookie = session_cookie(req, user.id, &ctx().await.secret);
    Ok((
        core::vary_cookie(core::with_header(response, "set-cookie", &cookie)),
        user.id,
    ))
}

// ---- POST /users/account/forgot-password ----

/// `POST /users/account/forgot-password`
pub async fn forgot_password(req: Req) -> Response {
    finish(forgot_password_inner(&req).await).await
}

async fn forgot_password_inner(req: &Req) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let email = get(&data, "email");
    if !truthy(email) {
        return Err(app("please input: email (text mandatory)"));
    }
    check_email(email)?;
    let mut tx = ctx().await.app.begin().await?;
    recaptcha_verify(&mut tx, get(&data, "recaptcha_token"), "forgot_password").await?;
    let Some(user) = user_by_email(&mut tx, email).await? else {
        return Err(app("email not found"));
    };
    if user.is_active != Some(true) {
        return Err(app(
            "user email has not been verified, please activate your account first",
        ));
    }
    let (token, expire) = encode_access_token(&mut tx, user.id).await?;
    drop(tx); // nothing written
    let fe = fe_url().await?;
    let html = render(
        TPL_FORGOT,
        json!({ "fe_url": escape_html(&fe), "name": escape_html(&user.name),
                "token": escape_html(&token), "expired": expire.div_euclid(60).to_string() }),
    )?;
    send_mail(&user.email, SUBJECT_FORGOT, html).await?;
    // render_template ran Flask-Login's context processor (current_user): Vary: Cookie
    Ok(core::vary_cookie(ok(
        json!({}),
        Some(200),
        "Reset Password Link is sent to your email",
    )))
}

// ---- POST /users/account/reset-password ----

/// `POST /users/account/reset-password` (the Bearer token is the one from the reset mail)
pub async fn reset_password(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return core::vary_cookie(
                finish(Err(app("Access token expired or token invalid."))).await,
            );
        }
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    core::vary_cookie(finish(reset_password_inner(&req, &user).await).await)
}

async fn reset_password_inner(req: &Req, user: &core::User) -> Result<Response, Fail> {
    let data = json_body(req)?; // no is_json check here: a form body is the 415, as a 500
    let password = get(&data, "password");
    if !truthy(password) {
        return Err(app("please input: password (text mandatory)"));
    }
    let mut tx = ctx().await.app.begin().await?;
    recaptcha_verify(&mut tx, get(&data, "recaptcha_token"), "password_reset").await?;
    let hash = hash_password(password)?;
    crate::pq::query("update tbl_users set password = $1 where id = $2")
        .bind(hash)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ok(json!({}), None, "Success"))
}

// ---- POST /users/account/change-password ----

/// `POST /users/account/change-password`
pub async fn change_password(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    core::vary_cookie(finish(change_password_inner(&req, &user).await).await)
}

async fn change_password_inner(req: &Req, user: &core::User) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let old_password = get(&data, "old_password");
    let new_password = get(&data, "new_password");
    if !truthy(old_password) {
        return Err(app("please input: old_password (text mandatory)"));
    }
    if !truthy(new_password) {
        return Err(app("please input: new_password (text mandatory)"));
    }
    let mut tx = ctx().await.app.begin().await?;
    // part of current_user's row in the original
    let hash: Option<String> =
        crate::pq::query_scalar("select password from tbl_users where id = $1")
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(hash) = hash else {
        return Err(other("not logged in")); // row gone since the login query
    };
    if !check_password(&hash, old_password)? {
        return Err(app("old password does not match"));
    }
    let new_hash = hash_password(new_password)?;
    crate::pq::query("update tbl_users set password = $1 where id = $2")
        .bind(new_hash)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ok(json!({}), None, "Password changed successfully"))
}

// ---- POST /users/account/request ----

/// `POST /users/account/request`
pub async fn account_request(req: Req) -> Response {
    // log: __session_id__ = None, __description_data__ = {}, __request_data__ =
    // request.get_json() (unset when the body is not JSON)
    finish(account_request_inner(&req).await).await
}

async fn account_request_inner(req: &Req) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = json_body(req)?;
    let email = get(&data, "email");
    let request_text = get(&data, "request_text");
    if !truthy(email) {
        return Err(app("please input: email (text mandatory)"));
    }
    if !truthy(request_text) {
        return Err(app(
            "please input: reason for broader area analysis (text mandatory)",
        ));
    }
    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    let Some(user) = user_by_email(&mut tx, email).await? else {
        return Err(app("User is not found"));
    };
    if user.is_active != Some(true) {
        // the view falls off its `if` and returns None: Flask's TypeError, outside the try
        return Err(Fail::Html500);
    }
    let organization: Option<(String,)> =
        crate::pq::query_as("select name from tbl_organization_types where id = $1 limit 1")
            .bind(user.organization_type_id) // NULL matches nothing, as `id IS NULL` does
            .fetch_optional(&mut *tx)
            .await?;

    // `user_request.email = known_user.id` sets a plain attribute: UserRequests has no `email`
    // column, so the row goes in with user_id NULL. Kept as the original writes it.
    // tbl_user_requests.user_request is String(4000): a longer text fails as a 500 in both.
    let (text, ty) = pg_literal(request_text)?;
    let sql = format!(
        "insert into tbl_user_requests (user_request) values ($1::{}) returning id",
        ty.unwrap_or("varchar")
    );
    let request_id: i32 = crate::pq::query_scalar(&sql)
        .bind(text)
        .fetch_one(&mut *tx)
        .await?;
    // db.session.refresh(user_request)
    crate::pq::query("select id, user_id, user_request from tbl_user_requests where id = $1")
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    // the route's create_access_token result is never used; it has no effect to port

    let admin_mails = setting(&ctx.app, "ADMIN_MAILS")
        .await?
        .ok_or_else(|| other("'NoneType' object has no attribute 'value'"))?;
    let Some((organization_name,)) = organization else {
        return Err(other("'NoneType' object has no attribute 'to_json'"));
    };
    let fe = fe_url().await?;
    let html = render(
        TPL_AREA_REQUEST,
        json!({
            "fe_url": escape_html(&fe),
            "user": {
                "name": escape_html(&user.name),
                "email": escape_html(&user.email),
                "organization_name": escape_html(&user.organization_name),
            },
            "organization_type": { "name": escape_html(&organization_name) },
            "reason": h(request_text),
        }),
    )?;
    send_mail(&admin_mails, SUBJECT_AREA_REQUEST, html).await?;
    // render_template ran Flask-Login's context processor (current_user): Vary: Cookie
    Ok(core::vary_cookie(ok(
        json!({}),
        Some(201),
        "Please wait for the NbS Tool Team to confirm your request",
    )))
}

// ---- shared with the other write modules ----

/// `str(uuid.uuid4())` from the OS RNG.
pub(crate) fn uuid4_string() -> Result<String, String> {
    uuid4().map_err(fail_text)
}

/// A JSON value pickled as `PickleType` stores it.
pub(crate) fn pickle(v: &Value) -> Result<Vec<u8>, String> {
    to_pickle(v).map_err(fail_text)
}

/// `BaseMail(to, subject, template, data).send_mail()`: Ok(false) when sending failed (logged).
pub(crate) async fn mail(
    to: &str,
    subject: &str,
    template: &str,
    data: Value,
) -> Result<bool, String> {
    let html = render(template, data).map_err(fail_text)?;
    send_mail(to, subject, html).await.map_err(fail_text)
}

fn fail_text(f: Fail) -> String {
    match f {
        Fail::App(m) | Fail::Other(m) => m,
        Fail::Db(e) => e.to_string(),
        Fail::Html500 => "500".into(),
    }
}

/// `CloudRecaptcha().verify(token, action)`: Err((true, m)) is an AppMessageException (400),
/// Err((false, m)) any other exception (500).
pub(crate) async fn recaptcha_check(token: &Value, action: &str) -> Result<(), (bool, String)> {
    let result = async {
        let mut conn = ctx().await.app.acquire().await?;
        recaptcha_verify(&mut conn, token, action).await
    }
    .await;
    result.map_err(|f| (matches!(f, Fail::App(_)), fail_text(f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_regex_matches_re_match() {
        assert!(email_ok("a@b.c"));
        assert!(email_ok("a@b.c@x")); // re.match is not anchored at the end
        assert!(email_ok("é@ü.é"));
        assert!(!email_ok("@b.c"));
        assert!(!email_ok("a@.c"));
        assert!(!email_ok("a@b."));
        assert!(!email_ok("a@bc"));
        assert!(!email_ok("a@b@c.d"));
        assert!(!email_ok("abc"));
    }

    #[test]
    fn python_value_semantics() {
        assert!(!truthy(&json!(0.0)) && !truthy(&json!("")) && !truthy(&json!({})));
        assert!(truthy(&json!("0")) && truthy(&json!([0])));
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(5)), "5");
        assert_eq!(py_str(&json!(2.0)), "2.0");
        assert_eq!(py_str(&json!(["a", 1, null])), "['a', 1, None]");
        assert_eq!(py_str(&json!({"k": "it's"})), "{'k': \"it's\"}");
        assert_eq!(py_title("indonesia"), "Indonesia");
        assert_eq!(py_title("NEW zealand"), "New Zealand");
        assert_eq!(py_title("o'neil 2nd"), "O'Neil 2Nd");
        assert_eq!(py_title(&py_str(&json!(["viet nam"]))), "['Viet Nam']");
    }

    #[test]
    fn int_conversion_follows_python() {
        assert_eq!(py_int_value(&json!(" 12 ")), Ok(Some(12)));
        assert_eq!(py_int_value(&json!(3.9)), Ok(Some(3)));
        assert_eq!(py_int_value(&json!(true)), Ok(Some(1)));
        assert_eq!(py_int_value(&json!("99999999999999999999")), Ok(None));
        assert_eq!(py_int_value(&json!("3.0")), Err(()));
        assert_eq!(py_int_value(&json!([1])), Err(()));
    }

    #[test]
    fn literals_follow_psycopg2() {
        assert!(matches!(literal(&json!("3"), "int4"), Ok(Bind::Literal(t, "int4")) if t == "3"));
        assert!(
            matches!(literal(&json!(3.0), "int4"), Ok(Bind::Literal(t, "numeric")) if t == "3.0")
        );
        assert!(
            matches!(literal(&json!(true), "int4"), Ok(Bind::Literal(t, "boolean")) if t == "true")
        );
        assert!(literal(&json!({}), "int4").is_err());
    }

    #[test]
    fn uuid4_shape() {
        let u = uuid4().ok().unwrap();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(u, uuid4().ok().unwrap());
    }

    #[test]
    fn activation_token_matches_flask_jwt_extended() {
        // jwt.encode({'fresh':False,'iat':1999998800,'jti':'0a1b2c3d-0000-4000-8000-000000000000',
        //  'type':'access','sub':7,'nbf':1999998800,'exp':2000085200},'k',algorithm='HS256')
        let token = sign_hs256(
            &activation_payload(1_999_998_800, "0a1b2c3d-0000-4000-8000-000000000000", 7),
            "k",
        )
        .ok()
        .unwrap();
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJmcmVzaCI6ZmFsc2UsImlhdCI6MTk5OTk5ODgwMCwian\
             RpIjoiMGExYjJjM2QtMDAwMC00MDAwLTgwMDAtMDAwMDAwMDAwMDAwIiwidHlwZSI6ImFjY2VzcyIsInN1Yi\
             I6NywibmJmIjoxOTk5OTk4ODAwLCJleHAiOjIwMDAwODUyMDB9.TvYLS5lzwwkX6wuHJ9qRL4rhZ1UJgR8FQ\
             MhYFIxR_s8"
        );
        // decode_token as the verification route runs it
        let claims = decode_token(&token, "k", true, 2_000_000_000.0).unwrap();
        assert_eq!(claims["sub"], json!(7));
        assert_eq!(
            decode_token(&token, "k", true, 1_999_998_000.0).unwrap_err(),
            "The token is not yet valid (iat)"
        );
        assert_eq!(
            decode_token(&token, "k", true, 2_000_085_200.0).unwrap_err(),
            "Signature has expired"
        );
        assert!(decode_token(&token, "k", false, 2_000_085_200.0).is_ok()); // allow_expired
        assert_eq!(
            decode_token(&token, "other", false, 2_000_000_000.0).unwrap_err(),
            "Signature verification failed"
        );
        assert_eq!(
            decode_token("abc", "k", false, 0.0).unwrap_err(),
            "Not enough segments"
        );
        let no_sub = sign_hs256(r#"{"iat":1}"#, "k").ok().unwrap();
        assert_eq!(
            decode_token(&no_sub, "k", true, 2.0).unwrap_err(),
            "Missing claim: sub"
        );
        assert_eq!(
            decode_token(&token, "", true, 0.0).unwrap_err(),
            NO_JWT_SECRET
        );
    }

    #[test]
    fn b64url_round_trip() {
        for s in [&b""[..], b"f", b"fo", b"foo", &[0xfb, 0xff]] {
            assert_eq!(b64url_decode(&b64url(s)).unwrap(), s);
        }
        assert!(b64url_decode("a").is_none());
    }

    #[test]
    fn bcrypt_reads_python_hashes() {
        // bcrypt.hashpw('päss'.encode(), bcrypt.gensalt(4)) with Python bcrypt 4.2.0
        let hash = "$2b$04$HdFYgfFIkbDej/B9.fLaBuX8NYsuc9wcbOowStuvSsQhvPxG/thS.";
        assert!(check_password(hash, &json!("päss")).ok().unwrap());
        assert!(!check_password(hash, &json!("pass")).ok().unwrap());
        assert!(check_password("garbage", &json!("x")).is_err());
        assert!(check_password(hash, &json!(5)).is_err());
    }

    #[test]
    fn pickle_is_the_dict() {
        let v = json!({"country": "Indonésia", "n": 3, "x": null, "l": [1.5, true]});
        let bytes = to_pickle(&v).ok().unwrap();
        // protocol 3 header: PROTO 3
        assert_eq!(&bytes[..2], &[0x80, 3]);
        let back = serde_pickle::value_from_slice(&bytes, serde_pickle::DeOptions::new()).unwrap();
        use serde_pickle::{HashableValue as H, Value as P};
        let P::Dict(map) = back else {
            panic!("not a dict")
        };
        assert_eq!(
            map[&H::String("country".into())],
            P::String("Indonésia".into())
        );
        assert_eq!(map[&H::String("n".into())], P::I64(3));
        assert_eq!(map[&H::String("x".into())], P::None);
        assert_eq!(
            map[&H::String("l".into())],
            P::List(vec![P::F64(1.5), P::Bool(true)])
        );
    }

    #[test]
    fn templates_render_as_flask() {
        let html = render(
            TPL_REGISTER,
            json!({ "fe_url": escape_html("https://x.org"), "name": escape_html("a<b>@c.d"),
                    "access_token": escape_html("t.k.n") }),
        )
        .ok()
        .unwrap();
        assert!(html.contains("<p>Hi a&lt;b&gt;@c.d,</p>"));
        assert!(html.contains("href=\"https://x.org/auth/activate?token=t.k.n\""));
        assert!(html.ends_with("</html>"));

        let html = render(
            TPL_FORGOT,
            json!({ "fe_url": "u", "name": "n", "token": "t", "expired": "60" }),
        )
        .ok()
        .unwrap();
        assert!(html.contains("valid for 60 minutes"));

        let html = render(
            TPL_AREA_REQUEST,
            json!({ "user": { "name": "N", "email": "e", "organization_name": "O" },
                    "organization_type": { "name": "T" }, "reason": h(&json!("it's & <x>")) }),
        )
        .ok()
        .unwrap();
        assert!(html.contains("<strong>Organization Type:</strong> T</li>"));
        assert!(html.contains("it&#39;s &amp; &lt;x&gt;"));
    }

    #[tokio::test]
    async fn forms_parse_as_werkzeug() {
        let body =
            b"--XyZ\r\nContent-Disposition: form-data; name=\"data\"\r\n\r\n{\"name\":\"A\"}\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"section\"\r\n\r\nprofile\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"logo\"; filename=\"Me.PNG\"\r\n\
Content-Type: image/png\r\n\r\n\x89PNG\r\n\x00\r\n--XyZ--\r\n";
        let form = parse_form("multipart/form-data; boundary=\"XyZ\"", body).await;
        assert_eq!(form.field("data"), Some("{\"name\":\"A\"}"));
        assert_eq!(form.field("section"), Some("profile"));
        let logo = form.file("logo").unwrap();
        assert_eq!(logo.filename, "Me.PNG");
        assert_eq!(logo.data, b"\x89PNG\r\n\x00");
        assert!(allowed_image_file(&logo.filename));
        assert!(!allowed_image_file("x.gif") && !allowed_image_file("png"));

        let form = parse_form(
            "application/x-www-form-urlencoded",
            b"section=x&data=%7B%7D",
        )
        .await;
        assert_eq!(form.field("data"), Some("{}"));
        assert!(
            parse_form("multipart/form-data; boundary=Q", b"junk")
                .await
                .fields
                .is_empty()
        );
        assert!(
            parse_form("text/plain", b"data=1")
                .await
                .field("data")
                .is_none()
        );
    }

    #[test]
    fn path_token_is_percent_decoded() {
        assert_eq!(percent_decode("a%2Eb%zz%4"), "a.b%zz%4");
    }
}
