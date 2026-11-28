//! `application/apis/geo_apis/layers/routes.py`.

use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::postgres::PgRow;
use sqlx::{Column, Row, TypeInfo};

use crate::core::{self, Req, ctx};

/// `GeoUtils.get_db(query)`: every row as `row._asdict()`, in the order Postgres returns them.
#[expect(
    clippy::result_large_err,
    reason = "the error is the finished axum Response, as in every handler"
)]
async fn get_db(query: &str) -> Result<Vec<Map<String, Value>>, Response> {
    let rows = match crate::pq::query(query).fetch_all(&ctx().await.gis).await {
        Ok(rows) => rows,
        Err(e) => return Err(core::db_error(e).await),
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        match row_dict(row) {
            Ok(d) => out.push(d),
            Err(e) => return Err(core::db_error(e).await),
        }
    }
    Ok(out)
}

/// One row as psycopg2 hands it to Python: ints stay ints, text stays str, NULL is None.
fn row_dict(row: &PgRow) -> Result<Map<String, Value>, sqlx::Error> {
    let mut d = Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let v = match col.type_info().name() {
            "INT2" => json!(row.try_get::<Option<i16>, _>(i)?),
            "INT4" => json!(row.try_get::<Option<i32>, _>(i)?),
            "INT8" => json!(row.try_get::<Option<i64>, _>(i)?),
            "BOOL" => json!(row.try_get::<Option<bool>, _>(i)?),
            "FLOAT4" => json!(row.try_get::<Option<f32>, _>(i)?.map(f64::from)),
            "FLOAT8" => json!(row.try_get::<Option<f64>, _>(i)?),
            "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => json!(row.try_get::<Option<String>, _>(i)?),
            // shortcut: the views only have the types above; a new column type needs a case here.
            other => {
                return Err(sqlx::Error::Decode(
                    format!("unsupported column type {other} for {}", col.name()).into(),
                ));
            }
        };
        d.insert(col.name().to_string(), v);
    }
    Ok(d)
}

/// `row.get(key)`: None (null) when the column is absent.
fn get(row: &Map<String, Value>, key: &str) -> Value {
    row.get(key).cloned().unwrap_or(Value::Null)
}

/// The item fields `geo_get_layers` and `serialize_rowdata` share.
fn base_item(row: &Map<String, Value>) -> Map<String, Value> {
    let mut m = Map::new();
    for (out, col) in [
        ("id", "layer_id"),
        ("name", "layer_name"),
        ("url", "gs_url"),
        ("matrix_set", "matrix_set"),
        ("service", "gs_type"),
        ("version", "gs_version"),
        ("layers", "gs_name"),
        ("srs", "gs_srs"),
        ("styles", "gs_style"),
        ("format", "gs_format"),
        ("source", "source"),
        ("source_link", "source_link"),
        ("citation", "citation"),
        ("description", "description"),
        ("short_description", "short_description"),
        ("content_date", "content_date"),
        ("disclaimer", "disclaimer"),
        ("data_format", "data_format"),
        ("spatial_resolution", "spatial_resolution"),
    ] {
        m.insert(out.into(), get(row, col));
    }
    m
}

/// Groups keyed by `order_no` in first-seen order, as a Python dict keeps them.
fn group_index(
    groups: &mut Vec<(Value, Value)>,
    key: &Value,
    make: impl FnOnce() -> Value,
) -> usize {
    match groups.iter().position(|(k, _)| k == key) {
        Some(i) => i,
        None => {
            groups.push((key.clone(), make()));
            groups.len() - 1
        }
    }
}

/// `GET /geos/layers`
pub async fn layers_v1(_req: Req) -> Response {
    let dt = match get_db(
        r#"
        select
            layer_id, layer_group, layer_name, source, source_link,
            citation, description, short_description, content_date,
            disclaimer, data_format, spatial_resolution, gs_name,
            gs_type, gs_url, gs_srs, gs_style, gs_format, gs_version,
            group_code, group_image, order_no, matrix_set
        from "public"."vwLayers"
        "#,
    )
    .await
    {
        Ok(dt) => dt,
        Err(r) => return r,
    };
    let mut groups: Vec<(Value, Value)> = Vec::new();
    for row in &dt {
        let order_no = get(row, "order_no");
        // the group title is the first row's layer_name, not its layer_group
        let i = group_index(&mut groups, &order_no, || {
            json!({
                "title": get(row, "layer_name"),
                "order_no": order_no.clone(),
                "group_image": get(row, "group_image"),
                "items": [],
            })
        });
        if let Some(items) = groups[i].1["items"].as_array_mut() {
            items.push(Value::Object(base_item(row)));
        }
    }
    let results: Vec<Value> = groups.into_iter().map(|(_, g)| g).collect();
    core::json(
        200,
        &core::success(json!({ "result": results }), None, None),
    )
}

/// Python's `str.title()`: upper after a non-letter, lower after a letter.
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_alphabetic();
    }
    out
}

/// `is_country`
fn is_country(country: &Value) -> bool {
    match country {
        Value::Null => false,
        Value::String(s) => !s.is_empty() && s != "Global",
        _ => true,
    }
}

/// Python's `str(KeyError(key))`: the repr of the missing key.
fn key_error(key: &Value) -> String {
    match key {
        Value::Null => "None".into(),
        Value::String(s) => format!("'{s}'"),
        other => other.to_string(),
    }
}

