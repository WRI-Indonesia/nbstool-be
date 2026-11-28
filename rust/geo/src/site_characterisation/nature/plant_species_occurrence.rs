//! Component 2.7 Plant Species Occurrence (`nature/plant_species_occurrence.py`).
//!
//! GBIF plant points of `sea.plant_occurrence` inside the AOI, in OBJECTID_1 order
//! (`db::load_plants_intersecting`); year filter, attribute cleaning, groupby species (sorted),
//! first CITES appendix in row order, counts, as the pandas body.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Result;
use crate::common::Aoi;
use crate::db::{
    Feature, PLANT_CITES, PLANT_SPECIES, PLANT_YEAR, load_plants_intersecting, py_str,
};
use crate::py::fmt_grouped;

const START_YEAR: i64 = 2000;
const END_YEAR: i64 = 2026;

/// Python `str.strip()`: `str.isspace` characters, which include U+001C..U+001F beyond Rust's
/// White_Space set.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `pd.to_numeric(x, errors="coerce")` of the year column (an integer column; text parsed).
fn year_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `get_cites(values)`: the first of I / II / III in row order, else "Not listed".
fn get_cites(values: &[String]) -> &str {
    values
        .iter()
        .map(String::as_str)
        .find(|x| matches!(*x, "I" | "II" | "III"))
        .unwrap_or("Not listed")
}

/// `analyze_plant_species(aoi)`
pub async fn analyze_plant_species(gis: &PgPool, aoi: &Aoi) -> Result<(Value, Value)> {
    let plants = load_plants_intersecting(gis, aoi).await?;
    Ok(plant_species_from(&plants))
}

/// The body after the load.
pub fn plant_species_from(plants: &[Feature]) -> (Value, Value) {
    // year between START and END (inclusive, NaN false) and species not null
    let kept: Vec<&Feature> = plants
        .iter()
        .filter(|f| {
            year_of(f.get(PLANT_YEAR))
                .is_some_and(|y| y >= START_YEAR as f64 && y <= END_YEAR as f64)
        })
        .filter(|f| !f.get(PLANT_SPECIES).is_null())
        .collect();

    // species: astype(str).str.strip(); CITES: fillna("").astype(str).strip().upper(), then the map
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &kept {
        let species = py_strip(&py_str(f.get(PLANT_SPECIES))).to_string();
        let cites_raw = match f.get(PLANT_CITES) {
            Value::Null => String::new(),
            v => py_str(v),
        };
        let cites = py_strip(&cites_raw).to_uppercase();
        let cites = match cites.as_str() {
            "APPENDIX I" | "1" => "I".to_string(),
            "APPENDIX II" | "2" => "II".to_string(),
            "APPENDIX III" | "3" => "III".to_string(),
            _ => cites,
        };
        groups.entry(species).or_default().push(cites);
    }

    let table: Vec<(String, &str, usize)> = groups
        .iter()
        .map(|(s, c)| (s.clone(), get_cites(c), c.len()))
        .collect();
    let total_species = table.len();
    let count = |a: &str| table.iter().filter(|(_, c, _)| *c == a).count();
    let (appendix_i, appendix_ii, appendix_iii) = (count("I"), count("II"), count("III"));

    let narrative = format!(
        "Based on GBIF occurrence records, the project area recorded occurrences of {} plant species between \
         {START_YEAR} and {END_YEAR}. Among these, {} species are listed under CITES Appendix I, {} under Appendix II, \
         and {} under Appendix III.",
        fmt_grouped(total_species as f64, 0),
        fmt_grouped(appendix_i as f64, 0),
        fmt_grouped(appendix_ii as f64, 0),
        fmt_grouped(appendix_iii as f64, 0),
    );

    let species_rows: Vec<Value> = table
        .iter()
        .map(|(s, c, n)| json!({"scientific_name": s, "cites_appendix": c, "occurrence_count": n}))
        .collect();

    let results = json!({
        "narrative": narrative,
        "tables": {"species": species_rows},
        "values": {
            "total_species": total_species,
            "appendix_i": appendix_i,
            "appendix_ii": appendix_ii,
            "appendix_iii": appendix_iii,
            "record_count": kept.len(),
            "start_year": START_YEAR,
            "end_year": END_YEAR,
        },
        "flags": [],
    });
    let view = json!({
        "plant_number_of_species": total_species,
        "plant_cites_appendix_i": appendix_i,
        "plant_cites_appendix_ii": appendix_ii,
        "plant_cites_appendix_iii": appendix_iii,
        "plant_period": {"from": START_YEAR, "to": END_YEAR},
        "plant_species": species_rows,
    });
    (results, view)
}
