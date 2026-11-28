//! `application/apis/project_apis/routes.py`: the project write routes (bind, update, privacy,
//! delete). Not wired: the original still answers these routes until they are recorded against a
//! scratch database (ADR-003).
//!
//! SQLAlchemy runs every statement of a request in one transaction that `db.session.commit()`
//! ends; an exception or an early return leaves it to the teardown rollback. Here that is one
//! sqlx transaction, committed where Python commits and dropped (rolled back) otherwise.

use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::core::{self, Req, ctx};
use crate::sa::{Lit, Stmt};
use sqlx::postgres::PgRow;

// ---- how a route can fail, as its `try/except` sorts it (copied from projects.rs) ----

enum Fail {
    /// `except Exception` from the database: 500.
    Db(sqlx::Error),
    /// `except Exception` from Python itself (an AttributeError, a TypeError ...): 500.
    Py(String),
    /// `except AppMessageException`: 400.
    App(&'static str),
    /// A failed flush (INSERT/UPDATE): 500 like `Py`, and it leaves the request's session
    /// expired and pending rollback, so the after_request log cannot read `current_user`.
    Flush(String),
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Py(e)
    }
}

async fn fail_response(fail: Fail) -> Response {
    match fail {
        Fail::Db(e) => core::db_error(e).await,
        Fail::Py(message) => core::json(500, &core::other_error(&message, None).await),
        Fail::App(message) => core::json(400, &core::app_error(message, None)),
        Fail::Flush(message) => core::json(500, &core::other_error(&message, None).await),
    }
}

/// `app_exception_handler('forbidden, ...', 403)`: a plain string, so redacted outside development.
async fn forbidden(message: &str) -> Response {
    core::json(403, &core::other_error(message, Some(403)).await)
}

// ---- Python's view of a JSON value (copied from projects.rs) ----

type PyResult<T> = Result<T, String>;

static NONE: Value = Value::Null;

fn type_name(v: &Value) -> &'static str {
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

/// `v.get(key, default)`: `default` only when the key is missing; AttributeError on a non-dict.
fn get_or<'a>(v: &'a Value, key: &str, default: &'a Value) -> PyResult<&'a Value> {
    match v {
        Value::Object(map) => Ok(map.get(key).unwrap_or(default)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            type_name(other)
        )),
    }
}

/// `v.get(key)`
fn get<'a>(v: &'a Value, key: &str) -> PyResult<&'a Value> {
    get_or(v, key, &NONE)
}

// ---- the request body, as Flask 3.0 / Werkzeug 3.0 read it ----

