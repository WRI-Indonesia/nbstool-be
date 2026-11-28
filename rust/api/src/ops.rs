//! Routes around the product: `POST /loggers/report-problem` (`logger_apis/routes.py`),
//! `POST /incubators/review`, `GET /users/ga4-report`, and the jobs `GET /documents/cleanup` and
//! `/documents/cleanup/reminder` (`document_apis/cleanup/routes.py`).
//!
//! Google calls go over REST (`gdrive.rs`, `ga4_run_report`) with the ADC token the client
//! libraries would use.

use axum::http::header;
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::core::{self, Req, ctx};
use crate::logging::LogEntry;

enum Fail {
    App(String),
    Py(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

fn app(m: impl Into<String>) -> Fail {
    Fail::App(m.into())
}

async fn fail_response(fail: Fail) -> Response {
    match fail {
        Fail::App(m) => core::json(400, &core::app_error(&m, None)),
        Fail::Py(m) => core::json(500, &core::other_error(&m, None).await),
        Fail::Db(e) => core::db_error(e).await,
    }
}

fn is_json(req: &Req) -> bool {
    let mimetype = req
        .parts
        .headers
        .get(header::CONTENT_TYPE)
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

/// shortcut: NaN/Infinity literals and UTF-16/32 bodies, which Python's json takes, fail here.
fn body_json(req: &Req) -> Result<Value, Fail> {
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    serde_json::from_slice(body).map_err(|_| Fail::Py(core::BAD_REQUEST_TEXT.into()))
}

fn pre_request_data(req: &Req) -> Value {
    if is_json(req) {
        body_json(req).unwrap_or(Value::Null)
    } else {
        Value::Null
    }
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

fn get<'a>(data: &'a Value, key: &str) -> Result<&'a Value, Fail> {
    static NONE: Value = Value::Null;
    match data {
        Value::Object(map) => Ok(map.get(key).unwrap_or(&NONE)),
        other => Err(Fail::Py(format!(
            "'{}' object has no attribute 'get'",
            py_type(other)
        ))),
    }
}

/// Python's `str(v)` for a JSON value.
/// shortcut: lists and dicts print as JSON, not as Python's repr.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if n.is_f64() => core::py_float_repr(n.as_f64().unwrap_or(f64::NAN)),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `s[:n]` in code points.
fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `str.strip()`: Python's whitespace also counts U+001C..U+001F.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `html.escape(s)` (quote=True).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

fn session_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(py_str(other)),
    }
}

async fn logged(
    req: &Req,
    api: &str,
    session_id: Option<String>,
    request_data: Value,
    response: Response,
) -> Response {
    if session_id.as_deref().is_none_or(str::is_empty) {
        return response;
    }
    let entry = LogEntry {
        log_it: true,
        api_name: Some(api.into()),
        session_id,
        request_data,
        description_data: json!({}),
        user_id: core::current_user(req).await.ok().flatten().map(|u| u.id),
    };
    // Flask-Login touches the session (Vary: Cookie) only when no user loads from the token
    let response = if entry.user_id.is_none() {
        core::vary_cookie(response)
    } else {
        response
    };
    crate::logging::log_response(&entry, response).await
}

// ---- POST /loggers/report-problem ----

const MAX_REPORT_DESCRIPTION: usize = 4000;
const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;
const ANALYTICS_ID_FIELDS: [&str; 5] = [
    "clarity_session_id",
    "clarity_user_id",
    "ga_client_id",
    "ga_session_id",
    "ga_user_id",
];

/// `_decode_screenshot(value)`: base64 (optionally a data URL) -> (bytes, ext).
fn decode_screenshot(value: &Value) -> Result<(Vec<u8>, &'static str), Fail> {
    let Value::String(value) = value else {
        return Err(app("invalid input: screenshot must be a base64 string"));
    };
    let mut value = py_strip(value);
    // `^data:image/[a-zA-Z0-9.+-]+;base64,`
    if let Some(rest) = value.strip_prefix("data:image/")
        && let Some(end) = rest.find(";base64,")
        && end > 0
        && rest[..end]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
    {
        value = &rest[end + ";base64,".len()..];
    }
    let too_big = || {
        app(format!(
            "invalid input: screenshot exceeds {} MB",
            MAX_SCREENSHOT_BYTES / (1024 * 1024)
        ))
    };
    if value.chars().count() > MAX_SCREENSHOT_BYTES * 4 / 3 + 4 {
        return Err(too_big());
    }
    // b64decode(validate=True): strict alphabet and padding; unused trailing bits are allowed
    use base64::Engine;
    use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
    const PY_B64: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    let raw = PY_B64
        .decode(value)
        .map_err(|_| app("invalid input: screenshot is not valid base64"))?;
    if raw.len() > MAX_SCREENSHOT_BYTES {
        return Err(too_big());
    }
    for (magic, ext) in [
        (&b"\x89PNG\r\n\x1a\n"[..], "png"),
        (b"\xff\xd8\xff", "jpg"),
        (b"RIFF", "webp"),
    ] {
        if raw.starts_with(magic) {
            return Ok((raw, ext));
        }
    }
    Err(app(
        "invalid input: screenshot must be a PNG, JPEG or WebP image",
    ))
}

