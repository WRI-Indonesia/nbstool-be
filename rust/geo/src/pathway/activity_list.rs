//! 4.2 Activity List (`pathway/activity_list.py`): the activities that apply to the AOI.
//!
//! Bands 3 (`cat_code`) and 2 (`ecosystem`) are read on ONE grid (`like=`) and cross-tabulated
//! into the unique `(cat_code, ecosystem)` pairs present, with area; each pair is joined to the
//! canonical_v3 catalog (`load_activity_table`, `common.py`). Dominant categories first: the order
//! of `values.by_category` is what F02-P5 (5.1's lists, 5.2's area sum) reads it in.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use crate::common::{Aoi, Resampling, load_raster_clipped, read_layer_csv};
use crate::config::{
    PATHWAY_CATCODE_BAND, PATHWAY_CATCODE_LABELS, PATHWAY_CODES, PATHWAY_ECOSYSTEM_BAND,
    PATHWAY_ECOSYSTEM_CODES, PATHWAY_RASTER,
};
use crate::py::{fmt_ha, np_argsort, round_ndigits};
use crate::settings::Settings;
use crate::{Error, Result};

const COMPONENT: &str = "4.2 Activity List";

/// `config.ACTIVITY_TABLE`
pub const ACTIVITY_TABLE: &str = "canonical_v3_activities.csv";

/// `config.PATHWAY_CATCODE_TO_PATHWAY`
pub const PATHWAY_CATCODE_TO_PATHWAY: [(i64, i64); 17] = [
    (1, 1),
    (2, 4),
    (3, 4),
    (4, 3),
    (5, 4),
    (6, 3),
    (7, 3),
    (8, 2),
    (9, 4),
    (10, 4),
    (11, 2),
    (12, 3),
    (13, 2),
    (14, 3),
    (15, 2),
    (16, 4),
    (17, 3),
];

fn lookup<V: Copy>(table: &[(i64, V)], k: i64) -> Option<V> {
    table.iter().find(|(c, _)| *c == k).map(|(_, v)| *v)
}

/// `(cat_code, ecosystem) -> activity rows` in catalog order.
pub type ActivityTable = HashMap<(i64, i64), Vec<Value>>;

/// pandas' default `na_values` (a field equal to one of these reads as NaN).
fn is_na(s: &str) -> bool {
    matches!(
        s,
        "" | "#N/A"
            | "#N/A N/A"
            | "#NA"
            | "-1.#IND"
            | "-1.#QNAN"
            | "-NaN"
            | "-nan"
            | "1.#IND"
            | "1.#QNAN"
            | "<NA>"
            | "N/A"
            | "NA"
            | "NULL"
            | "NaN"
            | "None"
            | "n/a"
            | "nan"
            | "null"
    )
}