/// `request.is_json`: the mimetype is `application/json` or `application/*+json`.
fn is_json(req: &Req) -> bool {
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

/// `request.get_json()` after `is_json` held. A body that does not parse raises Flask's
/// non-debug `BadRequest()`, which the route's `except Exception` turns into a 500.
/// shortcut: UTF-16/32 bodies and NaN/Infinity literals, which Python's json accepts, fail
/// here; upgrade if a client ever sends them.
fn body_json(req: &Req) -> Result<Value, Fail> {
    let body = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
    serde_json::from_slice(body).map_err(|_| {
        Fail::Py(
            "400 Bad Request: The browser (or proxy) sent a request that this server could not understand."
                .into(),
        )
    })
}

/// What the pre-`try` `g.__request_data__ = request.get_json()` leaves: None when it raised.
fn request_data(req: &Req) -> Option<Value> {
    if is_json(req) {
        body_json(req).ok()
    } else {
        None
    }
}

// ---- parameters as psycopg2 renders them ----

/// SQLAlchemy's `is_equal(new, loaded)` for a text column: only an equal string (or None to
/// NULL) is "unchanged".
fn same_text(new: &Value, loaded: &Option<String>) -> bool {
    match (new, loaded) {
        (Value::Null, None) => true,
        (Value::String(s), Some(l)) => s == l,
        _ => false,
    }
}

// ---- statements as SQLAlchemy sends them (sa::Stmt), so odd values fail as in the original ----

const US_COLUMNS: &str = "tbl_user_sessions.id AS tbl_user_sessions_id, \
tbl_user_sessions.user_id AS tbl_user_sessions_user_id, \
tbl_user_sessions.session_id AS tbl_user_sessions_session_id, \
tbl_user_sessions.is_active AS tbl_user_sessions_is_active, \
tbl_user_sessions.created_at AS tbl_user_sessions_created_at, \
tbl_user_sessions.project_name AS tbl_user_sessions_project_name, \
tbl_user_sessions.project_description AS tbl_user_sessions_project_description, \
tbl_user_sessions.is_project AS tbl_user_sessions_is_project, \
tbl_user_sessions.privacy_level AS tbl_user_sessions_privacy_level, \
tbl_user_sessions.analyzer_version AS tbl_user_sessions_analyzer_version, \
tbl_user_sessions.deletion_reminder_sent_at AS tbl_user_sessions_deletion_reminder_sent_at, \
tbl_user_sessions.updated_at AS tbl_user_sessions_updated_at, \
tbl_user_sessions.updated_by AS tbl_user_sessions_updated_by";

const DA_COLUMNS: &str = "\"DataAnalyzer\".id AS \"DataAnalyzer_id\", \
\"DataAnalyzer\".session_id AS \"DataAnalyzer_session_id\", \
\"DataAnalyzer\".site_information AS \"DataAnalyzer_site_information\", \
\"DataAnalyzer\".nature AS \"DataAnalyzer_nature\", \
\"DataAnalyzer\".climate AS \"DataAnalyzer_climate\", \
\"DataAnalyzer\".people AS \"DataAnalyzer_people\", \
\"DataAnalyzer\".benefit AS \"DataAnalyzer_benefit\", \
\"DataAnalyzer\".intervention_eligibility AS \"DataAnalyzer_intervention_eligibility\", \
\"DataAnalyzer\".site_information_json AS \"DataAnalyzer_site_information_json\", \
\"DataAnalyzer\".nature_json AS \"DataAnalyzer_nature_json\", \
\"DataAnalyzer\".climate_json AS \"DataAnalyzer_climate_json\", \
\"DataAnalyzer\".people_json AS \"DataAnalyzer_people_json\", \
\"DataAnalyzer\".benefit_json AS \"DataAnalyzer_benefit_json\", \
\"DataAnalyzer\".intervention_eligibility_json AS \"DataAnalyzer_intervention_eligibility_json\", \
\"DataAnalyzer\".threat_json AS \"DataAnalyzer_threat_json\"";

/// `filter_by(col=v)`: SQLAlchemy renders None as `IS NULL` and a bool inline, binds the rest.
fn eq_clause(column: &str, param: &str, v: &Value) -> (String, Option<(String, Lit)>) {
    match v {
        Value::Null => (format!("{column} IS NULL"), None),
        Value::Bool(b) => (format!("{column} = {b}"), None),
        v => (
            format!("{column} = %({param})s"),
            Some((param.to_string(), Lit::from_json(v))),
        ),
    }
}

/// `UserSessions.find_by_session_id(v)` (`_is_project` with `is_project`): `.first()`.
async fn find_user_session(
    conn: &mut sqlx::PgConnection,
    session_id: &Value,
    is_project: bool,
) -> Result<Option<PgRow>, Fail> {
    let (clause, param) = eq_clause("tbl_user_sessions.session_id", "session_id_1", session_id);
    let mut stmt = Stmt::new(format!(
        "SELECT {US_COLUMNS} \nFROM tbl_user_sessions \nWHERE {clause} AND tbl_user_sessions.is_active = %(is_active_1)s{} \n LIMIT %(param_1)s",
        if is_project {
            " AND tbl_user_sessions.is_project = true"
        } else {
            ""
        }
    ));
    stmt.params.extend(param);
    let stmt = stmt
        .param("is_active_1", Lit::Int(1))
        .param("param_1", Lit::Int(1));
    Ok(stmt
        .fetch_all(conn)
        .await
        .map_err(Fail::Py)?
        .into_iter()
        .next())
}

/// The flush's UPDATE of one `tbl_user_sessions` row: only the changed columns (table order),
/// plus `updated_at` (`onupdate=get_date`).
async fn update_user_session(
    conn: &mut sqlx::PgConnection,
    id: i32,
    changes: Vec<(&str, Lit)>,
) -> Result<(), Fail> {
    const ORDER: [&str; 8] = [
        "user_id",
        "is_active",
        "project_name",
        "project_description",
        "is_project",
        "privacy_level",
        "updated_at",
        "updated_by",
    ];
    let mut changes = changes;
    changes.push(("updated_at", Lit::timestamp(core::get_date())));
    changes.sort_by_key(|(col, _)| ORDER.iter().position(|c| c == col));
    let mut stmt = Stmt::new("");
    let mut sets = Vec::new();
    for (col, value) in changes {
        sets.push(format!("{col}=%({col})s"));
        stmt = stmt.param(col, value);
    }
    stmt = stmt.param("tbl_user_sessions_id", Lit::Int(id.into()));
    stmt.sql = format!(
        "UPDATE tbl_user_sessions SET {} WHERE tbl_user_sessions.id = %(tbl_user_sessions_id)s",
        sets.join(", ")
    );
    stmt.execute(conn).await.map_err(Fail::Flush)?;
    Ok(())
}

const PRIVACY_MESSAGE: &str = "privacy_level must be one of [1, 2, 3]";

/// `not isinstance(v, bool) and v in PROJECT_PRIVACY_LEVELS`: `in` uses `==`, so 2.0 passes.
fn privacy_level(v: &Value) -> Option<i16> {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f == 1.0 || f == 2.0 || f == 3.0 => Some(f as i16),
            _ => None,
        },
        _ => None,
    }
}