pub async fn report_problem(req: Req) -> Response {
    let mut request_data = pre_request_data(&req);
    if let Value::Object(map) = &mut request_data
        && map.contains_key("screenshot")
    {
        // MBs of base64: kept out of the activity log row
        map.insert("screenshot".into(), json!("<stripped>"));
    }
    let mut session_id = None;
    let mut vary = false;
    let response = match report_problem_inner(&req, &mut session_id, &mut vary).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    let response = if vary {
        core::vary_if_anonymous(&req, response).await
    } else {
        response
    };
    logged(
        &req,
        "logger_report_problem",
        session_id,
        request_data,
        response,
    )
    .await
}

/// `telegram.send_message` / `send_photo`; raise when unconfigured or on a failed answer.
async fn telegram(text: &str, photo: Option<(&[u8], String, String)>) -> Result<(), String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(key), Some(chat)) = (env("TELEGRAM_BOT_API_KEY"), env("TELEGRAM_BOT_CHAT_ID")) else {
        return Err(
            "telegram not configured: TELEGRAM_BOT_API_KEY / TELEGRAM_BOT_CHAT_ID missing".into(),
        );
    };
    let thread = env("TELEGRAM_BOT_CHAT_THREAD_ID");
    let client = crate::users_write::http()?;
    let mut payload = json!({ "chat_id": chat, "text": text, "parse_mode": "HTML", "disable_web_page_preview": true });
    if let Some(t) = &thread {
        payload["message_thread_id"] = json!(t);
    }
    let answer: Value = client
        .post(format!("https://api.telegram.org/bot{key}/sendMessage"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(payload.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))?;
    if answer.get("ok") != Some(&json!(true)) {
        return Err(format!(
            "telegram send failed: {}",
            answer.get("description").map(py_str).unwrap_or_default()
        ));
    }
    let Some((bytes, filename, caption)) = photo else {
        return Ok(());
    };
    let reply_to = answer.pointer("/result/message_id").cloned();
    // requests' multipart: fields first, then the file
    let boundary = format!("{:032x}", u128::from_be_bytes(*b"nbstool-report-p"));
    let mut fields: Vec<(&str, String)> = vec![
        ("chat_id", chat),
        ("caption", caption.chars().take(1024).collect()),
        ("parse_mode", "HTML".into()),
    ];
    if let Some(t) = thread {
        fields.push(("message_thread_id", t));
    }
    if let Some(r) = reply_to.filter(truthy) {
        fields.push(("reply_to_message_id", py_str(&r)));
    }
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"{filename}\"\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let answer: Value = client
        .post(format!("https://api.telegram.org/bot{key}/sendPhoto"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))?;
    if answer.get("ok") != Some(&json!(true)) {
        return Err(format!(
            "telegram send photo failed: {}",
            answer.get("description").map(py_str).unwrap_or_default()
        ));
    }
    Ok(())
}