/// Python `str.isspace` per character (Rust's White_Space plus the \x1c-\x1f separators).
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `" ".join(str(v).split())`, "" for NaN.
fn clean_text(s: &str) -> String {
    if is_na(s) {
        String::new()
    } else {
        s.split(py_space)
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `clean_id`: "" for NaN, `str(int(float(v)))` when it parses, else the stripped text.
fn clean_id(s: &str) -> String {
    if is_na(s) {
        return String::new();
    }
    match s.trim_matches(py_space).parse::<f64>() {
        Ok(f) if f.is_finite() => format!("{}", f.trunc() as i128),
        _ => s.trim_matches(py_space).to_string(),
    }
}

/// `as_bool`. shortcut: reads the field text; a float-typed column ("1.0") would read False in
/// pandas and True here, which no catalog column is.
fn as_bool(s: &str) -> bool {
    let t = if is_na(s) {
        "nan".to_string()
    } else {
        s.trim_matches(py_space).to_lowercase()
    };
    matches!(t.as_str(), "yes" | "y" | "true" | "1")
}

/// `_ACTIVITY_COL_ALIASES` after `c.strip().lower()`.
fn alias(c: &str) -> String {
    let c = c.trim_matches(py_space).to_lowercase();
    match c.as_str() {
        "cat_id" | "cat_code" => "cat_code",
        "activity id" | "activity_id" => "activity_id",
        "benefit nature" | "benefit_nature" => "benefit_nature",
        "benefit people" | "benefit_people" => "benefit_people",
        "benefit climate" | "benefit_climate" => "benefit_climate",
        "qb avoided emissions" | "qb_avoided" => "qb_avoided",
        "qb carbon sequestration" | "qb_sequestration" => "qb_sequestration",
        _ => return c,
    }
    .to_string()
}

fn parse_table(layer: &str, header: &[String], rows: &[Vec<String>]) -> Result<ActivityTable> {
    let cols: Vec<String> = header
        .iter()
        .enumerate()
        .map(|(i, c)| {
            alias(if i == 0 {
                c.trim_start_matches('\u{feff}')
            } else {
                c
            })
        })
        .collect();
    let required = [
        "activity",
        "activity_id",
        "benefit_climate",
        "benefit_nature",
        "benefit_people",
        "cat_code",
        "ecosystem",
        "qb_avoided",
        "qb_sequestration",
    ];
    let missing: Vec<String> = required
        .iter()
        .filter(|r| !cols.iter().any(|c| c == *r))
        .map(|r| format!("'{r}'"))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            "ValueError",
            format!("{layer} is missing columns [{}].", missing.join(", ")),
        ));
    }
    // pandas renames a repeated header `name.1`, so `df[name]` is the first occurrence
    let at = |name: &str| cols.iter().position(|c| c == name).unwrap_or(0);
    let field = |r: &Vec<String>, name: &str| r.get(at(name)).cloned().unwrap_or_default();
    let mut table: ActivityTable = HashMap::new();
    for r in rows {
        let eco = field(r, "ecosystem");
        if is_na(&eco) || eco.trim_matches(py_space).is_empty() {
            continue; // ineligible category, no (cat_code, ecosystem) join key
        }
        let s = eco.trim_matches(py_space);
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
            return Err(Error::new(
                "ValueError",
                format!("Unknown ecosystem '{eco}' in {layer}."),
            ));
        };
        let cc_text = field(r, "cat_code");
        let cc = cc_text
            .trim_matches(py_space)
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(|f| f.trunc() as i64)
            .ok_or_else(|| Error::new("ValueError", "cannot convert float NaN to integer"))?;
        table.entry((cc, ec)).or_default().push(json!({
            "activity_id": clean_id(&field(r, "activity_id")),
            "activity": clean_text(&field(r, "activity")),
            "benefit_nature": clean_text(&field(r, "benefit_nature")),
            "benefit_people": clean_text(&field(r, "benefit_people")),
            "benefit_climate": clean_text(&field(r, "benefit_climate")),
            "qb_avoided": as_bool(&field(r, "qb_avoided")),
            "qb_sequestration": as_bool(&field(r, "qb_sequestration")),
        }));
    }
    Ok(table)
}

/// `load_activity_table(layer)`: read once per process (`lru_cache(maxsize=1)`), keyed here by
/// the resolved path.
pub fn load_activity_table(settings: &Settings, layer: &str) -> Result<Arc<ActivityTable>> {
    static CACHE: Mutex<Option<(String, Arc<ActivityTable>)>> = Mutex::new(None);
    let path = settings.layer_path(layer)?;
    if let Ok(g) = CACHE.lock()
        && let Some((_, t)) = g.as_ref().filter(|(p, _)| *p == path)
    {
        return Ok(t.clone());
    }
    let (header, rows) = read_layer_csv(settings, layer)?;
    let table = Arc::new(parse_table(layer, &header, &rows)?);
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((path, table.clone()));
    }
    Ok(table)
}

/// `ComponentResult` as `to_jsonable` writes it.
fn component_result(
    applicable: bool,
    narrative: &str,
    tables: Value,
    values: Value,
    flags: Vec<String>,
    missing: Vec<String>,
) -> Value {
    json!({"component": COMPONENT, "applicable": applicable, "narrative": narrative, "tables": tables,
           "values": values, "flags": flags, "missing": missing})
}

