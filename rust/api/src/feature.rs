//! `application/apis/geo_apis/feature/routes.py` and `persist.py`: what the routes do around the
//! `nbstool-geo` runners (pre-stream checks and envelopes, `_limit_slots`, `_keepalive`,
//! `persist_ndjson` / `save_v3_sections`, the activity log rows).

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::core::{self, Req, ctx};
use crate::geo_write::{
    Fail, Py, PyResult, body_json, fail_response, is_json, py_err, py_str, str_param, to_json,
};
use crate::logging::{self, LogEntry};

/// `_SITECHAR_SLOTS`: staging runs five gunicorn processes with 2 slots each; this one process
/// holds their sum. A fair semaphore with `acquire_many` is the FIFO lock + slots of the original.
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(10);
const HEARTBEAT: &str = " ";
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

fn init_gdal() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(nbstool_geo::init_gdal);
}

// ---- request pieces ----

/// `request.args.to_dict(flat=False)`
fn args_lists(req: &Req) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in form_urlencoded::parse(req.parts.uri.query().unwrap_or("").as_bytes()) {
        if let Value::Array(list) = out.entry(k.into_owned()).or_insert_with(|| json!([])) {
            list.push(json!(v.into_owned()));
        }
    }
    out
}

/// `request.args.getlist(name)`
fn arg_list(req: &Req, name: &str) -> Vec<String> {
    form_urlencoded::parse(req.parts.uri.query().unwrap_or("").as_bytes())
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .collect()
}

/// `urllib.parse.quote_plus`
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `retry_url(process)`: `f"{script_root}{path}?{urlencode({'session_id': sid, 'process': p})}"`
fn retry_url(path: String, session_id: String) -> Box<dyn Fn(&str) -> String + Send> {
    Box::new(move |process| {
        format!(
            "{path}?session_id={}&process={}",
            quote_plus(&session_id),
            quote_plus(process)
        )
    })
}

// ---- checks before a stream opens ----

/// `Polygons.query.filter_by(session_id=...).first()`, not found -> 400.
async fn known_polygon(session_id: &Py) -> PyResult<String> {
    let not_found = || Fail::App("fail, session id Not found".into());
    let Some(sid) = str_param(session_id)? else {
        return Err(not_found());
    };
    let row: Option<i32> =
        crate::pq::query_scalar("select id from polygons where session_id = $1 limit 1")
            .bind(&sid)
            .fetch_optional(&ctx().await.app)
            .await?;
    row.map(|_| sid).ok_or_else(not_found)
}

/// `known_polygons.assert_area_size()`: only reads the setting (the size check itself is off).
async fn assert_area_size() -> PyResult<()> {
    let value: Option<Option<String>> =
        crate::pq::query_scalar("select value from tbl_master_settings where name = $1 limit 1")
            .bind("MAX_DRAW_AREA")
            .fetch_optional(&ctx().await.app)
            .await?;
    match value {
        None => Err(py_err("'NoneType' object has no attribute 'value'")),
        Some(None) => Err(py_err(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'",
        )),
        Some(Some(v)) => crate::geo_write::py_int(&v).map(|_| ()).ok_or_else(|| {
            py_err(format!(
                "invalid literal for int() with base 10: {}",
                crate::geo_write::py_repr(&Py::Str(v))
            ))
        }),
    }
}

fn geo_err(e: nbstool_geo::Error) -> Fail {
    py_err(e.message)
}

// ---- persist.py ----

/// persist_ndjson's start / end / aborted rows (`log_stream_event`). Recorded: the original
/// writes none of them. The run happens on the keepalive thread under
/// `copy_current_request_context`, which pushes a fresh app context, so `g.__session_id__` is
/// unset there and `log_stream_event` returns at once. Only the after_request row is written.
const STREAM_EVENT_ROWS: bool = false;

