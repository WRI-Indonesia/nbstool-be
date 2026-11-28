//! `application/apis/master_apis`.

use axum::response::Response;
use serde_json::{Value, json};
use sqlx::Row;

use crate::core::{self, Req, arg, ctx};
use crate::sa::{Lit, Stmt};

const DICT_SELECT: &str = "tbl_master_dictionary.id AS tbl_master_dictionary_id, \
    tbl_master_dictionary.code AS tbl_master_dictionary_code, \
    tbl_master_dictionary.name AS tbl_master_dictionary_name, \
    tbl_master_dictionary.type AS tbl_master_dictionary_type, \
    tbl_master_dictionary.description AS tbl_master_dictionary_description, \
    tbl_master_dictionary.rowstatus AS tbl_master_dictionary_rowstatus, \
    tbl_master_dictionary.created_by AS tbl_master_dictionary_created_by, \
    tbl_master_dictionary.created_date AS tbl_master_dictionary_created_date, \
    tbl_master_dictionary.modified_by AS tbl_master_dictionary_modified_by, \
    tbl_master_dictionary.modified_date AS tbl_master_dictionary_modified_date";

/// `GET /masters/organization/list`
pub async fn organization_list(req: Req) -> Response {
    let user = match core::current_user(&req).await {
        Ok(user) => user,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    // `request.args` values are strings, so only the text `true` counts (`1` does not).
    let is_active: i16 = if arg(&req, "is_active").as_deref() == Some("true") {
        1
    } else {
        0
    };
    let rows = crate::pq::query(
        "select id, name, is_active, created_at from tbl_organization_types where is_active = $1",
    )
    .bind(is_active)
    .fetch_all(&ctx().await.app)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut item = json!({
                "id": row.get::<i32, _>("id"),
                "name": row.get::<String, _>("name"),
                "is_active": row.get::<Option<i16>, _>("is_active"),
            });
            if user.is_some() {
                // map_attr: a datetime becomes isoformat() + ".000Z"
                let created: Option<chrono::NaiveDateTime> = row.get("created_at");
                item["created_at"] = json!(created.map(|d| core::py_isoformat(d) + ".000Z"));
            }
            item
        })
        .collect();
    core::vary_cookie(core::json(
        200,
        &core::success(json!({ "organizations": items }), None, None),
    ))
}

/// The plain columns of `Dictionary` (`tbl_master_dictionary`) that `?attr=` may name.
const DICT_COLUMNS: [&str; 10] = [
    "id",
    "code",
    "name",
    "type",
    "description",
    "rowstatus",
    "created_by",
    "created_date",
    "modified_by",
    "modified_date",
];

/// `set_attr`: only `[a-z0-9._,]` passes, split on `,` and stripped.
fn set_attr(attr: Option<&str>) -> Option<Vec<String>> {
    let attr = attr.filter(|a| !a.is_empty())?;
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || ".,_".contains(c);
    attr.chars()
        .all(allowed)
        .then(|| attr.split(',').map(|n| n.trim().to_string()).collect())
}

/// What `map_attr`'s `eval('data.<n>')` raises for an attr that is not a plain column.
/// shortcut: every non-column attr is an error here. Python would also answer for real ORM
/// attributes (`__tablename__`, `__module__`, `query`, ...) and dotted paths into a column
/// (`created_date.year`); only plain column names are accepted here, by design.
fn unknown_attr_message(n: &str) -> String {
    let head = n.split('.').next().unwrap_or("");
    let ident = head
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    if !ident {
        "invalid syntax (<string>, line 1)".into()
    } else if !DICT_COLUMNS.contains(&head) {
        format!("'Dictionary' object has no attribute '{head}'")
    } else {
        format!("unsupported attribute path '{n}'")
    }
}

/// Python's `int(str)` for ASCII input: surrounding whitespace, a sign, `_` between digits.
/// Saturates instead of growing without bound (the caller turns that into a range error).
fn py_int(s: &str) -> Option<i128> {
    let s = s.trim();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let mut n: i128 = 0;
    for c in digits.chars().filter(char::is_ascii_digit) {
        n = n
            .saturating_mul(10)
            .saturating_add(i128::from(c as u8 - b'0'));
    }
    Some(if neg { -n } else { n })
}

/// `get_default_list_param`'s text fields: missing or empty is `''`, cut to 1000 characters.
fn list_param(req: &Req, name: &str) -> String {
    arg(req, name)
        .unwrap_or_default()
        .chars()
        .take(1000)
        .collect()
}

