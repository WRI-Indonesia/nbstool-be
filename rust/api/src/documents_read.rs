//! The v3 document draft readers of `document_apis/feasibility/routes.py` and
//! `document_apis/monitoring/routes.py`, with `load_draft` of `document_apis/utils.py`.

use axum::response::Response;
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::core::{self, Req, arg, ctx};

/// `GET /documents/monitoring/v3/activities`
pub async fn monitoring_activities(_req: Req) -> Response {
    let result = async {
        let longform = LONGFORM
            .get_or_try_init(|| load_longform_table(LONGFORM_TABLE))
            .await?;
        let activity = ACTIVITY
            .get_or_try_init(|| load_activity_table(ACTIVITY_TABLE))
            .await?;
        Ok::<_, Fail>(activities_result(longform, activity))
    }
    .await;
    match result {
        Ok(results) => core::with_header(ok(results), "cache-control", "public, max-age=86400"),
        Err(e) => e.respond().await,
    }
}

/// The route body between the two loads and the response.
fn activities_result(longform: &[(String, LongformSlot)], activity: &ActivityTable) -> Value {
    // ids: {norm_activity: {activity_id: label}}, both in first-seen order (setdefault)
    #[expect(
        clippy::type_complexity,
        reason = "nested insertion-ordered dicts of the Python, kept as vectors"
    )]
    let mut ids: Vec<(String, Vec<(String, Option<&'static str>)>)> = Vec::new();
    for ((_, ec), rows) in activity {
        for (activity_id, text) in rows {
            let key = norm_activity(text);
            let pos = match ids.iter().position(|(k, _)| *k == key) {
                Some(p) => p,
                None => {
                    ids.push((key, Vec::new()));
                    ids.len() - 1
                }
            };
            let slot = &mut ids[pos].1;
            if !slot.iter().any(|(id, _)| id == activity_id) {
                slot.push((activity_id.clone(), pathway_ecosystem_label(*ec)));
            }
        }
    }
    let mut items: Vec<(&str, &str, &str, Value)> = Vec::new();
    for (key, slot) in longform {
        let pairs: Vec<(Value, Value)> = match ids.iter().find(|(k, _)| k == key) {
            Some((_, pairs)) => pairs
                .iter()
                .map(|(id, label)| (json!(id), json!(label)))
                .collect(),
            None => vec![(Value::Null, Value::Null)],
        };
        for (activity_id, label) in pairs {
            items.push((
                &slot.ecosystem,
                &slot.pathway,
                &slot.activity,
                json!({
                    "activity_id": activity_id,
                    "activity": slot.activity,
                    "ecosystem": slot.ecosystem,
                    "ecosystem_label": label,
                    "pathway": slot.pathway,
                    "indicators": slot.indicators,
                }),
            ));
        }
    }
    // list.sort is stable, and so is sort_by
    items.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    let items: Vec<Value> = items.into_iter().map(|i| i.3).collect();
    json!({ "activities": items })
}

// ---- the two v3 CSV catalogs of `geos/v3/common.py`, read as pandas.read_csv reads them ----

const ACTIVITY_TABLE: &str = "canonical_v3_activities.csv";
const LONGFORM_TABLE: &str = "activities_longform_v3.csv";

/// `load_activity_table`: ((cat_code, ecosystem), [(activity_id, activity)]) in first-seen
/// order. Only the two columns this route reads are kept.
type ActivityTable = Vec<((i64, i64), Vec<(String, String)>)>;

struct LongformSlot {
    activity: String,
    ecosystem: String,
    pathway: String,
    indicators: Vec<Value>,
}

/// `@lru_cache(maxsize=1)`: kept for the life of the process, a failure is not cached.
static ACTIVITY: tokio::sync::OnceCell<ActivityTable> = tokio::sync::OnceCell::const_new();
static LONGFORM: tokio::sync::OnceCell<Vec<(String, LongformSlot)>> =
    tokio::sync::OnceCell::const_new();

/// `PATHWAY_ECOSYSTEM_CODES.get(ec)`
fn pathway_ecosystem_label(ec: i64) -> Option<&'static str> {
    match ec {
        0 => Some("None"),
        1 => Some("Dryland forest"),
        2 => Some("Mangrove"),
        3 => Some("Peatland"),
        4 => Some("Savanna"),
        _ => None,
    }
}

