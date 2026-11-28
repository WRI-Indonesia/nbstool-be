//! 4.4 Activities by IUCN Conservation Actions (`pathway/conservation_actions.py`): a lookup of
//! 2.3's species table (>90% of the AOI) in the charismatic-species workbook. Stage-fed: a
//! dependent of `habitat area` in the union stream; reads no raster.
//!
//! The pandas steps are reproduced on plain rows: inner merge in left order, `drop_duplicates`
//! keeping the first, `sort_values` (stable lexsort; the mixed `code` column orders numbers before
//! text, as pandas' `safe_sort` does), `groupby` with sorted keys and dropped NaN keys.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use calamine::{Data, Reader, Xlsx};
use serde_json::{Value, json};

use crate::common::Aoi;
use crate::settings::{Settings, gdal_path};
use crate::{Error, Result};

/// `config.CONSERVATION_FILE`
pub const CONSERVATION_FILE: &str = "habitat_area_v3/Charismatic_conservation_v3.xlsx";
/// `config.OVERLAP_THRESHOLD` (an int in the Python config, and on the wire)
const OVERLAP_THRESHOLD: i64 = 90;

/// One workbook cell as `pd.read_excel` (openpyxl) yields it.
#[derive(Clone, Debug, PartialEq)]
enum Cell {
    Int(i64),
    Num(f64),
    Text(String),
    Bool(bool),
    Na,
}

impl Cell {
    fn from_data(d: &Data) -> Cell {
        match d {
            Data::Int(i) => Cell::Int(*i),
            // shortcut: openpyxl reads a numeric cell written without '.'/'E' as int; calamine gives
            // a float, taken as int when integral. Exact for every value Excel writes itself.
            Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => Cell::Int(*f as i64),
            Data::Float(f) => Cell::Num(*f),
            Data::String(s) => Cell::Text(s.clone()),
            Data::Bool(b) => Cell::Bool(*b),
            Data::DateTimeIso(s) | Data::DurationIso(s) => Cell::Text(s.clone()),
            Data::DateTime(d) => Cell::Num(d.as_f64()),
            Data::Error(_) | Data::Empty => Cell::Na,
        }
    }

    /// Python `str(v)`
    fn py_str(&self) -> String {
        match self {
            Cell::Int(i) => i.to_string(),
            Cell::Num(f) => crate::benefit::util::py_float_repr(*f),
            Cell::Text(s) => s.clone(),
            Cell::Bool(b) => {
                if *b {
                    "True".into()
                } else {
                    "False".into()
                }
            }
            Cell::Na => "nan".into(),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Cell::Int(i) => json!(i),
            Cell::Num(f) => json!(f),
            Cell::Text(s) => json!(s),
            Cell::Bool(b) => json!(b),
            Cell::Na => Value::Null,
        }
    }