/// `GET /masters/dictionary/list`
pub async fn dictionary_list(req: Req) -> Response {
    let fail = |message: String| async move {
        core::vary_cookie(core::json(500, &core::other_error(&message, None).await))
    };
    match core::current_user(&req).await {
        Ok(Some(_)) => {}
        Ok(None) => return core::not_logged_in().await,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    }

    let attr = set_attr(arg(&req, "attr").as_deref());
    let page_index = arg(&req, "page_index")
        .and_then(|s| py_int(&s))
        .unwrap_or(1);
    let page_size = arg(&req, "page_size")
        .and_then(|s| py_int(&s))
        .unwrap_or(10);
    let search_by = list_param(&req, "search_by");
    let keywords = list_param(&req, "keywords");
    let order_by_col = list_param(&req, "order_by_col");
    let order_by_type = list_param(&req, "order_by_type");
    let filter_by_col = list_param(&req, "filter_by_col");
    let filter_by_text = list_param(&req, "filter_by_text");

    let mut binds: Vec<String> = Vec::new();
    let mut sql_where = String::from("(");
    // the same filter as SQLAlchemy compiles it, for the count statement (sa::Stmt)
    let mut sa_where = String::from("(");
    let mut sa_params: Vec<(String, Lit)> = Vec::new();
    for (i, col) in ["name", "code", "type", "description"].iter().enumerate() {
        binds.push(if search_by.is_empty() || search_by == *col {
            format!("%{keywords}%")
        } else {
            "\0".into()
        });
        if i > 0 {
            sql_where.push_str(" OR ");
            sa_where.push_str(" OR ");
        }
        sql_where.push_str(&format!("coalesce({col}, '') LIKE ${}", binds.len()));
        sa_where.push_str(&format!(
            "coalesce(tbl_master_dictionary.{col}, %(coalesce_{})s) LIKE %(coalesce_{})s",
            2 * i + 1,
            2 * i + 2
        ));
        sa_params.push((format!("coalesce_{}", 2 * i + 1), Lit::Str(String::new())));
        sa_params.push((
            format!("coalesce_{}", 2 * i + 2),
            Lit::Str(binds[i].clone()),
        ));
    }
    sql_where.push_str(") AND rowstatus = 1");
    sa_where.push_str(") AND tbl_master_dictionary.rowstatus = %(rowstatus_1)s");
    sa_params.push(("rowstatus_1".into(), Lit::Int(1)));
    if !filter_by_col.is_empty() && !filter_by_text.is_empty() {
        for (col, text) in filter_by_col.split(',').zip(filter_by_text.split(',')) {
            let col = col.trim().to_lowercase();
            if ["name", "code", "type"].contains(&col.as_str()) && !text.is_empty() {
                binds.push(text.to_string());
                sql_where.push_str(&format!(" AND {col} = ${}", binds.len()));
                // SQLAlchemy numbers repeated names: type_1, type_2
                let prefix = format!("{col}_");
                let n = sa_params
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .count()
                    + 1;
                sa_where.push_str(&format!(" AND tbl_master_dictionary.{col} = %({col}_{n})s"));
                sa_params.push((format!("{col}_{n}"), Lit::Str(text.to_string())));
            }
        }
    }
    // psycopg2 refuses a NUL in a bound string before sending (and any search_by binds one).
    if binds.iter().any(|b| b.contains('\0')) {
        return fail("A string literal cannot contain NUL (0x00) characters.".into()).await;
    }

    let mut order_by: Vec<String> = Vec::new();
    if !order_by_col.is_empty() && !order_by_type.is_empty() {
        for (col, ty) in order_by_col.split(',').zip(order_by_type.split(',')) {
            let ty = if ty.trim().to_lowercase() == "desc" {
                "desc"
            } else {
                "asc"
            };
            let col = col.trim().to_lowercase();
            if ["name", "code", "type", "description"].contains(&col.as_str()) {
                // Python's text, table name and all: Postgres rejects it, as it does there.
                order_by.push(format!("master_dictionary.{col}  {ty}"));
            }
        }
    }
    let order_by = if order_by.is_empty() {
        "tbl_master_dictionary.id DESC".to_string()
    } else {
        order_by.join(",")
    };
    let select = format!(
        "SELECT id, code, name, type, description, rowstatus, created_by, created_date, \
         modified_by, modified_date FROM tbl_master_dictionary WHERE {sql_where} ORDER BY {order_by}"
    );

    let app = &ctx().await.app;
    // data.count(): the ordered query wrapped in a subquery
    // (sent as SQLAlchemy does, so a bad ORDER BY fails with the original's message)
    let count = Stmt {
        sql: format!(
            "SELECT count(*) AS count_1 \nFROM (SELECT {DICT_SELECT} \nFROM tbl_master_dictionary \nWHERE {sa_where} ORDER BY {order_by}) AS anon_1"
        ),
        params: sa_params,
    };
    let mut conn = match app.acquire().await {
        Ok(conn) => conn,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };
    let total_records: i64 = match count.fetch_all(&mut conn).await {
        Ok(rows) => match rows.first().map(|r| r.try_get(0)) {
            Some(Ok(n)) => n,
            Some(Err(e)) => return core::vary_cookie(core::db_error(e).await),
            None => 0,
        },
        Err(message) => return fail(message).await,
    };
    drop(conn);

    // paginate(error_out=False): a page below 1 is 1, a size below 1 is 20
    let page = page_index.max(1);
    let per_page = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1).saturating_mul(per_page);
    let (Ok(limit), Ok(offset)) = (i64::try_from(per_page), i64::try_from(offset)) else {
        return fail("bigint out of range".into()).await;
    };
    let page_sql = format!(
        "{select} LIMIT ${} OFFSET ${}",
        binds.len() + 1,
        binds.len() + 2
    );
    let mut items_query = crate::pq::query(&page_sql);
    for b in &binds {
        items_query = items_query.bind(b);
    }
    let rows = match items_query.bind(limit).bind(offset).fetch_all(app).await {
        Ok(rows) => rows,
        Err(e) => return core::vary_cookie(core::db_error(e).await),
    };

    let attr = attr.unwrap_or_else(|| vec!["code".into(), "name".into(), "type".into()]);
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut item = serde_json::Map::new();
        for n in &attr {
            let value = match n.as_str() {
                "id" => json!(row.get::<i32, _>("id")),
                "rowstatus" => json!(row.get::<Option<i32>, _>("rowstatus")),
                "created_date" | "modified_date" => {
                    let d: Option<chrono::NaiveDateTime> = row.get(n.as_str());
                    json!(d.map(|d| core::py_isoformat(d) + ".000Z"))
                }
                "code" | "name" | "type" | "description" | "created_by" | "modified_by" => {
                    json!(row.get::<Option<String>, _>(n.as_str()))
                }
                _ => return fail(unknown_attr_message(n)).await,
            };
            item.insert(n.clone(), value);
        }
        items.push(Value::Object(item));
    }
    core::vary_cookie(core::json(
        200,
        &core::success(
            json!({ "data": items, "total_records": total_records }),
            None,
            None,
        ),
    ))
}