/// `settings.layer_path(layer)` then `pd.read_csv` of it.
async fn read_layer(layer: &str) -> Result<Frame, Fail> {
    // get_setting(V3_BUCKET): `.first()`, no LIMIT
    let row: Option<(Option<String>,)> =
        crate::pq::query_as("select value from tbl_master_settings where name = $1")
            .bind("V3_BUCKET")
            .fetch_optional(&ctx().await.app)
            .await
            .map_err(Fail::Db)?;
    let Some(root) = row.and_then(|r| r.0).filter(|v| !v.is_empty()) else {
        return Err(Fail::Py(
            "tbl_master_settings has no usable 'V3_BUCKET' row. Insert it before running the v3 \
             site characterisation."
                .into(),
        ));
    };
    let root = root.trim_end_matches('/');
    let bytes = if root.starts_with("http") {
        let url = format!("{root}/{layer}");
        let response = reqwest::get(&url)
            .await
            .map_err(|e| Fail::Py(format!("<urlopen error {e}>")))?;
        let status = response.status();
        if !status.is_success() {
            // urllib's HTTPError
            return Err(Fail::Py(format!(
                "HTTP Error {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )));
        }
        response
            .bytes()
            .await
            .map_err(|e| Fail::Py(format!("<urlopen error {e}>")))?
            .to_vec()
    } else {
        let path = format!("{root}/v3_{layer}");
        std::fs::read(&path).map_err(|e| {
            Fail::Py(match e.kind() {
                std::io::ErrorKind::NotFound => {
                    format!("[Errno 2] No such file or directory: {}", repr_str(&path))
                }
                _ => e.to_string(),
            })
        })?
    };
    let text = String::from_utf8(bytes).map_err(|e| {
        Fail::Py(format!(
            "'utf-8' codec can't decode byte in position {}: invalid start byte",
            e.utf8_error().valid_up_to()
        ))
    })?;
    read_csv(&text).map_err(Fail::Py)
}

async fn load_activity_table(layer: &'static str) -> Result<ActivityTable, Fail> {
    parse_activity_table(layer, read_layer(layer).await?)
}

fn parse_activity_table(layer: &str, frame: Frame) -> Result<ActivityTable, Fail> {
    let columns: Vec<String> = frame
        .columns
        .iter()
        .map(|c| {
            let c = py_strip(c).to_lowercase();
            match c.as_str() {
                "cat_id" | "cat_code" => "cat_code".into(),
                "activity id" | "activity_id" => "activity_id".into(),
                "benefit nature" => "benefit_nature".into(),
                "benefit people" => "benefit_people".into(),
                "benefit climate" => "benefit_climate".into(),
                "qb avoided emissions" | "qb_avoided" => "qb_avoided".into(),
                "qb carbon sequestration" | "qb_sequestration" => "qb_sequestration".into(),
                _ => c,
            }
        })
        .collect();
    check_columns(
        layer,
        &columns,
        &[
            "activity",
            "activity_id",
            "benefit_climate",
            "benefit_nature",
            "benefit_people",
            "cat_code",
            "ecosystem",
            "qb_avoided",
            "qb_sequestration",
        ],
    )?;
    // shortcut: a duplicated column name reads the first one, where pandas hands back a Series
    let col = |name: &str| columns.iter().position(|c| c == name).unwrap_or(0);
    let (cat, eco, id, text) = (
        col("cat_code"),
        col("ecosystem"),
        col("activity_id"),
        col("activity"),
    );
    let mut table: ActivityTable = Vec::new();
    for row in &frame.rows {
        let e = &row[eco];
        if matches!(e, Cell::Na) || py_strip(&e.py_str()).is_empty() {
            continue; // ineligible category, no (cat_code, ecosystem) join key
        }
        let s = py_strip(&e.py_str()).to_string();
        let ec = if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
            s.parse::<i64>().ok()
        } else {
            match s.to_lowercase().as_str() {
                "dryland forest" => Some(1),
                "mangrove" => Some(2),
                "peatland" => Some(3),
                "savanna" => Some(4),
                _ => None,
            }
        };
        let Some(ec) = ec else {
            return Err(Fail::Py(format!(
                "Unknown ecosystem {} in {layer}.",
                e.py_repr()
            )));
        };
        let key = (py_int(&row[cat]).map_err(Fail::Py)?, ec);
        let entry = (
            clean_id(&row[id]).map_err(Fail::Py)?,
            clean_text(&row[text]),
        );
        match table.iter_mut().find(|(k, _)| *k == key) {
            Some((_, rows)) => rows.push(entry),
            None => table.push((key, vec![entry])),
        }
    }
    Ok(table)
}

async fn load_longform_table(layer: &'static str) -> Result<Vec<(String, LongformSlot)>, Fail> {
    parse_longform_table(layer, read_layer(layer).await?)
}

fn parse_longform_table(layer: &str, frame: Frame) -> Result<Vec<(String, LongformSlot)>, Fail> {
    check_columns(
        layer,
        &frame.columns,
        &[
            "activity",
            "benefit",
            "benefit_category",
            "definition",
            "ecosystem",
            "frequency",
            "indicator",
            "method",
            "pathway",
            "reference",
            "unit",
        ],
    )?;
    let col = |name: &str| frame.columns.iter().position(|c| c == name).unwrap_or(0);
    // .fillna(""): NaN reads as ""
    let get = |row: &[Cell], name: &str| match &row[col(name)] {
        Cell::Na => String::new(),
        cell => cell.py_str(),
    };
    let mut table: Vec<(String, LongformSlot)> = Vec::new();
    for row in &frame.rows {
        let eco = match py_strip(&get(row, "ecosystem")).to_lowercase().as_str() {
            "dryland" => "forest",
            "mangrove" => "mangrove",
            "peatland" => "peatland",
            _ => {
                let cell = &row[col("ecosystem")];
                let repr = if matches!(cell, Cell::Na) {
                    "''".into()
                } else {
                    cell.py_repr()
                };
                return Err(Fail::Py(format!("Unknown ecosystem {repr} in {layer}.")));
            }
        };
        let activity_cell = &row[col("activity")];
        // norm_activity(r["activity"]): `text or ""`
        let key = norm_activity(&if activity_cell.truthy() {
            activity_cell.py_str()
        } else {
            String::new()
        });
        let indicator = json!({
            "benefit_category": py_strip(&get(row, "benefit_category")),
            "benefit": collapse(&get(row, "benefit")),
            "indicator": collapse(&get(row, "indicator")),
            "unit": py_strip(&get(row, "unit")),
            "frequency": py_strip(&get(row, "frequency")),
            "method": collapse(&get(row, "method")),
            "reference": collapse(&get(row, "reference")),
            "definition": collapse(&get(row, "definition")),
        });
        match table.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => slot.indicators.push(indicator),
            None => table.push((
                key,
                LongformSlot {
                    activity: collapse(&get(row, "activity")),
                    ecosystem: eco.into(),
                    pathway: capitalize(py_strip(&get(row, "pathway"))),
                    indicators: vec![indicator],
                },
            )),
        }
    }
    Ok(table)
}

/// `missing = required - set(df.columns)`; `required` is given sorted.
fn check_columns(layer: &str, columns: &[String], required: &[&str]) -> Result<(), Fail> {
    let missing: Vec<String> = required
        .iter()
        .filter(|r| !columns.iter().any(|c| c == *r))
        .map(|r| format!("'{r}'"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Fail::Py(format!(
            "{layer} is missing columns [{}].",
            missing.join(", ")
        )))
    }
}

/// `norm_activity(text)` on a str.
fn norm_activity(text: &str) -> String {
    collapse(text)
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// `" ".join(s.split())`
fn collapse(s: &str) -> String {
    s.split(py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// str.isspace: Rust's White_Space plus the four separators U+001C..U+001F.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// str.capitalize
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.as_str().to_lowercase().chars())
            .collect(),
        None => String::new(),
    }
}

/// The text float()/int() accept: stripped, underscores allowed only between digits.
fn py_number_text(s: &str) -> Option<String> {
    let s = py_strip(s);
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'_'
            && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// `int(v)` of a cell.
fn py_int(cell: &Cell) -> Result<i64, String> {
    match cell {
        Cell::Int(i) => Ok(*i),
        Cell::Bool(b) => Ok(*b as i64),
        Cell::Float(f) => float_to_int(*f),
        Cell::Na => float_to_int(f64::NAN),
        Cell::Str(s) => py_number_text(s)
            .and_then(|t| t.parse::<i64>().ok())
            .ok_or_else(|| format!("invalid literal for int() with base 10: {}", repr_str(s))),
    }
}

fn float_to_int(f: f64) -> Result<i64, String> {
    if f.is_nan() {
        Err("cannot convert float NaN to integer".into())
    } else if f.is_infinite() {
        Err("cannot convert float infinity to integer".into())
    } else {
        Ok(f.trunc() as i64)
    }
}

/// `clean_id`: "" for NaN, else `str(int(float(v)))`, falling back to `str(v).strip()` on a
/// ValueError (OverflowError of an infinity is not caught, as in the Python).
fn clean_id(cell: &Cell) -> Result<String, String> {
    let f = match cell {
        Cell::Na => return Ok(String::new()),
        Cell::Int(i) => *i as f64,
        Cell::Float(f) => *f,
        Cell::Bool(b) => *b as i64 as f64,
        Cell::Str(s) => match py_number_text(s).and_then(|t| t.parse::<f64>().ok()) {
            Some(f) if !f.is_nan() => f,
            _ => return Ok(py_strip(s).to_string()),
        },
    };
    if f.is_infinite() {
        return Err("cannot convert float infinity to integer".into());
    }
    let t = f.trunc();
    Ok(if t == 0.0 {
        "0".into()
    } else {
        format!("{t:.0}")
    })
}

/// `clean_text`: "" for NaN, else whitespace-collapsed.
fn clean_text(cell: &Cell) -> String {
    match cell {
        Cell::Na => String::new(),
        cell => collapse(&cell.py_str()),
    }
}

/// One parsed value, typed by its column as pandas infers it.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Na,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Cell {
    /// `str(v)`
    fn py_str(&self) -> String {
        match self {
            Cell::Na => "nan".into(),
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => py_float_str(*f),
            Cell::Bool(b) => if *b { "True" } else { "False" }.into(),
            Cell::Str(s) => s.clone(),
        }
    }

    /// `repr(v)`
    fn py_repr(&self) -> String {
        match self {
            Cell::Str(s) => repr_str(s),
            other => other.py_str(),
        }
    }

    /// Truthiness after `.fillna("")`, the only place it is asked: NaN has become "".
    fn truthy(&self) -> bool {
        match self {
            Cell::Na => false,
            Cell::Int(i) => *i != 0,
            Cell::Float(f) => *f != 0.0,
            Cell::Bool(b) => *b,
            Cell::Str(s) => !s.is_empty(),
        }
    }
}

fn py_float_str(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        core::to_flask_json(&json!(f)).trim_end().to_string()
    }
}

