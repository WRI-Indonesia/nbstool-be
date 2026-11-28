//! The document write routes: `GET|DELETE /documents/docx` (`document_apis/routes.py`),
//! `GET|POST /documents/ccb` (`ccb/routes.py`) and the draft saves of `POST
//! /documents/feasibility/v3` and `/monitoring/v3` (`document_apis/utils.save_draft`).
//!
//! Rendering is in `documents_generate.rs` (nbstool-doccontext + nbstool-docx): the CCB docx of
//! `GET /documents/docx` and the v3 POST with `generate` set.
//!
//! One sqlx transaction stands for the request's SQLAlchemy session: committed where Python
//! commits, dropped (rolled back) on every other return.

use axum::http::header;
use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::core::{self, Req, ctx};
use crate::logging::LogEntry;

// ---- failures, as the routes' `except` arms sort them ----

enum Fail {
    /// `AppMessageException`: 400, message shown.
    App(String),
    /// any other exception: 500, message shown in development only.
    Py(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

fn gen_fail(e: crate::documents_generate::GenFail) -> Fail {
    match e {
        crate::documents_generate::GenFail::App(m) => Fail::App(m),
        crate::documents_generate::GenFail::Py(m) => Fail::Py(m),
        crate::documents_generate::GenFail::Db(e) => Fail::Db(e),
    }
}

fn app(m: &str) -> Fail {
    Fail::App(m.into())
}

async fn fail_response(fail: Fail) -> Response {
    match fail {
        Fail::App(m) => core::json(400, &core::app_error(&m, None)),
        Fail::Py(m) => core::json(500, &core::other_error(&m, None).await),
        Fail::Db(e) => core::db_error(e).await,
    }
}

// ---- the request, as Flask/werkzeug read it ----

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

/// `request.get_json()` after `is_json` held: bad JSON is werkzeug's BadRequest (500 here).
/// shortcut: NaN/Infinity literals and UTF-16/32 bodies, which Python's json takes, fail here.
fn body_json(req: &Req) -> Result<Value, Fail> {
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    serde_json::from_slice(body).map_err(|_| Fail::Py(core::BAD_REQUEST_TEXT.into()))
}

/// `g.__request_data__ = request.get_json()` before the try: None when it raised.
fn pre_request_data(req: &Req) -> Value {
    if is_json(req) {
        body_json(req).unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// `request.args.to_dict()`: the first value of every key.
fn args_dict(req: &Req) -> Value {
    let mut map = Map::new();
    for (k, v) in form_urlencoded::parse(req.parts.uri.query().unwrap_or("").as_bytes()) {
        map.entry(k.into_owned())
            .or_insert(Value::String(v.into_owned()));
    }
    Value::Object(map)
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

/// `data.get(key)` on the parsed body: AttributeError on a non-dict.
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

/// A `filter_by(col=value)` value: None stays NULL (`IS NULL`), a string binds as text.
/// shortcut: a number/bool/list id makes psycopg2 send another type and Postgres fail with its own
/// message (development only); here it is one generic 500.
fn text_param(v: &Value) -> Result<Option<String>, Fail> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        other => Err(Fail::Py(format!("a {} id is not ported", py_type(other)))),
    }
}

/// `col = $n`, or `col IS NULL` for None (the parameter stays referenced and typed).
fn eq(col: &str, n: usize) -> String {
    format!("({col} = ${n} OR (${n}::text IS NULL AND {col} IS NULL))")
}

/// The log text of `g.__session_id__`.
fn session_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// The after_request activity log; `log_func` reads `current_user`, so Flask-Login touches the
/// session and the response gets `Vary: Cookie`. No session id: no log, no header.
async fn logged(
    req: &Req,
    api: &str,
    session_id: Option<String>,
    request_data: Value,
    description: Value,
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
        description_data: description,
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

// ---- DocumentList / DocumentData ----

struct Document {
    id: i32,
    project_id: String,
    document_type: String,
    document_name: String,
}

/// `DocumentList.find_by_document_id(v)`: active rows only, `.first()`.
async fn find_document(
    conn: &mut sqlx::PgConnection,
    document_id: &Value,
) -> Result<Option<Document>, Fail> {
    let id = text_param(document_id)?;
    let row = crate::pq::query(&format!(
        "SELECT id, project_id, document_type, document_name FROM \"DocumentList\" \
         WHERE {} AND is_active = 1 LIMIT 1",
        eq("document_id", 1)
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    row.map(|r| {
        Ok(Document {
            id: r.try_get("id")?,
            project_id: r.try_get("project_id")?,
            document_type: r.try_get("document_type")?,
            document_name: r.try_get("document_name")?,
        })
    })
    .transpose()
    .map_err(Fail::Db)
}

/// The folder a document's file lives in.
fn folder(document_type: &str) -> (&'static str, &'static str) {
    if document_type.to_lowercase() == "ccb" {
        ("generated-file/docx-ccb/", "CCB Documentation")
    } else {
        ("generated-file/docx/", "Feasibility Report")
    }
}

// ---- GET /documents/docx ----

pub async fn docx_get(req: Req) -> Response {
    let mut session_id = None;
    let mut description = json!({});
    let response = match docx_get_inner(&req, &mut session_id, &mut description).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    logged(
        &req,
        "documents_get_document_docx",
        session_id,
        args_dict(&req),
        description,
        response,
    )
    .await
}

async fn docx_get_inner(
    req: &Req,
    session_id: &mut Option<String>,
    description: &mut Value,
) -> Result<Response, Fail> {
    let document_id = core::arg(req, "document_id").map_or(Value::Null, Value::String);
    let mut conn = ctx().await.app.acquire().await?;
    let Some(doc) = find_document(&mut conn, &document_id).await? else {
        return Err(app("File not found"));
    };
    drop(conn);
    *session_id = Some(doc.project_id.clone());
    description["type"] = json!(folder(&doc.document_type).1);
    let path = format!("{}{}", folder(&doc.document_type).0, doc.document_name);
    if doc.document_type.to_lowercase() == "ccb" {
        // CCB saves don't render; built from the saved sections now (always current)
        crate::documents_generate::render_ccb_document(&doc.project_id, &doc.document_type)
            .await
            .map_err(gen_fail)?;
    } else {
        crate::gcs::download(&path).await;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return Err(app("File not found"));
    };
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(core::json(
        200,
        &core::success(json!({ "data": data }), Some(200), None),
    ))
}

// ---- DELETE /documents/docx ----

pub async fn docx_delete(req: Req) -> Response {
    let mut session_id = None;
    let mut description = json!({});
    let response = match docx_delete_inner(&req, &mut session_id, &mut description).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    // the route logs `request.args`, not the body
    logged(
        &req,
        "documents_delete_document_docx",
        session_id,
        args_dict(&req),
        description,
        response,
    )
    .await
}

async fn docx_delete_inner(
    req: &Req,
    session_id: &mut Option<String>,
    description: &mut Value,
) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = body_json(req)?;
    let document_id = get(&data, "document_id")?.clone();
    let mut tx = ctx().await.app.begin().await?;
    let Some(doc) = find_document(&mut tx, &document_id).await? else {
        return Err(app("File not found"));
    };
    *session_id = Some(doc.project_id.clone());
    let (dir, kind) = folder(&doc.document_type);
    description["type"] = json!(kind);
    let path = format!("{dir}{}", doc.document_name);
    crate::gcs::delete(&path).await;
    if std::fs::remove_file(&path).is_err() {
        eprintln!("document delete: file not found, cant unlink");
    }
    crate::pq::query("UPDATE \"DocumentList\" SET is_active = 0 WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(core::json(
        200,
        &core::success(
            json!({ "result": {} }),
            Some(200),
            Some("Document has succesfully deleted"),
        ),
    ))
}

// ---- GET|POST /documents/ccb ----

/// `_not_session_owner(session_id)`: a row owned by another (non-anonymous) user.
async fn not_session_owner(
    conn: &mut sqlx::PgConnection,
    session_id: &Option<String>,
    user_id: Option<i32>,
) -> Result<bool, Fail> {
    let owned: Option<i32> = crate::pq::query_scalar(&format!(
        "SELECT id FROM tbl_user_sessions WHERE {} AND user_id IS NOT NULL AND user_id != 0 \
         AND ($2::int4 IS NULL OR user_id != $2) LIMIT 1",
        eq("session_id", 1)
    ))
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(conn)
    .await?;
    Ok(owned.is_some())
}

/// `_forbidden()`: a plain string to app_exception_handler, so redacted outside development.
async fn forbidden() -> Response {
    let message = "forbidden, only the project owner can access this document";
    core::json(403, &core::other_error(message, Some(403)).await)
}

async fn sessions_auth_exists(
    conn: &mut sqlx::PgConnection,
    session_id: &Option<String>,
) -> Result<bool, Fail> {
    let found: Option<i32> = crate::pq::query_scalar(&format!(
        "SELECT id FROM tbl_sessions WHERE {} LIMIT 1",
        eq("session_id", 1)
    ))
    .bind(session_id)
    .fetch_optional(conn)
    .await?;
    Ok(found.is_some())
}

const SECTIONS: [&str; 5] = [
    "section_1",
    "section_2",
    "section_3",
    "section_4",
    "section_5",
];

pub async fn ccb_get(req: Req) -> Response {
    let mut session_id = None;
    let response = match ccb_get_inner(&req, &mut session_id).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    // current_user is read by the owner check: always Vary
    let response = core::vary_if_anonymous(&req, response).await;
    logged(
        &req,
        "documents_get_document_data_ccb",
        session_id,
        args_dict(&req),
        Value::Null,
        response,
    )
    .await
}

async fn ccb_get_inner(req: &Req, log_session: &mut Option<String>) -> Result<Response, Fail> {
    let session_id = core::arg(req, "session_id");
    let document_type = core::arg(req, "document_type");
    let user = core::current_user(req).await?;
    let mut conn = ctx().await.app.acquire().await?;
    if not_session_owner(&mut conn, &session_id, user.map(|u| u.id)).await? {
        return Ok(forbidden().await);
    }
    let row = crate::pq::query(&format!(
        "SELECT section_1, section_2, section_3, section_4, section_5, section_1_json, \
         section_2_json, section_3_json, section_4_json, section_5_json FROM \"DocumentData\" \
         WHERE {} AND {} LIMIT 1",
        eq("session_id", 1),
        eq("certification_type", 2)
    ))
    .bind(&session_id)
    .bind(&document_type)
    .fetch_optional(&mut *conn)
    .await?;
    let sid = json!(session_id);
    let results = match row {
        None => {
            if !sessions_auth_exists(&mut conn, &session_id).await? {
                return Err(app("fail, session id Not found"));
            }
            *log_session = session_id;
            json!({
                "section_1": {}, "section_2": {}, "section_3": {}, "section_4": {},
                "section_5": { "project_fauna": [], "summary_fauna": [] },
                "session_id": sid,
            })
        }
        Some(row) => {
            *log_session = session_id;
            let mut results = Map::new();
            results.insert("session_id".into(), sid);
            for col in SECTIONS {
                let bytes: Option<Vec<u8>> = row.try_get(col)?;
                // the pickle, not the JSONB twin: legacy rows differ between the two
                let value = match bytes {
                    None => Value::Null,
                    Some(b) => crate::masters_write::pickle_json(&b).map_err(Fail::Py)?,
                };
                results.insert(col.into(), value);
            }
            Value::Object(results)
        }
    };
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), Some(200), None),
    ))
}

