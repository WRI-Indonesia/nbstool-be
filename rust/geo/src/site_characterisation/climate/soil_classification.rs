//! Component 3.6 Soil Classification, WRB 2006 (`climate/soil_classification.py`).
//!
//! The configured categorical path: `soil_groups_v3.tif` (nearest, warped) counted with
//! `np.unique`, named through `soil_class_lookup.csv`, ranked by share of the valid area.
// shortcut: WRB_MODE "probability" is not ported; no probability rasters exist in the bucket.
// Port `_soil_from_probability` when they are published and the mode is switched.

use serde_json::{Value, json};

use crate::Error;
use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped, not_applicable, read_layer_csv};
use crate::config::{
    SOIL_CLASS_RASTER, SOIL_CLASS_TABLE, WRB_DISPLAY_TOP_N, WRB_MIN_PROBABILITY_PCT, WRB_MODE,
};
use crate::generated::{WRB_DESCRIPTION_FALLBACK, WRB_DESCRIPTIONS};
use crate::py::{fsum_py, safe_pct};
use crate::settings::Settings;

/// `_soil_class_rows`: (code, name, hex, description), `fillna("")`.
fn soil_class_rows(settings: &Settings) -> Result<Vec<(i64, String, String, String)>> {
    let (header, rows) = read_layer_csv(settings, SOIL_CLASS_TABLE)?;
    let col = |n: &str| {
        header
            .iter()
            .position(|h| h == n)
            .ok_or_else(|| Error::new("KeyError", format!("'{n}'")))
    };
    let (c, n, h, d) = (col("code")?, col("name")?, col("hex")?, col("description")?);
    rows.iter()
        .map(|r| {
            let get = |i: usize| r.get(i).cloned().unwrap_or_default();
            let code = get(c);
            let code = code.trim().parse::<i64>().map_err(|_| {
                Error::new(
                    "ValueError",
                    format!("invalid literal for int() with base 10: '{code}'"),
                )
            })?;
            Ok((code, get(n), get(h), get(d)))
        })
        .collect()
}

/// `describe_soil`
pub fn describe_soil(group: &str) -> &'static str {
    WRB_DESCRIPTIONS
        .iter()
        .find(|(k, _)| *k == group)
        .map_or(WRB_DESCRIPTION_FALLBACK, |(_, v)| v)
}

fn empty(reason: &str) -> (Value, Value) {
    let (narrative, flags, missing) = not_applicable(reason);
    (
        json!({"narrative": narrative, "tables": {"soil_groups": []}, "values": {}, "flags": flags, "missing": missing}),
        json!({"soil_classifications": []}),
    )
}

/// `_slug`
fn slug(name: &str) -> String {
    format!("soil_{}", name.to_lowercase().replace(' ', "_"))
}

/// `analyze_soil_classification`
pub fn analyze_soil_classification(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    // _soil_from_class_raster
    let raster = load_raster_clipped(
        settings,
        SOIL_CLASS_RASTER,
        aoi,
        Resampling::Nearest,
        1,
        None,
    )?;
    if raster.valid_area_ha() <= 0.0 {
        return Ok(empty(
            "No soil classification data is available for this project area.",
        ));
    }
    let lookup = soil_class_rows(settings)?;
    let mut percent: Vec<(String, f64)> = Vec::new(); // dict, insertion order
    let mut unmapped: Vec<String> = Vec::new();
    for (code, count) in raster.unique_counts() {
        let code = code as i64;
        let name = match lookup.iter().rev().find(|r| r.0 == code) {
            Some(r) => r.1.clone(), // dict comprehension: the last row of a code wins
            None => {
                let n = format!("Unmapped code {code}");
                unmapped.push(n.clone());
                n
            }
        };
        let share = safe_pct(count as f64 * raster.pixel_area_ha, raster.valid_area_ha());
        match percent.iter_mut().find(|(k, _)| *k == name) {
            Some((_, p)) => *p += share,
            None => percent.push((name, 0.0 + share)),
        }
    }
    let n_pixels = raster.valid_count();
    drop(raster);
    if percent.is_empty() {
        return Ok(empty(
            "No soil classification data is available for this project area.",
        ));
    }

    let total_pct = fsum_py(percent.iter().map(|(_, p)| *p));
    let mut flags: Vec<String> = Vec::new();
    if !unmapped.is_empty() {
        flags.push(format!(
            "3.6: {} soil class code(s) in the raster are missing from the lookup table: {}. They are listed unnamed rather than dropped.",
            unmapped.len(),
            unmapped.join(", ")
        ));
    }
    let mut ranked = percent;
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)); // stable, as sorted(reverse=True)
    let groups: Vec<(usize, String, f64, &str)> = ranked
        .into_iter()
        .enumerate()
        .filter(|(_, (_, p))| *p >= WRB_MIN_PROBABILITY_PCT)
        .map(|(i, (n, p))| {
            let d = describe_soil(&n);
            (i + 1, n, p, d)
        })
        .collect();
    let Some(dom) = groups.first() else {
        return Ok(empty(
            "No soil type reaches a meaningful share of this project area.",
        ));
    };
    let narrative = format!(
        "Based on the World Reference Base for Soil Resources (WRB) 2006, the soils in this area are predominantly {}, {}. \
         The distribution of all identified soil types is presented below.",
        dom.1, dom.3
    );
    let results = json!({
        "narrative": narrative,
        "tables": {"soil_groups": groups.iter().map(|(r, n, p, d)| json!({"rank": r, "name": n, "percent": p, "description": d})).collect::<Vec<_>>()},
        "values": {
            "measure": "share_of_area",
            "mode": WRB_MODE,
            "chart_series": "soil_groups",
            "chart_unit": "%",
            "chart_axis_label": "Share of area (%)",
            "display_top_n": WRB_DISPLAY_TOP_N,
            "dominant_group": dom.1,
            "dominant_percent": dom.2,
            "dominant_description": dom.3,
            "listed_group_count": groups.len(),
            "percent_sum": total_pct,
            "n_pixels": n_pixels,
            "reference": "WRB 2006",
        },
        "flags": flags,
    });
    // load_soil_class_presentation: {name: description}, the last row of a name wins
    let description_of = |name: &str| {
        lookup
            .iter()
            .rev()
            .find(|r| r.1 == name)
            .map(|r| r.3.as_str())
    };
    let view_results = json!({
        "soil_classifications": groups.iter().map(|(r, n, p, d)| json!({
            "id": r.to_string(),
            "dict": {"key": slug(n), "fallback": n},
            "description_dict": {
                "key": format!("{}_description", slug(n)),
                "fallback": description_of(n).filter(|s| !s.is_empty()).unwrap_or(d),
            },
            "percentage": p,
        })).collect::<Vec<_>>(),
    });
    Ok((results, view_results))
}