struct Frame {
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

/// pandas' default `na_values`.
const NA_VALUES: &[&str] = &[
    "", "#N/A", "#N/A N/A", "#NA", "-1.#IND", "-1.#QNAN", "-NaN", "-nan", "1.#IND", "1.#QNAN",
    "<NA>", "N/A", "NA", "NULL", "NaN", "None", "n/a", "nan", "null",
];

/// `pd.read_csv(text)` with default options: comma, double-quote, BOM dropped, blank lines
/// skipped, header row with duplicate names mangled, then per-column dtype inference
/// (int64, float64 when NaN or a decimal is present, bool, else object).
fn read_csv(text: &str) -> Result<Frame, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records: Vec<Vec<String>> = Vec::new();
    let (mut fields, mut field) = (Vec::new(), String::new());
    let (mut in_quotes, mut quoted, mut any) = (false, false, false);
    let mut chars = text.chars().peekable();
    let mut end_record = |fields: &mut Vec<String>, field: &mut String, quoted: bool| {
        fields.push(std::mem::take(field));
        let blank =
            !quoted && fields.len() == 1 && fields[0].chars().all(|c| c == ' ' || c == '\t');
        let record = std::mem::take(fields);
        if !blank {
            records.push(record);
        }
    };
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            ',' => {
                fields.push(std::mem::take(&mut field));
                quoted = false;
            }
            '\n' | '\r' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_record(&mut fields, &mut field, quoted);
                quoted = false;
                any = false;
            }
            c => field.push(c),
        }
    }
    if any {
        end_record(&mut fields, &mut field, quoted);
    }
    if in_quotes {
        return Err(format!(
            "Error tokenizing data. C error: EOF inside string starting at row {}",
            records.len().saturating_sub(1)
        ));
    }
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Err("No columns to parse from file".into());
    };
    let mut columns: Vec<String> = Vec::new();
    for name in header {
        let mut candidate = name.clone();
        let mut n = 0;
        while columns.contains(&candidate) {
            n += 1;
            candidate = format!("{name}.{n}");
        }
        columns.push(candidate);
    }
    let width = columns.len();
    let mut raw: Vec<Vec<String>> = Vec::new();
    for (i, record) in records.enumerate() {
        if record.len() > width {
            // shortcut: pandas turns a uniformly one-longer body into an index; reported as an error
            return Err(format!(
                "Error tokenizing data. C error: Expected {width} fields in line {}, saw {}\n",
                i + 2,
                record.len()
            ));
        }
        raw.push(record);
    }
    let mut rows: Vec<Vec<Cell>> = vec![Vec::with_capacity(width); raw.len()];
    for c in 0..width {
        let cells: Vec<Option<&str>> = raw
            .iter()
            .map(|r| {
                r.get(c)
                    .map(String::as_str)
                    .filter(|v| !NA_VALUES.contains(v))
            })
            .collect();
        let present = || cells.iter().flatten();
        let has_na = cells.iter().any(Option::is_none);
        let as_int = |v: &str| py_strip(v).parse::<i64>().ok();
        let as_float = |v: &str| {
            let t = py_strip(v);
            t.parse::<f64>()
                .ok()
                .filter(|_| !t.to_ascii_lowercase().contains("nan"))
        };
        let as_bool = |v: &str| match v {
            "True" | "TRUE" | "true" => Some(true),
            "False" | "FALSE" | "false" => Some(false),
            _ => None,
        };
        let typed: Box<dyn Fn(&str) -> Cell> = if present().all(|v| as_int(v).is_some()) && !has_na
        {
            Box::new(|v| as_int(v).map_or(Cell::Na, Cell::Int))
        } else if present().all(|v| as_float(v).is_some()) {
            Box::new(|v| as_float(v).map_or(Cell::Na, Cell::Float))
        } else if present().all(|v| as_bool(v).is_some()) {
            Box::new(|v| as_bool(v).map_or(Cell::Na, Cell::Bool))
        } else {
            Box::new(|v| Cell::Str(v.to_string()))
        };
        for (row, cell) in rows.iter_mut().zip(&cells) {
            row.push(cell.map_or(Cell::Na, &typed));
        }
    }
    Ok(Frame { columns, rows })
}

