//! 3.1 All Ecosystem (Overview) (`threat/all_ecosystems.py`): total ecosystem and disturbed area,
//! per class.
//!
//! Four classes: 0 Other, 1 Dryland forest (savanna 4 folds into it), 2 Mangrove, 3 Peatland.
//! `forest_disturbance_v3.tif` > 0 counts as disturbed. Both rasters are read on their OWN grid
//! (EPSG:4326, no warp) through `raster_geometry_mask(crop=True)` + a masked window read, must
//! share the clipped grid (raises otherwise), and AREA IS GEODESIC, row by row on WGS84.

use serde_json::{Map, Value, json};

use crate::affine::Affine;
use crate::common::{
    Aoi, Ring, aoi_in_raster_crs, calculate_mask_area_ha, open_layer, pixel_area_by_row,
    raster_geometry_mask_crop, read_window_masked,
};
use crate::config::{THREAT_DISTURBANCE, THREAT_ECOSYSTEM, THREAT_ECOSYSTEM_CLASSES};
use crate::py::round_ndigits;
use crate::settings::Settings;
use crate::{Error, Result};

/// One clipped raster: data, validity (inside the AOI and not masked), grid.
struct Masked {
    data: Vec<f64>,
    valid: Vec<bool>,
    transform: Affine,
    width: usize,
    height: usize,
}

/// `read_masked_raster`: only the AOI window, valid = inside & not nodata.
fn read_masked_raster(settings: &Settings, layer: &str, aoi: &Aoi) -> Result<Masked> {
    let src = open_layer(settings, layer)?;
    let raster_aoi: Vec<Vec<Ring>> = aoi_in_raster_crs(aoi, &src)?;
    let (outside, transform, window) =
        raster_geometry_mask_crop(&src, &raster_aoi, aoi.is_multi, false)?;
    let (data, masked) = read_window_masked(&src, 1, &window)?;
    let valid = outside.iter().zip(&masked).map(|(o, m)| !o && !m).collect();
    Ok(Masked {
        data,
        valid,
        transform,
        width: window.width as usize,
        height: window.height as usize,
    })
}

/// `np.allclose` of two affine transforms (all nine coefficients).
fn allclose(a: &Affine, b: &Affine) -> bool {
    let (x, y) = (
        [a.a, a.b, a.c, a.d, a.e, a.f, 0.0, 0.0, 1.0],
        [b.a, b.b, b.c, b.d, b.e, b.f, 0.0, 0.0, 1.0],
    );
    x.iter()
        .zip(&y)
        .all(|(p, q)| (p - q).abs() <= 1e-8 + 1e-5 * q.abs())
}

/// `x / total * 100 if total > 0 else 0`, then `round(_, 2)`: the int 0 stays an int.
fn pct_or_int_zero(part: f64, total: f64) -> Value {
    if total > 0.0 {
        json!(round_ndigits(part / total * 100.0, 2))
    } else {
        json!(0)
    }
}

/// `analyze_all_ecosystem`
pub fn analyze_all_ecosystem(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    // 2. Read Ecosystem Raster; 4 Savanna -> 1 Dryland forest
    let mut eco = read_masked_raster(settings, THREAT_ECOSYSTEM, aoi)?;
    for v in eco.data.iter_mut() {
        if *v == 4.0 {
            *v = 1.0;
        }
    }
    // 3. Read Disturbance Raster
    let dist = read_masked_raster(settings, THREAT_DISTURBANCE, aoi)?;

    // Both rasters must share the same clipped grid for direct boolean masking.
    if (eco.height, eco.width) != (dist.height, dist.width) {
        return Err(Error::new(
            "ValueError",
            "Ecosystem and disturbance raster grids do not match after clipping.",
        ));
    }
    if !allclose(&eco.transform, &dist.transform) {
        return Err(Error::new(
            "ValueError",
            "Ecosystem and disturbance raster transforms do not match.",
        ));
    }
    let row_areas = pixel_area_by_row(&eco.transform, eco.height);
    let area = |m: &[bool]| calculate_mask_area_ha(m, eco.width, &row_areas);

    // 4. Total Ecosystem Mask
    let codes: Vec<f64> = THREAT_ECOSYSTEM_CLASSES
        .iter()
        .map(|(c, _)| *c as f64)
        .collect();
    let total_ecosystem_mask: Vec<bool> = eco
        .valid
        .iter()
        .zip(&eco.data)
        .map(|(v, d)| *v && codes.contains(d))
        .collect();
    let total_ecosystem_area_ha = area(&total_ecosystem_mask);

    // 5. Total Disturbed Area
    let disturbance_mask: Vec<bool> = dist
        .valid
        .iter()
        .zip(&dist.data)
        .map(|(v, d)| *v && *d > 0.0)
        .collect();
    drop(dist);
    let total_disturbed_mask: Vec<bool> = total_ecosystem_mask
        .iter()
        .zip(&disturbance_mask)
        .map(|(a, b)| *a && *b)
        .collect();
    let total_disturbed_area_ha = area(&total_disturbed_mask);

    // 6. Ecosystem Breakdown
    let mut ecosystem_results = Map::new();
    for &(class_value, class_name) in &THREAT_ECOSYSTEM_CLASSES {
        let ecosystem_mask: Vec<bool> = eco
            .valid
            .iter()
            .zip(&eco.data)
            .map(|(v, d)| *v && *d == class_value as f64)
            .collect();
        let ecosystem_area_ha = area(&ecosystem_mask);
        let disturbed: Vec<bool> = ecosystem_mask
            .iter()
            .zip(&disturbance_mask)
            .map(|(a, b)| *a && *b)
            .collect();
        let ecosystem_disturbed_area_ha = area(&disturbed);
        ecosystem_results.insert(
            class_name.to_string(),
            json!({
                "area_ha": round_ndigits(ecosystem_area_ha, 2),
                "percentage_total": pct_or_int_zero(ecosystem_area_ha, total_ecosystem_area_ha),
                "disturbed_area_ha": round_ndigits(ecosystem_disturbed_area_ha, 2),
                "disturbed_percentage": pct_or_int_zero(ecosystem_disturbed_area_ha, ecosystem_area_ha),
            }),
        );
    }

    Ok(json!({
        "total_ecosystem_area_ha": round_ndigits(total_ecosystem_area_ha, 2),
        "total_disturbed_area_ha": round_ndigits(total_disturbed_area_ha, 2),
        "total_disturbed_percentage": pct_or_int_zero(total_disturbed_area_ha, total_ecosystem_area_ha),
        "ecosystems": ecosystem_results,
    }))
}
