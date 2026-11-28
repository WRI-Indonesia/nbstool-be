//! `application/apis/project_apis/routes.py`: the read-only project routes.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::postgres::PgRow;
use sqlx::{Row, TypeInfo, ValueRef};

use crate::core::{self, Req, arg, ctx};

// ---- how a route can fail, as its `try/except` sorts it ----

enum Fail {
    /// `except Exception` from the database: 500.
    Db(sqlx::Error),
    /// `except Exception` from Python itself (an AttributeError, a TypeError ...): 500.
    Py(String),
    /// `except AppMessageException`: 400.
    App(&'static str),
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
    }
}

// ---- Python's view of a JSONB value ----

type PyResult<T> = Result<T, String>;

static EMPTY_DICT: LazyLock<Value> = LazyLock::new(|| Value::Object(Map::new()));
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

/// `v or {}`
fn or_dict(v: &Value) -> &Value {
    if truthy(v) { v } else { &EMPTY_DICT }
}

/// `v.get(key)`: None when missing, AttributeError when `v` is not a dict.
fn get<'a>(v: &'a Value, key: &str) -> PyResult<&'a Value> {
    match v {
        Value::Object(map) => Ok(map.get(key).unwrap_or(&NONE)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            type_name(other)
        )),
    }
}

/// `for x in (v or [])`
fn iter_or_empty(v: &Value) -> PyResult<Vec<Value>> {
    if !truthy(v) {
        return Ok(vec![]);
    }
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(map.keys().map(|k| json!(k)).collect()),
        Value::String(s) => Ok(s.chars().map(|c| json!(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", type_name(other))),
    }
}

/// `len(v or [])`
fn len_or_empty(v: &Value) -> PyResult<usize> {
    if !truthy(v) {
        return Ok(0);
    }
    match v {
        Value::Array(items) => Ok(items.len()),
        Value::Object(map) => Ok(map.len()),
        Value::String(s) => Ok(s.chars().count()),
        other => Err(format!(
            "object of type '{}' has no len()",
            type_name(other)
        )),
    }
}

/// `(v or 0) > 0`
fn gt_zero(v: &Value) -> PyResult<bool> {
    match v {
        _ if !truthy(v) => Ok(false),
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => Ok(n.as_f64().is_some_and(|f| f > 0.0)),
        other => Err(format!(
            "'>' not supported between instances of '{}' and 'int'",
            type_name(other)
        )),
    }
}

// ---- helpers of routes.py ----

const ECO_SELECTION_KEYS: [(&str, &str); 3] = [
    ("forest", "Forest"),
    ("mangrove", "Mangrove"),
    ("peatland", "Peatland"),
];
const PATHWAY_ORDER: [&str; 3] = ["Protect", "Manage", "Restore"];

/// `_pathway_json`: the column when it is a dict, else `{}`.
fn pathway_json(intervention_eligibility_json: &Value) -> &Value {
    match intervention_eligibility_json {
        v @ Value::Object(_) => v,
        _ => &EMPTY_DICT,
    }
}

/// `_project_ecosystems(analyzer)`; both arguments are null when there is no analyzer.
fn project_ecosystems(benefit_json: &Value, eligibility_json: &Value) -> PyResult<Value> {
    let benefit = or_dict(benefit_json);
    let selections = or_dict(get(or_dict(get(benefit, "assumptions")?), "selections")?);
    let mut items = vec![];
    for (key, name) in ECO_SELECTION_KEYS {
        let chosen = or_dict(get(selections, key)?);
        let mut pathways = vec![];
        for pw in PATHWAY_ORDER {
            if truthy(get(chosen, &pw.to_lowercase())?) {
                pathways.push(pw);
            }
        }
        if !pathways.is_empty() {
            items.push(json!({ "name": name, "key": key, "pathways": pathways }));
        }
    }
    if !items.is_empty() {
        return Ok(Value::Array(items));
    }
    let pathway = pathway_json(eligibility_json);
    for card in iter_or_empty(get(pathway, "ecosystems")?)? {
        let mut eligible = vec![];
        for i in iter_or_empty(get(&card, "interventions")?)? {
            if gt_zero(get(&i, "area_ha")?)? {
                match get(&i, "intervention")? {
                    v @ (Value::Array(_) | Value::Object(_)) => {
                        return Err(format!("unhashable type: '{}'", type_name(v)));
                    }
                    v => eligible.push(v.clone()),
                }
            }
        }
        let pathways: Vec<&str> = PATHWAY_ORDER
            .into_iter()
            .filter(|pw| eligible.iter().any(|e| e.as_str() == Some(pw)))
            .collect();
        if !pathways.is_empty() {
            let label = get(&card, "label")?;
            let (name, key) = match label {
                Value::String(s) if !s.is_empty() => (label.clone(), s.to_lowercase()),
                v if !truthy(v) => (json!(""), String::new()),
                v => {
                    return Err(format!(
                        "'{}' object has no attribute 'lower'",
                        type_name(v)
                    ));
                }
            };
            items.push(json!({ "name": name, "key": key, "pathways": pathways }));
        }
    }
    Ok(Value::Array(items))
}

/// `sum(i['area_ha'] or 0 for i in items)` (Python 3.12: compensated once a float joins).
fn sum_areas<'a>(areas: impl Iterator<Item = &'a Value>) -> PyResult<Value> {
    let items: Vec<Value> = areas
        .map(|a| if truthy(a) { a.clone() } else { json!(0) })
        .collect();
    if let Some(pos) = items
        .iter()
        .position(|v| !(v.is_number() || v.is_boolean()))
    {
        let acc = if items[..pos].iter().any(Value::is_f64) {
            "float"
        } else {
            "int"
        };
        return Err(format!(
            "unsupported operand type(s) for +: '{acc}' and '{}'",
            type_name(&items[pos])
        ));
    }
    core::py_sum_numbers(&items).ok_or_else(|| "Python int too large to convert to C long".into())
}

/// `datetime.isoformat()` of a `timestamptz` read by psycopg2; the server's zone is UTC.
fn iso_tz(d: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    json!(d.map(|d| core::py_isoformat(d.naive_utc()) + "+00:00"))
}

fn iso(d: Option<chrono::NaiveDateTime>) -> Value {
    json!(d.map(core::py_isoformat))
}

/// `urllib.parse.quote_plus` with werkzeug's `safe="!$'()*,/:;?@"`.
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b' ' => out.push('+'),
            b if b.is_ascii_alphanumeric() || b"_.-~!$'()*,/:;?@".contains(&b) => {
                out.push(b as char)
            }
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `hmac.new(key, msg, 'sha256').hexdigest()`.
pub(crate) fn hmac_sha256_hex(key: &str, message: &str) -> String {
    use hmac::Mac as _;
    // INFALLIBLE: HMAC takes a key of any length
    let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()) else {
        return String::new();
    };
    mac.update(message.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `_boundary_map_url`: `url_for('project_apis.projects_boundary_map', project_id=, sig=)`.
fn boundary_map_url(secret: &str, session_id: &str) -> String {
    let sig = hmac_sha256_hex(secret, &format!("boundary-map:{session_id}"));
    format!(
        "/projects/boundary-map?project_id={}&sig={sig}",
        quote_plus(session_id)
    )
}

// ---- GET /projects/statistics ----

const STATS_TTL: Duration = Duration::from_secs(300);
/// `_stats_cache` + `_stats_lock`: one query per TTL; the lock is held across it (no stampede).
static STATS: tokio::sync::Mutex<Option<(Instant, Value)>> = tokio::sync::Mutex::const_new(None);

/// One column of the view's row, as the dict comprehension leaves it (Decimal -> float).
fn stats_value(row: &PgRow, i: usize) -> Result<Value, sqlx::Error> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let type_name = raw.type_info().name().to_string();
    Ok(match type_name.as_str() {
        "INT2" => json!(row.try_get::<i16, _>(i)?),
        "INT4" => json!(row.try_get::<i32, _>(i)?),
        "INT8" => json!(row.try_get::<i64, _>(i)?),
        "FLOAT4" => json!(row.try_get::<f32, _>(i)? as f64),
        "FLOAT8" => json!(row.try_get::<f64, _>(i)?),
        "BOOL" => json!(row.try_get::<bool, _>(i)?),
        "TEXT" | "VARCHAR" | "NAME" | "BPCHAR" => json!(row.try_get::<String, _>(i)?),
        // `float(Decimal)`: the decimal text (simple query protocol), correctly rounded
        "NUMERIC" => json!(
            raw.as_str()
                .map_err(sqlx::Error::Decode)?
                .parse::<f64>()
                .ok()
        ),
        // shortcut: the view has only ints and a numeric today; another type falls to 503
        // (or the stale copy) until it is mapped here.
        other => {
            return Err(sqlx::Error::Decode(
                format!("vwstatistics column of type {other} is not mapped").into(),
            ));
        }
    })
}

async fn query_statistics() -> Result<Value, sqlx::Error> {
    let rows = crate::pq::query("SELECT * FROM public.vwstatistics")
        .fetch_all(&ctx().await.app)
        .await?;
    // `.mappings().one()`
    let [row] = rows.as_slice() else {
        return Err(sqlx::Error::RowNotFound);
    };
    let mut data = Map::new();
    for (i, column) in row.columns().iter().enumerate() {
        use sqlx::Column;
        data.insert(column.name().to_string(), stats_value(row, i)?);
    }
    data.insert(
        "as_of".into(),
        json!(core::py_isoformat(chrono::Utc::now().naive_utc())),
    );
    Ok(Value::Object(data))
}

/// `_landing_statistics()`
async fn landing_statistics() -> Option<Value> {
    let mut cache = STATS.lock().await;
    if let Some((at, data)) = cache.as_ref()
        && at.elapsed() < STATS_TTL
    {
        return Some(data.clone());
    }
    let data = match query_statistics().await {
        Ok(data) => data,
        // stale beats a broken landing page; with nothing cached the route answers 503
        Err(_) => cache.as_ref()?.1.clone(),
    };
    *cache = Some((Instant::now(), data.clone()));
    Some(data)
}

/// `GET /projects/statistics`
pub async fn statistics(_req: Req) -> Response {
    match landing_statistics().await {
        Some(data) => {
            let response = core::json(
                200,
                &core::success(json!({ "result": data }), Some(200), None),
            );
            let response = core::with_header(response, "cache-control", "public, max-age=300");
            core::with_header(response, "x-content-type-options", "nosniff")
        }
        None => {
            let response = core::json(503, &core::app_error("statistics unavailable", None));
            let response = core::with_header(response, "retry-after", "60");
            core::with_header(response, "cache-control", "no-store")
        }
    }
}

// ---- GET /projects/list ----

/// `GET /projects/list`
pub async fn list(req: Req) -> Response {
    let response = match list_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    core::vary_cookie(response)
}

async fn list_inner(req: &Req) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    let ctx = ctx().await;
    let org_name: Option<String> =
        crate::pq::query_scalar("SELECT name FROM tbl_organization_types WHERE id = $1 LIMIT 1")
            .bind(user.organization_type_id)
            .fetch_optional(&ctx.app)
            .await?;

    let projects = crate::pq::query(
        "SELECT session_id, project_name, project_description, privacy_level, created_at, updated_at \
         FROM tbl_user_sessions WHERE user_id = $1 AND is_active = 1 AND is_project = true \
         AND analyzer_version = 'v3' ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&ctx.app)
    .await?;
    let ids: Vec<String> = projects
        .iter()
        .map(|p| p.try_get("session_id"))
        .collect::<Result<_, _>>()?;

    // (site_information_json, benefit_json, intervention_eligibility_json), first row per id
    let mut analyzers: HashMap<String, (Value, Value, Value)> = HashMap::new();
    let mut doc_counts: HashMap<String, i64> = HashMap::new();
    let mut monitored: HashSet<String> = HashSet::new();
    let mut polygon_areas: HashMap<String, Option<f64>> = HashMap::new();
    if !ids.is_empty() {
        let rows = crate::pq::query(
            "SELECT session_id, site_information_json, benefit_json, intervention_eligibility_json \
             FROM \"DataAnalyzer\" WHERE session_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&ctx.app)
        .await?;
        for row in rows {
            let sid: String = row.try_get(0)?;
            let json_col = |i: usize| -> Result<Value, sqlx::Error> {
                Ok(row.try_get::<Option<Value>, _>(i)?.unwrap_or(Value::Null))
            };
            let entry = (json_col(1)?, json_col(2)?, json_col(3)?);
            analyzers.entry(sid).or_insert(entry);
        }
        let rows: Vec<(String, i64)> = crate::pq::query_as(
            "SELECT project_id, count(id) FROM \"DocumentList\" \
             WHERE project_id = ANY($1) AND is_active = 1 GROUP BY project_id",
        )
        .bind(&ids)
        .fetch_all(&ctx.app)
        .await?;
        doc_counts.extend(rows);
        let rows: Vec<String> = crate::pq::query_scalar(
            "SELECT session_id FROM \"DocumentData\" WHERE session_id = ANY($1) \
             AND certification_type = 'MonitoringV3' AND (form ->> 'mpPlan') IS NOT NULL",
        )
        .bind(&ids)
        .fetch_all(&ctx.app)
        .await?;
        monitored.extend(rows);
        let rows: Vec<(String, Option<f64>)> = crate::pq::query_as(
            "SELECT session_id, project_area_size::float8 FROM polygons WHERE session_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&ctx.app)
        .await?;
        for (sid, area) in rows {
            polygon_areas.entry(sid).or_insert(area);
        }
    }

    let no_analyzer = (Value::Null, Value::Null, Value::Null);
    let mut items = vec![];
    for (project, sid) in projects.iter().zip(&ids) {
        let (site_json, benefit_json, eligibility_json) =
            analyzers.get(sid).unwrap_or(&no_analyzer);
        let site = or_dict(site_json);
        let mut area = get(pathway_json(eligibility_json), "project_area_ha")?.clone();
        if area.is_null() {
            area = json!(polygon_areas.get(sid).copied().flatten());
        }
        let created: Option<chrono::DateTime<chrono::Utc>> = project.try_get("created_at")?;
        let updated: Option<chrono::DateTime<chrono::Utc>> = project.try_get("updated_at")?;
        items.push(json!({
            "project_id": sid,
            "project_name": project.try_get::<Option<String>, _>("project_name")?,
            "project_description": project.try_get::<Option<String>, _>("project_description")?,
            "country": get(site, "country")?,
            "province": get(site, "province")?,
            "area_ha": area,
            "ecosystems": project_ecosystems(benefit_json, eligibility_json)?,
            "monitoring_status": if monitored.contains(sid) { "Monitoring Active" } else { "Waiting for Monitoring Plan" },
            "documents": doc_counts.get(sid).copied().unwrap_or(0),
            "privacy_level": project.try_get::<Option<i16>, _>("privacy_level")?,
            "boundary_map_url": boundary_map_url(&ctx.secret, sid),
            "created_at": iso_tz(created),
            "updated_at": iso_tz(updated.or(created)),
        }));
    }
    let area_covered = sum_areas(items.iter().map(|i| &i["area_ha"]))?;
    let results = json!({
        "user": {
            "user_fullname": user.name,
            "user_email": user.email,
            "user_org_type": org_name,
            "user_org_name": user.organization_name,
        },
        "stats": {
            "total_projects": items.len(),
            "area_covered_ha": area_covered,
        },
        "projects": items,
    });
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), Some(200), None),
    ))
}