/// `GET /documents/feasibility/v3`
pub async fn feasibility_v3_get(req: Req) -> Response {
    let Some(session_id) = arg(&req, "session_id").filter(|s| !s.is_empty()) else {
        return core::json(400, &core::app_error("please provide session_id", None));
    };
    match load_draft(&session_id, "FeasibilityV3", &[]).await {
        Ok((_, form, user_input)) => ok(json!({ "form": form, "user_input": user_input })),
        Err(e) => e.respond().await,
    }
}

/// `GET /documents/monitoring/v3`
pub async fn monitoring_v3_get(req: Req) -> Response {
    let Some(session_id) = arg(&req, "session_id").filter(|s| !s.is_empty()) else {
        return core::json(400, &core::app_error("please provide session_id", None));
    };
    let result = async {
        let (analyzer, form, user_input) =
            load_draft(&session_id, "MonitoringV3", &["FeasibilityV3"]).await?;
        let plan_seed = plan_seed(analyzer.as_ref()).map_err(Fail::Py)?;
        Ok::<_, Fail>(json!({ "form": form, "user_input": user_input, "plan_seed": plan_seed }))
    }
    .await;
    match result {
        Ok(results) => ok(results),
        Err(e) => e.respond().await,
    }
}

fn ok(results: Value) -> Response {
    core::json(
        200,
        &core::success(json!({ "result": results }), None, None),
    )
}

