//! Peat depth (`climate/peat_depth.py`): peatland area, depth range and the shallow / medium /
//! deep split, on the raster's own ESRI:54034 grid (`mask(crop=True, nodata=0, filled=True)`).

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, aoi_in_raster_crs, mask_crop, open_layer};
use crate::config::PEAT_DEPTH_RASTER;
use crate::py::fmt_grouped;
use crate::settings::Settings;

const NO_PEAT: &str = "No peat depth data found within the project area.";

fn no_peat() -> (Value, Value) {
    (
        json!({"narrative": NO_PEAT, "tables": {"peat_depth": []}, "values": {}, "flags": [], "missing": [NO_PEAT]}),
        json!({"peat_area_ha": 0.0, "peat_depth_min_cm": null, "peat_depth_max_cm": null, "peat_depths": []}),
    )
}

/// `analyze_peat_depth`
pub fn analyze_peat_depth(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let src = open_layer(settings, PEAT_DEPTH_RASTER)?;
    let polygons = aoi_in_raster_crs(aoi, &src)?;
    let clip = match mask_crop(&src, &polygons, aoi.is_multi, false) {
        Ok(c) => c,
        Err(e) if e.kind == "ValueError" => return Ok(no_peat()), // the polygon does not reach the raster
        Err(e) => return Err(e),
    };
    drop(src);
    let peat = clip.filled(0.0);
    let transform = clip.transform;
    drop(clip);
    let pixel_area_ha = (transform.a * transform.e).abs() / 10000.0;

    let valid: Vec<f64> = peat.into_iter().filter(|v| *v > 0.0).collect();
    if valid.is_empty() {
        return Ok(no_peat());
    }
    let min_depth = valid.iter().copied().fold(f64::INFINITY, f64::min);
    let max_depth = valid.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let area = |pred: &dyn Fn(f64) -> bool| {
        valid.iter().filter(|v| pred(**v)).count() as f64 * pixel_area_ha
    };
    let peatland_ha = valid.len() as f64 * pixel_area_ha;
    let shallow_ha = area(&|v| v < 100.0);
    let medium_ha = area(&|v| (100.0..=300.0).contains(&v));
    let deep_ha = area(&|v| v > 300.0);
    let shallow_pct = shallow_ha / peatland_ha * 100.0;
    let medium_pct = medium_ha / peatland_ha * 100.0;
    let deep_pct = deep_ha / peatland_ha * 100.0;

    let mut narrative = format!(
        "This area contains {} ha of peatland, with peat depth ranging from {min_depth:.2} to {max_depth:.2} meters.",
        fmt_grouped(peatland_ha, 2)
    );
    if shallow_ha > 0.0 {
        narrative += &format!(
            "\n\n{shallow_pct:.2}% of peatland within the project area has shallow peat with a thickness of less than 100 cm. \
             These areas may be suitable for crop cultivation with traditional systems and wet-tolerant crop species to \
             minimize peat degradation and fire risk."
        );
    }
    if medium_ha > 0.0 {
        narrative += &format!(
            "\n\n{medium_pct:.2}% of peatland within the project area has medium peat with a thickness ranging from 100 to \
             300 cm. These areas may be cultivated with sustainable water management and soil conservation to enhance peat \
             ecosystem services."
        );
    }
    if deep_ha > 0.0 {
        narrative += &format!(
            "\n\n{deep_pct:.2}% of peatland within the project area has deep peat deposits exceeding 300 cm. These areas \
             should be conserved to preserve existing carbon stocks, maintain hydrological functions, and sustain ecosystem \
             services."
        );
    }
    let rows = json!([
        {"id": "shallow", "name": "Shallow", "area": shallow_ha, "percentage": shallow_pct},
        {"id": "medium", "name": "Medium", "area": medium_ha, "percentage": medium_pct},
        {"id": "deep", "name": "Deep", "area": deep_ha, "percentage": deep_pct},
    ]);
    let results = json!({
        "narrative": narrative,
        "tables": {"peat_depth": rows},
        "values": {
            "peatland_ha": peatland_ha,
            "min_depth": min_depth,
            "max_depth": max_depth,
            "pixel_area_ha": pixel_area_ha,
            "peat_pixels": valid.len(),
        },
        "flags": [],
    });
    let view_results = json!({
        "peat_area_ha": peatland_ha,
        "peat_depth_min_cm": min_depth,
        "peat_depth_max_cm": max_depth,
        "peat_depths": rows,
    });
    Ok((results, view_results))
}
