//! The activity log: `application/utils/logger` (`logging`, `log_func`, `log_stream_event`) and
//! the `after_request` hook of `utils/extensions` that decides when to log.
//!
//! # API for route modules
//!
//! A route that sets `g.__log_it__` builds a [`LogEntry`] from what it put on `flask.g` and,
//! just before returning its response, calls
//!
//! ```ignore
//! crate::logging::log_request(&entry, status, content_type, Some(body_bytes)).await;
//! ```
//!
//! with the status, `Content-Type` and body bytes of the response it returns (`None` as the body
//! for a streamed response: Python logs only "streamed response opened" then). A streamed run
//! writes its lifecycle rows from inside the stream with [`log_stream_event`].
//!
//! Neither function ever fails or changes the response: every failure is a warning on stderr,
//! as the original's `app.logger.warning`.
//!
//! `entry.user_id` is `current_user.id` when `current_user.is_authenticated` (Python loads the
//! user inside `log_func` even when the route did not; a route that has not called
//! `core::current_user` must do so to fill it). Note Flask-Login touching the session in
//! `after_request` makes Flask add `Vary: Cookie` to every logged response
//! (`core::vary_cookie`); that header is the route's job, not this module's.

use serde_json::{Value, json};

/// What `log_func` reads from `flask.g` and `current_user`.
#[derive(Debug, Clone, Default)]
pub struct LogEntry {
    /// `g.__log_it__`
    pub log_it: bool,
    /// `g.__api_name__`: the `activity type` code in `tbl_master_dictionary`.
    pub api_name: Option<String>,
    /// `g.__session_id__` (Python never requires a str; routes pass it as text).
    pub session_id: Option<String>,
    /// `g.__request_data__`; `Value::Null` when unset.
    pub request_data: Value,
    /// `g.__description_data__`; `Value::Null` when unset.
    pub description_data: Value,
    /// `current_user.id` when authenticated.
    pub user_id: Option<i32>,
}

fn warn(msg: String) {
    eprintln!("WARNING {msg}");
}

fn api(entry: &LogEntry) -> &str {
    entry.api_name.as_deref().unwrap_or("None") // str(None)
}

fn has_session(entry: &LogEntry) -> bool {
    entry.session_id.as_deref().is_some_and(|s| !s.is_empty())
}

/// `after_request_callback` + `logging(response)`.
pub async fn log_request(entry: &LogEntry, status: u16, content_type: &str, body: Option<&[u8]>) {
    if !(entry.log_it && has_session(entry)) {
        return;
    }
    let log_type = if status == 200 {
        "LOG_SUCCESS"
    } else {
        "LOG_FAILED"
    };
    let resp_data = response_data(entry, status, content_type, body);
    if let Err(e) = log_func(entry, log_type, resp_data).await {
        warn(format!("error in logging (after_request): {e}"));
    }
}

/// `log_request` for a finished (not streamed) response: buffers its body, logs, rebuilds it.
pub async fn log_response(
    entry: &LogEntry,
    response: axum::response::Response,
) -> axum::response::Response {
    if !(entry.log_it && has_session(entry)) {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        warn(format!(
            "{} - after_request: unreadable response body",
            api(entry)
        ));
        return axum::response::Response::from_parts(parts, axum::body::Body::empty());
    };
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    log_request(entry, parts.status.as_u16(), content_type, Some(&bytes)).await;
    axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
}

/// `log_stream_event(data, failed)`: one row per streamed run's start / end / aborted.
pub async fn log_stream_event(entry: &LogEntry, data: Value, failed: bool) {
    if !has_session(entry) {
        return;
    }
    let log_type = if failed { "LOG_FAILED" } else { "LOG_SUCCESS" };
    let resp_data = json!({ "status_code": 200, "json_response": true, "data": data });
    if let Err(e) = log_func(entry, log_type, resp_data).await {
        warn(format!("{} - log_stream_event: {e}", api(entry)));
    }
}

/// A route's own `log_func(db.session, resp_data)` call (with `g.log_type_code` set by it).
pub async fn log_manual(entry: &LogEntry, log_type: &str, resp_data: Value) {
    if let Err(e) = log_func(entry, log_type, resp_data).await {
        warn(format!("{} - log_func: {e}", api(entry)));
    }
}

/// The `resp_data` dict `logging` builds from the response.
fn response_data(entry: &LogEntry, status: u16, content_type: &str, body: Option<&[u8]>) -> Value {
    let Some(body) = body else {
        return json!({
            "status_code": status,
            "json_response": false,
            "data": { "message": "streamed response opened" },
        });
    };
    // werkzeug Response.get_json(): None unless the mimetype is JSON
    let mimetype = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_json = mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"));
    if !is_json {
        return json!({ "status_code": status, "json_response": true, "data": null });
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(data) => json!({ "status_code": status, "json_response": true, "data": data }),
        Err(e) => {
            // shortcut: serde_json's message, not Python json's ("Expecting value: line 1 ..."),
            // unreachable for bodies core::json writes.
            warn(format!(
                "{} - after_request - logger (get_json): {e}",
                api(entry)
            ));
            json!({
                "status_code": status,
                "json_response": false,
                "data": {
                    "message": format!("error get response json: {e}"),
                    "text": py_bytes_repr(body),
                },
            })
        }
    }
}