/// `analyze_activity_list`: the `ComponentResult` as JSON (`values.by_category` in its order).
pub fn analyze_activity_list(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let catcode = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_CATCODE_BAND,
        None,
    )?;
    let ecosystem = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_ECOSYSTEM_BAND,
        Some(catcode.grid()),
    )?;

    // np.unique(pairs, axis=0, return_counts=True): lexicographic (cat, eco)
    let mut uniq: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for i in 0..catcode.values.len() {
        if !catcode.mask[i] && !ecosystem.mask[i] {
            *uniq
                .entry((catcode.values[i] as i64, ecosystem.values[i] as i64))
                .or_default() += 1;
        }
    }
    let px_area = catcode.pixel_area_ha;
    drop((catcode, ecosystem));
    if uniq.is_empty() {
        let reason = "The pathway layer does not cover this project area, so no activities apply.";
        return Ok(component_result(
            false,
            reason,
            json!({}),
            json!({}),
            vec![],
            vec![reason.into()],
        ));
    }
    let uniq: Vec<((i64, i64), usize)> = uniq.into_iter().collect();

    let table = load_activity_table(settings, ACTIVITY_TABLE)?;

    // np.argsort(counts)[::-1]: dominant categories first
    let counts: Vec<f64> = uniq.iter().map(|(_, n)| *n as f64).collect();
    let mut order = np_argsort(&counts);
    order.reverse();

    let mut rows: Vec<Value> = Vec::new();
    let mut by_category = Map::new();
    let mut flags: Vec<String> = Vec::new();
    for idx in order {
        let ((cc, ec), n) = uniq[idx];
        if cc == 0 {
            continue; // mask
        }
        let area_ha = n as f64 * px_area;
        let pathway_code = lookup(&PATHWAY_CATCODE_TO_PATHWAY, cc);
        let pathway = pathway_code
            .and_then(|p| lookup(&PATHWAY_CODES, p))
            .unwrap_or("Unknown");
        let cat_label =
            lookup(&PATHWAY_CATCODE_LABELS, cc).map_or_else(|| format!("cat {cc}"), str::to_string);
        let eco_label = lookup(&PATHWAY_ECOSYSTEM_CODES, ec)
            .map_or_else(|| format!("ecosystem {ec}"), str::to_string);

        let acts: Vec<Value> = if ec == 0 {
            flags.push(format!(
                "4.2: {cat_label} appears with ecosystem 0 (no reference) on {}; the pathway script should have masked these pixels.",
                fmt_ha(area_ha)
            ));
            vec![]
        } else if pathway_code == Some(4) {
            vec![] // Ineligible: no activity by design
        } else {
            let acts = table.get(&(cc, ec)).cloned().unwrap_or_default();
            if acts.is_empty() {
                flags.push(format!(
                    "4.2: no catalog row for ({cat_label}, {eco_label}); {} left without an activity.",
                    fmt_ha(area_ha)
                ));
            }
            acts
        };

        let rounded = round_ndigits(area_ha, 1);
        if acts.is_empty() {
            let note = if pathway_code == Some(4) {
                "(ineligible, no activity)"
            } else {
                "(no catalog match)"
            };
            rows.push(json!({"pathway": pathway, "category": cat_label, "ecosystem": eco_label, "area_ha": rounded,
                             "activity_id": "", "activity": note}));
        } else {
            for a in &acts {
                rows.push(json!({"pathway": pathway, "category": cat_label, "ecosystem": eco_label, "area_ha": rounded,
                                 "activity_id": a["activity_id"], "activity": a["activity"]}));
            }
        }
        by_category.insert(
            format!("{cat_label} | {eco_label}"),
            json!({"pathway": pathway, "area_ha": area_ha, "activities": acts}),
        );
    }

    let activity_count = rows
        .iter()
        .filter(|r| r["activity_id"].as_str().is_some_and(|s| !s.is_empty()))
        .count();
    let category_count = by_category.len();
    let values = json!({"by_category": by_category, "category_count": category_count, "activity_count": activity_count});
    Ok(component_result(
        true,
        "",
        json!({"activities": rows}),
        values,
        flags,
        vec![],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_fields_like_pandas() {
        let header: Vec<String> = [
            "Cat_ID",
            "Ecosystem",
            "Activity ID",
            "Activity",
            "Benefit Nature",
            "Benefit People",
            "Benefit Climate",
            "QB Avoided Emissions",
            "QB Carbon Sequestration",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let row = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let rows = vec![
            row(&[
                "1",
                "Dryland Forest",
                "11",
                "A  b\n c",
                "x",
                "y",
                "z\n",
                "Yes",
                "No",
            ]),
            row(&["2", "", "", "skip", "", "", "", "", ""]),
            row(&["4", "Savanna", "", "B", "NA", "", "", "yes", "1"]),
        ];
        let t = parse_table("t.csv", &header, &rows).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[&(1, 1)][0],
            json!({"activity_id": "11", "activity": "A b c", "benefit_nature": "x", "benefit_people": "y",
            "benefit_climate": "z", "qb_avoided": true, "qb_sequestration": false})
        );
        assert_eq!(t[&(4, 4)][0]["activity_id"], json!(""));
        assert_eq!(t[&(4, 4)][0]["benefit_nature"], json!(""));
        assert_eq!(t[&(4, 4)][0]["qb_sequestration"], json!(true));
    }
}
