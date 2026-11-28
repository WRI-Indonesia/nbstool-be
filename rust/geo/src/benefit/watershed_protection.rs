//! 5.8 Protection of watershed function (`benefit/watershed_protection.py`).

use serde_json::{Value, json};

use super::config::ECOSYSTEM_NAMES;
use super::util::{NativeGrid, na};
use crate::Result;
use crate::common::Aoi;
use crate::config::{PATHWAY_ECOSYSTEM_BAND, PATHWAY_RASTER};
use crate::settings::Settings;

fn narrative_of(name: &str) -> &'static str {
    match name {
        "forest" => {
            "Safeguarding this forest ecosystem helps maintain the natural capacity of the landscape to capture, \
             store, filter, and slowly release water.Keeping the site under healthy natural cover support more \
             stable river flow,lower erosion and sediment loads, and better water quality downstream."
        }
        "mangrove" => {
            "Safeguarding this mangrove ecosystem helps maintain hydrological connectivity between land and coast, \
             supporting sediment retention, nutrient regulation and water-quality functions while stabilizing the \
             coastal landscape."
        }
        _ => {
            "Safeguarding this peatland ecosystem helps maintain natural water storage and water-table regulation. \
             Protecting peat soils and hydrological connectivity can reduce drainage, erosion and sediment losses \
             while supporting downstream water quality and more resilient watershed function."
        }
    }
}

/// Python `str.title()` for the three ASCII names.
fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// Component 5.8.
pub fn analyze_watershed_protection(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let (grid, data, mask) =
        NativeGrid::masked(settings, PATHWAY_RASTER, aoi, PATHWAY_ECOSYSTEM_BAND)?;
    // eco.filled(0).astype(int); savanna -> forest
    let eco: Vec<i64> = data
        .iter()
        .zip(&mask)
        .map(|(d, m)| if *m { 0 } else { *d as i64 })
        .map(|c| if c == 4 { 1 } else { c })
        .collect();
    let areas: Vec<(&str, f64)> = ECOSYSTEM_NAMES
        .iter()
        .map(|(code, name)| {
            (
                *name,
                grid.area_where(&eco.iter().map(|c| c == code).collect::<Vec<_>>()),
            )
        })
        .collect();
    let present: Vec<&(&str, f64)> = areas.iter().filter(|(_, a)| *a > 0.0).collect();
    if present.is_empty() {
        return Ok(na(
            "No forest, mangrove or peatland ecosystem is mapped on this project area, so there is no watershed \
             function to describe.",
        ));
    }
    let narrative = present
        .iter()
        .map(|(n, _)| narrative_of(n))
        .collect::<Vec<_>>()
        .join("\n\n");
    let values: serde_json::Map<String, Value> = areas
        .iter()
        .map(|(n, a)| (n.to_string(), json!(a)))
        .collect();
    let ecosystems: Vec<Value> = present
        .iter()
        .map(|(n, a)| json!({"name": title(n), "area_ha": a}))
        .collect();
    Ok((
        json!({"narrative": narrative, "tables": {}, "values": values, "flags": []}),
        json!({"applicable": true, "narrative": narrative, "ecosystems": ecosystems}),
    ))
}
