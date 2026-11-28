//! `application/apis/geo_apis`: the read-only GET routes of `polygon/routes.py` and
//! `map/routes.py`.

use axum::response::Response;
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::core::{self, Req, arg, ctx};

/// `GET /geos/polygon`
pub async fn polygon_get(req: Req) -> Response {
    // `Polygons` lives in the app database (db.session); the original loads the first row, then
    // runs ST_AsGeoJSON on its geom. One query picks the same (unordered) first row.
    let row: Result<Option<Option<String>>, sqlx::Error> = crate::pq::query_scalar(
        "select ST_AsGeoJSON(geom) from polygons where session_id = $1 limit 1",
    )
    .bind(arg(&req, "session_id"))
    .fetch_optional(&ctx().await.app)
    .await;
    let geojson = match row {
        Ok(Some(geojson)) => geojson,
        Ok(None) => return core::json(400, &core::app_error("fail, session id not found", None)),
        Err(e) => return core::db_error(e).await,
    };
    // json.loads(None) on a row without geom
    let Some(geojson) = geojson else {
        let message = "the JSON object must be str, bytes or bytearray, not NoneType";
        return core::json(500, &core::other_error(message, None).await);
    };
    // PostGIS prints at most 9 decimals, so every coordinate parses on serde_json's exact fast
    // path and prints back as Python's repr; integral text stays an int, as json.loads keeps it.
    match serde_json::from_str::<Value>(&geojson) {
        Ok(result) => core::json(200, &core::success(json!({ "result": result }), None, None)),
        Err(e) => core::json(500, &core::other_error(&e.to_string(), None).await),
    }
}

/// `GET /geos/map/explorer`
pub async fn map_explorer_get(req: Req) -> Response {
    // filter_by(session_id=None) is `IS NULL`; `is not distinct from` covers both cases.
    let row = crate::pq::query(
        r#"select session_id, project_duration, estimated_unplanned_deforestation,
            rest_target, intervention
        from "MapExplorer" where session_id is not distinct from $1 limit 1"#,
    )
    .bind(arg(&req, "session_id"))
    .fetch_optional(&ctx().await.app)
    .await;
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return core::json(400, &core::app_error("fail, session id not found", None)),
        Err(e) => return core::db_error(e).await,
    };
    match map_explorer_json(&row) {
        Ok(result) => core::json(200, &core::success(json!({ "result": result }), None, None)),
        Err(message) => core::json(500, &core::other_error(&message, None).await),
    }
}

/// `MapExplorer.to_json()`. Staging (2026-11-27, 1911 rows) holds only protocol-5 pickles of
/// lists of str (and one bare str), with no globals/reduce ops, so serde-pickle decodes every
/// row. A pickle holding a non-builtin object (numpy etc.) fails here and answers the 500
/// envelope, where the original would have unpickled it: a known gap, no such row exists.
fn map_explorer_json(row: &PgRow) -> Result<Value, String> {
    let unpickle = |col: &str| -> Result<Value, String> {
        match row
            .try_get::<Option<Vec<u8>>, _>(col)
            .map_err(|e| e.to_string())?
        {
            None => Ok(Value::Null),
            Some(bytes) => serde_pickle::from_slice(&bytes, serde_pickle::DeOptions::new())
                .map_err(|e| e.to_string()),
        }
    };
    Ok(json!({
        "session_id": row.try_get::<Option<String>, _>("session_id").map_err(|e| e.to_string())?,
        "project_duration": row.try_get::<Option<i32>, _>("project_duration").map_err(|e| e.to_string())?,
        "estimated_unplanned_deforestation": row
            .try_get::<Option<f64>, _>("estimated_unplanned_deforestation")
            .map_err(|e| e.to_string())?,
        "rest_target": unpickle("rest_target")?,
        "intervention": unpickle("intervention")?,
    }))
}

/// `GeoUtils.get_db` on the GIS database: one retry after a failure, as the original refreshes
/// its connection and runs the query again.
async fn gis_rows(sql: &str, pattern: &str) -> Result<Vec<PgRow>, sqlx::Error> {
    let gis = &ctx().await.gis;
    // the value inlined as psycopg2 inlines it (an untyped literal), sent unprepared: the GIS
    // database sits behind a pooler that loses prepared statements (26000) under concurrency
    let run = || {
        crate::pq::query(sql)
            .bind(crate::pq::Untyped(pattern))
            .fetch_all(gis)
    };
    match run().await {
        Ok(rows) => Ok(rows),
        Err(_) => run().await,
    }
}

/// `f"%{filter_text or ''}%"`
fn like_pattern(filter: Option<String>) -> String {
    format!("%{}%", filter.unwrap_or_default())
}

/// The list routes answer `success_handler(items)`: the list, or `{}` when it is empty.
async fn list_response(sql: &str, req: &Req, item: fn(&PgRow) -> Value) -> Response {
    match gis_rows(sql, &like_pattern(arg(req, "filter"))).await {
        Ok(rows) => {
            let items: Vec<Value> = rows.iter().map(item).collect();
            core::json(200, &core::success(Value::Array(items), None, None))
        }
        Err(e) => core::db_error(e).await,
    }
}

/// `GET /geos/map/location`
pub async fn map_location_get(req: Req) -> Response {
    let sql = r#"select location_value, location_name, display_name, location_type
        from public."mvwSearchLocation"
        where lower(location_name) ~~ lower($1)
        limit 10"#;
    list_response(sql, &req, |row| {
        json!({
            "location_value": row.get::<Option<String>, _>("location_value"),
            "location_name": row.get::<Option<String>, _>("display_name"),
            "location_type": row.get::<Option<String>, _>("location_type"),
        })
    })
    .await
}

/// `GET /geos/map/location-search`
pub async fn map_location_search_get(req: Req) -> Response {
    // loc_id is a uuid; jsonify writes str(UUID), which is Postgres' text form.
    let sql = r#"select loc_id::text as loc_id, loc_name, info_text
        from public."mvwLocation"
        where lower(loc_name) ~~ lower($1)
        limit 10"#;
    list_response(sql, &req, |row| {
        json!({
            "location_value": row.get::<Option<String>, _>("loc_id"),
            "location_name": row.get::<Option<String>, _>("loc_name"),
            "location_info": row.get::<Option<String>, _>("info_text"),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_wraps_filter() {
        assert_eq!(like_pattern(None), "%%");
        assert_eq!(like_pattern(Some("a_b".into())), "%a_b%");
    }

    #[test]
    fn pickled_list_of_str_decodes_as_jsonify_writes_it() {
        // pickle.dumps(['a', 'é'], protocol=5)
        let bytes = b"\x80\x05\x95\x0e\x00\x00\x00\x00\x00\x00\x00]\x94(\x8c\x01a\x94\x8c\x02\xc3\xa9\x94e.";
        let v: Value = serde_pickle::from_slice(bytes, serde_pickle::DeOptions::new()).unwrap();
        assert_eq!(core::to_flask_json(&v), "[\"a\",\"\\u00e9\"]\n");
    }

    #[test]
    fn geojson_numbers_survive_as_python_prints_them() {
        let text = r#"{"type":"Polygon","coordinates":[[[114.455383,4.45],[104.630257878,-1.977075121],[114,-2]]]}"#;
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            core::to_flask_json(&json!({ "result": v })),
            "{\"result\":{\"coordinates\":[[[114.455383,4.45],[104.630257878,-1.977075121],[114,-2]]],\"type\":\"Polygon\"}}\n"
        );
    }
}