/// Python's `str(bytes)`: `b'...'`.
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

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// markupsafe's `escape`.
fn html_escape(s: &str, out: &mut String) {
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
}

/// `render_template_string(description, data=...)`: Flask autoescapes string templates, and
/// Jinja prints values with Python's `str()` (`None`, `True`, float repr).
fn render_description(src: &str, data: &Value) -> Result<String, minijinja::Error> {
    use minijinja::value::ValueKind;
    let mut env = minijinja::Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::Html);
    env.set_formatter(|out, state, value| {
        let text = match value.kind() {
            ValueKind::Undefined => String::new(),
            ValueKind::None => "None".into(),
            ValueKind::Bool => if value.is_true() { "True" } else { "False" }.into(),
            ValueKind::Number => serde_json::to_value(value)
                .map(|n| crate::core::to_flask_json(&n).trim_end().to_string())
                .unwrap_or_else(|_| value.to_string()),
            // shortcut: lists/dicts print minijinja-style, not Python repr; descriptions use scalars
            _ => value.to_string(),
        };
        let escape = state.auto_escape() != minijinja::AutoEscape::None && !value.is_safe();
        if escape {
            let mut s = String::new();
            html_escape(&text, &mut s);
            out.write_str(&s)?;
        } else {
            out.write_str(&text)?;
        }
        Ok(())
    });
    env.render_str(
        src,
        minijinja::context! { data => minijinja::Value::from_serialize(data) },
    )
}

/// The PickleType bind: `None` is SQL NULL, anything else a pickle (protocol 3 here; Python
/// writes HIGHEST_PROTOCOL, both unpickle to the same value).
fn pickle(v: &Value) -> Result<Option<Vec<u8>>, serde_pickle::Error> {
    if v.is_null() {
        return Ok(None);
    }
    serde_pickle::to_vec(v, serde_pickle::SerOptions::new()).map(Some)
}

/// `Dictionary.query.filter_by(code=..., type=...).first()` -> (id, name, description).
async fn dictionary(
    tx: &mut sqlx::PgConnection,
    code: Option<&str>,
    kind: &str,
) -> Result<Option<(i32, String, Option<String>)>, sqlx::Error> {
    // `is not distinct from`: filter_by(code=None) is `code IS NULL`
    crate::pq::query_as(
        "select id, name, description from tbl_master_dictionary \
         where code is not distinct from $1 and type = $2 limit 1",
    )
    .bind(code)
    .bind(kind)
    .fetch_optional(tx)
    .await
}