// ---- GET /projects ----

/// `GET /projects`
pub async fn detail(req: Req) -> Response {
    let response = match detail_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    core::vary_cookie(response)
}

async fn detail_inner(req: &Req) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    let ctx = ctx().await;
    let session_id = arg(req, "project_id");

    let polygon_area: Option<Option<f64>> = crate::pq::query_scalar(
        "SELECT project_area_size::float8 FROM polygons WHERE session_id = $1 LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&ctx.app)
    .await?;
    let Some(polygon_area) = polygon_area else {
        return Err(Fail::App("fail, session id Not found"));
    };

    let geom: Option<Option<String>> = crate::pq::query_scalar(
        "SELECT ST_AsGeoJSON(polygons.geom) FROM polygons WHERE polygons.session_id = $1 LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&ctx.app)
    .await?;
    let geom = match geom {
        None => return Err(Fail::Py("'NoneType' object is not subscriptable".into())),
        Some(None) => {
            return Err(Fail::Py(
                "the JSON object must be str, bytes or bytearray, not NoneType".into(),
            ));
        }
        Some(Some(text)) => serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?,
    };

    let project = crate::pq::query(
        "SELECT user_id, project_name, project_description, privacy_level, created_at, updated_at \
         FROM tbl_user_sessions WHERE session_id = $1 AND is_active = 1 LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&ctx.app)
    .await?;
    let Some(project) = project else {
        return Err(Fail::App("fail, project Not found"));
    };
    if project.try_get::<Option<i32>, _>("user_id")? != Some(user.id) {
        let body = core::other_error(
            "forbidden, only the project owner can view this project",
            Some(403),
        )
        .await;
        return Ok(core::json(403, &body));
    }

    let analyzer = crate::pq::query(
        "SELECT site_information_json, nature_json, climate_json, people_json, threat_json, \
         intervention_eligibility_json, benefit_json FROM \"DataAnalyzer\" WHERE session_id = $1 LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&ctx.app)
    .await?;
    // `analyzer.<col> if analyzer else None`: a missing row reads as all null
    let col = |name: &str| -> Result<Value, sqlx::Error> {
        match &analyzer {
            Some(row) => Ok(row
                .try_get::<Option<Value>, _>(name)?
                .unwrap_or(Value::Null)),
            None => Ok(Value::Null),
        }
    };
    let (site_json, benefit_json, eligibility_json) = (
        col("site_information_json")?,
        col("benefit_json")?,
        col("intervention_eligibility_json")?,
    );
    let site = or_dict(&site_json);
    let benefit = or_dict(&benefit_json);
    let assumptions = or_dict(get(benefit, "assumptions")?);
    let selections = or_dict(get(assumptions, "selections")?);

    let mut activities_selected = 0;
    for (key, _) in ECO_SELECTION_KEYS {
        activities_selected += len_or_empty(get(or_dict(get(selections, key)?), "activities")?)?;
    }

    let mp_form: Option<Option<Value>> = crate::pq::query_scalar(
        "SELECT form FROM \"DocumentData\" WHERE session_id = $1 \
         AND certification_type = 'MonitoringV3' LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&ctx.app)
    .await?;
    let mp_form = match mp_form.flatten() {
        Some(form @ Value::Object(_)) => form,
        _ => EMPTY_DICT.clone(),
    };
    let mp_plan = get(&mp_form, "mpPlan")?;
    let mut monitoring_plan = Value::Null;
    if truthy(mp_plan) {
        let mut activities = vec![];
        for eco in iter_or_empty(get(mp_plan, "ecos")?)? {
            activities.extend(iter_or_empty(get(&eco, "activities")?)?);
        }
        let mut indicators = 0;
        for a in &activities {
            for g in iter_or_empty(get(a, "groups")?)? {
                indicators += len_or_empty(get(&g, "items")?)?;
            }
        }
        monitoring_plan = json!({ "activities": activities.len(), "indicators": indicators });
    }
    // `_monitoring_status` reads the same row again; one read serves both
    let monitoring_status = if truthy(mp_plan) {
        "Monitoring Active"
    } else {
        "Waiting for Monitoring Plan"
    };

    let rows = crate::pq::query(
        "SELECT document_id, document_type, document_status, client_name, created_at \
         FROM \"DocumentList\" WHERE project_id = $1 AND is_active = 1 ORDER BY created_at DESC",
    )
    .bind(&session_id)
    .fetch_all(&ctx.app)
    .await?;
    let mut documents = vec![];
    for d in &rows {
        documents.push(json!({
            "document_id": d.try_get::<String, _>("document_id")?,
            "document_type": d.try_get::<String, _>("document_type")?,
            "document_status": d.try_get::<String, _>("document_status")?,
            "document_name": d.try_get::<Option<String>, _>("client_name")?,
            "created_at": iso(d.try_get("created_at")?),
        }));
    }

    let country = get(site, "country")?.clone();
    let province = get(site, "province")?.clone();
    let district = get(site, "district")?.clone();
    // `_project_area_ha`: its Polygons lookup is the row already found above
    let mut area = get(pathway_json(&eligibility_json), "project_area_ha")?.clone();
    if area.is_null() {
        area = json!(polygon_area);
    }
    // the polygon was found by this id, so it is a string here
    let sid = session_id.unwrap_or_default();
    let created: Option<chrono::DateTime<chrono::Utc>> = project.try_get("created_at")?;
    let updated: Option<chrono::DateTime<chrono::Utc>> = project.try_get("updated_at")?;

    let results = json!({
        "polygon": geom,
        "project_id": sid,
        "project_name": project.try_get::<Option<String>, _>("project_name")?,
        "project_description": project.try_get::<Option<String>, _>("project_description")?,
        "country": country,
        "province": province,
        "district": district,
        "area_ha": area,
        "boundary_map_url": boundary_map_url(&ctx.secret, &sid),
        "monitoring_status": monitoring_status,
        "ecosystems": project_ecosystems(&benefit_json, &eligibility_json)?,
        "intervention": {
            "duration_years": get(assumptions, "duration_years")?,
            "activities_selected": if activities_selected == 0 { Value::Null } else { json!(activities_selected) },
        },
        "documents": documents,
        "monitoring_plan": monitoring_plan,
        "privacy_level": project.try_get::<Option<i16>, _>("privacy_level")?,
        "created_at": iso_tz(created),
        "updated_at": iso_tz(updated.or(created)),
        "analysis": {
            "site_information": site_json,
            "nature": col("nature_json")?,
            "climate": col("climate_json")?,
            "people": col("people_json")?,
            "threat": col("threat_json")?,
            "pathway": eligibility_json,
            "benefit": benefit_json,
        },
    });
    Ok(core::json(
        200,
        &core::success(json!({ "result": results }), Some(200), None),
    ))
}

// ---- GET /projects/documents ----

/// `GET /projects/documents`
pub async fn documents(req: Req) -> Response {
    let response = match documents_inner(&req).await {
        Ok(response) => response,
        Err(fail) => fail_response(fail).await,
    };
    core::vary_cookie(response)
}

async fn documents_inner(req: &Req) -> Result<Response, Fail> {
    let Some(user) = core::current_user(req).await? else {
        return Ok(core::not_logged_in().await);
    };
    let ctx = ctx().await;
    let project_id = arg(req, "project_id");
    let owner: Option<Option<i32>> = crate::pq::query_scalar(
        "SELECT user_id FROM tbl_user_sessions WHERE session_id = $1 AND is_active = 1 LIMIT 1",
    )
    .bind(&project_id)
    .fetch_optional(&ctx.app)
    .await?;
    if owner.flatten() != Some(user.id) {
        let body = core::other_error(
            "forbidden, only the project owner can view its documents",
            Some(403),
        )
        .await;
        return Ok(core::json(403, &body));
    }
    let rows = crate::pq::query(
        "SELECT document_id, document_type, document_status, client_name, project_id \
         FROM \"DocumentList\" WHERE project_id = $1 AND is_active = 1 ORDER BY created_at DESC",
    )
    .bind(&project_id)
    .fetch_all(&ctx.app)
    .await?;
    let mut items = vec![];
    for d in &rows {
        items.push(json!({
            "document_id": d.try_get::<String, _>("document_id")?,
            "document_type": d.try_get::<String, _>("document_type")?,
            "document_status": d.try_get::<String, _>("document_status")?,
            "document_name": d.try_get::<Option<String>, _>("client_name")?,
            "project_id": d.try_get::<String, _>("project_id")?,
        }));
    }
    // the view returns the bare list; Flask jsonifies it
    Ok(core::json(200, &Value::Array(items)))
}

// ---- GET /projects/boundary-map ----

/// `GET /projects/boundary-map` (`projects_boundary_map`): the card picture from GCS, or the stock
/// picture, served with `send_file`.
pub async fn boundary_map(req: Req) -> Response {
    let fail = |status: u16, message: &str| core::json(status, &core::app_error(message, None));
    let Some(session_id) = arg(&req, "project_id").filter(|s| !s.is_empty()) else {
        return fail(400, "please provide project_id");
    };
    let sig = arg(&req, "sig").unwrap_or_default();
    let expected = hmac_sha256_hex(&ctx().await.secret, &format!("boundary-map:{session_id}"));
    // hmac.compare_digest on str refuses non-ASCII
    if !sig.is_ascii() {
        let message = "comparing strings with non-ASCII characters is not supported";
        return core::json(500, &core::other_error(message, None).await);
    }
    if sig != expected {
        return core::json(
            403,
            &core::other_error("forbidden, invalid map link", Some(403)).await,
        );
    }
    let polygon: Result<Option<i32>, sqlx::Error> =
        crate::pq::query_scalar("select id from polygons where session_id = $1 limit 1")
            .bind(&session_id)
            .fetch_optional(&ctx().await.app)
            .await;
    match polygon {
        Err(e) => return core::db_error(e).await,
        Ok(None) => return fail(400, "fail, project id Not found"),
        Ok(Some(_)) => {}
    }
    // CloudStorage(): get_bucket, then the download; a miss leaves no file behind
    crate::gcs::get_bucket().await;
    let present = |p: &str| std::fs::metadata(p).is_ok_and(|m| m.len() > 0);
    let mut path = crate::figures::boundary_map_path(&session_id);
    if let Err(e) = std::fs::create_dir_all(crate::figures::BOUNDARY_MAP_FOLDER) {
        return core::json(500, &core::other_error(&e.to_string(), None).await);
    }
    crate::gcs::download(&path).await;
    if !present(&path) {
        // not drawn (yet, or it failed at /bind): the stock picture, fetched once
        path = crate::figures::boundary_map_path("IMAGE_PROJECT MAP");
        if !present(&path) {
            crate::gcs::download(&path).await;
        }
        if !present(&path) {
            return fail(400, "map not available");
        }
    }
    match crate::figures::send_file(&path, Some("image/png"), false, None) {
        Ok(response) => response,
        Err(e) => core::json(500, &core::other_error(&e.to_string(), None).await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        assert_eq!(
            hmac_sha256_hex("Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn quote_plus_matches_werkzeug() {
        assert_eq!(quote_plus("a b/c?d&e=f%é"), "a+b/c?d%26e%3Df%25%C3%A9");
        assert_eq!(quote_plus("abc-123_x.y~"), "abc-123_x.y~");
    }

    #[test]
    fn area_sum_is_python_sum() {
        let v = [json!(null), json!(2), json!(true)];
        assert_eq!(sum_areas(v.iter()).ok(), Some(json!(3)));
        let v = [json!(1), json!(0.5), json!(null)];
        assert_eq!(sum_areas(v.iter()).ok(), Some(json!(1.5)));
        assert!(sum_areas([json!("x")].iter()).is_err());
    }

    #[test]
    fn ecosystems_prefer_selections_then_cards() {
        let benefit =
            json!({"assumptions": {"selections": {"mangrove": {"manage": true, "protect": 1}}}});
        assert_eq!(
            project_ecosystems(&benefit, &Value::Null).ok(),
            Some(
                json!([{"name": "Mangrove", "key": "mangrove", "pathways": ["Protect", "Manage"]}])
            )
        );
        let pathway = json!({"ecosystems": [{"label": "Forest", "interventions": [
            {"intervention": "Restore", "area_ha": 2.0},
            {"intervention": "Protect", "area_ha": 0}]}]});
        assert_eq!(
            project_ecosystems(&Value::Null, &pathway).ok(),
            Some(json!([{"name": "Forest", "key": "forest", "pathways": ["Restore"]}]))
        );
        assert!(project_ecosystems(&json!([1]), &Value::Null).is_err());
    }
}