/// What the route's `except Exception` catches: a database error or a Python runtime error.
enum Fail {
    Db(sqlx::Error),
    Py(String),
}

impl Fail {
    async fn respond(self) -> Response {
        match self {
            Fail::Db(e) => core::db_error(e).await,
            Fail::Py(message) => core::json(500, &core::other_error(&message, None).await),
        }
    }
}

/// The `DataAnalyzer` jsonb columns the draft readers use.
struct Analyzer {
    people_json: Value,
    benefit_json: Value,
    intervention_eligibility_json: Value,
}

/// `load_draft(session_id, cert_type, base_types)`: `(analyzer, form, user_input)`.
async fn load_draft(
    session_id: &str,
    cert_type: &str,
    base_types: &[&str],
) -> Result<(Option<Analyzer>, Value, Value), Fail> {
    let pool = &ctx().await.app;
    // `DataAnalyzer.find_by_session_id`: filter_by(...).first(), no ORDER BY.
    let analyzer = crate::pq::query(
        "select people_json, benefit_json, intervention_eligibility_json from \"DataAnalyzer\" \
         where session_id = $1 limit 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(Fail::Db)?
    .map(|row| Analyzer {
        people_json: row.get::<Option<Value>, _>(0).unwrap_or(Value::Null),
        benefit_json: row.get::<Option<Value>, _>(1).unwrap_or(Value::Null),
        intervention_eligibility_json: row.get::<Option<Value>, _>(2).unwrap_or(Value::Null),
    });
    let mut form = feasibility_prefill(analyzer.as_ref()).map_err(Fail::Py)?;
    let mut user_input = Map::new();
    for template_type in base_types.iter().chain([&cert_type]) {
        // `DocumentData.find_by_session_id_and_type`: filter_by(...).first(), no ORDER BY.
        let row = crate::pq::query(
            "select form, user_input from \"DocumentData\" \
             where session_id = $1 and certification_type = $2 limit 1",
        )
        .bind(session_id)
        .bind(*template_type)
        .fetch_optional(pool)
        .await
        .map_err(Fail::Db)?;
        if let Some(row) = row {
            // merge_form(_stored_dict(row.form), form): stored keys win
            form.extend(stored_dict(row.get(0)));
            user_input.extend(stored_dict(row.get(1)));
        }
    }
    Ok((analyzer, Value::Object(form), Value::Object(user_input)))
}

/// `_stored_dict`: a jsonb value that is not an object reads as empty.
fn stored_dict(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// `feasibility_prefill(analyzer)`.
fn feasibility_prefill(analyzer: Option<&Analyzer>) -> Result<Map<String, Value>, String> {
    let Some(analyzer) = analyzer else {
        return Ok(Map::new());
    };
    let people = or(&analyzer.people_json, json!({}));
    let demography = get_or(&people, "social_demography", json!({}))?;
    let economy = get_or(&people, "economy", json!({}))?;
    let mut prefill = Map::new();
    for (key, value) in [
        ("seHouseholds", get(&demography, "household_number")?),
        ("seIncome", get(&economy, "avg_household_income")?),
    ] {
        // `value not in (None, "", [], {})`
        let empty = match &value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        };
        if !empty {
            prefill.insert(key.into(), value);
        }
    }
    Ok(prefill)
}

/// The read-only `plan_seed` of `documents_monitoring_v3_get`, evaluated in the Python's order so
/// the first error raised is the same one.
fn plan_seed(analyzer: Option<&Analyzer>) -> Result<Value, String> {
    let null = Value::Null;
    let benefit = or(analyzer.map_or(&null, |a| &a.benefit_json), json!({}));
    let general = or(&get(&benefit, "general_benefit")?, json!({}));
    let pathway = analyzer.map_or(&null, |a| &a.intervention_eligibility_json);
    let cards = match pathway {
        Value::Object(_) => or(&get(pathway, "ecosystems")?, json!([])),
        _ => json!([]),
    };
    let selections = or(
        &get(&or(&get(&benefit, "assumptions")?, json!({})), "selections")?,
        json!({}),
    );
    let activities = or(&get(&general, "activities")?, json!([]));
    let mut ecosystems = Vec::new();
    for card in py_iter(&cards)? {
        if !card.is_object() {
            continue;
        }
        let ecosystem = get(&card, "ecosystem")?;
        let key = py_str(&or(&get(&card, "label")?, json!(""))).to_lowercase();
        let label = get(&card, "label")?;
        let mut interventions = Vec::new();
        for i in py_iter(&or(&get(&card, "interventions")?, json!([])))? {
            let intervention = get(&i, "intervention")?;
            let mut acts = Vec::new();
            for a in py_iter(&or(&get(&i, "activities")?, json!([])))? {
                acts.push(json!({
                    "activity_id": get(&a, "activity_id")?,
                    "activity": get(&a, "activity")?,
                    "ecosystem_label": get(&a, "ecosystem_label")?,
                }));
            }
            interventions.push(json!({ "intervention": intervention, "activities": acts }));
        }
        ecosystems.push(json!({
            "ecosystem": ecosystem,
            "key": key,
            "label": label,
            "interventions": interventions,
        }));
    }
    Ok(json!({ "selections": selections, "activities": activities, "ecosystems": ecosystems }))
}

// ---- Python semantics over JSON values ----

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
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `v or default`
fn or(v: &Value, default: Value) -> Value {
    if truthy(v) { v.clone() } else { default }
}

/// `v.get(key)`: only a dict has `.get`.
fn get(v: &Value, key: &str) -> Result<Value, String> {
    get_or(v, key, Value::Null)
}

/// `v.get(key, default)`
fn get_or(v: &Value, key: &str, default: Value) -> Result<Value, String> {
    match v {
        Value::Object(map) => Ok(map.get(key).cloned().unwrap_or(default)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            py_type(other)
        )),
    }
}