async fn report_problem_inner(
    req: &Req,
    log_session: &mut Option<String>,
    vary: &mut bool,
) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = body_json(req)?;
    let description = get(&data, "description")?;
    let description = match description {
        v if !truthy(v) => String::new(),
        Value::String(s) => py_strip(s).to_string(),
        other => {
            return Err(Fail::Py(format!(
                "'{}' object has no attribute 'strip'",
                py_type(other)
            )));
        }
    };
    if description.chars().count() > MAX_REPORT_DESCRIPTION {
        return Err(app(format!(
            "invalid input: description exceeds {MAX_REPORT_DESCRIPTION} characters"
        )));
    }
    let screenshot = match get(&data, "screenshot")? {
        v if truthy(v) => Some(decode_screenshot(v)?),
        _ => None,
    };
    crate::users_write::recaptcha_check(get(&data, "recaptcha_token")?, "report_problem")
        .await
        .map_err(|(is_app, m)| if is_app { Fail::App(m) } else { Fail::Py(m) })?;

    let session_value = get(&data, "session_id")?.clone();
    *log_session = session_text(&session_value);
    *vary = true;
    let user = core::current_user(req).await?;
    let reporter = user
        .as_ref()
        .map_or("anonymous".to_string(), |u| u.email.clone());
    let host = req
        .parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let env = format!("{} @ {host}", ctx().await.env);
    let user_agent = match get(&data, "user_agent")? {
        v if truthy(v) => Some(py_str(v)),
        _ => req
            .parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|h| h.to_str().ok())
            .map(String::from),
    };
    let field = |key: &str, n: usize| -> Result<Option<String>, Fail> {
        let v = get(&data, key)?;
        Ok(truthy(v).then(|| cut(&py_str(v), n)))
    };
    let page_url = field("page_url", 2048)?;
    let locale = field("locale", 16)?;
    let mut ids = Vec::new();
    for k in ANALYTICS_ID_FIELDS {
        ids.push(field(k, 255)?);
    }

    let mut tx = ctx().await.app.begin().await?;
    let id: i32 = crate::pq::query_scalar(
        "INSERT INTO tbl_logger_problem_reports (session_id, user_id, reporter, description, page_url, \
         locale, user_agent, env, screenshot_path, clarity_session_id, clarity_user_id, ga_client_id, \
         ga_session_id, ga_user_id, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10, \
         $11, $12, $13, $14) RETURNING id",
    )
    .bind(session_text(&session_value))
    .bind(user.as_ref().map(|u| u.id))
    .bind(&reporter)
    .bind((!description.is_empty()).then_some(&description))
    .bind(&page_url)
    .bind(&locale)
    .bind(user_agent.as_ref().map(|u| cut(u, 1024)))
    .bind(cut(&env, 128))
    .bind(&ids[0]).bind(&ids[1]).bind(&ids[2]).bind(&ids[3]).bind(&ids[4])
    .bind(core::get_date())
    .fetch_one(&mut *tx)
    .await?;

    if let Some((raw, ext)) = &screenshot {
        let sep = std::path::MAIN_SEPARATOR;
        let dir = format!("generated-file{sep}report-problem");
        std::fs::create_dir_all(&dir).map_err(|e| Fail::Py(e.to_string()))?;
        let path = format!("{dir}{sep}report_{id}.{ext}");
        std::fs::write(&path, raw).map_err(|e| Fail::Py(e.to_string()))?;
        crate::gcs::get_bucket().await; // CloudStorage() is built per call here
        crate::users_write::gcs_upload(&path, raw, ext).await;
        crate::pq::query(
            "UPDATE tbl_logger_problem_reports SET screenshot_path = $1 WHERE id = $2",
        )
        .bind(path.replace(sep, "/"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // notify; a Telegram outage must not lose (or 500) a report already saved
    let session_str = session_text(&session_value).filter(|_| truthy(&session_value));
    let page = get(&data, "page_url")?;
    let locale_raw = get(&data, "locale")?;
    let fields: Vec<(&str, Option<String>)> = vec![
        ("Report", Some(format!("#{id}"))),
        ("Env", Some(env.clone())),
        ("Reporter", Some(reporter.clone())),
        ("Session", session_str),
        ("Page", truthy(page).then(|| py_str(page))),
        ("Locale", truthy(locale_raw).then(|| py_str(locale_raw))),
        ("User agent", user_agent.clone().filter(|u| !u.is_empty())),
        ("Clarity session", ids[0].clone().filter(|v| !v.is_empty())),
        ("Clarity user", ids[1].clone().filter(|v| !v.is_empty())),
        ("GA client", ids[2].clone().filter(|v| !v.is_empty())),
        ("GA session", ids[3].clone().filter(|v| !v.is_empty())),
        ("GA user", ids[4].clone().filter(|v| !v.is_empty())),
        (
            "Time (UTC)",
            Some(core::get_date().format("%Y-%m-%d %H:%M:%S").to_string()),
        ),
    ];
    let mut lines = vec!["\u{1F41E} <b>Problem report</b>".to_string()];
    for (k, v) in fields {
        if let Some(v) = v {
            lines.push(format!("<b>{k}:</b> {}", html_escape(&v)));
        }
    }
    if !description.is_empty() {
        lines.push(String::new());
        lines.push(html_escape(&description));
    }
    let photo = screenshot.as_ref().map(|(raw, ext)| {
        (
            raw.as_slice(),
            format!("report_{id}.{ext}"),
            format!("Screenshot for report #{id}"),
        )
    });
    if let Err(e) = telegram(&lines.join("\n"), photo).await {
        eprintln!("WARNING report-problem #{id} saved but telegram failed | {e}");
    }

    let mut results = Map::new();
    results.insert("report_id".into(), json!(id));
    Ok(core::json(
        201,
        &core::success(
            Value::Object(results),
            Some(201),
            Some("Thank you, your report has been received"),
        ),
    ))
}

// ---- POST /incubators/review ----

pub async fn incubator_review(req: Req) -> Response {
    let request_data = pre_request_data(&req);
    let mut session_id = None;
    let response = match incubator_review_inner(&req, &mut session_id).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    // current_user is read first: always Vary
    logged(
        &req,
        "post_incubator_review",
        session_id,
        request_data,
        core::vary_if_anonymous(&req, response).await,
    )
    .await
}

async fn incubator_review_inner(
    req: &Req,
    log_session: &mut Option<String>,
) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::json(
            401,
            &core::other_error("not logged in", Some(401)).await,
        ));
    };
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = body_json(req)?;
    let document_id = get(&data, "document_id")?;
    if !truthy(document_id) {
        return Err(app("please provide document id"));
    }
    let Value::String(document_id) = document_id else {
        // shortcut: psycopg2 would send a number/bool/list and Postgres fail on its own terms
        return Err(Fail::Py(format!(
            "a {} document id is not ported",
            py_type(document_id)
        )));
    };
    let mut conn = ctx().await.app.acquire().await?;
    #[expect(clippy::type_complexity, reason = "the tuple mirrors the SELECT's columns one to one")]
    let document: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> = crate::pq::query_as(
        "SELECT project_id, document_name, document_type, client_name FROM \"DocumentList\" WHERE document_id = $1 LIMIT 1",
    )
    .bind(document_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((Some(project_id), document_name, document_type, client_name)) = document else {
        return Err(app("invalid document id: document not found"));
    };
    let owner: Option<Option<i32>> = crate::pq::query_scalar(
        "SELECT user_id FROM tbl_user_sessions WHERE session_id = $1 LIMIT 1",
    )
    .bind(&project_id)
    .fetch_optional(&mut *conn)
    .await?;
    match owner {
        None => {
            return Err(app(
                "sorry, you dont have permission to submit this document for incubators (1)",
            ));
        }
        Some(owner) if owner != Some(user.id) => {
            return Err(app(
                "sorry, you dont have permission to submit this document for incubators (2)",
            ));
        }
        Some(_) => {}
    }
    *log_session = Some(project_id);

    let organization_type: Option<Option<String>> =
        crate::pq::query_scalar("SELECT name FROM tbl_organization_types WHERE id = $1 LIMIT 1")
            .bind(user.organization_type_id)
            .fetch_optional(&mut *conn)
            .await?;
    // `user = current_user.to_json(); user.update(current_user.extended_data)`: only its
    // organization_name is read (the folder name)
    let extended: Option<Vec<u8>> =
        crate::pq::query_scalar("SELECT extended_data FROM tbl_users WHERE id = $1")
            .bind(user.id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
    let mut organization_name = json!(user.organization_name);
    if let Some(bytes) = extended
        && let Value::Object(map) = crate::masters_write::pickle_json(&bytes).map_err(Fail::Py)?
        && let Some(name) = map.get("organization_name")
    {
        organization_name = name.clone();
    }
    let setting = |name: &'static str, what: &'static str| async move {
        crate::users_write::setting(&ctx().await.app, name)
            .await?
            .ok_or_else(|| Fail::Py(format!("incubator review: {what} invalid or not found")))
    };
    let email_list = setting("INCUBATOR_EMAIL_LIST", "incubator email list").await?;
    let filename = py_str_or_none(&document_name);
    let filepath = if document_type.as_deref() == Some("CCB") {
        format!("generated-file/docx-ccb/{filename}")
    } else {
        format!("generated-file/docx/{filename}")
    };
    crate::gcs::download(&filepath).await;
    let drive_id = setting("INCUBATOR_DRIVE_ID", "incubator drive id").await?;
    let folder_name = format!(
        "[{}] {} Document's",
        naive_utc_as_local_timestamp(core::get_date()),
        py_str(&organization_name)
    );
    let drive = crate::gdrive::Drive::new().await.map_err(Fail::Py)?;
    let folder = drive
        .create_anyone_folder(&folder_name, &drive_id)
        .await
        .map_err(Fail::Py)?;
    let folder_id = folder
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    drive
        .create_document(&filepath, &py_str_or_none(&client_name), &folder_id)
        .await
        .map_err(Fail::Py)?;

    // BaseMail(...): FE_URL_V3 first, then `organization_type.to_json()`
    let fe_url = crate::users_write::setting(&ctx().await.app, "FE_URL_V3")
        .await?
        .ok_or_else(|| Fail::Py("utils common mail: fe url v3 invalid or not found".into()))?;
    let Some(organization_type) = organization_type else {
        return Err(Fail::Py(
            "'NoneType' object has no attribute 'to_json'".into(),
        ));
    };
    let e = crate::users_write::escape_html;
    let link = folder
        .get("webViewLink")
        .map(py_str)
        .unwrap_or_else(|| "None".into());
    let mail_data = json!({
        "fe_url": e(&fe_url),
        "user": {
            "name": e(&user.name),
            "email": e(&user.email),
            "organization_type_id": user.organization_type_id,
            "organization_name": e(&user.organization_name),
            "permission_policy": user.permission_policy,
        },
        "document_link": e(&link),
        "organization_type": { "name": e(&py_str_or_none(&organization_type)) },
    });
    crate::users_write::mail(
        &email_list,
        SUBJECT_REVIEW_INCUBATOR,
        TPL_REVIEW_INCUBATOR,
        mail_data.clone(),
    )
    .await
    .map_err(Fail::Py)?;
    crate::users_write::mail(&user.email, SUBJECT_REVIEW_USER, TPL_REVIEW_USER, mail_data)
        .await
        .map_err(Fail::Py)?;
    Ok(core::json(
        200,
        &core::success(json!({ "result": {} }), None, None),
    ))
}

const TPL_REVIEW_INCUBATOR: &str =
    include_str!("../../../application/apis/mail_apis/templates/document_review_incubator.html");
const TPL_REVIEW_USER: &str =
    include_str!("../../../application/apis/mail_apis/templates/document_review_user.html");
const SUBJECT_REVIEW_INCUBATOR: &str = "[NbS Tool] Review Request for User's Project Document";
const SUBJECT_REVIEW_USER: &str = "[NbS Tool] Project Document Review Submission Confirmation";

/// `str(value)` of a nullable text column.
fn py_str_or_none(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "None".into())
}

