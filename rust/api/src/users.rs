//! `user_apis/routes.py` (`GET /users/account`, `GET /users/account/get-token`) and
//! `logger_apis/routes.py` (`GET /loggers/user/activity`).

use axum::response::Response;
use serde_json::{Value, json};
use sqlx::Row;

use crate::core::{self, Req, arg, ctx};

/// `GET /users/account`
pub async fn account_get(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let row = crate::pq::query("select avatar, extended_data from tbl_users where id = $1")
        .bind(user.id)
        .fetch_optional(&ctx().await.app)
        .await;
    let (avatar, extended): (Option<Vec<u8>>, Option<Vec<u8>>) = match row {
        Ok(Some(row)) => (row.get("avatar"), row.get("extended_data")),
        Ok(None) => return core::not_logged_in().await, // row gone since the login query
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let mut results = json!({
        "name": user.name,
        "email": user.email,
        "organization_type_id": user.organization_type_id,
        "organization_name": user.organization_name,
        "permission_policy": user.permission_policy,
        "user_id": user.id,
        // `if current_user.avatar:` empty bytes count as no avatar
        "avatar": avatar.filter(|a| !a.is_empty()).map(|a| b64(&a)),
    });
    // PickleType unpickles on load; `if current_user.extended_data:` then `results.update(...)`
    if let Some(bytes) = extended {
        match unpickle_dict(&bytes) {
            Ok(map) => {
                if let Value::Object(results) = &mut results {
                    results.extend(map);
                }
            }
            Err(message) => {
                return core::vary_cookie(core::json(
                    500,
                    &core::other_error(&message, None).await,
                ));
            }
        }
    }
    core::vary_cookie(core::json(200, &core::success(results, None, None)))
}

/// Standard base64 with padding, `base64.b64encode(...).decode('ascii')`.
fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The unpickled `extended_data` as the entries `results.update` adds. Staging holds only
/// dicts of str -> str/int/None; other shapes fail as `jsonify` would (500).
fn unpickle_dict(bytes: &[u8]) -> Result<serde_json::Map<String, Value>, String> {
    use serde_pickle::{HashableValue, Value as P};
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
            P::F64(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .ok_or_else(|| "Out of range float values are not JSON compliant".to_string())?,
            P::String(s) => Value::String(s),
            P::List(items) | P::Tuple(items) => {
                Value::Array(items.into_iter().map(conv).collect::<Result<_, _>>()?)
            }
            P::Dict(map) => Value::Object(entries(map)?),
            P::Bytes(_) => return Err("Object of type bytes is not JSON serializable".into()),
            P::Set(_) => return Err("Object of type set is not JSON serializable".into()),
            P::FrozenSet(_) => {
                return Err("Object of type frozenset is not JSON serializable".into());
            }
        })
    }
    fn entries(
        map: std::collections::BTreeMap<HashableValue, P>,
    ) -> Result<serde_json::Map<String, Value>, String> {
        map.into_iter()
            .map(|(k, v)| match k {
                HashableValue::String(k) => Ok((k, conv(v)?)),
                // sort_keys over mixed key types raises TypeError in jsonify
                k => Err(format!("unsupported extended_data key {k:?}")),
            })
            .collect()
    }
    match serde_pickle::value_from_slice(bytes, serde_pickle::DeOptions::new()) {
        Ok(P::Dict(map)) => entries(map),
        Ok(P::None) => Ok(serde_json::Map::new()), // falsy: no update
        Ok(_) => Err("extended_data is not a dict".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Python's `int(str)` for the plain cases: surrounding whitespace, a sign, digits with single
/// underscores between them.
fn py_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    t.replace('_', "").parse().ok()
}

/// base64url without padding, as PyJWT writes the segments.
fn b64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `jwt.encode(dict(exp=..., iat=..., user_id=...), key, algorithm="HS256")` byte for byte:
/// PyJWT sorts the header keys and keeps the payload in insertion order.
fn encode_jwt(exp: i64, iat: i64, user_id: i32, secret: &str) -> Result<String, String> {
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64url(format!(r#"{{"exp":{exp},"iat":{iat},"user_id":{user_id}}}"#).as_bytes());
    let message = format!("{header}.{payload}");
    let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());
    jsonwebtoken::crypto::sign(message.as_bytes(), &key, jsonwebtoken::Algorithm::HS256)
        .map(|signature| format!("{message}.{signature}"))
        .map_err(|e| e.to_string())
}

// datetime.min / datetime.max as epoch seconds: `now + timedelta` outside raises OverflowError.
const PY_DATETIME_MIN: i64 = -62_135_596_800;
const PY_DATETIME_MAX: i64 = 253_402_300_799;

fn expire_at(now: i64, seconds: i64) -> Result<i64, &'static str> {
    now.checked_add(seconds)
        .filter(|t| (PY_DATETIME_MIN..=PY_DATETIME_MAX).contains(t))
        .ok_or("date value out of range")
}

/// `GET /users/account/get-token`: `UserLogic.get_auth_success_response`. Reads only.
pub async fn get_token(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let ctx = ctx().await;
    let now = chrono::Utc::now().timestamp(); // timegm() drops the microseconds

    // User.encode_access_token
    let mut values = Vec::new();
    for name in ["TOKEN_EXPIRE_HOURS", "TOKEN_EXPIRE_MINUTES"] {
        let row = crate::pq::query("select value from tbl_master_settings where name = $1 limit 1")
            .bind(name)
            .fetch_optional(&ctx.app)
            .await;
        match row {
            Ok(row) => values.push(row.map(|r| r.get::<String, _>("value"))),
            Err(e) => return core::vary_cookie(core::db_error(e).await),
        }
    }
    let ages = match values.as_slice() {
        [Some(h), Some(m)] => py_int(h).zip(py_int(m)),
        _ => None,
    };
    let fail = |message: String| async move {
        core::vary_cookie(core::json(500, &core::other_error(&message, None).await))
    };
    let Some((hours, minutes)) = ages else {
        return fail("encode access token settings invalid or not found".into()).await;
    };
    let Some(expires_in) = hours
        .checked_mul(3600)
        .and_then(|h| minutes.checked_mul(60).and_then(|m| h.checked_add(m)))
    else {
        return fail("date value out of range".into()).await;
    };
    let tokens = expire_at(now, expires_in)
        .map_err(String::from)
        .and_then(|exp| encode_jwt(exp, now, user.id, &ctx.secret))
        .and_then(|access| {
            // User.encode_refresh_token: 24 hours
            encode_jwt(now + 24 * 3600, now, user.id, &ctx.secret).map(|refresh| (access, refresh))
        });
    let (access_token, refresh_token) = match tokens {
        Ok(tokens) => tokens,
        Err(message) => return fail(message).await,
    };
    let body = json!({
        "status": "success",
        "user_id": user.id,
        "message": "successfully generated user token",
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": "bearer",
        "expires_in": expires_in,
        "assistance_eligible": user.assistance_eligible,
    });
    let response = core::with_header(core::json(200, &body), "cache-control", "no-store");
    core::vary_cookie(core::with_header(response, "pragma", "no-cache"))
}

/// `get_default_list_param`: `[:1000]` slices code points.
fn param(req: &Req, name: &str) -> String {
    arg(req, name)
        .unwrap_or_default()
        .chars()
        .take(1000)
        .collect()
}

/// Flask's `jsonify` of a naive datetime (werkzeug `http_date`, naive taken as UTC).
fn http_date(d: chrono::NaiveDateTime) -> String {
    d.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// The WHERE clause of `logger_user_activity` and its text binds ($1 is the user id).
fn activity_filters(
    search_by: &str,
    keywords: &str,
    filter_by_col: &str,
    filter_by_text: &str,
) -> (String, Vec<String>) {
    let pattern = format!("%{keywords}%");
    let pick = |field: &str| {
        if search_by.is_empty() || search_by == field {
            pattern.clone()
        } else {
            "\0".to_string()
        }
    };
    let mut binds = vec![pick("description"), pick("activity_type")];
    let mut sql = String::from(
        "(coalesce(tbl_logger_logs.description, '') like $2 \
         or coalesce(tbl_master_dictionary.name, '') like $3) \
         and tbl_logger_logs.created_by = $1",
    );
    if !filter_by_col.is_empty() && !filter_by_text.is_empty() {
        for (col, text) in filter_by_col.split(',').zip(filter_by_text.split(',')) {
            let col = col.trim().to_lowercase();
            if text.is_empty() {
                continue;
            }
            if col == "activity_type_code" {
                binds.push(text.to_string());
                sql += &format!(" and tbl_master_dictionary.code = ${}", binds.len() + 1);
            } else if col == "type" && text == "project" {
                sql += " and (tbl_master_dictionary.code like 'projects_%' \
                        or tbl_master_dictionary.code like 'post_%')";
            } else if col == "type" && text == "document" {
                sql += " and tbl_master_dictionary.code like 'documents_%'";
            }
        }
    }
    (sql, binds)
}

/// `GET /loggers/user/activity`
pub async fn user_activity(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(Some(user)) => user,
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    // get_default_list_param, then Flask-SQLAlchemy's paginate(error_out=False) clamps
    let page = arg(&req, "page_index")
        .as_deref()
        .and_then(py_int)
        .unwrap_or(1)
        .max(1);
    let per_page = arg(&req, "page_size")
        .as_deref()
        .and_then(py_int)
        .unwrap_or(10);
    let per_page = if per_page < 1 { 20 } else { per_page };
    let (filters, binds) = activity_filters(
        &param(&req, "search_by"),
        &param(&req, "keywords"),
        &param(&req, "filter_by_col"),
        &param(&req, "filter_by_text"),
    );
    // psycopg2 refuses a NUL in any bound string before the query reaches the server; the
    // route's own '\x00' "match nothing" pattern hits this whenever search_by is set.
    if binds.iter().any(|b| b.contains('\0')) {
        let message = "A string literal cannot contain NUL (0x00) characters.";
        return core::vary_cookie(core::json(500, &core::other_error(message, None).await));
    }
    let select = format!(
        "select distinct tbl_master_dictionary.name, tbl_logger_logs.description, \
         tbl_logger_logs.created_at, tbl_logger_logs.id from tbl_logger_logs \
         left outer join tbl_master_dictionary \
         on tbl_logger_logs.activity_type_id = tbl_master_dictionary.id where {filters}"
    );
    let app = &ctx().await.app;

    let count_sql = format!("select count(*) from ({select}) as anon_1");
    let mut count = crate::pq::query_scalar::<i64>(&count_sql).bind(user.id);
    for b in &binds {
        count = count.bind(b);
    }
    let total = match count.fetch_one(app).await {
        Ok(total) => total,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };

    let n = binds.len() + 2;
    let page_sql = format!(
        "{select} order by tbl_logger_logs.id desc limit ${n} offset ${}",
        n + 1
    );
    let mut rows = crate::pq::query(&page_sql).bind(user.id);
    for b in &binds {
        rows = rows.bind(b);
    }
    let offset = (page - 1).saturating_mul(per_page);
    let rows = match rows.bind(per_page).bind(offset).fetch_all(app).await {
        Ok(rows) => rows,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let created: Option<chrono::NaiveDateTime> = row.get("created_at");
            json!({
                "activity_type": row.get::<Option<String>, _>("name"),
                "description": row.get::<Option<String>, _>("description"),
                "created_at": created.map(http_date),
            })
        })
        .collect();
    core::vary_cookie(core::json(
        200,
        &core::success(json!({ "data": items, "total_records": total }), None, None),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_int_follows_python() {
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("+3"), Some(3));
        assert_eq!(py_int("-3"), Some(-3));
        assert_eq!(py_int("1_000"), Some(1000));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("3.0"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("+"), None);
    }

    #[test]
    fn jwt_matches_pyjwt() {
        // jwt.encode({'exp':2000000000,'iat':1999998800,'user_id':7}, 'k', algorithm='HS256')
        assert_eq!(
            encode_jwt(2_000_000_000, 1_999_998_800, 7, "k").unwrap(),
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
             eyJleHAiOjIwMDAwMDAwMDAsImlhdCI6MTk5OTk5ODgwMCwidXNlcl9pZCI6N30.\
             F9hWHPelLgqiiGsqGihjYJLpSSc0son6hl6sdbZNeUA"
        );
    }

    #[test]
    fn b64url_pads_nothing() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn b64_matches_python() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn unpickles_protocol5_dict() {
        // pickle.dumps({'country':'Indonésia','n':3,'x':None}, protocol=5)
        let p: &[u8] = &[
            128, 5, 149, 39, 0, 0, 0, 0, 0, 0, 0, 125, 148, 40, 140, 7, 99, 111, 117, 110, 116,
            114, 121, 148, 140, 10, 73, 110, 100, 111, 110, 195, 169, 115, 105, 97, 148, 140, 1,
            110, 148, 75, 3, 140, 1, 120, 148, 78, 117, 46,
        ];
        let m = unpickle_dict(p).unwrap();
        assert_eq!(
            Value::Object(m),
            json!({"country": "Indonésia", "n": 3, "x": null})
        );
        assert!(unpickle_dict(b"garbage").is_err());
    }

    #[test]
    fn expire_overflow_is_python_overflow() {
        assert_eq!(expire_at(0, 60), Ok(60));
        assert!(expire_at(0, PY_DATETIME_MAX + 1).is_err());
    }

    #[test]
    fn http_date_is_rfc822() {
        let d = chrono::NaiveDate::from_ymd_opt(2030, 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .unwrap();
        assert_eq!(http_date(d), "Tue, 01 Jan 2030 00:00:00 GMT");
    }

    #[test]
    fn filters_follow_the_route() {
        let (sql, binds) = activity_filters("", "x", "type,activity_type_code", "project,c1");
        assert_eq!(binds, vec!["%x%", "%x%", "c1"]);
        assert!(sql.contains("'projects_%'") && sql.ends_with("code = $4"));
        let (_, binds) = activity_filters("description", "", "", "");
        assert_eq!(binds, vec!["%%", "\0"]);
    }
}
