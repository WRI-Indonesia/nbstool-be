//! 7.3 Land Degradation (`threat/land_degradation.py`): degraded / stable / improved area over
//! the AOI, split by the 2024 forest mask.
//!
//! Both layers are ESRI:54034. Each is clipped with `mask(crop=True, nodata=0, filled=True)` and
//! cast to float32; the degradation clip is then `reproject`ed (nearest, src/dst nodata 0) onto
//! the forest clip's grid through the same GDAL warp rasterio drives for an ndarray source.

use gdal::spatial_ref::SpatialRef;
use serde_json::{Value, json};

use crate::common::{Aoi, mask_crop, open_layer};
use crate::config::{REFERENCE_CRS_WKT, THREAT_FOREST_2024_54034, THREAT_LAND_DEGRADATION};
use crate::ffi::{self, MemDataset, gdal_err};
use crate::py::fmt_grouped;
use crate::settings::Settings;
use crate::{Error, Result};

/// One clip as the cell holds it: float32 values after `filled(0).astype("float32")`.
struct Clip {
    values: Vec<f32>,
    transform: crate::affine::Affine,
    width: usize,
    height: usize,
}

fn clip(settings: &Settings, layer: &str, aoi: &Aoi) -> Result<Clip> {
    let src = open_layer(settings, layer)?;
    let reference = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    if src.spatial_ref().ok().as_ref() != Some(&reference) {
        // shortcut: pyproj's str(src.crs) is not reproduced (the WKT is printed); only a
        // misprojected upload reaches this line.
        return Err(Error::new(
            "RuntimeError",
            format!("{layer} is {}, expected ESRI:54034.", src.projection()),
        ));
    }
    let m = mask_crop(&src, &aoi.polygons, aoi.is_multi, false)?;
    let values = m.filled(0.0).into_iter().map(|v| v as f32).collect();
    Ok(Clip {
        values,
        transform: m.transform,
        width: m.width,
        height: m.height,
    })
}

/// `analyze_land_degradation`
pub fn analyze_land_degradation(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let forest = clip(settings, THREAT_FOREST_2024_54034, aoi)?;
    let mut deg = clip(settings, THREAT_LAND_DEGRADATION, aoi)?;

    let mut deg_aligned = vec![0f32; forest.width * forest.height];
    {
        let src = MemDataset::over(
            &mut deg.values,
            deg.width,
            deg.height,
            deg.transform.to_gdal(),
            Some(REFERENCE_CRS_WKT),
        )?;
        let dst = MemDataset::over(
            &mut deg_aligned,
            forest.width,
            forest.height,
            forest.transform.to_gdal(),
            Some(REFERENCE_CRS_WKT),
        )?;
        ffi::reproject_band(
            &src.ds,
            1,
            &dst,
            gdal_sys::GDALResampleAlg::GRA_NearestNeighbour,
            Some(0.0),
            Some(0.0),
        )?;
    }
    drop(deg);

    let forest_1: Vec<bool> = forest.values.iter().map(|v| *v == 1.0).collect();
    let pixel_area_ha = (forest.transform.a * forest.transform.e).abs() / 10000.0;
    let count = |pred: &dyn Fn(usize, f32) -> bool| {
        deg_aligned
            .iter()
            .enumerate()
            .filter(|(i, v)| pred(*i, **v))
            .count()
    };

    let mut rows = Vec::new();
    for value in [1.0f32, 2.0, 3.0] {
        let inside = count(&|i, v| forest_1[i] && v > 0.0 && v == value);
        let outside = count(&|i, v| !forest_1[i] && v > 0.0 && v == value);
        rows.push(json!({
            "class": value as i64,
            "inside_forest_pixels": inside,
            "inside_forest_ha": inside as f64 * pixel_area_ha,
            "outside_forest_pixels": outside,
            "outside_forest_ha": outside as f64 * pixel_area_ha,
        }));
    }
    let ha = |pred: &dyn Fn(usize, f32) -> bool| count(pred) as f64 * pixel_area_ha;
    let degraded_ha = ha(&|_, v| v == 1.0);
    let stable_ha = ha(&|_, v| v == 2.0);
    let improved_ha = ha(&|_, v| v == 3.0);
    let degraded_inside_forest_ha = ha(&|i, v| forest_1[i] && v == 1.0);
    let degraded_outside_forest_ha = ha(&|i, v| !forest_1[i] && v == 1.0);

    let mut narrative = format!(
        "A total of {} ha within the project area showed signs of land degradation between 2014 and 2024. \
         Of this degraded area, {} ha occurred within forest areas, while {} ha occurred outside forest areas.",
        fmt_grouped(degraded_ha, 2),
        fmt_grouped(degraded_inside_forest_ha, 2),
        fmt_grouped(degraded_outside_forest_ha, 2),
    );
    if improved_ha > 0.0 {
        narrative += &format!(
            "\n\nIn contrast, {} ha within the project area showed positive changes in land condition between 2014 and 2024.",
            fmt_grouped(improved_ha, 2)
        );
    }
    if stable_ha > 0.0 {
        narrative += &format!(
            "\n\nA total of {} ha was classified as stable, indicating no significant improvement or deterioration in \
             the assessed land condition indicators between 2014 and 2024.",
            fmt_grouped(stable_ha, 2)
        );
    }

    Ok(json!({
        "degraded_ha": degraded_ha,
        "stable_ha": stable_ha,
        "improved_ha": improved_ha,
        "degraded_inside_forest_ha": degraded_inside_forest_ha,
        "degraded_outside_forest_ha": degraded_outside_forest_ha,
        "table": rows,
        "narrative": narrative,
    }))
}
