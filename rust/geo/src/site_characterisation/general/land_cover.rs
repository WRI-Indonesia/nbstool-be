//! Component 1.8 Land Cover (`general/land_cover.py`).
//!
//! Land cover composition of the project area, from the 2024 20-class RLCMS map. Denominator is
//! the whole site, and a "No data / other" row absorbs code 0 plus anything outside the raster,
//! so the full table sums to 100. Two tables: `land_cover_full` (every class present) and
//! `land_cover_top6` (the six largest real classes, no No-data row).

use serde_json::{Value, json};

use crate::Result;
use crate::common::{
    Aoi, ClassShare, Code, Resampling, load_raster_clipped, not_applicable, sort_by_area,
    tabulate_classes,
};
use crate::config::{LC_TOP_N, LC2024_CLASSES, LC2024_RASTER, land_cover_color, land_cover_key};
use crate::py::{fsum_py, safe_pct};
use crate::settings::Settings;

const LC_NODATA_LABEL: &str = "No data / other";

fn code_of(row: &ClassShare) -> i64 {
    match row.code {
        Code::Int(c) => c,
        Code::Str(_) => -1,
    }
}

/// `_row_view`: one bar of the chart.
fn row_view(row: &ClassShare) -> Value {
    json!({
        "id": row.code.as_string(),
        "dict": {"key": land_cover_key(code_of(row)), "fallback": row.label},
        "name": row.label,
        "area": row.area_ha,
        "percentage": row.pct,
        "color": land_cover_color(code_of(row)),
    })
}

/// `analyze_land_cover`
pub fn analyze_land_cover(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let lc = load_raster_clipped(settings, LC2024_RASTER, aoi, Resampling::Nearest, 1, None)?;
    if lc.valid_area_ha() <= 0.0 {
        let (narrative, flags, missing) =
            not_applicable("No land cover data is available for this project area.");
        let results = json!({"narrative": narrative,
                             "tables": {"land_cover_full": [], "land_cover_top6": []},
                             "values": {}, "flags": flags, "missing": missing});
        return Ok((results, json!({"land_cover_class": []})));
    }

    // Denominator is the whole site; a No-data/other row absorbs code 0 so the full table sums
    // to 100. Snow, Water and Other land are kept as their own classes, not folded into No-data.
    let mut rows = tabulate_classes(&lc, &LC2024_CLASSES, aoi.area_ha);
    drop(lc);
    let mapped_ha = fsum_py(rows.iter().map(|r| r.area_ha));
    let other_ha = 0.0f64.max(aoi.area_ha - mapped_ha);
    rows.push(ClassShare {
        code: Code::Int(0),
        label: LC_NODATA_LABEL.into(),
        area_ha: other_ha,
        pct: safe_pct(other_ha, aoi.area_ha),
    });

    let full = sort_by_area(rows.into_iter().filter(|r| r.area_ha > 0.0).collect()); // sums to 100
    let mut top6 = sort_by_area(full.iter().filter(|r| code_of(r) != 0).cloned().collect());
    top6.truncate(LC_TOP_N);
    let dom = top6.first();

    let results = json!({
        "narrative": "The major land cover categories in the selected area are:",
        "tables": {
            "land_cover_full": full.iter().map(ClassShare::to_json).collect::<Vec<_>>(),
            "land_cover_top6": top6.iter().map(ClassShare::to_json).collect::<Vec<_>>(),
        },
        "values": {
            "chart_series": "land_cover_full",
            "chart_unit": "%",
            "chart_axis_label": "Share of project area (%)",
            "dominant_class": dom.map(|d| d.label.clone()),
            "class_count": full.iter().filter(|r| code_of(r) != 0).count(),
        },
        "flags": [],
    });

    let view_results = json!({"land_cover_class": full.iter().map(row_view).collect::<Vec<_>>()});
    Ok((results, view_results))
}