/// `str(uuid.uuid4())` (OS RNG).
fn uuid4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `string.capwords(s)`: split on whitespace, `str.capitalize()` each word, join with spaces.
/// Python's first-letter titlecase is approximated by uppercase (differs only for digraphs such
/// as `ǆ` and for `ß`).
fn capwords(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capwords_value(v: &Value) -> PyResult<String> {
    match v {
        Value::String(s) => Ok(capwords(s)),
        other => Err(format!(
            "'{}' object has no attribute 'split'",
            type_name(other)
        )),
    }
}

/// bind's default name: `{District} district Project in {Country}`, else `Project {sid[:8]}`.
fn default_project_name(site_json: &Value, session_id: &str) -> PyResult<String> {
    let empty = Value::Object(Map::new());
    let site = if truthy(site_json) { site_json } else { &empty };
    let district = get(site, "district")?;
    let place = if truthy(district) {
        district
    } else {
        get(site, "province")?
    };
    let country = get(site, "country")?;
    if truthy(place) && truthy(country) {
        Ok(format!(
            "{} district Project in {}",
            capwords_value(place)?,
            capwords_value(country)?
        ))
    } else {
        Ok(format!(
            "Project {}",
            session_id.chars().take(8).collect::<String>()
        ))
    }
}

/// `feasibility_prefill(analyzer)` (utils/document_generator/v3/prefill.py); `people_json` is
/// None when there is no DataAnalyzer row.
fn feasibility_prefill(people_json: Option<&Value>) -> PyResult<Value> {
    let Some(people_json) = people_json else {
        return Ok(json!({}));
    };
    let empty = json!({});
    let people = if truthy(people_json) {
        people_json
    } else {
        &empty
    };
    let demography = get_or(people, "social_demography", &empty)?;
    let economy = get_or(people, "economy", &empty)?;
    let mut prefill = Map::new();
    for (key, value) in [
        ("seHouseholds", get(demography, "household_number")?),
        ("seIncome", get(economy, "avg_household_income")?),
    ] {
        // `value not in (None, "", [], {})`
        let blank = match value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        };
        if !blank {
            prefill.insert(key.into(), value.clone());
        }
    }
    Ok(Value::Object(prefill))
}

/// `template_type is not None and str(template_type).lower() != 'general'`; a non-string never
/// prints as `general`.
fn template_disabled(template_type: &Value) -> bool {
    match template_type {
        Value::Null => false,
        Value::String(s) => s.to_lowercase() != "general",
        _ => true,
    }
}

/// `generate_boundary_map(session_id)`: draws the polygon over basemap tiles (rasterio /
/// matplotlib, `render_boundary_map`) and uploads the PNG with `CloudStorage().upload`.
/// The figure through nbstool-fig, then the GCS upload (`figures::generate_boundary_map`). A
/// failure is only a warning in bind, as in Python.
async fn generate_boundary_map(session_id: &str) -> Result<Option<String>, String> {
    crate::figures::generate_boundary_map(session_id).await
}

// ---- what after_request logs (g.__log_it__ routes) ----

/// `g.__session_id__` and `g.__description_data__['project_name']` as they stand at return.
struct Log {
    session_id: Value,
    project_name: Value,
}