/// A node with its `child` list filled, as the shared dicts look when jsonify walks them.
/// `stack` catches the cycle jsonify would reject with "Circular reference detected".
fn build_node(
    id: &Value,
    nodes: &[(Value, Map<String, Value>)],
    children: &[(Value, Vec<Value>)],
    stack: &mut Vec<Value>,
) -> Result<Value, String> {
    if stack.contains(id) {
        return Err("Circular reference detected".into());
    }
    let Some((_, node)) = nodes.iter().find(|(k, _)| k == id) else {
        return Err(key_error(id));
    };
    stack.push(id.clone());
    let mut kids = Vec::new();
    if let Some((_, ids)) = children.iter().find(|(k, _)| k == id) {
        for c in ids {
            kids.push(build_node(c, nodes, children, stack)?);
        }
    }
    stack.pop();
    let mut node = node.clone();
    node.insert("child".into(), Value::Array(kids));
    Ok(Value::Object(node))
}

/// `func_get_layers_v2`. The error string is what Python's `str(e)` would be.
#[expect(
    clippy::result_large_err,
    reason = "the error is the finished axum Response, as in every handler"
)]
pub(crate) async fn func_get_layers_v2() -> Result<Vec<Value>, Response> {
    let mut query =
        String::from("\n    select\n        *\n    from \"public\".\"vwLayers_v2\"\n    ");
    if ctx().await.env == "production" {
        query.push_str(" where staging_only = false ");
    }
    let dt = get_db(&query).await?;
    let fail = |msg: String| async move { core::json(500, &core::other_error(&msg, None).await) };

    // nodes: layer_id -> row (a later duplicate id replaces the earlier one, as in a dict)
    let mut nodes: Vec<(Value, Map<String, Value>)> = Vec::new();
    for row in &dt {
        let mut row = row.clone();
        let country = match row.get("country") {
            Some(Value::String(s)) if !s.is_empty() => py_title(s),
            _ => String::new(),
        };
        row.insert("country".into(), json!(country));
        row.insert("child".into(), json!([]));
        let id = get(&row, "layer_id");
        match nodes.iter_mut().find(|(k, _)| *k == id) {
            Some(slot) => slot.1 = row,
            None => nodes.push((id, row)),
        }
    }

    // records (top level) and each parent's children, in row order
    let mut records: Vec<Value> = Vec::new();
    let mut children: Vec<(Value, Vec<Value>)> = Vec::new();
    for row in &dt {
        let id = get(row, "layer_id");
        let parent = get(row, "parent_id");
        if parent == json!(0) {
            records.push(id);
        } else {
            if !nodes.iter().any(|(k, _)| *k == parent) {
                return Err(fail(key_error(&parent)).await);
            }
            match children.iter_mut().find(|(k, _)| *k == parent) {
                Some(slot) => slot.1.push(id),
                None => children.push((parent, vec![id])),
            }
        }
    }

    let mut groups: Vec<(Value, Value)> = Vec::new();
    for id in &records {
        let node = match build_node(id, &nodes, &children, &mut Vec::new()) {
            Ok(Value::Object(node)) => node,
            Ok(_) => continue,
            Err(msg) => return Err(fail(msg).await),
        };
        let order_no = get(&node, "order_no");
        let i = group_index(&mut groups, &order_no, || {
            json!({
                "title": get(&node, "layer_group"),
                "order_no": order_no.clone(),
                "group_image": get(&node, "group_image"),
                "items": {
                    "country": {
                        "Indonesia": [],
                        "Philippine": [],
                        "Thailand": [],
                        "Malaysia": [],
                        "Cambodia": [],
                    },
                    "global": [],
                },
            })
        });
        // serialize_rowdata
        let mut item = base_item(&node);
        item.insert(
            "has_child".into(),
            json!(get(&node, "has_child") == json!(1)),
        );
        item.insert("child".into(), get(&node, "child"));
        let country = get(&node, "country");
        let items = &mut groups[i].1["items"];
        let list = if is_country(&country) {
            let key = country.as_str().unwrap_or_default().to_string();
            items["country"]
                .as_object_mut()
                .map(|m| m.entry(key).or_insert_with(|| json!([])))
        } else {
            Some(&mut items["global"])
        };
        if let Some(Value::Array(list)) = list {
            list.push(Value::Object(item));
        }
    }
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}

/// `GET /geos/layers/v2`
pub async fn layers_v2(_req: Req) -> Response {
    match func_get_layers_v2().await {
        Ok(results) => core::json(
            200,
            &core::success(json!({ "result": results }), None, None),
        ),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_matches_python() {
        assert_eq!(py_title("indonesia"), "Indonesia");
        assert_eq!(py_title("GLOBAL"), "Global");
        assert_eq!(py_title("south-east asia"), "South-East Asia");
        assert_eq!(py_title("o'neil"), "O'Neil");
    }

    #[test]
    fn country_rules() {
        assert!(!is_country(&json!("")));
        assert!(!is_country(&json!("Global")));
        assert!(!is_country(&Value::Null));
        assert!(is_country(&json!("Indonesia")));
    }

    #[test]
    fn tree_and_errors() {
        let row = |id: i64| {
            let mut m = Map::new();
            m.insert("layer_id".into(), json!(id));
            (json!(id), m)
        };
        let nodes = vec![row(1), row(2)];
        let children = vec![(json!(1), vec![json!(2)])];
        let n = build_node(&json!(1), &nodes, &children, &mut Vec::new()).unwrap();
        assert_eq!(n["child"][0]["layer_id"], json!(2));
        assert_eq!(n["child"][0]["child"], json!([]));
        let cyc = vec![(json!(1), vec![json!(2)]), (json!(2), vec![json!(1)])];
        assert!(build_node(&json!(1), &nodes, &cyc, &mut Vec::new()).is_err());
        assert_eq!(key_error(&Value::Null), "None");
        assert_eq!(key_error(&json!(7)), "7");
    }
}
