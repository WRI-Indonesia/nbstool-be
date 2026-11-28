//! Component 1.1 Ecosystem Type (`general/ecosystem_type.py`).
//!
//! Derived from the pathway raster's ecosystem band (band 2), remapped to three Axis 3 classes:
//! dryland forest (1) and savanna (4) both become Dryland, mangrove (2) and peatland (3) keep
//! their own class, and 0 (none) falls into Other. Percentage denominator = total AOI area; an
//! "Other/Unclassified" slice absorbs nodata and non ecosystem pixels so the pie sums to 100.
//! Composition uses pure presence: a class is present if its area is above zero.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, ClassShare, Code, Resampling, load_raster_clipped};
use crate::config::{
    ECOSYSTEM_CLASSES, PATHWAY_ECO_TO_AXIS3, PATHWAY_ECOSYSTEM_BAND, PATHWAY_RASTER,
    ecosystem_color, ecosystem_key,
};
use crate::py::{fsum_py, safe_pct};
use crate::settings::Settings;

const DRYLAND: i64 = 1;
const MANGROVE: i64 = 2;
const PEATLAND: i64 = 3;

/// ECOSYSTEM_NARRATIVE, keyed by the sorted present set.
fn ecosystem_narrative(present: &[i64]) -> &'static str {
    match present {
        [DRYLAND] => {
            "This area sits on mineral soil that is not regularly flooded, unlike peatland or \
             mangrove. Its natural reference ecosystem is dryland forest, dominated by trees \
             growing on well-drained land."
        }
        [MANGROVE] => {
            "This area sits in a coastal zone with salt or brackish water shaped by the tides. Its \
             natural reference ecosystem is mangrove, made up of trees and shrubs adapted to \
             waterlogged, salty ground."
        }
        [PEATLAND] => {
            "This area sits on peat, a soil built up from layers of organic material that stays wet \
             for most of the year. Its natural reference ecosystem is peatland, a waterlogged \
             system shaped by its deep organic soil."
        }
        [MANGROVE, PEATLAND] => {
            "This area is a coastal zone with tidal salt or brackish water that also sits on peat \
             soil. It combines mangrove and peatland features in the same place."
        }
        [DRYLAND, PEATLAND] => {
            "This area includes both mineral-soil ground and peat soil. Part of it follows a \
             dryland reference, and part follows a peatland reference shaped by its wet, organic \
             soil."
        }
        [DRYLAND, MANGROVE] => {
            "This area spans both non-flooded mineral soil inland and a tidal coastal zone. It \
             combines dryland and mangrove references across different parts of the site."
        }
        [DRYLAND, MANGROVE, PEATLAND] => {
            "This area spans all three settings: non-flooded mineral soil inland, a tidal coastal \
             zone with salt or brackish water, and ground built on wet peat soil. It combines \
             dryland, mangrove, and peatland references across different parts of the site."
        }
        // Eighth case, see the Open item in the Python module docstring.
        _ => {
            "This area does not fall within any mapped ecosystem type. It may be open water or \
             land outside the mapped extent. No reference ecosystem can be assigned."
        }
    }
}

/// `analyze_ecosystem_type`: returns `(results, view_results)` as JSON.
pub fn analyze_ecosystem_type(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let band = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_ECOSYSTEM_BAND,
        None,
    )?;
    let mut rows: Vec<ClassShare> = Vec::new();
    for &(code, label) in &ECOSYSTEM_CLASSES {
        let src_codes: Vec<f64> = PATHWAY_ECO_TO_AXIS3
            .iter()
            .filter(|(_, d)| *d == code)
            .map(|(s, _)| *s as f64)
            .collect();
        let count = band.isin_filled(&src_codes).iter().filter(|b| **b).count();
        let area_ha = count as f64 * band.pixel_area_ha;
        rows.push(ClassShare {
            code: Code::Int(code),
            label: label.into(),
            area_ha,
            pct: safe_pct(area_ha, aoi.area_ha),
        });
    }
    drop(band);

    let mapped_ha = fsum_py(rows.iter().map(|r| r.area_ha));
    let other_ha = 0.0f64.max(aoi.area_ha - mapped_ha);
    rows.push(ClassShare {
        code: Code::Str("other"),
        label: "Other/Unclassified".into(),
        area_ha: other_ha,
        pct: safe_pct(other_ha, aoi.area_ha),
    });

    // Pure presence. "Other" is never part of present_set; it does not drive ecosystem logic.
    let mut present_set: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r.code {
            Code::Int(c) if r.area_ha > 0.0 => Some(c),
            _ => None,
        })
        .collect();
    present_set.sort_by_key(|c| c.to_string()); // to_jsonable: sorted(frozenset, key=str)

    let mut missing: Vec<String> = Vec::new();
    if present_set.is_empty() {
        missing.push(
            "UNRESOLVED 1.1: AOI has no mapped ecosystem. Downstream modules that require an \
             Axis 3 reference ecosystem cannot run."
                .into(),
        );
    }
    let mut by_code = present_set.clone();
    by_code.sort();

    let results = json!({
        "narrative": ecosystem_narrative(&by_code),
        "tables": {"ecosystem_composition": rows.iter().map(ClassShare::to_json).collect::<Vec<_>>()},
        "values": {"present_set": present_set, "mapped_ha": mapped_ha, "other_ha": other_ha},
        "flags": Vec::<String>::new(),
        "missing": missing,
    });

    let view_results = json!({
        "ecosystems": rows.iter().map(|r| {
            let id = r.code.as_string();
            json!({
                "id": id,
                "dict": {"key": ecosystem_key(&id), "fallback": r.label},
                "name": r.label,
                "area": r.area_ha,
                "color": ecosystem_color(&id),
                "percentage": r.pct,
            })
        }).collect::<Vec<_>>()
    });

    Ok((results, view_results))
}