/// The after_request activity log of bind and delete.
async fn write_log(
    req: &Req,
    api: &str,
    request_data: Option<Value>,
    log: Log,
    response: Response,
) -> Response {
    let entry = crate::logging::LogEntry {
        log_it: true,
        api_name: Some(api.into()),
        session_id: match log.session_id {
            Value::Null => None,
            Value::String(s) => Some(s),
            // psycopg2 inlines a number or bool; the varchar column keeps its text
            other => Some(other.to_string()),
        },
        request_data: request_data.unwrap_or(Value::Null),
        description_data: json!({ "project_name": log.project_name }),
        user_id: core::current_user(req).await.ok().flatten().map(|u| u.id),
    };
    crate::logging::log_response(&entry, response).await
}

// ---- POST /projects/bind ----

/// `POST /projects/bind`
pub async fn bind(req: Req) -> Response {
    let request_data = request_data(&req);
    let mut log = Log {
        session_id: Value::Null,
        project_name: json!(""),
    };
    let mut flushed = false;
    let response = match bind_inner(&req, &mut log).await {
        Ok(response) => response,
        Err(fail) => {
            flushed = matches!(fail, Fail::Flush(_));
            fail_response(fail).await
        }
    };
    if flushed {
        // log_func reads current_user.is_authenticated off the expired user: it raises, no row
        return core::vary_cookie(response);
    }
    write_log(
        &req,
        "projects_bind_project",
        request_data,
        log,
        core::vary_cookie(response),
    )
    .await
}