    fn num(&self) -> Option<f64> {
        match self {
            Cell::Int(i) => Some(*i as f64),
            Cell::Num(f) => Some(*f),
            Cell::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// pandas' object equality as hashing sees it (1 == 1.0, NaN == NaN for duplicates)
    fn same(&self, o: &Cell) -> bool {
        match (self.num(), o.num()) {
            (Some(a), Some(b)) => a == b,
            _ => self == o,
        }
    }
}

/// The order `Categorical(values, ordered=True)` / a sorted groupby gives a mixed object column:
/// numbers ascending, then text ascending, NaN last.
fn code_cmp(a: &Cell, b: &Cell) -> Ordering {
    let rank = |c: &Cell| match c {
        Cell::Na => 2,
        Cell::Text(_) => 1,
        _ => 0,
    };
    match (a, b) {
        (Cell::Text(x), Cell::Text(y)) => x.cmp(y),
        _ if rank(a) != rank(b) => rank(a).cmp(&rank(b)),
        _ => a.num().unwrap_or(0.0).total_cmp(&b.num().unwrap_or(0.0)),
    }
}

/// The workbook: header and rows (first sheet, header row 0).
struct Workbook {
    header: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

impl Workbook {
    fn col(&self, name: &str) -> Result<usize> {
        self.header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| Error::new("KeyError", format!("'{name}'")))
    }
}

/// `_read_conservation(path)`: once per process and per resolved path (`lru_cache(maxsize=4)`).
fn read_conservation(settings: &Settings) -> Result<Arc<Workbook>> {
    static CACHE: Mutex<Vec<(String, Arc<Workbook>)>> = Mutex::new(Vec::new());
    let path = settings.layer_path(CONSERVATION_FILE)?;
    if let Some((_, w)) = CACHE
        .lock()
        .ok()
        .and_then(|g| g.iter().find(|(p, _)| *p == path).cloned())
    {
        return Ok(w);
    }
    let bytes = crate::ffi::read_vsi_file(&gdal_path(&path))?;
    let mut xlsx: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::new("BadZipFile", e.to_string()))?;
    let range = xlsx
        .worksheet_range_at(0)
        .ok_or_else(|| Error::new("IndexError", "list index out of range"))?
        .map_err(|e| Error::new("ValueError", e.to_string()))?;
    let mut rows = range.rows();
    let header: Vec<String> = rows
        .next()
        .map(|r| r.iter().map(|c| Cell::from_data(c).py_str()).collect())
        .unwrap_or_default();
    let rows: Vec<Vec<Cell>> = rows
        .map(|r| r.iter().map(Cell::from_data).collect())
        .collect();
    let wb = Arc::new(Workbook { header, rows });
    if let Ok(mut g) = CACHE.lock() {
        if g.len() >= 4 {
            g.remove(0);
        }
        g.push((path, wb.clone()));
    }
    Ok(wb)
}

/// `species_key(value)`
fn species_key(s: &str) -> String {
    s.trim().to_lowercase().replace('_', " ")
}

/// Python `str()` of a JSON scalar from 2.3's table.
fn value_str(v: &Value) -> String {
    crate::benefit::util::py_str(v)
}

struct Matched {
    species: Value,
    class: Value,
    iucn_status: Value,
    percentage_aoi: Value,
    code: Cell,
    action: Cell,
    activities: Cell,
}

fn text_cmp(a: &Value, b: &Value) -> Ordering {
    match (a.as_str(), b.as_str()) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// `analyze_conservation_actions(aoi, habitat)`: `habitat` is 2.3's `(results, view_results)`.
pub fn analyze_conservation_actions(
    settings: &Settings,
    _aoi: &Aoi,
    habitat: &(Value, Value),
) -> Result<(Value, Value)> {
    let results = &habitat.0;
    let species_rows = results
        .get("tables")
        .and_then(|t| t.get("species"))
        .filter(|v| !v.is_null());
    let values_ok = results.get("values").is_some_and(crate::py::truthy);
    let retryable = results.get("retryable").is_some_and(crate::py::truthy);
    let Some(species_rows) = species_rows.filter(|_| values_ok && !retryable) else {
        return Ok((
            json!({"narrative": "", "tables": {}, "values": {}, "flags": [],
                   "missing": ["4.4: the habitat area analysis did not complete, so no conservation actions can be listed."],
                   "retryable": true}),
            json!({}),
        ));
    };
    let rows: Vec<&Value> = species_rows
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let field = |r: &Value, k: &str| r.get(k).cloned().unwrap_or(Value::Null);

    let wb = read_conservation(settings)?;
    let (c_sci, c_code, c_action, c_activities) = (
        wb.col("Scientific name")?,
        wb.col("code")?,
        wb.col("Conservation Action")?,
        wb.col("Conservation Activities")?,
    );
    let cell = |r: &Vec<Cell>, i: usize| r.get(i).cloned().unwrap_or(Cell::Na);
    let right_keys: Vec<String> = wb
        .rows
        .iter()
        .map(|r| species_key(&cell(r, c_sci).py_str()))
        .collect();

    // over_90 = result[result.percentage_aoi > OVERLAP_THRESHOLD]
    let over_90: Vec<&Value> = rows
        .iter()
        .copied()
        .filter(|r| {
            r.get("percentage_aoi")
                .and_then(Value::as_f64)
                .is_some_and(|p| p > OVERLAP_THRESHOLD as f64)
        })
        .collect();

    // inner merge on _key, left order, right matches in workbook order; then drop_duplicates
    let mut matched: Vec<Matched> = Vec::new();
    for left in &over_90 {
        let key = species_key(&value_str(&field(left, "species")));
        for (r, rk) in wb.rows.iter().zip(&right_keys) {
            if *rk != key {
                continue;
            }
            let m = Matched {
                species: field(left, "species"),
                class: field(left, "class"),
                iucn_status: field(left, "iucn_status"),
                percentage_aoi: field(left, "percentage_aoi"),
                code: cell(r, c_code),
                action: cell(r, c_action),
                activities: cell(r, c_activities),
            };
            let dup = matched.iter().any(|x| {
                x.species == m.species && x.code.same(&m.code) && x.action.same(&m.action)
            });
            if !dup {
                matched.push(m);
            }
        }
    }
    // sort_values(["species", "code"]): stable
    matched
        .sort_by(|a, b| text_cmp(&a.species, &b.species).then_with(|| code_cmp(&a.code, &b.code)));

    // groupby(["code", "Conservation Action"]): sorted keys, NaN keys dropped
    let mut groups: Vec<(Cell, String, Vec<String>)> = Vec::new();
    for m in &matched {
        let Cell::Text(action) = &m.action else {
            continue;
        };
        if m.code == Cell::Na {
            continue;
        }
        let sp = value_str(&m.species);
        match groups
            .iter_mut()
            .find(|(c, a, _)| c.same(&m.code) && a == action)
        {
            Some(g) => g.2.push(sp),
            None => groups.push((m.code.clone(), action.clone(), vec![sp])),
        }
    }
    groups.sort_by(|a, b| code_cmp(&a.0, &b.0).then_with(|| a.1.cmp(&b.1)));
    let mut summary: Vec<(Cell, String, usize, String)> = groups
        .into_iter()
        .map(|(code, action, sp)| {
            let set: BTreeSet<String> = sp.into_iter().collect();
            let n = set.len();
            (
                code,
                action,
                n,
                set.into_iter().collect::<Vec<_>>().join(", "),
            )
        })
        .collect();
    // sort_values(["species_count", "code"], ascending=[False, True]): stable
    summary.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| code_cmp(&a.0, &b.0)));

