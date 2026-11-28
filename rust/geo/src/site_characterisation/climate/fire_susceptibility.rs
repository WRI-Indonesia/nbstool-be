//! Component 3.5 Fire Susceptibility (`climate/fire_susceptibility.py`).
//!
//! The distribution of fire susceptibility classes over the AOI. Reads `risk_fire_v3.tif`, the
//! layer 1.7 reads, on its 1..4 legend. Denominator is the valid raster area, so the bars sum to
//! 100; every class is shown, including those at zero. The DOMINANT class is the largest by area
//! (not 1.7's representative level). The narrative is fixed text.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{
    Aoi, ClassShare, Resampling, dominant, load_raster_clipped, not_applicable, tabulate_classes,
};
use crate::config::{FIRE_HAZARD_RASTER, FIRE_LEVELS};
use crate::settings::Settings;

const FIRE_NARRATIVE: &str = "This shows how likely the land is to burn under baseline conditions, based on factors \
     such as land cover, dryness, and climate. It is not a forecast of current fire danger.";

/// `analyze_fire_susceptibility`
pub fn analyze_fire_susceptibility(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let raster = load_raster_clipped(
        settings,
        FIRE_HAZARD_RASTER,
        aoi,
        Resampling::Nearest,
        1,
        None,
    )?;

    if raster.valid_area_ha() <= 0.0 {
        let (narrative, flags, missing) =
            not_applicable("No fire susceptibility data is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"fire_susceptibility": []},
                             "values": {}, "flags": flags, "missing": missing});
        return Ok((
            results,
            json!({"fire_susceptibilities": [], "fire_susceptibility_class": null}),
        ));
    }

    // Denominator is the valid hazard area, so the bars sum to 100. Same base as 1.7.
    let assessed_ha = raster.valid_area_ha();
    let rows = tabulate_classes(&raster, &FIRE_LEVELS, assessed_ha);
    drop(raster);
    let dom = dominant(&rows);

    let results = json!({
        "narrative": FIRE_NARRATIVE,
        "tables": {"fire_susceptibility": rows.iter().map(ClassShare::to_json).collect::<Vec<_>>()},
        "values": {
            "chart_series": "fire_susceptibility",
            "chart_unit": "%",
            "chart_axis_label": "Share of project area (%)",
            "dominant_class": dom.map(|d| d.code.to_json()),
            "dominant_label": dom.map(|d| d.label.clone()),
            "assessed_ha": assessed_ha,
        },
        "flags": [],
    });

    let view_results = json!({
        "fire_susceptibilities": rows.iter().map(|row| json!({
            "id": row.code.as_string(), "name": row.label, "area": row.area_ha, "percentage": row.pct,
        })).collect::<Vec<_>>(),
        "fire_susceptibility_class": dom.map(|d| json!({"id": d.code.as_string(), "name": d.label})),
    });
    Ok((results, view_results))
}