async fn bind_inner(req: &Req, log: &mut Log) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    if !is_json(req) {
        return Err(Fail::App("please provide json data"));
    }
    let data = body_json(req)?;

    let session_id = get(&data, "session_id")?.clone();
    if !truthy(&session_id) {
        return Err(Fail::App("session_id is required"));
    }
    let template_type = get(&data, "template_type")?.clone();
    let privacy = get(&data, "privacy_level")?;
    let privacy = match privacy {
        Value::Null => None,
        v => Some(privacy_level(v).ok_or(Fail::App(PRIVACY_MESSAGE))?),
    };
    let mut message = "Project is updated successfully";
    log.session_id = session_id.clone();
    let document_id = uuid4();
    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    // DataAnalyzer.find_by_session_id: the first query, where a non-string id fails
    let (clause, param) = eq_clause("\"DataAnalyzer\".session_id", "session_id_1", &session_id);
    let mut stmt = Stmt::new(format!(
        "SELECT {DA_COLUMNS} \nFROM \"DataAnalyzer\" \nWHERE {clause} \n LIMIT %(param_1)s"
    ));
    stmt.params.extend(param);
    let analyzer: Option<(Option<Value>, Option<Value>)> = match stmt
        .param("param_1", Lit::Int(1))
        .fetch_all(&mut tx)
        .await
        .map_err(Fail::Py)?
        .first()
    {
        Some(row) => Some((
            row.try_get("DataAnalyzer_site_information_json")?,
            row.try_get("DataAnalyzer_people_json")?,
        )),
        None => None,
    };
    // a truthy non-string id fails above; a bool one (inlined) does not, and fails at
    // `session_id[:8]` below in Python too
    let sid = match &session_id {
        Value::String(s) => s.clone(),
        other => {
            return Err(Fail::Py(format!(
                "'{}' object is not subscriptable",
                type_name(other)
            )));
        }
    };

    let mut project_name = get(&data, "project_name")?.clone();
    if !truthy(&project_name) {
        let site = analyzer
            .as_ref()
            .and_then(|a| a.0.clone())
            .unwrap_or(Value::Null);
        project_name = json!(default_project_name(&site, &sid)?);
    }
    log.project_name = project_name.clone();

    let known = find_user_session(&mut tx, &session_id, false).await?;
    // anonymous (NULL or legacy 0) owners stay claimable; inactive rows count as owned
    let foreign: Option<i32> = crate::pq::query_scalar(
        "SELECT id FROM tbl_user_sessions WHERE session_id = $1 AND user_id IS NOT NULL \
         AND user_id != 0 AND user_id != $2 LIMIT 1",
    )
    .bind(&sid)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    if foreign.is_some() {
        return Ok(forbidden("forbidden, only the project owner can bind this project").await);
    }

    let description = get(&data, "description")?;
    let privacy_raw = get(&data, "privacy_level")?;

    // the autoflush before the DocumentData query writes the session row
    let row_id: i32 = match known {
        None => {
            message = "Project is created successfully";
            let insert = Stmt::new(
                "INSERT INTO tbl_user_sessions (user_id, session_id, is_active, created_at, project_name, \
                 project_description, is_project, privacy_level, analyzer_version, deletion_reminder_sent_at, \
                 updated_at, updated_by) VALUES (%(user_id)s, %(session_id)s, %(is_active)s, %(created_at)s, \
                 %(project_name)s, %(project_description)s, %(is_project)s, %(privacy_level)s, \
                 %(analyzer_version)s, %(deletion_reminder_sent_at)s, %(updated_at)s, %(updated_by)s) \
                 RETURNING tbl_user_sessions.id",
            )
            .param("user_id", Lit::Int(user.id.into()))
            .param("session_id", Lit::Str(sid.clone()))
            .param("is_active", Lit::Int(1))
            .param("created_at", Lit::timestamp(core::get_date()))
            .param("project_name", Lit::from_json(&project_name))
            .param("project_description", Lit::from_json(description))
            .param("is_project", Lit::Bool(true))
            .param("privacy_level", if privacy.is_some() { Lit::from_json(privacy_raw) } else { Lit::Int(1) })
            .param("analyzer_version", Lit::Null)
            .param("deletion_reminder_sent_at", Lit::Null)
            .param("updated_at", Lit::Null)
            .param("updated_by", Lit::Int(0));
            let rows = insert.fetch_all(&mut tx).await.map_err(Fail::Flush)?;
            match rows.first() {
                Some(row) => row.try_get(0)?,
                None => return Err(Fail::Py("INSERT returned no row".into())),
            }
        }
        Some(row) => {
            let id: i32 = row.try_get("tbl_user_sessions_id")?;
            // SQLAlchemy emits no UPDATE (and no updated_at) when every value equals the loaded one
            let mut changes = Vec::new();
            if row.try_get::<Option<i32>, _>("tbl_user_sessions_user_id")? != Some(user.id) {
                changes.push(("user_id", Lit::Int(user.id.into())));
            }
            if !same_text(
                &project_name,
                &row.try_get("tbl_user_sessions_project_name")?,
            ) {
                changes.push(("project_name", Lit::from_json(&project_name)));
            }
            if !description.is_null()
                && !same_text(
                    description,
                    &row.try_get("tbl_user_sessions_project_description")?,
                )
            {
                changes.push(("project_description", Lit::from_json(description)));
            }
            if row.try_get::<Option<bool>, _>("tbl_user_sessions_is_project")? != Some(true) {
                changes.push(("is_project", Lit::Bool(true)));
            }
            if privacy.is_some_and(|p| {
                row.try_get::<Option<i16>, _>("tbl_user_sessions_privacy_level")
                    .ok()
                    .flatten()
                    != Some(p)
            }) {
                changes.push(("privacy_level", Lit::from_json(privacy_raw)));
            }
            if !changes.is_empty() {
                update_user_session(&mut tx, id, changes).await?;
            }
            id
        }
    };

    // shortcut: like the DataAnalyzer read, only the id is selected (Python unpickles section_*)
    let known_form: Option<i32> = crate::pq::query_scalar(
        "SELECT id FROM \"DocumentData\" WHERE session_id = $1 \
         AND certification_type = 'FeasibilityV3' LIMIT 1",
    )
    .bind(&sid)
    .fetch_optional(&mut *tx)
    .await?;
    let new_form = match known_form {
        Some(_) => None,
        None => Some(feasibility_prefill(
            analyzer.as_ref().map(|a| a.1.as_ref().unwrap_or(&NONE)),
        )?),
    };

    if template_disabled(&template_type) {
        // the pending DocumentData insert is never flushed; the session row rolls back
        return Err(Fail::App("fail, document templates are disabled"));
    }

    if let Some(form) = new_form {
        // section_* are PickleType set to None: NULL, no pickle written
        crate::pq::query(
            "INSERT INTO \"DocumentData\" (session_id, certification_type, section_1, section_2, \
             section_3, section_4, section_5, form) \
             VALUES ($1, 'FeasibilityV3', NULL, NULL, NULL, NULL, NULL, $2)",
        )
        .bind(&sid)
        .bind(sqlx::types::Json(form))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    if let Err(e) = generate_boundary_map(&sid).await {
        eprintln!("WARNING boundary map not generated for {sid} | {e}");
    }

    // `known_project.privacy_level` after commit: the expired row is refreshed by its key
    let privacy_now: Option<Option<i16>> =
        crate::pq::query_scalar("SELECT privacy_level FROM tbl_user_sessions WHERE id = $1")
            .bind(row_id)
            .fetch_optional(&ctx.app)
            .await?;
    let Some(privacy_now) = privacy_now else {
        return Err(Fail::Py(
            "Instance '<UserSessions>' has been deleted, or its row is otherwise not present."
                .into(),
        ));
    };

    let results = json!({
        "message": message,
        "project_id": session_id,
        "document_id": document_id,
        "project_name": project_name,
        "privacy_level": privacy_now,
    });
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), None, None),
    ))
}