pub async fn ccb_post(req: Req) -> Response {
    let request_data = pre_request_data(&req);
    let mut session_id = None;
    let mut description = json!({});
    let mut vary = false;
    let response = match ccb_post_inner(&req, &mut session_id, &mut description, &mut vary).await {
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
        "documents_update_document_data_ccb",
        session_id,
        request_data,
        description,
        response,
    )
    .await
}

async fn ccb_post_inner(
    req: &Req,
    log_session: &mut Option<String>,
    description: &mut Value,
    vary: &mut bool,
) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let body = body_json(req)?;
    let session_value = get(&body, "session_id")?.clone();
    let document_type = text_param(get(&body, "document_type")?)?;
    let section = get(&body, "section")?.clone();
    let data = get(&body, "data")?.clone();
    let session_id = text_param(&session_value)?;

    let mut tx = ctx().await.app.begin().await?;
    if !sessions_auth_exists(&mut tx, &session_id).await? {
        return Err(app("Fail, session id not found"));
    }
    *vary = true;
    let user = core::current_user(req).await?;
    if not_session_owner(&mut tx, &session_id, user.map(|u| u.id)).await? {
        return Ok(forbidden().await);
    }
    *log_session = session_id.clone();
    description["operation"] = json!("Update");
    let mut status_code = 200;
    let mut message = "Updated. Data is updated successfully";

    #[expect(
        clippy::type_complexity,
        reason = "the tuple mirrors the SELECT's columns one to one"
    )]
    let known: Option<(i32, Vec<Option<Vec<u8>>>, Vec<Option<Value>>)> =
        crate::pq::query(&format!(
            "SELECT id, section_1, section_2, section_3, section_4, section_5, section_1_json, \
         section_2_json, section_3_json, section_4_json, section_5_json FROM \"DocumentData\" \
         WHERE {} AND {} LIMIT 1",
            eq("session_id", 1),
            eq("certification_type", 2)
        ))
        .bind(&session_id)
        .bind(&document_type)
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| -> Result<_, sqlx::Error> {
            let pickles = SECTIONS
                .iter()
                .map(|c| r.try_get(*c))
                .collect::<Result<_, _>>()?;
            let jsons = SECTIONS
                .iter()
                .map(|c| r.try_get(format!("{c}_json").as_str()))
                .collect::<Result<_, _>>()?;
            Ok((r.try_get("id")?, pickles, jsons))
        })
        .transpose()?;

    // section '1'..'5' (a string; anything else sets nothing)
    let slot = match &section {
        Value::String(s) => SECTIONS
            .iter()
            .position(|c| c.strip_prefix("section_") == Some(s.as_str())),
        _ => None,
    };
    // sanitize_for_jsonb changes nothing on serde values (no NaN/inf/Decimal)
    let pickle = |v: &Value| -> Result<Option<Vec<u8>>, Fail> {
        if v.is_null() {
            Ok(None)
        } else {
            crate::users_write::pickle(v).map(Some).map_err(Fail::Py)
        }
    };

    let document_id: Option<String> = match known {
        None => {
            description["operation"] = json!("Initialize");
            status_code = 201;
            message = "Created. Data is created successfully";
            let new_id = crate::users_write::uuid4_string().map_err(Fail::Py)?;
            let sid = session_id.clone().unwrap_or_default();
            // DocumentList first (added first), then DocumentData; every mapped column listed
            crate::pq::query(
                "INSERT INTO \"DocumentList\" (project_id, document_id, document_type, document_status, \
                 document_name, client_name, created_at, is_active) \
                 VALUES ($1, $2, $3, 'Draft', $4, NULL, $5, 1)",
            )
            .bind(&session_id)
            .bind(&new_id)
            .bind(&document_type)
            .bind(format!("{sid}-ccb-templates.docx"))
            .bind(core::get_date())
            .execute(&mut *tx)
            .await?;
            let mut pickles: Vec<Option<Vec<u8>>> = vec![None; 5];
            let mut jsons: Vec<Option<sqlx::types::Json<Value>>> = vec![None; 5];
            if let Some(i) = slot {
                pickles[i] = pickle(&data)?;
                // a JSONB attribute set to None is JSON null, not SQL NULL
                jsons[i] = Some(sqlx::types::Json(data.clone()));
            }
            crate::pq::query(
                "INSERT INTO \"DocumentData\" (session_id, certification_type, section_1, section_2, \
                 section_3, section_4, section_5, section_1_json, section_2_json, section_3_json, \
                 section_4_json, section_5_json) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(&session_id)
            .bind(&document_type)
            .bind(&pickles[0]).bind(&pickles[1]).bind(&pickles[2]).bind(&pickles[3]).bind(&pickles[4])
            .bind(&jsons[0]).bind(&jsons[1]).bind(&jsons[2]).bind(&jsons[3]).bind(&jsons[4])
            .execute(&mut *tx)
            .await?;
            Some(new_id)
        }
        Some((id, pickles, jsons)) => {
            if let Some(i) = slot {
                // SQLAlchemy writes a column only when its value changed (PickleType compares the
                // unpickled values, JSONB the JSON)
                // shortcut: serde compares 1 and 1.0 as different, Python as equal (an extra
                // identical write, same stored value)
                let old_pickle = match &pickles[i] {
                    None => Some(Value::Null),
                    Some(b) => crate::masters_write::pickle_json(b).ok(),
                };
                let pickle_changed = old_pickle.as_ref() != Some(&data);
                // SQL NULL and JSON null both load as None
                let json_changed = jsons[i].as_ref().unwrap_or(&Value::Null) != &data;
                let col = SECTIONS[i];
                if pickle_changed {
                    crate::pq::query(&format!(
                        "UPDATE \"DocumentData\" SET {col} = $1 WHERE id = $2"
                    ))
                    .bind(pickle(&data)?)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                }
                if json_changed {
                    crate::pq::query(&format!(
                        "UPDATE \"DocumentData\" SET {col}_json = $1 WHERE id = $2"
                    ))
                    .bind(sqlx::types::Json(data.clone()))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            // `find_by_project_id_and_document_type`: newest, active or not
            crate::pq::query_scalar(&format!(
                "SELECT document_id FROM \"DocumentList\" WHERE {} AND {} ORDER BY created_at DESC LIMIT 1",
                eq("project_id", 1),
                eq("document_type", 2)
            ))
            .bind(&session_id)
            .bind(&document_type)
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    let Some(document_id) = document_id else {
        return Err(Fail::Py(
            "'NoneType' object has no attribute 'document_id'".into(),
        ));
    };
    Ok(core::json(
        200,
        &core::success(
            json!({ "result": document_id }),
            Some(status_code),
            Some(message),
        ),
    ))
}

// ---- POST /documents/feasibility/v3, /documents/monitoring/v3: the draft save ----

pub async fn feasibility_v3_post(req: Req) -> Response {
    draft_post(req, "documents_feasibility_v3", "FeasibilityV3").await
}

pub async fn monitoring_v3_post(req: Req) -> Response {
    draft_post(req, "documents_monitoring_v3", "MonitoringV3").await
}

async fn draft_post(req: Req, api: &str, cert: &str) -> Response {
    let request_data = pre_request_data(&req);
    let mut session_id = None;
    let response = match draft_post_inner(&req, cert, &mut session_id).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    logged(&req, api, session_id, request_data, json!({}), response).await
}

async fn draft_post_inner(
    req: &Req,
    cert: &str,
    log_session: &mut Option<String>,
) -> Result<Response, Fail> {
    if !is_json(req) {
        return Err(app("please provide json data"));
    }
    let data = body_json(req)?;
    let session_value = get(&data, "session_id")?.clone();
    if !truthy(&session_value) {
        return Err(app("please provide session_id"));
    }
    let generate = truthy(get(&data, "generate")?);
    *log_session = session_text(&session_value);
    let empty = json!({});
    let form = get(&data, "form")?;
    let form = if truthy(form) { form } else { &empty };
    let user_input = get(&data, "user_input")?;
    let user_input = if truthy(user_input) {
        user_input
    } else {
        &empty
    };
    if !form.is_object() || !user_input.is_object() {
        return Err(app("fail, form and user_input must be json objects"));
    }
    let session_id = text_param(&session_value)?;
    let mut tx = ctx().await.app.begin().await?;
    let stored: Option<(i32, Option<Value>, Option<Value>)> = crate::pq::query_as(&format!(
        "SELECT id, form, user_input FROM \"DocumentData\" WHERE {} AND certification_type = $2 LIMIT 1",
        eq("session_id", 1)
    ))
    .bind(&session_id)
    .bind(cert)
    .fetch_optional(&mut *tx)
    .await?;
    match stored {
        None => {
            crate::pq::query(
                "INSERT INTO \"DocumentData\" (session_id, certification_type, section_1, section_2, \
                 section_3, section_4, section_5, form, user_input) \
                 VALUES ($1, $2, NULL, NULL, NULL, NULL, NULL, $3, $4)",
            )
            .bind(&session_id)
            .bind(cert)
            .bind(sqlx::types::Json(form))
            .bind(sqlx::types::Json(user_input))
            .execute(&mut *tx)
            .await?;
        }
        Some((id, stored_form, stored_input)) => {
            for (col, patch, old) in [
                ("form", form, stored_form),
                ("user_input", user_input, stored_input),
            ] {
                if !truthy(patch) {
                    continue;
                }
                // `coalesce(col, '{}') || patch`, merged by Postgres; a non-dict column is replaced
                let sql = if old.as_ref().is_some_and(Value::is_object) {
                    format!(
                        "UPDATE \"DocumentData\" SET {col} = coalesce(\"DocumentData\".{col}, '{{}}'::jsonb) || $1::jsonb WHERE id = $2"
                    )
                } else {
                    format!("UPDATE \"DocumentData\" SET {col} = $1::jsonb WHERE id = $2")
                };
                crate::pq::query(&sql)
                    .bind(sqlx::types::Json(patch))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    if generate {
        // the draft save is part of the generate transaction: a failed generate rolls it back
        return crate::documents_generate::generate_v3(
            req,
            tx,
            cert,
            session_id.unwrap_or_default(),
        )
        .await
        .map_err(gen_fail);
    }
    tx.commit().await?;
    Ok(core::json(
        200,
        &core::success(json!({ "result": { "message": "saved" } }), None, None),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn protocol5_shared_keys_unpickle() {
        let hex = "8005954b000000000000007d94288c0161945d94287d94288c0173944b008c0163948c017894757d942868044b0168056806757d942868044b0268056806757d942868044b036805680675658c0162948c017994752e";
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let v = crate::masters_write::pickle_json(&bytes);
        assert_eq!(
            v,
            Ok(
                serde_json::json!({"a": [{"s": 0, "c": "x"}, {"s": 1, "c": "x"}, {"s": 2, "c": "x"}, {"s": 3, "c": "x"}], "b": "y"})
            )
        );
    }
}
