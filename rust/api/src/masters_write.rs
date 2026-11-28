//! `master_apis/organization/routes.py` (`POST`/`PUT /masters/organization`) and
//! `logger_apis/routes.py` (`GET /loggers/list`). Neither route sets `g.__log_it__`, so neither
//! writes a `tbl_logger_logs` row. Not wired yet: the original still answers these routes.

use axum::http::header;
use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::core::{self, Req, arg, ctx};
use crate::sa::{Lit, Stmt};

async fn bad(message: &str) -> Response {
    core::vary_cookie(core::json(400, &core::app_error(message, None)))
}

async fn internal(message: &str) -> Response {
    core::vary_cookie(core::json(500, &core::other_error(message, None).await))
}

// ---- request.get_json() ----

/// `request.is_json`: `application/json` or `application/*+json`, parameters ignored.
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

/// `request.get_json()` once `is_json` holds: a parse failure is Werkzeug's `BadRequest`, which
/// the route's `except Exception` turns into a 500. The text after the colon is serde's, not
/// Python's `json` module's (only shown in development).
fn get_json(body: &[u8]) -> Result<Value, String> {
    // json.loads(bytes) accepts a UTF-8 BOM
    let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    serde_json::from_slice(body).map_err(|_| core::BAD_REQUEST_TEXT.to_string()) // `flask run` is not in debug mode
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

/// `type(value).__name__`, for the AttributeError of `data.get` on a non-dict body.
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

/// `json.dumps(float)` for a pickled dict key (`float.__repr__`).
fn num_text(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if n.is_f64() => core::py_float_repr(f),
        _ => n.to_string(),
    }
}

/// What the route reads from the body, validated in the route's order.
#[derive(Debug, PartialEq)]
struct OrgInput {
    id: Value,
    name: Value,
    is_active: i16,
}

/// `Err((status, message))`: 400 for AppMessageException, 500 for the AttributeError.
fn org_input(data: &Value) -> Result<OrgInput, (u16, String)> {
    let Value::Object(map) = data else {
        return Err((
            500,
            format!("'{}' object has no attribute 'get'", py_type(data)),
        ));
    };
    let get = |k: &str| map.get(k).cloned().unwrap_or(Value::Null);
    let (id, name, is_active) = (get("id"), get("name"), get("is_active"));
    if !truthy(&name) {
        return Err((400, "please input: name (text mandatory)".into()));
    }
    if is_active.is_null() {
        return Err((400, "please input: is_active (boolean mandatory)".into()));
    }
    // `in (True, False, 0, 1)` compares with ==, so 1.0 and 0.0 pass too
    let ok = match &is_active {
        Value::Bool(_) => true,
        Value::Number(n) => n.as_f64().is_some_and(|f| f == 0.0 || f == 1.0),
        _ => false,
    };
    if !ok {
        return Err((
            400,
            "invalid input format: is_active not in [true, false, 0, 1]".into(),
        ));
    }
    let is_active = if truthy(&is_active) { 1 } else { 0 };
    if !truthy(&id) {
        return Err((400, "not yet implemented".into())); // prevent create
    }
    Ok(OrgInput {
        id,
        name,
        is_active,
    })
}

const ORG_COLUMNS: &str = "tbl_organization_types.id AS tbl_organization_types_id, \
    tbl_organization_types.name AS tbl_organization_types_name, \
    tbl_organization_types.is_active AS tbl_organization_types_is_active, \
    tbl_organization_types.created_at AS tbl_organization_types_created_at, \
    tbl_organization_types.created_by AS tbl_organization_types_created_by, \
    tbl_organization_types.updated_at AS tbl_organization_types_updated_at";