// ---- POST and PUT /projects ----

/// `POST /projects` and `PUT /projects`: one handler, the method is not read.
pub async fn save(req: Req) -> Response {
    let response = match save_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    core::vary_cookie(response)
}

async fn save_inner(req: &Req) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    if !is_json(req) {
        return Err(Fail::App("please provide json data"));
    }
    let data = body_json(req)?;
    let project_id = get(&data, "project_id")?.clone();
    let project_name = get(&data, "project_name")?.clone();

    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    let Some(row) = find_user_session(&mut tx, &project_id, false).await? else {
        return Err(Fail::App("fail. project not found"));
    };
    if row.try_get::<Option<i32>, _>("tbl_user_sessions_user_id")? != Some(user.id) {
        return Ok(forbidden("forbidden, only the project owner can update this project").await);
    }
    if !same_text(
        &project_name,
        &row.try_get("tbl_user_sessions_project_name")?,
    ) {
        let id = row.try_get("tbl_user_sessions_id")?;
        update_user_session(
            &mut tx,
            id,
            vec![("project_name", Lit::from_json(&project_name))],
        )
        .await?;
    }
    tx.commit().await?;

    let results = json!({
        "message": "Project is updated successfully",
        "project_id": project_id,
        "project_name": project_name,
    });
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), Some(200), None),
    ))
}

// ---- PATCH /projects/privacy ----

/// `PATCH /projects/privacy`
pub async fn privacy(req: Req) -> Response {
    let response = match privacy_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    core::vary_cookie(response)
}

async fn privacy_inner(req: &Req) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    if !is_json(req) {
        return Err(Fail::App("please provide json data"));
    }
    let data = body_json(req)?;
    let project_id = get(&data, "project_id")?.clone();
    let requested = get(&data, "privacy_level")?.clone();
    let level = privacy_level(&requested).ok_or(Fail::App(PRIVACY_MESSAGE))?;

    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    let Some(row) = find_user_session(&mut tx, &project_id, true).await? else {
        return Err(Fail::App("fail. project not found"));
    };
    if row.try_get::<Option<i32>, _>("tbl_user_sessions_user_id")? != Some(user.id) {
        return Ok(forbidden("forbidden, only the project owner can change its privacy").await);
    }
    let mut changes = Vec::new();
    if row.try_get::<Option<i16>, _>("tbl_user_sessions_privacy_level")? != Some(level) {
        changes.push(("privacy_level", Lit::from_json(&requested)));
    }
    if row.try_get::<Option<i32>, _>("tbl_user_sessions_updated_by")? != Some(user.id) {
        changes.push(("updated_by", Lit::Int(user.id.into())));
    }
    if !changes.is_empty() {
        let id = row.try_get("tbl_user_sessions_id")?;
        update_user_session(&mut tx, id, changes).await?;
    }
    tx.commit().await?;

    let results = json!({
        "message": "Project privacy is updated successfully",
        "project_id": project_id,
        // the request's value as sent: 2.0 stays 2.0
        "privacy_level": requested,
    });
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), Some(200), None),
    ))
}

// ---- DELETE /projects ----

/// `DELETE /projects`: a soft delete (`is_active = 0`).
pub async fn delete(req: Req) -> Response {
    let request_data = request_data(&req);
    let mut log = Log {
        session_id: Value::Null,
        project_name: json!(""),
    };
    let mut flushed = false;
    let response = match delete_inner(&req, &mut log).await {
        Ok(response) => response,
        Err(fail) => {
            flushed = matches!(fail, Fail::Flush(_));
            fail_response(fail).await
        }
    };
    if flushed {
        // log_func reads current_user.is_authenticated off the expired user: it raises, no row
        return core::vary_cookie(response);
    }
    write_log(
        &req,
        "projects_delete",
        request_data,
        log,
        core::vary_cookie(response),
    )
    .await
}