/// `for x in v`: a list yields items, a dict its keys, a str its characters.
fn py_iter(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(jsonb_keys(map).into_iter().map(|k| json!(k)).collect()),
        Value::String(s) => Ok(s.chars().map(|c| json!(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", py_type(other))),
    }
}

/// psycopg2 hands back jsonb objects in jsonb's own key order: shorter keys first, then bytes.
fn jsonb_keys(map: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by(|a, b| a.len().cmp(&b.len()).then(a.as_bytes().cmp(b.as_bytes())));
    keys
}

/// `str(v)`
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(v)` for a value json.loads produced.
fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        // a JSON number prints as Python's repr does
        Value::Number(_) => core::to_flask_json(v).trim_end().to_string(),
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            jsonb_keys(map)
                .into_iter()
                .map(|k| format!("{}: {}", repr_str(k), py_repr(&map[k])))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `repr(str)`. shortcut: non-printable non-ASCII (e.g. U+00AD) is kept as is, where
/// Python escapes it; only matters for a non-string pathway label.
fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzer(people: Value, benefit: Value, pathway: Value) -> Analyzer {
        Analyzer {
            people_json: people,
            benefit_json: benefit,
            intervention_eligibility_json: pathway,
        }
    }

    #[test]
    fn prefill_skips_empty_values_and_raises_like_python() {
        assert_eq!(feasibility_prefill(None), Ok(Map::new()));
        let a = analyzer(
            json!({"social_demography": {"household_number": 0}, "economy": {"avg_household_income": ""}}),
            json!(null),
            json!(null),
        );
        assert_eq!(
            Value::Object(feasibility_prefill(Some(&a)).unwrap()),
            json!({"seHouseholds": 0})
        );
        let a = analyzer(json!({"economy": null}), json!(null), json!(null));
        assert_eq!(
            feasibility_prefill(Some(&a)),
            Err("'NoneType' object has no attribute 'get'".into())
        );
    }

    #[test]
    fn plan_seed_shapes_cards() {
        let a = analyzer(
            json!(null),
            json!({"assumptions": {"selections": {"forest": true}}, "general_benefit": {"activities": [1]}}),
            json!({"ecosystems": [
                "skip",
                {"label": "Mangrove", "ecosystem": 2, "interventions": [
                    {"intervention": "Restore", "activities": [{"activity_id": "5", "extra": 1}]}
                ]},
                {"label": 3.0}
            ]}),
        );
        assert_eq!(
            plan_seed(Some(&a)).unwrap(),
            json!({
                "selections": {"forest": true},
                "activities": [1],
                "ecosystems": [
                    {"ecosystem": 2, "key": "mangrove", "label": "Mangrove", "interventions": [
                        {"intervention": "Restore", "activities": [
                            {"activity_id": "5", "activity": null, "ecosystem_label": null}
                        ]}
                    ]},
                    {"ecosystem": null, "key": "3.0", "label": 3.0, "interventions": []}
                ]
            })
        );
        assert_eq!(
            plan_seed(None).unwrap(),
            json!({"selections": {}, "activities": [], "ecosystems": []})
        );
        let legacy = analyzer(json!(null), json!(null), json!([1, 2]));
        assert_eq!(plan_seed(Some(&legacy)).unwrap()["ecosystems"], json!([]));
        let bad = analyzer(json!(null), json!(null), json!({"ecosystems": 7}));
        assert_eq!(
            plan_seed(Some(&bad)),
            Err("'int' object is not iterable".into())
        );
        let bad = analyzer(json!(null), json!([1]), json!(null));
        assert_eq!(
            plan_seed(Some(&bad)),
            Err("'list' object has no attribute 'get'".into())
        );
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(["a'b", 1])), "[\"a'b\", 1]");
        assert_eq!(py_str(&json!({"bb": 1, "a": null})), "{'a': None, 'bb': 1}");
    }
}

#[cfg(test)]
mod monitoring_activities_tests {
    use super::*;

    #[test]
    fn read_csv_types_columns_like_pandas() {
        let frame =
            read_csv("\u{feff}a,b,c,a\r\n1,\"x,\"\"y\"\"\n z\",1.5,true\n\n2,,NA,false\n").unwrap();
        assert_eq!(frame.columns, ["a", "b", "c", "a.1"]);
        assert_eq!(
            frame.rows[0],
            [
                Cell::Int(1),
                Cell::Str("x,\"y\"\n z".into()),
                Cell::Float(1.5),
                Cell::Bool(true)
            ]
        );
        assert_eq!(
            frame.rows[1],
            [Cell::Int(2), Cell::Na, Cell::Na, Cell::Bool(false)]
        );
        assert!(read_csv("a\n\"open").is_err());
    }

    #[test]
    fn cleaners_match_python() {
        assert_eq!(clean_id(&Cell::Float(11.0)), Ok("11".into()));
        assert_eq!(clean_id(&Cell::Na), Ok("".into()));
        assert_eq!(clean_id(&Cell::Str(" 1_2 ".into())), Ok("12".into()));
        assert_eq!(clean_id(&Cell::Str(" 3A ".into())), Ok("3A".into()));
        assert!(clean_id(&Cell::Str("inf".into())).is_err());
        assert_eq!(py_float_str(11.0), "11.0");
        assert_eq!(norm_activity(" Restore  mangroves.\n"), "restore mangroves");
        assert_eq!(capitalize("PROTECT"), "Protect");
        assert_eq!(
            py_int(&Cell::Str("x".into())),
            Err("invalid literal for int() with base 10: 'x'".into())
        );
    }

    #[test]
    fn items_join_by_text_and_sort_stably() {
        let longform = read_csv(
            "pathway,ecosystem,activity,benefit_category,benefit,indicator,unit,frequency,method,reference,definition\n\
             RESTORE,MANGROVE,Plant mangroves.,Nature,b,i1,u,f,m,r,d\n\
             PROTECT,DRYLAND,Guard  forest,People,b,i2,u,f,m,r,d\n\
             RESTORE,MANGROVE,plant mangroves,Nature,b,i3,u,f,m,r,d\n\
             PROTECT,DRYLAND,New one,Nature,b,i4,u,f,m,r,d\n",
        )
        .unwrap();
        let longform = parse_longform_table("lf", longform).ok().unwrap();
        let activity = read_csv(
            "Cat_ID,Ecosystem,Activity ID,Activity,Benefit Nature,Benefit People,Benefit Climate,QB Avoided Emissions,QB Carbon Sequestration\n\
             1,Dryland Forest,11,Guard forest,,,,Yes,No\n\
             2,Mangrove,,Plant mangroves,,,,No,No\n\
             1,Savanna,14,Guard forest,,,,Yes,No\n\
             3,,,ineligible,,,,No,No\n",
        )
        .unwrap();
        let activity = parse_activity_table("act", activity).ok().unwrap();
        let out = activities_result(&longform, &activity);
        let rows: Vec<(Value, Value, Value)> = out["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["activity"].clone(),
                    i["activity_id"].clone(),
                    i["ecosystem_label"].clone(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (json!("Guard forest"), json!("11"), json!("Dryland forest")),
                (json!("Guard forest"), json!("14"), json!("Savanna")),
                (json!("New one"), json!(null), json!(null)),
                (json!("Plant mangroves."), json!(""), json!("Mangrove")),
            ]
        );
        assert_eq!(
            out["activities"][3]["indicators"].as_array().unwrap().len(),
            2
        );
        assert_eq!(out["activities"][3]["pathway"], json!("Restore"));
    }

    /// The recorded body, from the two CSVs fetched into MONACT_DIR (not kept in the repo).
    #[test]
    #[ignore]
    fn matches_recording() {
        let dir = std::env::var("MONACT_DIR").unwrap();
        let read =
            |n: &str| read_csv(&std::fs::read_to_string(format!("{dir}/{n}")).unwrap()).unwrap();
        let longform = parse_longform_table(LONGFORM_TABLE, read(LONGFORM_TABLE))
            .ok()
            .unwrap();
        let activity = parse_activity_table(ACTIVITY_TABLE, read(ACTIVITY_TABLE))
            .ok()
            .unwrap();
        let body = core::to_flask_json(&core::success(
            json!({ "result": activities_result(&longform, &activity) }),
            None,
            None,
        ));
        let expected = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../app-cases/http/monitoring-activities.body"
        ))
        .unwrap();
        assert!(body == expected, "body differs");
    }
}