/// `POST /masters/organization` and `PUT /masters/organization` (same code for both methods).
pub async fn organization_save(req: Req) -> Response {
    match core::current_user(&req).await {
        Ok(Some(_)) => {}
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    }
    if !is_json(&req) {
        return bad("please provide json data").await;
    }
    let data = match get_json(&req.body) {
        Ok(data) => data,
        Err(message) => return internal(&message).await,
    };
    let input = match org_input(&data) {
        Ok(input) => input,
        Err((400, message)) => return bad(&message).await,
        Err((_, message)) => return internal(&message).await,
    };
    let ctx = ctx().await;
    let mut tx = match ctx.app.begin().await {
        Ok(tx) => tx,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    // Organization.query.filter_by(id=id).first(): SQLAlchemy inlines `== True` as `= true`
    let select = match &input.id {
        Value::Bool(b) => Stmt::new(format!(
            "SELECT {ORG_COLUMNS} \nFROM tbl_organization_types \nWHERE tbl_organization_types.id = {b} \n LIMIT %(param_1)s"
        )),
        id => Stmt::new(format!(
            "SELECT {ORG_COLUMNS} \nFROM tbl_organization_types \nWHERE tbl_organization_types.id = %(id_1)s \n LIMIT %(param_1)s"
        ))
        .param("id_1", Lit::from_json(id)),
    }
    .param("param_1", Lit::Int(1));
    let rows = match select.fetch_all(&mut tx).await {
        Ok(rows) => rows,
        Err(message) => return internal(&message).await, // tx drops: rollback
    };
    let Some(row) = rows.first() else {
        return bad("Data is failed to change, not found").await;
    };
    let read = || -> Result<(i32, Option<String>, Option<i16>), sqlx::Error> {
        Ok((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?))
    };
    let (pk, old_name, old_active) = match read() {
        Ok(v) => v,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    // the flush SETs only what changed (Python ==); updated_at always does
    let new_name = Lit::from_json(&input.name);
    let mut sets = Vec::new();
    let mut update = Stmt::new("");
    if !matches!((&new_name, &old_name), (Lit::Str(n), Some(o)) if n == o) {
        sets.push("name=%(name)s");
        update = update.param("name", new_name);
    }
    if old_active != Some(input.is_active) {
        sets.push("is_active=%(is_active)s");
        update = update.param("is_active", Lit::Int(input.is_active.into()));
    }
    sets.push("updated_at=%(updated_at)s");
    update = update
        .param("updated_at", Lit::timestamp(core::get_date()))
        .param("tbl_organization_types_id", Lit::Int(pk.into()));
    update.sql = format!(
        "UPDATE tbl_organization_types SET {} WHERE tbl_organization_types.id = %(tbl_organization_types_id)s",
        sets.join(", ")
    );
    match update.execute(&mut tx).await {
        Ok(1) => {}
        // SQLAlchemy's StaleDataError when the row vanished between SELECT and UPDATE
        Ok(n) => {
            return internal(&format!(
                "UPDATE statement on table 'tbl_organization_types' expected to update 1 row(s); {n} were matched."
            ))
            .await;
        }
        Err(message) => return internal(&message).await,
    }
    if let Err(e) = tx.commit().await {
        return core::vary_cookie(core::db_error(e).await);
    }
    core::vary_cookie(core::json(
        200,
        &core::success(json!({}), None, Some("Data succesfully changed")),
    ))
}

// ---- GET /loggers/list ----

/// An unpickled `request_data`/`response_data` as `jsonify` writes it; `Err` is the TypeError
/// (or UnpicklingError) that makes the original answer 500.
pub(crate) fn pickle_json(bytes: &[u8]) -> Result<Value, String> {
    use serde_pickle::{HashableValue as H, Value as P};
    fn key(k: H) -> Result<String, String> {
        Ok(match k {
            H::String(s) => s,
            H::None => "null".into(),
            H::Bool(b) => b.to_string(),
            H::I64(i) => i.to_string(),
            H::Int(n) => n.to_string(),
            // shortcut: json writes float.__repr__; differs only in the exponent ranges
            H::F64(f) => num_text(&serde_json::Number::from_f64(f).ok_or("nan key")?),
            H::Bytes(_) => {
                return Err("keys must be str, int, float, bool or None, not bytes".into());
            }
            H::Tuple(_) => {
                return Err("keys must be str, int, float, bool or None, not tuple".into());
            }
            H::FrozenSet(_) => {
                return Err("keys must be str, int, float, bool or None, not frozenset".into());
            }
        })
    }
    fn conv(v: P) -> Result<Value, String> {
        Ok(match v {
            P::None => Value::Null,
            P::Bool(b) => json!(b),
            P::I64(i) => json!(i),
            P::Int(n) => match n.to_string().parse::<u64>() {
                Ok(u) => json!(u),
                // shortcut: serde_json without arbitrary_precision cannot hold Python big ints
                Err(_) => return Err(format!("integer {n} out of range")),
            },
            // shortcut: jsonify writes NaN/Infinity; serde_json::Value cannot hold them
            P::F64(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .ok_or_else(|| "Out of range float values are not JSON compliant".to_string())?,
            P::String(s) => Value::String(s),
            P::List(items) | P::Tuple(items) => {
                Value::Array(items.into_iter().map(conv).collect::<Result<_, _>>()?)
            }
            P::Dict(map) => {
                // sort_keys: more than one key of incomparable types is a TypeError
                let numeric = |k: &H| matches!(k, H::Bool(_) | H::I64(_) | H::Int(_) | H::F64(_));
                let all_str = map.keys().all(|k| matches!(k, H::String(_)));
                if map.len() > 1 && !all_str && !map.keys().all(numeric) {
                    return Err("'<' not supported between instances of mixed key types".into());
                }
                // shortcut: numeric keys come out in string order here, numeric order in Python
                let mut out = Map::new();
                for (k, v) in map {
                    out.insert(key(k)?, conv(v)?);
                }
                Value::Object(out)
            }
            P::Bytes(_) => return Err("Object of type bytes is not JSON serializable".into()),
            P::Set(_) => return Err("Object of type set is not JSON serializable".into()),
            P::FrozenSet(_) => {
                return Err("Object of type frozenset is not JSON serializable".into());
            }
        })
    }
    // shortcut: pickled class instances (datetime, werkzeug MultiDict) fail here; the original
    // unpickles them (jsonify writes a datetime as an HTTP date). The logger stores plain dicts.
    let value = crate::pickle::loads(bytes)?;
    conv(value)
}

/// `Logs.to_json()` for one row.
fn log_json(row: &sqlx::postgres::PgRow, p: &str) -> Result<Value, String> {
    let e = |e: sqlx::Error| e.to_string();
    let pickled = |col: &str| -> Result<Value, String> {
        match row
            .try_get::<Option<Vec<u8>>, _>(format!("{p}{col}").as_str())
            .map_err(e)?
        {
            Some(bytes) => pickle_json(&bytes),
            None => Ok(Value::Null),
        }
    };
    let s = |col: &str| {
        row.try_get::<Option<String>, _>(format!("{p}{col}").as_str())
            .map_err(e)
    };
    let i = |col: &str| {
        row.try_get::<Option<i32>, _>(format!("{p}{col}").as_str())
            .map_err(e)
    };
    // sa.DateTime() -> `timestamp without time zone` (migration 975320c3c290)
    let created_at: Option<chrono::NaiveDateTime> =
        row.try_get(format!("{p}created_at").as_str()).map_err(e)?;
    Ok(json!({
        "id": row.try_get::<i32, _>(format!("{p}id").as_str()).map_err(e)?,
        "log_type_id": i("log_type_id")?,
        "log_type_code": s("log_type_code")?,
        "log_type_name": s("log_type_name")?,
        "activity_type_id": i("activity_type_id")?,
        "activity_type_code": s("activity_type_code")?,
        "activity_type_name": s("activity_type_name")?,
        "description": s("description")?,
        "session_id": s("session_id")?,
        "request_data": pickled("request_data")?,
        "response_data": pickled("response_data")?,
        "created_at": created_at.map(core::py_isoformat),
        "created_by": i("created_by")?,
    }))
}

/// Every mapped column of `Logs`, in model order (what SQLAlchemy selects).
const LOG_ALL_COLUMNS: [&str; 15] = [
    "id",
    "log_type_id",
    "log_type_code",
    "log_type_name",
    "activity_type_id",
    "activity_type_code",
    "activity_type_name",
    "description",
    "session_id",
    "request_data",
    "response_data",
    "request_data_json",
    "response_data_json",
    "created_at",
    "created_by",
];

// The JSONB twins are not read: to_json never shows them (SQLAlchemy loads them anyway).
const LOG_COLUMNS: &str = "id, log_type_id, log_type_code, log_type_name, activity_type_id, \
    activity_type_code, activity_type_name, description, session_id, request_data, \
    response_data, created_at, created_by";

/// `GET /loggers/list`
pub async fn logger_list(req: Req) -> Response {
    match core::current_user(&req).await {
        Ok(Some(_)) => {}
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    }
    let id = arg(&req, "id");
    let ctx = ctx().await;
    // `request_token != SECRET_KEY`; a missing token never matches (SECRET_KEY unset in Python
    // would be None == None, but then no login token could be verified to get here).
    if arg(&req, "token").as_deref() != Some(ctx.secret.as_str()) {
        return bad("you dont have permission to do this!").await;
    }
    let items = match id.filter(|id| !id.is_empty()) {
        Some(id) => {
            // filter_by(id=<str>).first(): the quoted string resolves to the int column
            // (sent as SQLAlchemy does: a bad id fails with the original's message)
            let cols: Vec<String> = LOG_ALL_COLUMNS
                .iter()
                .map(|c| format!("tbl_logger_logs.{c} AS tbl_logger_logs_{c}"))
                .collect();
            let stmt = Stmt::new(format!(
                "SELECT {} \nFROM tbl_logger_logs \nWHERE tbl_logger_logs.id = %(id_1)s \n LIMIT %(param_1)s",
                cols.join(", ")
            ))
            .param("id_1", Lit::Str(id))
            .param("param_1", Lit::Int(1));
            let mut conn = match ctx.app.acquire().await {
                Ok(conn) => conn,
                Err(e) => return core::vary_cookie(core::db_error(e).await),
            };
            match stmt.fetch_all(&mut conn).await {
                Ok(rows) => match rows.first() {
                    Some(row) => match log_json(row, "tbl_logger_logs_") {
                        Ok(item) => item, // a dict, not a list, as the original returns it
                        Err(message) => return internal(&message).await,
                    },
                    None => json!([]),
                },
                Err(message) => return internal(&message).await,
            }
        }
        None => {
            // shortcut: Logs.query.all() loads the whole activity log (GBs) like the original
            let sql = format!("SELECT {LOG_COLUMNS} FROM tbl_logger_logs");
            let rows = match crate::pq::query(&sql).fetch_all(&ctx.app).await {
                Ok(rows) => rows,
                Err(e) => return core::vary_cookie(core::db_error(e).await),
            };
            match rows
                .iter()
                .map(|r| log_json(r, ""))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(items) => Value::Array(items),
                Err(message) => return internal(&message).await,
            }
        }
    };
    core::vary_cookie(core::json(
        200,
        &core::success(json!({ "result": items }), None, None),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(v: Value) -> Result<OrgInput, (u16, String)> {
        org_input(&v)
    }

    #[test]
    fn org_validation_follows_the_route_order() {
        assert_eq!(
            input(json!([1])).unwrap_err(),
            (500, "'list' object has no attribute 'get'".into())
        );
        assert_eq!(
            input(Value::Null).unwrap_err().1,
            "'NoneType' object has no attribute 'get'"
        );
        assert_eq!(
            input(json!({"name": "", "is_active": "x"})).unwrap_err(),
            (400, "please input: name (text mandatory)".into())
        );
        assert_eq!(
            input(json!({"name": "a"})).unwrap_err().1,
            "please input: is_active (boolean mandatory)"
        );
        assert_eq!(
            input(json!({"name": "a", "is_active": "1"})).unwrap_err().1,
            "invalid input format: is_active not in [true, false, 0, 1]"
        );
        assert_eq!(
            input(json!({"name": "a", "is_active": 2})).unwrap_err().1,
            "invalid input format: is_active not in [true, false, 0, 1]"
        );
        assert_eq!(
            input(json!({"name": "a", "is_active": true, "id": 0})).unwrap_err(),
            (400, "not yet implemented".into())
        );
        let ok = input(json!({"name": "a", "is_active": 1.0, "id": "7"})).unwrap();
        assert_eq!(ok.is_active, 1);
        assert_eq!(ok.id, json!("7"));
        assert_eq!(
            input(json!({"name": "a", "is_active": false, "id": 3}))
                .unwrap()
                .is_active,
            0
        );
    }

    #[test]
    fn json_body_rules() {
        assert!(get_json(b"\xef\xbb\xbf{\"a\":1}").is_ok());
        assert_eq!(get_json(b"").unwrap_err(), core::BAD_REQUEST_TEXT);
    }

    #[test]
    fn unpickles_logger_rows() {
        let v = json!({"status_code": 200, "json_response": true,
                       "data": {"a": [1, 2.5, null], "s": "é"}});
        let p = serde_pickle::to_vec(&v, serde_pickle::SerOptions::new()).unwrap();
        assert_eq!(pickle_json(&p).unwrap(), v);
        // pickle.dumps({1: 'a', 2: 'b'}): int keys become strings
        let ints = b"\x80\x04\x95\x11\x00\x00\x00\x00\x00\x00\x00}\x94(K\x01\x8c\x01a\x94K\x02\x8c\x01b\x94u.";
        assert_eq!(pickle_json(ints).unwrap(), json!({"1": "a", "2": "b"}));
        // pickle.dumps({1: 'a', 'x': 'b'}): sort_keys over int and str raises
        let mixed = b"\x80\x04\x95\x13\x00\x00\x00\x00\x00\x00\x00}\x94(K\x01\x8c\x01a\x94\x8c\x01x\x94\x8c\x01b\x94u.";
        assert!(
            pickle_json(mixed)
                .unwrap_err()
                .starts_with("'<' not supported")
        );
        // pickle.dumps(b'x')
        assert!(pickle_json(b"\x80\x04\x95\x05\x00\x00\x00\x00\x00\x00\x00C\x01x\x94.").is_err());
        assert!(pickle_json(b"garbage").is_err());
    }
}