/// `GET /masters/faq/list`
pub async fn faq_list(_req: Req) -> Response {
    // __log_it__ is set but __session_id__ is None, so the original writes no log row.
    let rows = crate::pq::query(
        "\n        select\n            faq_id, group_id, group_name,\n            question, answer\n        from public.\"vwFAQs\"\n        ",
    )
    .fetch_all(&ctx().await.app)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return core::db_error(e).await,
    };
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "faq_id": row.get::<Option<i32>, _>("faq_id"),
                "group_id": row.get::<Option<i32>, _>("group_id"),
                "group_name": row.get::<Option<String>, _>("group_name"),
                "question": row.get::<Option<String>, _>("question"),
                "answer": row.get::<Option<String>, _>("answer"),
            })
        })
        .collect();
    core::json(200, &core::success(json!({ "result": items }), None, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_attr_matches_python() {
        assert_eq!(set_attr(None), None);
        assert_eq!(set_attr(Some("")), None);
        assert_eq!(set_attr(Some("Code")), None);
        assert_eq!(set_attr(Some("a b")), None);
        assert_eq!(
            set_attr(Some("code,name,")),
            Some(vec!["code".into(), "name".into(), "".into()])
        );
    }

    #[test]
    fn py_int_matches_python() {
        assert_eq!(py_int(" 5 "), Some(5));
        assert_eq!(py_int("+7"), Some(7));
        assert_eq!(py_int("-3"), Some(-3));
        assert_eq!(py_int("1_000"), Some(1000));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1.5"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("-"), None);
    }

    #[test]
    fn unknown_attrs_answer_like_attribute_errors() {
        assert_eq!(
            unknown_attr_message("foo"),
            "'Dictionary' object has no attribute 'foo'"
        );
        assert_eq!(
            unknown_attr_message(""),
            "invalid syntax (<string>, line 1)"
        );
        assert_eq!(
            unknown_attr_message("1x"),
            "invalid syntax (<string>, line 1)"
        );
    }
}