/// `str(get_date().timestamp()).split('.')[0]`: `timestamp()` of a naive datetime reads it as
/// local time (the host's zone), so a non-UTC host shifts it, as it does the original.
fn naive_utc_as_local_timestamp(d: chrono::NaiveDateTime) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .from_local_datetime(&d)
        .earliest()
        .map_or_else(|| d.and_utc().timestamp(), |t| t.timestamp())
}

// ---- GET /users/ga4-report ----

pub async fn ga4_report(req: Req) -> Response {
    let secret: Vec<char> = ctx().await.secret.chars().collect();
    // SECRET_KEY[13:-13]
    let passkey: String = if secret.len() > 26 {
        secret[13..secret.len() - 13].iter().collect()
    } else {
        String::new()
    };
    let answer = |m: &str| core::json(400, &core::app_error(m, None));
    if core::arg(&req, "passkey").as_deref() != Some(passkey.as_str()) {
        return answer("you dont have permission to do this.");
    }
    if core::arg(&req, "start_date").is_none_or(|s| s.is_empty()) {
        return answer("bad request [1]");
    }
    if core::arg(&req, "end_date").is_none_or(|s| s.is_empty()) {
        return answer("bad request [2]");
    }
    let (Some(start), Some(end)) = (
        core::arg(&req, "start_date")
            .as_deref()
            .and_then(arrow_get_date),
        core::arg(&req, "end_date")
            .as_deref()
            .and_then(arrow_get_date),
    ) else {
        // both parse errors carry the start-date text in the original
        return answer("invalid start date format");
    };
    let report = async {
        let scene = ga4_statistics("416743691", start, end).await?;
        let nbs = ga4_statistics("", start, end).await?;
        Ok::<_, String>(json!({ "scene-coalition": scene, "nbs-tools": nbs }))
    };
    match report.await {
        Ok(results) => core::json(200, &core::success(results, None, Some("Success"))),
        Err(message) => core::json(500, &core::other_error(&message, None).await),
    }
}