async fn delete_inner(req: &Req, log: &mut Log) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    if !is_json(req) {
        return Err(Fail::App("please provide json data"));
    }
    let data = body_json(req)?;
    let project_id = get(&data, "project_id")?.clone();
    log.session_id = project_id.clone();

    let ctx = ctx().await;
    let mut tx = ctx.app.begin().await?;
    let Some(row) = find_user_session(&mut tx, &project_id, false).await? else {
        return Err(Fail::App("fail. project not found"));
    };
    if row.try_get::<Option<i32>, _>("tbl_user_sessions_user_id")? != Some(user.id) {
        return Ok(forbidden("forbidden, only the project owner can delete this project").await);
    }
    log.project_name = json!(row.try_get::<Option<String>, _>("tbl_user_sessions_project_name")?);

    // is_active always changes 1 -> 0; updated_by only when it differs
    let mut changes = vec![("is_active", Lit::Int(0))];
    if row.try_get::<Option<i32>, _>("tbl_user_sessions_updated_by")? != Some(user.id) {
        changes.push(("updated_by", Lit::Int(user.id.into())));
    }
    update_user_session(&mut tx, row.try_get("tbl_user_sessions_id")?, changes).await?;
    tx.commit().await?;

    Ok(core::json(
        200,
        &core::success(
            json!({}),
            Some(200),
            Some("project is deleted successfully"),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_matches_python_in() {
        assert_eq!(privacy_level(&json!(1)), Some(1));
        assert_eq!(privacy_level(&json!(3.0)), Some(3));
        assert_eq!(privacy_level(&json!(true)), None);
        assert_eq!(privacy_level(&json!(4)), None);
        assert_eq!(privacy_level(&json!(1.5)), None);
        assert_eq!(privacy_level(&json!("1")), None);
        assert_eq!(privacy_level(&Value::Null), None);
    }

    #[test]
    fn capwords_matches_python() {
        assert_eq!(capwords("  kota  BANDA\taceh "), "Kota Banda Aceh");
        assert_eq!(capwords("o'neil-x"), "O'neil-x");
        assert_eq!(capwords(""), "");
    }

    #[test]
    fn default_name_falls_back_to_session_prefix() {
        let site = json!({"district": "", "province": "jawa barat", "country": "INDONESIA"});
        assert_eq!(
            default_project_name(&site, "abc").ok(),
            Some("Jawa Barat district Project in Indonesia".into())
        );
        assert_eq!(
            default_project_name(&json!({"district": "x"}), "0123456789").ok(),
            Some("Project 01234567".into())
        );
        assert_eq!(
            default_project_name(&Value::Null, "ab").ok(),
            Some("Project ab".into())
        );
        assert!(default_project_name(&json!([1]), "ab").is_err());
        assert!(default_project_name(&json!({"district": 5, "country": "x"}), "ab").is_err());
    }

    #[test]
    fn prefill_matches_prefill_py() {
        assert_eq!(feasibility_prefill(None).ok(), Some(json!({})));
        assert_eq!(
            feasibility_prefill(Some(&Value::Null)).ok(),
            Some(json!({}))
        );
        let people = json!({"social_demography": {"household_number": 0},
                            "economy": {"avg_household_income": ""}});
        assert_eq!(
            feasibility_prefill(Some(&people)).ok(),
            Some(json!({"seHouseholds": 0}))
        );
        assert!(feasibility_prefill(Some(&json!({"economy": null}))).is_err());
        assert!(feasibility_prefill(Some(&json!([1]))).is_err());
    }

    #[test]
    fn template_check() {
        assert!(!template_disabled(&Value::Null));
        assert!(!template_disabled(&json!("General")));
        assert!(template_disabled(&json!("ccb")));
        assert!(template_disabled(&json!(0)));
    }

    #[test]
    fn filters_and_change_checks() {
        assert_eq!(eq_clause("c", "p", &Value::Null).0, "c IS NULL");
        assert_eq!(eq_clause("c", "p", &json!(true)).0, "c = true");
        assert_eq!(
            eq_clause("c", "p", &json!(5)).1,
            Some(("p".into(), Lit::Int(5)))
        );
        assert!(same_text(&json!("a"), &Some("a".into())));
        assert!(!same_text(&json!(1), &Some("1".into())));
        assert!(same_text(&Value::Null, &None));
    }

    #[test]
    fn uuid4_shape() {
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!("89ab".contains(&u[19..20]));
        assert_ne!(u, uuid4());
    }
}