/// `log_func(session, resp_data)`. Err only where Python raises out of it (the description
/// render); database failures are warned and swallowed here, as in Python.
async fn log_func(entry: &LogEntry, log_type_code: &str, resp_data: Value) -> Result<(), String> {
    let api_name = api(entry);
    // The new scoped_session: its own connection and transaction, apart from the route's.
    let mut tx = match crate::core::ctx().await.app.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            // shortcut: Python would warn three times (each lookup and the commit); one here
            warn(format!(
                "{api_name} - after_request - logger (db commit): {e}"
            ));
            return Ok(());
        }
    };
    // A failed lookup leaves the transaction aborted, so later statements fail as in Python.
    let log_type = dictionary(&mut tx, Some(log_type_code), "log type")
        .await
        .unwrap_or_else(|e| {
            warn(format!(
                "{api_name} - after_request - logger (get log type): {e}"
            ));
            None
        });
    let activity = dictionary(&mut tx, entry.api_name.as_deref(), "activity type")
        .await
        .unwrap_or_else(|e| {
            warn(format!(
                "{api_name} - after_request - logger (get activity type): {e}"
            ));
            None
        });
    let activity_description = activity.as_ref().and_then(|a| a.2.clone());

    let description = match &activity_description {
        Some(d) if !d.is_empty() && d.contains("{{") && truthy(&entry.description_data) => {
            // raised: the dropped transaction rolls back, no row
            Some(render_description(d, &entry.description_data).map_err(|e| e.to_string())?)
        }
        d => d.clone(),
    };

    let (request_pickle, response_pickle) = match (pickle(&entry.request_data), pickle(&resp_data))
    {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            // PickleType pickles at flush, inside the commit's try
            warn(format!(
                "{api_name} - after_request - logger (db commit): {e}"
            ));
            return Ok(());
        }
    };

    // created_at: get_date(), naive UTC. Column is `timestamp without time zone` per
    // migration 975320c3c290 (sa.DateTime()); not confirmed against the live DB.
    let inserted = crate::pq::query(
        "insert into tbl_logger_logs (log_type_id, log_type_code, log_type_name, \
         activity_type_id, activity_type_code, activity_type_name, description, session_id, \
         request_data, response_data, request_data_json, response_data_json, created_at, \
         created_by) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(log_type.as_ref().map(|t| t.0))
    .bind(log_type_code)
    .bind(log_type.as_ref().map(|t| t.1.clone()))
    .bind(activity.as_ref().map(|a| a.0))
    .bind(entry.api_name.as_deref())
    .bind(activity.as_ref().map(|a| a.1.clone()))
    .bind(description)
    .bind(entry.session_id.as_deref())
    .bind(request_pickle)
    .bind(response_pickle)
    // sanitize_for_jsonb is the identity on serde_json values (no NaN/Inf/Decimal possible);
    // a None request_data is JSON `null`, not SQL NULL (JSONB none_as_null=False)
    .bind(sqlx::types::Json(&entry.request_data))
    .bind(sqlx::types::Json(&resp_data))
    .bind(chrono::Utc::now().naive_utc())
    .bind(entry.user_id)
    .execute(&mut *tx)
    .await;
    let committed = match inserted {
        Ok(_) => tx.commit().await,
        Err(e) => Err(e),
    };
    if let Err(e) = committed {
        warn(format!(
            "{api_name} - after_request - logger (db commit): {e}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> LogEntry {
        LogEntry {
            log_it: true,
            session_id: Some("s".into()),
            ..Default::default()
        }
    }

    #[test]
    fn response_data_shapes() {
        let e = entry();
        assert_eq!(
            response_data(&e, 200, "application/json", Some(b"{\"a\":1}\n")),
            json!({"status_code": 200, "json_response": true, "data": {"a": 1}})
        );
        assert_eq!(
            response_data(&e, 400, "text/html; charset=utf-8", Some(b"<p>")),
            json!({"status_code": 400, "json_response": true, "data": null})
        );
        assert_eq!(
            response_data(&e, 200, "application/x-ndjson", None),
            json!({"status_code": 200, "json_response": false,
                   "data": {"message": "streamed response opened"}})
        );
        let bad = response_data(&e, 500, "application/problem+json", Some(b"nope"));
        assert_eq!(bad["json_response"], json!(false));
        assert_eq!(bad["data"]["text"], json!("b'nope'"));
        assert!(
            bad["data"]["message"]
                .as_str()
                .unwrap()
                .starts_with("error get response json: ")
        );
    }

    #[test]
    fn bytes_repr_matches_python() {
        assert_eq!(py_bytes_repr(b"a'b"), "b\"a'b\"");
        assert_eq!(py_bytes_repr(b"a'\"b"), "b'a\\'\"b'");
        assert_eq!(py_bytes_repr(b"\n\x00\xff\\"), "b'\\n\\x00\\xff\\\\'");
    }

    #[test]
    fn truthiness() {
        assert!(!truthy(&json!({})));
        assert!(!truthy(&json!(0.0)));
        assert!(!truthy(&json!("")));
        assert!(truthy(&json!({"project_name": ""})));
    }

    #[test]
    fn renders_like_flask() {
        let d = json!({"project_name": "A & <b>", "project_area": 12.0, "x": null, "t": true});
        assert_eq!(
            render_description(
                "{{ data.project_name }} {{ data.project_area }} {{ data.x }} {{ data.t }} {{ data.missing }}|",
                &d
            )
            .unwrap(),
            "A &amp; &lt;b&gt; 12.0 None True |"
        );
        assert_eq!(
            render_description("'{{ data.q }}'", &json!({"q": "\"'"})).unwrap(),
            "'&#34;&#39;'"
        );
        assert!(render_description("{{ data.missing.x }}", &d).is_err());
    }

    #[test]
    fn pickle_shape() {
        assert_eq!(pickle(&Value::Null).unwrap(), None);
        let bytes = pickle(&json!({"status_code": 200, "data": {"a": [1, 2.5, "é"]}}))
            .unwrap()
            .unwrap();
        use serde_pickle::{HashableValue as H, Value as P};
        let P::Dict(top) =
            serde_pickle::value_from_slice(&bytes, serde_pickle::DeOptions::new()).unwrap()
        else {
            panic!("not a dict")
        };
        assert_eq!(
            top.get(&H::String("status_code".into())),
            Some(&P::I64(200))
        );
        let Some(P::Dict(data)) = top.get(&H::String("data".into())) else {
            panic!("data")
        };
        assert_eq!(
            data.get(&H::String("a".into())),
            Some(&P::List(vec![
                P::I64(1),
                P::F64(2.5),
                P::String("é".into())
            ]))
        );
    }
}