/// `arrow.get(text).datetime.strftime('%Y-%m-%d')`: arrow 1.3's `parse_iso`. One divider (`T`, or
/// exactly one space), the date tried against its format list (`YYYY-MM-DD`, `YYYY-M-D` and the
/// `/` and `.` forms, `YYYYMMDD`, `YYYY-DDDD`, `YYYYDDDD`, `YYYY-MM`/`/`/`.`, `YYYY`, ISO week
/// `W`), the time against `_TIME_RE` (all colons or none), then an optional zone after `+`, `-`
/// or `Z`. The zone does not move the date; `24:00` is the next day, as arrow makes it.
fn arrow_get_date(text: &str) -> Option<chrono::NaiveDate> {
    use chrono::NaiveDate;
    let spaces = text.matches(' ').count();
    let has_t = text.contains('T');
    if (spaces > 0 && spaces != 1) || (has_t && spaces > 0) {
        return None;
    }
    let (date, time) = if spaces == 1 {
        text.split_once(' ').map(|(d, t)| (d, Some(t)))?
    } else if has_t {
        text.split_once('T').map(|(d, t)| (d, Some(t)))?
    } else {
        (text, None)
    };
    let digits = |s: &str, n: std::ops::RangeInclusive<usize>| {
        (n.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    let mut next_day = false;
    if let Some(time) = time {
        let (clock, zone) = match time.find(['+', '-', 'Z', 'z']) {
            Some(i) => (&time[..i], Some(&time[i..])),
            None => (time, None),
        };
        let (hms, frac) = match clock.find(['.', ',']) {
            Some(i) => (&clock[..i], Some(&clock[i + 1..])),
            None => (clock, None),
        };
        if frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        let fields: Vec<&str> = if hms.contains(':') {
            hms.split(':').collect()
        } else {
            hms.as_bytes()
                .chunks(2)
                .map(|c| std::str::from_utf8(c).unwrap_or("x"))
                .collect()
        };
        if fields.is_empty()
            || fields.len() > 3
            || fields.iter().any(|f| digits(f, 2..=2).is_none())
        {
            return None;
        }
        if frac.is_some() && fields.len() != 3 {
            return None;
        }
        let v: Vec<u32> = fields.iter().filter_map(|f| f.parse().ok()).collect();
        let (h, m, sec) = (
            v[0],
            v.get(1).copied().unwrap_or(0),
            v.get(2).copied().unwrap_or(0),
        );
        let frac_zero = frac.is_none_or(|f| f.bytes().all(|b| b == b'0'));
        if h == 24 && m == 0 && sec == 0 && frac_zero {
            next_day = true;
        } else if h > 23 || m > 59 || sec > 59 {
            return None;
        }
        if let Some(zone) = zone {
            let ok = match zone.as_bytes()[0] {
                b'Z' | b'z' => zone.len() == 1,
                _ => {
                    let z = &zone[1..];
                    let (zh, zm) = match z.split_once(':') {
                        Some((a, b)) => (digits(a, 2..=2), digits(b, 2..=2)),
                        None if z.len() == 4 => (digits(&z[..2], 2..=2), digits(&z[2..], 2..=2)),
                        None => (digits(z, 2..=2), Some(0)),
                    };
                    matches!((zh, zm), (Some(h), Some(m)) if h * 60 + m < 24 * 60)
                }
            };
            if !ok {
                return None;
            }
        }
    }
    let year = |s: &str| digits(s, 4..=4).map(|y| y as i32);
    let week = |s: &str| -> Option<NaiveDate> {
        // `(\d{4})[\-]?W(\d{2})[\-]?(\d)?`
        let (y, rest) = (s.get(..4)?, s.get(4..)?);
        let rest = rest.strip_prefix('-').unwrap_or(rest).strip_prefix('W')?;
        let (w, day) = (rest.get(..2)?, rest.get(2..)?);
        let day = day.strip_prefix('-').unwrap_or(day);
        let day = if day.is_empty() {
            1
        } else {
            digits(day, 1..=1)?
        };
        week_date(year(y)?, digits(w, 2..=2)?, day)
    };
    let date = if let Some(d) = date.contains('W').then(|| week(date)) {
        d
    } else {
        let sep = date.find(['-', '/', '.']).map(|i| &date[i..=i]);
        let parts: Vec<&str> = match sep {
            Some(sep) => date.split(sep).collect(),
            None => vec![date],
        };
        match (sep, parts.as_slice()) {
            (None, [y]) if y.len() == 4 => NaiveDate::from_ymd_opt(year(y)?, 1, 1),
            (None, [ymd]) if ymd.len() == 8 => NaiveDate::from_ymd_opt(
                year(&ymd[..4])?,
                digits(&ymd[4..6], 2..=2)?,
                digits(&ymd[6..], 2..=2)?,
            ),
            (None, [yd]) if yd.len() == 7 => {
                NaiveDate::from_yo_opt(year(&yd[..4])?, digits(&yd[4..], 3..=3)?)
            }
            (Some("-"), [y, d]) if d.len() == 3 => {
                NaiveDate::from_yo_opt(year(y)?, digits(d, 3..=3)?)
            }
            (Some(_), [y, m]) => NaiveDate::from_ymd_opt(year(y)?, digits(m, 2..=2)?, 1),
            (Some(_), [y, m, d]) => {
                NaiveDate::from_ymd_opt(year(y)?, digits(m, 1..=2)?, digits(d, 1..=2)?)
            }
            _ => None,
        }
    }?;
    if next_day {
        date.succ_opt()
    } else {
        Some(date)
    }
}

fn week_date(year: i32, week: u32, day: u32) -> Option<chrono::NaiveDate> {
    let weekday = match day {
        1 => chrono::Weekday::Mon,
        2 => chrono::Weekday::Tue,
        3 => chrono::Weekday::Wed,
        4 => chrono::Weekday::Thu,
        5 => chrono::Weekday::Fri,
        6 => chrono::Weekday::Sat,
        7 => chrono::Weekday::Sun,
        _ => return None,
    };
    chrono::NaiveDate::from_isoywd_opt(year, week, weekday)
}

/// `UserLogic.get_ga4_report_statistics(property, start, end)`: two `runReport` calls of the
/// Analytics Data API v1beta (REST; the client library sends the same RPC over gRPC).
async fn ga4_statistics(
    property: &str,
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
) -> Result<Value, String> {
    let client = crate::users_write::http()?;
    let token = crate::users_write::google_access_token_for(
        &client,
        Some(&[
            "https://www.googleapis.com/auth/analytics",
            "https://www.googleapis.com/auth/analytics.readonly",
        ]),
    )
    .await?;
    let range = json!([{ "startDate": start.format("%Y-%m-%d").to_string(), "endDate": end.format("%Y-%m-%d").to_string() }]);
    let metrics: Vec<Value> = [
        "activeUsers",
        "newUsers",
        "engagedSessions",
        "engagementRate",
        "userEngagementDuration",
        "eventCount",
    ]
    .iter()
    .map(|m| json!({ "name": m }))
    .collect();
    let url =
        format!("https://analyticsdata.googleapis.com/v1beta/properties/{property}:runReport");
    let run = |body: Value| {
        let request = client
            .post(&url)
            .bearer_auth(&token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        async move {
            crate::users_write::read_json(request.send().await.map_err(|e| e.to_string())?).await
        }
    };
    let property_name = format!("properties/{property}");
    let engagement =
        run(json!({ "property": property_name, "dateRanges": range, "dimensions": [], "metrics": metrics })).await?;
    let countries = run(json!({
        "property": property_name,
        "dateRanges": range,
        "dimensions": [{ "name": "country" }],
        "metrics": [{ "name": "activeUsers" }],
    }))
    .await?;
    ga4_statistics_from(&engagement, &countries)
}

/// The statistics dict from the two runReport answers (proto3 JSON: unset fields absent).
fn ga4_statistics_from(engagement: &Value, countries: &Value) -> Result<Value, String> {
    let values: Vec<String> = engagement
        .pointer("/rows/0/metricValues")
        .and_then(Value::as_array)
        .ok_or("list index out of range")?
        .iter()
        .map(|m| {
            m.get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect();
    let at = |i: usize| {
        values
            .get(i)
            .cloned()
            .ok_or_else(|| "list index out of range".to_string())
    };
    let float = |i: usize| -> Result<f64, String> {
        let s = at(i)?;
        s.trim().parse::<f64>().map_err(|_| {
            format!(
                "could not convert string to float: {}",
                crate::sa::py_repr_str(&s)
            )
        })
    };
    let (active, engaged, duration) = (float(0)?, float(2)?, float(4)?);
    if active == 0.0 {
        return Err("float division by zero".into());
    }
    // Python's float % (the result takes the divisor's sign)
    let rem = duration - active * (duration / active).floor();
    let mut stats = Map::new();
    stats.insert("active_users".into(), json!(at(0)?));
    stats.insert("new_user".into(), json!(at(1)?));
    stats.insert("engaged_sessions".into(), json!(at(2)?));
    stats.insert(
        "engagement_rate".into(),
        json!(py_round2(float(3)? * 100.0)),
    );
    stats.insert(
        "engaged_session_per_active_user".into(),
        json!(py_round2(engaged / active)),
    );
    stats.insert(
        "average_engagement_time_per_active_user".into(),
        json!(format!(
            "{}m {}s",
            (duration / active / 60.0).floor() as i64,
            rem.floor() as i64
        )),
    );
    stats.insert("event_count".into(), json!(at(0)?));
    // row_count is an int32 in the proto; proto3 JSON leaves 0 out
    stats.insert(
        "user_countries".into(),
        json!(
            countries
                .get("rowCount")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        ),
    );
    Ok(Value::Object(stats))
}

/// Python's `round(x, 2)`: correctly rounded from the exact binary value, ties to even.
fn py_round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

// ---- GET /documents/cleanup/reminder, GET /documents/cleanup ----

/// `_check_jobs_token()`: JOBS_TOKEN unset = every call refused.
fn jobs_token_ok(req: &Req) -> bool {
    match std::env::var("JOBS_TOKEN").ok().filter(|t| !t.is_empty()) {
        Some(token) => core::arg(req, "token").as_deref() == Some(token.as_str()),
        None => false,
    }
}

const TPL_REMINDER: &str =
    include_str!("../../../application/apis/mail_apis/templates/project_deletion_reminder.html");

pub async fn cleanup_reminder(req: Req) -> Response {
    match cleanup_reminder_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    }
}

async fn cleanup_reminder_inner(req: &Req) -> Result<Response, Fail> {
    if !jobs_token_ok(req) {
        return Err(app("you dont have permission to do this!"));
    }
    let pool = &ctx().await.app;
    let rows: Vec<(i32, String, Option<String>, String, chrono::NaiveDateTime)> =
        crate::pq::query_as(
            "select us.id, us.session_id, us.project_name, u.email, \
         (s.created_at + interval '24 hours')::timestamp as deletion_at \
         from tbl_user_sessions us join tbl_sessions s on s.session_id = us.session_id \
         join tbl_users u on u.id = us.user_id \
         where us.is_active = 1 and s.is_active = 1 and us.is_project and u.permission_policy = 1 \
         and coalesce(us.privacy_level, 1) = 1 and us.deletion_reminder_sent_at is null \
         and s.created_at <= current_timestamp - interval '12 hours' \
         and s.created_at > current_timestamp - interval '24 hours'",
        )
        .fetch_all(pool)
        .await?;
    let (mut sent, mut failed) = (0, 0);
    if !rows.is_empty() {
        let fe_url = crate::users_write::setting(pool, "FE_URL_V3")
            .await?
            .ok_or_else(|| Fail::Py("utils common mail: fe url v3 invalid or not found".into()))?;
        // minijinja has no str.rstrip: the value is computed here
        let template = TPL_REMINDER.replace("data.fe_url.rstrip('/')", "data.fe_url_rstrip");
        for (id, session_id, project_name, email, deletion_at) in rows {
            let project_name = project_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Untitled project".into());
            let e = crate::users_write::escape_html;
            let data = json!({
                "fe_url": e(&fe_url),
                "fe_url_rstrip": e(fe_url.trim_end_matches('/')),
                "user_email": e(&email),
                "project_name": e(&project_name),
                "session_id": e(&session_id),
                "deletion_date_time": e(&deletion_at.format("%d %B %Y, %H:%M UTC").to_string()),
            });
            let subject = format!("[Action needed] \"{project_name}\" will be deleted");
            if !crate::users_write::mail(&email, &subject, &template, data)
                .await
                .map_err(Fail::Py)?
            {
                failed += 1;
                continue;
            }
            // updated_at kept as it is: the reminder is not a user edit
            crate::pq::query(
                "UPDATE tbl_user_sessions SET deletion_reminder_sent_at = $1, \
                 updated_at = tbl_user_sessions.updated_at WHERE tbl_user_sessions.id = $2",
            )
            .bind(core::get_date())
            .bind(id)
            .execute(pool)
            .await?;
            sent += 1;
        }
    }
    Ok(core::json(
        200,
        &core::success(
            json!({ "result": { "sent_records": sent, "failed_records": failed } }),
            Some(200),
            Some("Deletion reminders sent"),
        ),
    ))
}

pub async fn cleanup(req: Req) -> Response {
    let mut vary = false;
    let response = match cleanup_inner(&req, &mut vary).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    if vary {
        core::vary_if_anonymous(&req, response).await
    } else {
        response
    }
}

/// Files of `dir` named `<session_id>*<ext>`: deleted from GCS (`dir\name`), then locally.
async fn delete_session_files(dir: &str, session_id: &str, ext: &str) {
    let sep = std::path::MAIN_SEPARATOR;
    let local = format!("generated-file{sep}{dir}");
    let Ok(entries) = std::fs::read_dir(&local) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(session_id) && name.ends_with(ext) && !name.starts_with('.') {
            crate::gcs::delete(&format!("{local}{sep}{name}")).await;
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

async fn cleanup_inner(req: &Req, vary: &mut bool) -> Result<Response, Fail> {
    if !jobs_token_ok(req) {
        return Err(app("you dont have permission to do this!"));
    }
    let pool = &ctx().await.app;
    let expired: Vec<Option<String>> =
        crate::pq::query_scalar("select session_id from \"vwExpiredSession\"")
            .fetch_all(pool)
            .await?;
    let user_id = core::current_user(req).await.ok().flatten().map(|u| u.id);
    let (mut total, mut retained) = (0, 0);
    for session_id in expired {
        let mut tx = pool.begin().await?;
        let session: Option<i32> = crate::pq::query_scalar(
            "SELECT id FROM tbl_sessions WHERE (session_id = $1 OR ($1::text IS NULL AND session_id IS NULL)) LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(session_row) = session else { continue };
        let project: Option<(i32, Option<i16>)> = crate::pq::query_as(
            "SELECT id, privacy_level FROM tbl_user_sessions WHERE session_id = $1 AND is_active = 1 LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(&mut *tx)
        .await?;
        if project.is_some_and(|(_, level)| matches!(level, Some(2 | 3))) {
            retained += 1;
            continue;
        }
        total += 1;
        crate::pq::query(
            "UPDATE tbl_sessions SET is_active = 0 WHERE id = $1 AND is_active IS DISTINCT FROM 0",
        )
        .bind(session_row)
        .execute(&mut *tx)
        .await?;
        if let Some((project_row, _)) = project {
            // `updated_at` has onupdate=get_date
            crate::pq::query(
                "UPDATE tbl_user_sessions SET is_active = 0, updated_at = $1 WHERE id = $2",
            )
            .bind(core::get_date())
            .bind(project_row)
            .execute(&mut *tx)
            .await?;
        }
        crate::pq::query(
            "UPDATE \"DocumentList\" SET is_active = 0 WHERE project_id = $1 AND is_active = 1",
        )
        .bind(&session_id)
        .execute(&mut *tx)
        .await?;
        let sid = session_id.clone().unwrap_or_else(|| "None".into());
        let temp = std::path::Path::new("temp_file").join(&sid);
        if temp.is_dir() {
            let _ = std::fs::remove_dir_all(&temp);
        }
        for (dir, ext) in [
            ("csv", ".csv"),
            ("docx", ".docx"),
            ("docx-ccb", ".docx"),
            ("graph", ".jpg"),
            ("logo", ".jpg"),
            ("project-area", ".jpg"),
            ("xlsx", ".xlsx"),
        ] {
            delete_session_files(dir, &sid, ext).await;
        }
        tx.commit().await?;
        *vary = true;
        let entry = LogEntry {
            log_it: true,
            api_name: Some("documents_cleanup_data".into()),
            session_id: session_id.clone(),
            request_data: Value::Null,
            description_data: Value::Null,
            user_id,
        };
        let resp_data = json!({ "json_response": false,
            "data": { "message": "manual logging on documents_cleanup_data api", "text": "" } });
        crate::logging::log_manual(&entry, "LOG_SUCCESS", resp_data).await;
    }
    Ok(core::json(
        200,
        &core::success(
            json!({ "result": { "total_records": total, "retained_records": retained } }),
            Some(200),
            Some("Expired data has succesfully clean"),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GA4 statistics and arrow.get against `proof/ga4_stats.py` (the original's function body
    /// run on the same runReport answers; arrow 1.3 on the same strings).
    #[test]
    fn ga4_matches_the_python() {
        for line in include_str!("../../app-cases/proof/ga4_stats.expected.jsonl").lines() {
            let case: Value = serde_json::from_str(line).unwrap();
            if let Some(text) = case.get("arrow").and_then(Value::as_str) {
                let got = arrow_get_date(text).map(|d| d.format("%Y-%m-%d").to_string());
                assert_eq!(json!(got), case["date"], "arrow.get({text:?})");
                continue;
            }
            let got = ga4_statistics_from(&case["engagement"], &case["countries"]);
            match (got, case.get("ok"), case.get("err")) {
                (Ok(v), Some(ok), _) => {
                    assert_eq!(core::to_flask_json(&v), core::to_flask_json(ok))
                }
                (Err(e), _, Some(err)) => assert_eq!(json!(e), *err),
                (got, ..) => panic!("{line}: {got:?}"),
            }
        }
        assert_eq!(
            arrow_get_date("2026-01-01T24:00"),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
        );
    }

    #[test]
    fn screenshots_decode_as_python() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        assert_eq!(
            decode_screenshot(&json!(format!(" data:image/png;base64,{png} ")))
                .ok()
                .map(|r| r.1),
            Some("png")
        );
        assert!(
            matches!(decode_screenshot(&json!("!!!")), Err(Fail::App(m)) if m.contains("not valid base64"))
        );
        assert!(
            matches!(decode_screenshot(&json!("aGVsbG8=")), Err(Fail::App(m)) if m.contains("PNG, JPEG"))
        );
        assert!(
            matches!(decode_screenshot(&json!(5)), Err(Fail::App(m)) if m.contains("base64 string"))
        );
    }

    #[test]
    fn python_strings() {
        assert_eq!(py_strip("\x1c a \n"), "a");
        assert_eq!(html_escape("<b>'&\""), "&lt;b&gt;&#x27;&amp;&quot;");
        assert_eq!(cut("éé", 1), "é");
    }
}