    let distinct = |f: &dyn Fn(&Matched) -> Option<String>| {
        matched.iter().filter_map(f).collect::<BTreeSet<_>>().len()
    };
    let charismatic = distinct(&|m| m.species.as_str().map(str::to_string));
    let action_count = distinct(&|m| match &m.action {
        Cell::Na => None,
        c => Some(format!("{c:?}")),
    });

    let actions: Vec<Value> = summary
        .iter()
        .map(|(code, action, n, species)| {
            json!({"code": code.py_str(), "conservation_action": action, "species_count": n,
                   "species": species.split(", ").map(|s| s.replace('_', " ")).collect::<Vec<_>>()})
        })
        .collect();

    // groupby(["species", "Class", "iucn_status", "percentage_aoi"]): sorted, NaN keys dropped
    let mut by_species: Vec<(&Matched, Vec<String>)> = Vec::new();
    for m in &matched {
        if m.species.is_null()
            || m.class.is_null()
            || m.iucn_status.is_null()
            || m.percentage_aoi.as_f64().is_none()
        {
            continue;
        }
        let action = m.action.py_str();
        match by_species.iter_mut().find(|(k, _)| {
            k.species == m.species
                && k.class == m.class
                && k.iucn_status == m.iucn_status
                && k.percentage_aoi.as_f64() == m.percentage_aoi.as_f64()
        }) {
            Some((_, acts)) => {
                if !acts.contains(&action) {
                    acts.push(action);
                }
            }
            None => by_species.push((m, vec![action])),
        }
    }
    by_species.sort_by(|(a, _), (b, _)| {
        text_cmp(&a.species, &b.species)
            .then_with(|| text_cmp(&a.class, &b.class))
            .then_with(|| text_cmp(&a.iucn_status, &b.iucn_status))
            .then_with(|| {
                a.percentage_aoi
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&b.percentage_aoi.as_f64().unwrap_or(0.0))
            })
    });
    let species: Vec<Value> = by_species
        .iter()
        .map(|(m, acts)| {
            json!({"scientific_name": value_str(&m.species).replace('_', " "), "taxon_class": m.class,
                   "iucn_status": m.iucn_status, "percentage_aoi": m.percentage_aoi.as_f64(), "conservation_actions": acts})
        })
        .collect();

    let table: Vec<Value> = matched
        .iter()
        .map(|m| {
            json!({"species": m.species, "Class": m.class, "iucn_status": m.iucn_status, "percentage_aoi": m.percentage_aoi,
                   "code": m.code.to_json(), "Conservation Action": m.action.to_json(),
                   "Conservation Activities": m.activities.to_json()})
        })
        .collect();
    let results = json!({
        "narrative": "",
        "tables": {"species_actions": table},
        "values": {"charismatic_species_count": charismatic, "conservation_action_count": action_count,
                   "over_90_species_count": over_90.len(), "overlap_threshold": OVERLAP_THRESHOLD},
        "flags": [],
    });
    let view = json!({"conservation_actions": {
        "overlap_threshold": OVERLAP_THRESHOLD,
        "charismatic_species_count": charismatic,
        "conservation_action_count": action_count,
        "actions": actions,
        "species": species,
    }});
    Ok((results, view))
}