/// `dict(base or {})`: a stored non-dict value is read the way `dict()` reads it.
fn py_dict(base: Option<&Value>) -> Result<Map<String, Value>, String> {
    let type_name = |v: &Value| match v {
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        _ => "object",
    };
    let base = match base {
        None => return Ok(Map::new()),
        Some(v) if !truthy(v) => return Ok(Map::new()),
        Some(v) => v,
    };
    let pairs: Vec<Value> = match base {
        Value::Object(m) => return Ok(m.clone()),
        Value::Array(items) => items.clone(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        other => return Err(format!("'{}' object is not iterable", type_name(other))),
    };
    let mut out = Map::new();
    for (i, item) in pairs.iter().enumerate() {
        let (k, v) = match item {
            Value::String(s) if s.chars().count() == 2 => {
                let mut c = s.chars();
                (
                    c.next().map(String::from),
                    c.next().map(|c| Value::String(c.to_string())),
                )
            }
            Value::String(s) => {
                return Err(format!(
                    "dictionary update sequence element #{i} has length {}; 2 is required",
                    s.chars().count()
                ));
            }
            Value::Array(kv) if kv.len() == 2 => match &kv[0] {
                Value::String(k) => (Some(k.clone()), Some(kv[1].clone())),
                // shortcut: a non-str key (int, float, None) would key the dict by that value
                _ => return Err("a non-str key is not ported".into()),
            },
            Value::Array(kv) => {
                return Err(format!(
                    "dictionary update sequence element #{i} has length {}; 2 is required",
                    kv.len()
                ));
            }
            Value::Object(m) if m.len() != 2 => {
                return Err(format!(
                    "dictionary update sequence element #{i} has length {}; 2 is required",
                    m.len()
                ));
            }
            Value::Object(_) => return Err("a dict pair element is not ported".into()),
            _ => {
                return Err(format!(
                    "cannot convert dictionary update sequence element #{i} to a sequence"
                ));
            }
        };
        if let (Some(k), Some(v)) = (k, v) {
            out.insert(k, v);
        }
    }
    Ok(out)
}

/// `_merge(base, patch)`: one level; a dict value merges key by key, anything else replaces.
fn merge(base: Option<&Value>, patch: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut merged = py_dict(base)?;
    for (key, value) in patch {
        match (value, merged.get_mut(key)) {
            (Value::Object(v), Some(Value::Object(m))) => {
                for (k, x) in v {
                    m.insert(k.clone(), x.clone());
                }
            }
            _ => {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(merged)
}

/// `save_v3_sections(session_id, updates)`: merge each `{column: patch}` into the session's
/// DataAnalyzer row (created when missing), stamp `analyzer_version = 'v3'`, commit.
async fn save_v3_sections(
    session_id: &str,
    updates: &[(&'static str, Map<String, Value>)],
) -> Result<(), Fail> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut tx = ctx().await.app.begin().await?;
    let cols: Vec<&str> = updates.iter().map(|(c, _)| *c).collect();
    let select = format!(
        r#"select id, {} from "DataAnalyzer" where session_id = $1 limit 1"#,
        cols.iter()
            .map(|c| format!("{c}::text as {c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let row = crate::pq::query(&select)
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;
    let mut merged = Vec::new();
    for (column, patch) in updates {
        let stored: Option<Value> = match &row {
            Some(r) => r
                .try_get::<Option<String>, _>(*column)?
                .and_then(|t| serde_json::from_str(&t).ok()),
            None => None,
        };
        let value = Value::Object(merge(stored.as_ref(), patch).map_err(py_err)?);
        // SQLAlchemy emits no UPDATE for a value equal to the loaded one
        if row.is_some() && stored.as_ref().is_some_and(|s| py_json_eq(s, &value)) {
            continue;
        }
        merged.push((*column, core::to_flask_json(&value)));
    }
    match row {
        Some(_) if merged.is_empty() => {}
        Some(r) => {
            let id: i32 = r.try_get("id")?;
            let sets: Vec<String> = merged
                .iter()
                .enumerate()
                .map(|(i, (c, _))| format!("{c} = ${}::jsonb", i + 1))
                .collect();
            let sql = format!(
                r#"update "DataAnalyzer" set {} where id = ${}"#,
                sets.join(", "),
                merged.len() + 1
            );
            let mut q = crate::pq::query(&sql);
            for (_, text) in &merged {
                q = q.bind(text);
            }
            q.bind(id).execute(&mut *tx).await?;
        }
        None => {
            let names: Vec<&str> = merged.iter().map(|(c, _)| *c).collect();
            let values: Vec<String> = (0..merged.len())
                .map(|i| format!("${}::jsonb", i + 2))
                .collect();
            let sql = format!(
                r#"insert into "DataAnalyzer" (session_id, {}) values ($1, {})"#,
                names.join(", "),
                values.join(", ")
            );
            let mut q = crate::pq::query(&sql).bind(session_id);
            for (_, text) in &merged {
                q = q.bind(text);
            }
            q.execute(&mut *tx).await?;
        }
    }
    // UserSessions.updated_at has onupdate=get_date: it is set with the change
    crate::pq::query(
        "update tbl_user_sessions set analyzer_version = 'v3', updated_at = $2 where id = \
         (select id from tbl_user_sessions where session_id = $1 limit 1) \
         and analyzer_version is distinct from 'v3'",
    )
    .bind(session_id)
    .bind(core::get_date())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Python's `==` on json-loaded values: numbers by value (1 == 1.0), dicts regardless of order.
fn py_json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(i), Some(j)) => i == j,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Bool(x), Value::Number(n)) | (Value::Number(n), Value::Bool(x)) => {
            n.as_f64() == Some(f64::from(u8::from(*x)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| py_json_eq(v, w)))
        }
        _ => a == b,
    }
}

/// Where a stream line's `data` is saved: `(column, nested)`.
type Spec = fn(&str) -> Option<(&'static str, bool)>;

/// `persist_ndjson`'s state over one run.
#[derive(Default)]
struct Persist {
    updates: Vec<(&'static str, Map<String, Value>)>,
    statuses: Map<String, Value>,
    emitted: u64,
    ended: bool,
}

impl Persist {
    async fn feed(&mut self, line: &str, session_id: &str, spec: Spec, entry: &LogEntry) {
        // json.loads(line): NaN / Infinity tokens read; sanitize_for_jsonb turns them into null
        let payload = match crate::geo_write::json_loads(line) {
            Ok(p) => to_json(&p),
            Err(e) => {
                eprintln!("WARNING persist_ndjson: unreadable line: {e}");
                return;
            }
        };
        let process = payload["process"].as_str().unwrap_or_default().to_string();
        let error_status = &payload["error_status"];
        if truthy(error_status) {
            self.statuses.insert(process.clone(), error_status.clone());
        }
        if process == "end" {
            self.ended = true;
            if let Err(e) = save_v3_sections(session_id, &self.updates).await {
                let text = match e {
                    Fail::Db(e) => e.to_string(),
                    Fail::App(m) | Fail::Py(m) | Fail::Flush(m) => m,
                };
                eprintln!("ERROR failed to persist v3 results for session {session_id}: {text}");
            }
            let failed = self.statuses.values().any(|s| s["state"] == "partial");
            let statuses = if self.statuses.is_empty() {
                Value::Null
            } else {
                Value::Object(self.statuses.clone())
            };
            if STREAM_EVENT_ROWS {
                logging::log_stream_event(
                entry,
                json!({ "stream_run": "end", "components_emitted": self.emitted, "error_status": statuses }),
                failed,
            )
            .await;
            }
        } else if process != "preparation" {
            self.emitted += 1;
            let data = &payload["data"];
            if truthy(data)
                && let Some((column, nested)) = spec(&process)
            {
                let patch = if nested {
                    let mut m = Map::new();
                    m.insert(process.clone(), data.clone());
                    m
                } else {
                    data.as_object().cloned().unwrap_or_default()
                };
                match self.updates.iter_mut().find(|(c, _)| *c == column) {
                    // both sides are dicts here: the merge cannot fail
                    Some((_, acc)) => {
                        *acc = merge(Some(&Value::Object(acc.clone())), &patch).unwrap_or_default()
                    }
                    None => self.updates.push((column, patch)),
                }
            }
        }
    }

    async fn aborted(&self, entry: &LogEntry) {
        if self.ended || !STREAM_EVENT_ROWS {
            return;
        }
        let statuses = if self.statuses.is_empty() {
            Value::Null
        } else {
            Value::Object(self.statuses.clone())
        };
        logging::log_stream_event(
            entry,
            json!({ "stream_run": "aborted", "components_emitted": self.emitted, "error_status": statuses }),
            true,
        )
        .await;
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

/// The NDJSON response: `_limit_slots(persist_ndjson(run), weight)` with `_keepalive`, the
/// after_request row ("streamed response opened") first. `start` begins the run once the slots
/// are held (the runners start their threads when called).
/// `?error_test=` on the client side of persist: `stream` keeps preparation + 3 lines then stops
/// without `end`; `data` rewrites each component's `error_status` in rotation.
enum Post {
    Plain,
    Truncate,
    Overlay(Box<dyn Fn(&str) -> String + Send>),
}

const ERROR_TEST_KEEP: usize = 4;

/// `_error_test_states(retry_url)[slot](name)`
fn error_test_state(slot: usize, name: &str, retry: &dyn Fn(&str) -> String) -> Value {
    match slot {
        0 => json!({"state": "partial", "retryable": true, "retry_url": retry(name),
                    "messages": [format!("SIMULATED: {name} was made to crash by error_test=data, so it carries no data. Nothing is actually wrong.")]}),
        1 => json!({"state": "partial", "retryable": true, "retry_url": retry(name),
                    "messages": [format!("SIMULATED: {name} was made to report an incomplete result by error_test=data. Nothing is actually wrong.")]}),
        2 => json!({"state": "failed", "retryable": false, "retry_url": null,
                    "messages": [format!("SIMULATED: {name} was made to report that this is the final answer by error_test=data. Nothing is actually wrong.")]}),
        _ => Value::Null,
    }
}

async fn ndjson(
    entry: LogEntry,
    weight: u32,
    session_id: String,
    spec: Spec,
    post: Post,
    start: Box<
        dyn FnOnce() -> Result<std::sync::mpsc::Receiver<String>, nbstool_geo::Error> + Send,
    >,
) -> Response {
    logging::log_request(&entry, 200, "application/x-ndjson", None).await;
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(1);
    tokio::spawn(async move {
        let permit = loop {
            match tokio::time::timeout(HEARTBEAT_EVERY, SLOTS.acquire_many(weight)).await {
                Ok(Ok(permit)) => break permit,
                Ok(Err(_)) => return,
                Err(_) => {
                    if tx
                        .send(Bytes::from_static(HEARTBEAT.as_bytes()))
                        .await
                        .is_err()
                    {
                        return; // gone while queued: nothing was started, nothing is logged
                    }
                }
            }
        };
        if STREAM_EVENT_ROWS {
            logging::log_stream_event(&entry, json!({ "stream_run": "start" }), false).await;
        }
        let mut persist = Persist::default();
        let lines = match tokio::task::spawn_blocking(start).await {
            Ok(Ok(lines)) => lines,
            _ => {
                persist.aborted(&entry).await;
                return;
            }
        };
        // the runner's one-line channel, handed to this task one line at a time
        let (ltx, mut lrx) = tokio::sync::mpsc::channel::<String>(1);
        std::thread::spawn(move || {
            for line in lines {
                if ltx.blocking_send(line).is_err() {
                    return;
                }
            }
        });
        let mut index: i64 = -1;
        loop {
            let line = match tokio::time::timeout(HEARTBEAT_EVERY, lrx.recv()).await {
                Err(_) => {
                    if tx
                        .send(Bytes::from_static(HEARTBEAT.as_bytes()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                Ok(None) => break,
                Ok(Some(line)) => line,
            };
            persist.feed(&line, &session_id, spec, &entry).await;
            index += 1;
            let line = match &post {
                Post::Plain => line,
                Post::Truncate if index as usize >= ERROR_TEST_KEEP => {
                    // the original's producer thread runs persist one queued line plus one pulled
                    // line ahead of the client: it pulls (and persists) the line after this one
                    // before it notices the client is gone, so a short run is still saved
                    if let Some(next) = lrx.recv().await {
                        persist.feed(&next, &session_id, spec, &entry).await;
                    }
                    break;
                }
                Post::Truncate => line,
                Post::Overlay(retry) => match serde_json::from_str::<Value>(&line) {
                    Ok(mut payload) => {
                        let process = payload["process"].as_str().unwrap_or_default().to_string();
                        if process != "preparation" && process != "end" {
                            let slot = ((index - 1).rem_euclid(4)) as usize;
                            payload["error_status"] =
                                error_test_state(slot, &process, retry.as_ref());
                            if slot == 0 {
                                payload["data"] = json!({});
                            }
                            nbstool_geo::pipeline::pack(&payload)
                        } else {
                            line
                        }
                    }
                    Err(_) => line,
                },
            };
            if tx.send(Bytes::from(line)).await.is_err() {
                break; // client gone: the runner stops at its next line
            }
        }
        persist.aborted(&entry).await;
        drop(lrx);
        drop(permit);
    });
    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|b| (Ok::<_, std::convert::Infallible>(b), rx))
    });
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/x-ndjson"),
    );
    let template = core::respond(200, "application/x-ndjson", "");
    for (k, v) in template.headers() {
        response.headers_mut().insert(k.clone(), v.clone());
    }
    core::vary_cookie(response)
}

fn threat_spec(_: &str) -> Option<(&'static str, bool)> {
    Some(("threat_json", true))
}

fn benefit_spec(_: &str) -> Option<(&'static str, bool)> {
    Some(("benefit_json", true))
}

/// A failure before the stream: the envelope, logged by after_request when a session is set.
async fn pre_stream_failure(entry: &LogEntry, fail: Fail) -> Response {
    let response = fail_response(fail).await;
    let response = if entry.session_id.as_deref().is_some_and(|s| !s.is_empty()) {
        core::vary_cookie(response)
    } else {
        response
    };
    logging::log_response(entry, response).await
}

// ---- GET /geos/feature/threat ----

/// `GET /geos/feature/threat` (`geo_feature_threat`)
pub async fn threat(req: Req) -> Response {
    init_gdal();
    let user = core::current_user(&req).await.ok().flatten();
    let session_id = core::arg(&req, "session_id");
    let entry = LogEntry {
        log_it: true,
        api_name: Some("geo_feature_threat".into()),
        session_id: session_id.clone(),
        request_data: Value::Object(args_lists(&req)),
        description_data: Value::Null,
        user_id: user.as_ref().map(|u| u.id),
    };
    let wanted = {
        let w = arg_list(&req, "process");
        if w.is_empty() { None } else { Some(w) }
    };
    let pre = async {
        let sid = known_polygon(&session_id.clone().map_or(Py::None, Py::Str)).await?;
        assert_area_size().await?;
        if let Some(w) = &wanted {
            let unknown: Vec<&str> = w
                .iter()
                .map(String::as_str)
                .filter(|n| !nbstool_geo::threat::run_threat::SECTIONS.contains(n))
                .collect();
            if !unknown.is_empty() {
                return Err(Fail::App(format!(
                    "fail, unknown process: {}",
                    unknown.join(", ")
                )));
            }
        }
        let app = &ctx().await.app;
        let aoi = nbstool_geo::common::prepare_aoi_from_session(app, &sid)
            .await
            .map_err(geo_err)?;
        let settings = nbstool_geo::settings::Settings::load(app)
            .await
            .map_err(geo_err)?;
        let keys = nbstool_geo::db::MappingKeys::load(app).await;
        Ok::<_, Fail>((sid, Arc::new(aoi), Arc::new(settings), Arc::new(keys)))
    };
    let (sid, aoi, settings, keys) = match pre.await {
        Ok(v) => v,
        Err(fail) => return pre_stream_failure(&entry, fail).await,
    };
    let retry = retry_url(req.parts.uri.path().to_string(), sid.clone());
    // the stream borrows `wanted`: both live on the producer thread (pipeline::spawn_lines' job)
    let start = Box::new(move || {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            match nbstool_geo::threat::run_threat::stream_threat(
                settings,
                keys,
                aoi,
                wanted.as_deref(),
                Some(retry),
            ) {
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    for line in stream {
                        if tx.send(line).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        ready_rx.recv().unwrap_or_else(|_| {
            Err(nbstool_geo::Error::new(
                "RuntimeError",
                "threat producer stopped",
            ))
        })?;
        Ok(rx)
    });
    ndjson(entry, 1, sid, threat_spec, Post::Plain, start).await
}

// ---- POST /geos/feature/benefit ----

/// `payload.get(name, default)`: the default only when the key is absent.
fn param(payload: &Py, name: &str, default: Py) -> PyResult<Py> {
    match payload {
        Py::Dict(items) => Ok(items
            .iter()
            .find(|(k, _)| k == name)
            .map_or(default, |(_, v)| v.clone())),
        other => Err(py_err(format!(
            "'{}' object has no attribute 'get'",
            other.type_name()
        ))),
    }
}

/// `int(v)`: Err(None) is a TypeError/ValueError (the route's 400), Err(Some) another exception.
fn py_int_of(v: &Py) -> Result<i64, Option<String>> {
    match v {
        Py::Bool(b) => Ok(*b as i64),
        Py::Int(i) => Ok(*i),
        Py::Float(f) if f.is_nan() => Err(None),
        Py::Float(f) if f.is_infinite() => {
            Err(Some("cannot convert float infinity to integer".into()))
        }
        Py::Float(f) => Ok(f.trunc() as i64),
        Py::Str(s) => crate::geo_write::py_int(s).ok_or(None),
        _ => Err(None),
    }
}

/// `float(v)`: Err is a TypeError/ValueError.
fn py_float_of(v: &Py) -> Result<f64, ()> {
    crate::geo_write::py_float(v).map_err(|_| ())
}

fn session_text(v: &Py) -> Option<String> {
    match v {
        Py::None => None,
        other => Some(py_str(other)),
    }
}

/// `POST /geos/feature/benefit` (`geo_feature_benefit_v3`)
pub async fn benefit(req: Req) -> Response {
    init_gdal();
    let mut entry = LogEntry {
        log_it: true,
        api_name: Some("geo_feature_benefit_v3".into()),
        ..LogEntry::default()
    };
    let user = core::current_user(&req).await.ok().flatten();
    entry.user_id = user.as_ref().map(|u| u.id);
    let pre = benefit_pre(&req, &mut entry).await;
    let (sid, aoi, settings, request) = match pre {
        Ok(v) => v,
        Err(fail) => return pre_stream_failure(&entry, fail).await,
    };
    let start =
        Box::new(move || nbstool_geo::benefit::stream_benefit_lines(settings, aoi, request, 16));
    ndjson(entry, 1, sid, benefit_spec, Post::Plain, start).await
}

type BenefitRun = (
    String,
    Arc<nbstool_geo::common::Aoi>,
    Arc<nbstool_geo::settings::Settings>,
    nbstool_geo::benefit::BenefitRequest,
);

async fn benefit_pre(req: &Req, entry: &mut LogEntry) -> PyResult<BenefitRun> {
    if !is_json(req) {
        return Err(Fail::App("please provide json data".into()));
    }
    let payload = body_json(req).map_err(py_err)?;
    entry.request_data = to_json(&payload);
    let session_id = param(&payload, "session_id", Py::None)?;
    entry.session_id = session_text(&session_id);
    let sid = known_polygon(&session_id).await?;
    assert_area_size().await?;

    let numbers = || {
        Fail::App(
            "fail, duration_years/ecosystem_class/leakage/uncertainty/buffer must be numbers"
                .into(),
        )
    };
    let int_of = |v: Py| py_int_of(&v).map_err(|e| e.map_or_else(numbers, py_err));
    let duration_years = int_of(param(&payload, "duration_years", Py::Int(30))?)?;
    let ecosystem_class = int_of(param(&payload, "ecosystem_class", Py::Int(1))?)?;
    let deduction = |name: &str| -> PyResult<Option<f64>> {
        match param(&payload, name, Py::None)? {
            Py::None => Ok(None),
            v => py_float_of(&v).map(Some).map_err(|_| numbers()),
        }
    };
    let leakage = deduction("leakage")?;
    let uncertainty = deduction("uncertainty")?;
    let buffer = deduction("buffer")?;
    if !(1..=3).contains(&ecosystem_class) {
        return Err(Fail::App(
            "fail, ecosystem_class must be 1 (forest), 2 (mangrove) or 3 (peatland)".into(),
        ));
    }
    let carbon_project = ["yes", "y", "true", "1"].contains(
        &py_str(&param(&payload, "carbon_project", Py::Str("yes".into()))?)
            .to_lowercase()
            .as_str(),
    );
    let selections = payload.get("selections")?;
    if !matches!(selections, Py::None | Py::Dict(_)) {
        return Err(Fail::App(
            "fail, selections must be an object keyed by ecosystem".into(),
        ));
    }
    let mut people_benefits = payload.get("people_benefits")?;
    if !matches!(people_benefits, Py::None | Py::Dict(_)) {
        return Err(Fail::App(
            "fail, people_benefits must be an object keyed by card".into(),
        ));
    }
    let wanted = payload.get("process")?;
    let wanted = if wanted.truthy() {
        let names = wanted.iter()?;
        let unknown: Vec<&Py> = names
            .iter()
            .filter(|n| match n {
                Py::Str(s) => {
                    s == "activity_stage"
                        || !nbstool_geo::benefit::WEIGHTS.iter().any(|(w, _)| w == s)
                }
                _ => true,
            })
            .collect();
        if !unknown.is_empty() {
            let mut texts = Vec::new();
            for (i, n) in unknown.iter().enumerate() {
                match n {
                    Py::Str(s) => texts.push(s.clone()),
                    other => {
                        return Err(py_err(format!(
                            "sequence item {i}: expected str instance, {} found",
                            other.type_name()
                        )));
                    }
                }
            }
            return Err(Fail::App(format!(
                "fail, unknown process: {}",
                texts.join(", ")
            )));
        }
        Some(names.iter().map(py_str).collect::<Vec<_>>())
    } else {
        None
    };
    nbstool_geo::benefit::validate(
        duration_years,
        leakage.unwrap_or(15.0),
        uncertainty.unwrap_or(10.0),
        buffer.unwrap_or(12.0),
    )
    .map_err(|e| Fail::App(format!("fail, {}", e.message)))?;

    let app = &ctx().await.app;
    let row = crate::pq::query(
        r#"select site_information_json::text as site, benefit_json::text as benefit from "DataAnalyzer" where session_id = $1 limit 1"#,
    )
    .bind(&sid)
    .fetch_optional(app)
    .await?;
    let column = |name: &str| -> PyResult<Py> {
        let text: Option<String> = match &row {
            Some(r) => r.try_get(name)?,
            None => None,
        };
        Ok(text
            .and_then(|t| crate::geo_write::json_loads(&t).ok())
            .unwrap_or(Py::None))
    };
    let site = column("site")?;
    let site = if site.truthy() {
        site
    } else {
        Py::Dict(vec![])
    };
    let rate_pct = match site.get("historical_deforestation_percentage")? {
        Py::Int(i) => Some(i as f64),
        Py::Float(f) => Some(f),
        // shortcut: a non-numeric stored rate reaches 5.2 in the original (its error line); here None
        _ => None,
    };
    if matches!(people_benefits, Py::None) {
        let stored = column("benefit")?;
        let stored = if stored.truthy() {
            stored
        } else {
            Py::Dict(vec![])
        };
        let assumptions = stored.get("assumptions")?;
        let assumptions = if assumptions.truthy() {
            assumptions
        } else {
            Py::Dict(vec![])
        };
        people_benefits = assumptions.get("people_benefits")?;
    }
    let aoi = nbstool_geo::common::prepare_aoi_from_session(app, &sid)
        .await
        .map_err(geo_err)?;
    let settings = nbstool_geo::settings::Settings::load(app)
        .await
        .map_err(geo_err)?;
    let request = nbstool_geo::benefit::BenefitRequest {
        duration_years,
        rate_pct,
        carbon_project,
        leakage,
        uncertainty,
        buffer,
        ecosystem_class,
        selections: (!matches!(selections, Py::None)).then(|| to_json(&selections)),
        wanted,
        people_benefits: (!matches!(people_benefits, Py::None)).then(|| to_json(&people_benefits)),
    };
    Ok((sid, Arc::new(aoi), Arc::new(settings), request))
}

// ---- POST /geos/feature/benefit/people ----

/// `POST /geos/feature/benefit/people` (`geo_feature_benefit_people`)
pub async fn benefit_people(req: Req) -> Response {
    let mut entry = LogEntry {
        log_it: true,
        api_name: Some("geo_feature_benefit_people".into()),
        ..LogEntry::default()
    };
    let user = core::current_user(&req).await.ok().flatten();
    entry.user_id = user.as_ref().map(|u| u.id);
    let response = match benefit_people_run(&req, &mut entry).await {
        Ok(body) => core::json(200, &body),
        Err(fail) => fail_response(fail).await,
    };
    let response = if entry.session_id.as_deref().is_some_and(|s| !s.is_empty()) {
        core::vary_cookie(response)
    } else {
        response
    };
    logging::log_response(&entry, response).await
}

async fn benefit_people_run(req: &Req, entry: &mut LogEntry) -> PyResult<Value> {
    if !is_json(req) {
        return Err(Fail::App("please provide json data".into()));
    }
    let payload = body_json(req).map_err(py_err)?;
    entry.request_data = to_json(&payload);
    let session_id = param(&payload, "session_id", Py::None)?;
    entry.session_id = session_text(&session_id);
    let sid = known_polygon(&session_id).await?;
    let keys = nbstool_geo::benefit::PEOPLE_BENEFIT_KEYS;
    let patch = payload.get("people_benefits")?;
    let items = match &patch {
        Py::Dict(items) if !items.is_empty() => items.clone(),
        _ => {
            return Err(Fail::App(
                "fail, people_benefits must be a non-empty object keyed by card".into(),
            ));
        }
    };
    let unknown: Vec<&str> = items
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !keys.contains(k))
        .collect();
    if !unknown.is_empty() {
        return Err(Fail::App(format!(
            "fail, unknown card: {} (cards: {})",
            unknown.join(", "),
            keys.join(", ")
        )));
    }
    let bad: Vec<&str> = items
        .iter()
        .filter(|(_, v)| !matches!(v, Py::None | Py::Dict(_)))
        .map(|(k, _)| k.as_str())
        .collect();
    if !bad.is_empty() {
        return Err(Fail::App(format!(
            "fail, a card is an object (statement, answer) or null: {}",
            bad.join(", ")
        )));
    }
    let stored: Option<String> = crate::pq::query_scalar(
        r#"select benefit_json::text from "DataAnalyzer" where session_id = $1 limit 1"#,
    )
    .bind(&sid)
    .fetch_optional(&ctx().await.app)
    .await?
    .flatten();
    let stored = stored
        .and_then(|t| crate::geo_write::json_loads(&t).ok())
        .unwrap_or(Py::None);
    let stored = if stored.truthy() {
        stored
    } else {
        Py::Dict(vec![])
    };
    let assumptions = stored.get("assumptions")?;
    let assumptions = if assumptions.truthy() {
        assumptions
    } else {
        Py::Dict(vec![])
    };
    let stored = assumptions.get("people_benefits")?;
    let stored = if stored.truthy() {
        stored
    } else {
        Py::Dict(vec![])
    };
    let mut slots = Map::new();
    for key in keys {
        let value = match items.iter().find(|(k, _)| k == key) {
            Some((_, v)) => v.clone(),
            None => stored.get(key)?,
        };
        slots.insert(key.to_string(), to_json(&value));
    }
    let mut assumptions_patch = Map::new();
    assumptions_patch.insert("people_benefits".into(), Value::Object(slots.clone()));
    let mut patch = Map::new();
    patch.insert("assumptions".into(), Value::Object(assumptions_patch));
    save_v3_sections(&sid, &[("benefit_json", patch)]).await?;
    Ok(core::success(
        json!({ "people_benefits": slots }),
        None,
        None,
    ))
}

// ---- GET /geos/feature/site-characterisation, /analysis, /pathway ----

/// The module columns of `SITECHAR_COLUMNS` (run_general/nature/climate/people `processes`).
fn sitechar_column(name: &str) -> Option<&'static str> {
    const GENERAL: [&str; 8] = [
        "ecosystem type",
        "administrative boundaries",
        "protected areas",
        "terrain",
        "historical deforestation",
        "deforestation risk",
        "natural disaster risks",
        "land cover",
    ];
    const NATURE: [&str; 7] = [
        "forest landscape integrity",
        "key biodiversity areas",
        "habitat area",
        "key species presence",
        "conservation significance",
        "endangered trees",
        "plant species occurrence",
    ];
    const CLIMATE: [&str; 9] = [
        "current carbon storage",
        "soil organic carbon",
        "carbon shares",
        "annual temperature",
        "annual precipitation",
        "fire susceptibility",
        "burned area",
        "soil classification",
        "peat depth",
    ];
    const PEOPLE: [&str; 3] = [
        "people demography",
        "vulnerability assessment",
        "social statistics",
    ];
    if GENERAL.contains(&name) {
        Some("site_information_json")
    } else if NATURE.contains(&name) {
        Some("nature_json")
    } else if CLIMATE.contains(&name) {
        Some("climate_json")
    } else if PEOPLE.contains(&name) {
        Some("people_json")
    } else {
        None
    }
}

fn sitechar_spec(name: &str) -> Option<(&'static str, bool)> {
    sitechar_column(name).map(|c| (c, false))
}

/// `ANALYSIS_COLUMNS`
fn analysis_spec(name: &str) -> Option<(&'static str, bool)> {
    if let Some(c) = sitechar_column(name) {
        return Some((c, false));
    }
    if nbstool_geo::threat::run_threat::SECTIONS.contains(&name) {
        return Some(("threat_json", true));
    }
    matches!(name, "pathway" | "conservation actions")
        .then_some(("intervention_eligibility_json", false))
}

/// The pools the database-backed components read (geo's own: simple query protocol).
fn geo_pools() -> Result<(sqlx::PgPool, sqlx::PgPool), nbstool_geo::Error> {
    static POOLS: std::sync::OnceLock<(sqlx::PgPool, sqlx::PgPool)> = std::sync::OnceLock::new();
    if let Some(p) = POOLS.get() {
        return Ok(p.clone());
    }
    let var = |n: &str| std::env::var(n).unwrap_or_default();
    let gis = nbstool_geo::db::gis_pool(&var("GIS_DB_SQLALCHEMY_URI"))?;
    let social = nbstool_geo::db::social_pool(&var("DB_SQLALCHEMY_URI"))?;
    Ok(POOLS.get_or_init(|| (gis, social)).clone())
}

/// The checks every GET stream route makes before it opens: polygon, area setting, `process`.
async fn stream_pre(
    req: &Req,
    known: Option<&[String]>,
) -> PyResult<(String, Option<Vec<String>>)> {
    let session_id = core::arg(req, "session_id");
    let sid = known_polygon(&session_id.map_or(Py::None, Py::Str)).await?;
    assert_area_size().await?;
    let wanted = arg_list(req, "process");
    let wanted = if wanted.is_empty() {
        None
    } else {
        Some(wanted)
    };
    if let (Some(w), Some(known)) = (&wanted, known) {
        let unknown: Vec<&str> = w
            .iter()
            .map(String::as_str)
            .filter(|n| !known.iter().any(|k| k == n))
            .collect();
        if !unknown.is_empty() {
            return Err(Fail::App(format!(
                "fail, unknown process: {}",
                unknown.join(", ")
            )));
        }
    }
    Ok((sid, wanted))
}

/// `prepare_aoi_from_session` plus the settings, mapping keys and database components of a run.
async fn run_inputs(
    sid: &str,
) -> PyResult<nbstool_geo::site_characterisation::run_site_characterisation::RunInputs> {
    let app = &ctx().await.app;
    let aoi = nbstool_geo::common::prepare_aoi_from_session(app, sid)
        .await
        .map_err(geo_err)?;
    let settings = nbstool_geo::settings::Settings::load(app)
        .await
        .map_err(geo_err)?;
    let keys = Arc::new(nbstool_geo::db::MappingKeys::load(app).await);
    let (gis, social) = geo_pools().map_err(geo_err)?;
    let provided = nbstool_geo::site_characterisation::run_site_characterisation::db_components(
        gis,
        social,
        keys.clone(),
        tokio::runtime::Handle::current(),
    );
    Ok(
        nbstool_geo::site_characterisation::run_site_characterisation::RunInputs {
            settings: Arc::new(settings),
            keys,
            aoi: Arc::new(aoi),
            provided: Arc::new(provided),
        },
    )
}

fn stream_entry(req: &Req, api: &str, user: Option<&core::User>) -> LogEntry {
    LogEntry {
        log_it: true,
        api_name: Some(api.into()),
        session_id: core::arg(req, "session_id"),
        request_data: Value::Object(args_lists(req)),
        description_data: Value::Null,
        user_id: user.map(|u| u.id),
    }
}

type Start =
    Box<dyn FnOnce() -> Result<std::sync::mpsc::Receiver<String>, nbstool_geo::Error> + Send>;

/// A runner started on its own thread with `wanted` (the stream borrows it).
fn start_on_thread<F, I>(wanted: Option<Vec<String>>, run: F) -> Start
where
    F: FnOnce(Option<&[String]>) -> Result<I, nbstool_geo::Error> + Send + 'static,
    I: Iterator<Item = String>,
{
    Box::new(move || {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || match run(wanted.as_deref()) {
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
            Ok(lines) => {
                let _ = ready_tx.send(Ok(()));
                for line in lines {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            }
        });
        ready_rx.recv().unwrap_or_else(|_| {
            Err(nbstool_geo::Error::new(
                "RuntimeError",
                "stream producer stopped",
            ))
        })?;
        Ok(rx)
    })
}

/// `GET /geos/feature/site-characterisation` (`geo_feature_site_characterisation`)
pub async fn site_characterisation(req: Req) -> Response {
    init_gdal();
    let user = core::current_user(&req).await.ok().flatten();
    let entry = stream_entry(&req, "geo_feature_site_characterisation", user.as_ref());
    let known: Vec<String> =
        nbstool_geo::site_characterisation::run_site_characterisation::components()
            .into_iter()
            .map(|c| c.0)
            .collect();
    let pre = async {
        let (sid, wanted) = stream_pre(&req, Some(&known)).await?;
        let error_test = core::arg(&req, "error_test").filter(|e| !e.is_empty());
        if let Some(e) = &error_test
            && !["api", "stream", "data"].contains(&e.as_str())
        {
            return Err(Fail::App(format!(
                "fail, unknown error_test: {e}. Expected one of: api, stream, data"
            )));
        }
        if error_test.as_deref() == Some("api") {
            return Err(py_err(
                "SIMULATED: this request was made to fail by error_test=api. Nothing is actually wrong.",
            ));
        }
        let inputs = run_inputs(&sid).await?;
        Ok((sid, wanted, error_test, inputs))
    };
    let (sid, wanted, error_test, inputs) = match pre.await {
        Ok(v) => v,
        Err(fail) => return pre_stream_failure(&entry, fail).await,
    };
    let path = req.parts.uri.path().to_string();
    let retry = retry_url(path.clone(), sid.clone());
    let post = match error_test.as_deref() {
        Some("stream") => Post::Truncate,
        Some("data") => Post::Overlay(retry_url(path, sid.clone())),
        _ => Post::Plain,
    };
    let start = start_on_thread(wanted, move |w| {
        nbstool_geo::site_characterisation::run_site_characterisation::stream_site_characterisation(
            inputs,
            w,
            Some(retry),
        )
    });
    ndjson(entry, 1, sid, sitechar_spec, post, start).await
}

/// `GET /geos/feature/analysis` (`geo_feature_analysis`): the union, weight 2.
pub async fn analysis(req: Req) -> Response {
    init_gdal();
    let user = core::current_user(&req).await.ok().flatten();
    let entry = stream_entry(&req, "geo_feature_analysis", user.as_ref());
    let known: Vec<String> = nbstool_geo::run_analysis::components()
        .into_iter()
        .map(|c| c.0)
        .collect();
    let pre = async {
        let (sid, wanted) = stream_pre(&req, Some(&known)).await?;
        let inputs = run_inputs(&sid).await?;
        Ok::<_, Fail>((sid, wanted, inputs))
    };
    let (sid, wanted, inputs) = match pre.await {
        Ok(v) => v,
        Err(fail) => return pre_stream_failure(&entry, fail).await,
    };
    let retry = retry_url(req.parts.uri.path().to_string(), sid.clone());
    let start = start_on_thread(wanted, move |w| {
        nbstool_geo::run_analysis::stream_analysis(inputs, w, Some(retry))
    });
    ndjson(entry, 2, sid, analysis_spec, Post::Plain, start).await
}

/// `GET /geos/feature/pathway` (`geo_feature_pathway`): plain JSON, no slot; saved into
/// `intervention_eligibility_json` (a save failure is the route's 500).
pub async fn pathway(req: Req) -> Response {
    init_gdal();
    let user = core::current_user(&req).await.ok().flatten();
    let entry = stream_entry(&req, "geo_feature_pathway", user.as_ref());
    let run = async {
        let (sid, _) = stream_pre(&req, None).await?;
        let app = &ctx().await.app;
        let aoi = nbstool_geo::common::prepare_aoi_from_session(app, &sid)
            .await
            .map_err(geo_err)?;
        let settings = nbstool_geo::settings::Settings::load(app)
            .await
            .map_err(geo_err)?;
        let result = tokio::task::spawn_blocking(move || {
            nbstool_geo::pathway::run_pathway::run_pathway(&settings, &aoi)
        })
        .await
        .map_err(|e| py_err(e.to_string()))?
        .map_err(geo_err)?;
        let patch = result.as_object().cloned().unwrap_or_default();
        save_v3_sections(&sid, &[("intervention_eligibility_json", patch)]).await?;
        Ok::<_, Fail>(core::success(json!({ "result": result }), None, None))
    };
    let response = match run.await {
        Ok(body) => core::json(200, &body),
        Err(fail) => fail_response(fail).await,
    };
    let response = if entry.session_id.as_deref().is_some_and(|s| !s.is_empty()) {
        core::vary_cookie(response)
    } else {
        response
    };
    logging::log_response(&entry, response).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plus_is_pythons() {
        assert_eq!(quote_plus("all ecosystems"), "all+ecosystems");
        assert_eq!(quote_plus("a~b*c/é"), "a~b%2Ac%2F%C3%A9");
    }

    #[test]
    fn merge_is_one_level() {
        let base = json!({"a": {"x": 1, "y": 2}, "b": [1], "c": 1});
        let patch = json!({"a": {"y": 3}, "b": {"k": 1}, "d": 4});
        assert_eq!(
            Value::Object(merge(Some(&base), patch.as_object().unwrap()).unwrap()),
            json!({"a": {"x": 1, "y": 3}, "b": {"k": 1}, "c": 1, "d": 4})
        );
        assert!(py_json_eq(
            &json!({"a": [1, 2.0], "b": true}),
            &json!({"b": 1, "a": [1.0, 2]})
        ));
        assert!(!py_json_eq(&json!({"a": 1}), &json!({"a": 1, "b": null})));
        let err = merge(Some(&json!(["abcdefghijklmnopqrstu"])), &Map::new()).unwrap_err();
        assert_eq!(
            err,
            "dictionary update sequence element #0 has length 21; 2 is required"
        );
        assert_eq!(merge(Some(&json!([])), &Map::new()).unwrap(), Map::new());
    }
}
