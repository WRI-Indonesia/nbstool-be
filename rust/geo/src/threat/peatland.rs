//! 3.4 Peatland Ecosystem (`threat/peatland.py`): remaining peat forest, disturbance, conversion,
//! and the three hydrological indicators (canal proximity, drainage pressure, fire risk).
//!
//! Masked-array semantics as in 3.2, except that `area_ha` here takes `np.asarray(mask)`, i.e. the
//! RAW data. A layer that does not overlap the AOI falls back to unmasked zeros on the ecosystem
//! grid. Canal proximity polygonises the current peat forest and the canal masks (GDALPolygonize,
//! 4-connected), dissolves each (GEOS unary union), moves both to the AOI's UTM zone
//! (`estimate_utm_crs`) and takes the GEOS distance.

use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use gdal::vector::Geometry;
use serde_json::{Value, json};

use crate::affine::Affine;
use crate::common::{
    Aoi, MaBool, MaskedWindow, aoi_in_raster_crs, area_ha_by_row_loop, mask_crop, open_layer,
    raster_geometry_mask_crop, unique_unmasked_at,
};
use crate::config::{
    REFERENCE_CRS_WKT, THREAT_CURRENT_FOREST, THREAT_DISTURBANCE, THREAT_DISTURBANCE_THRESHOLD,
    THREAT_ECOSYSTEM, THREAT_FIRE_RISK, THREAT_FOREST_2024, THREAT_FOREST_LOSS, THREAT_HISTORICAL,
    THREAT_PEAT_CANAL, THREAT_PEAT_CANALS_DENSITY, THREAT_PEATLAND, THREAT_REMAINING_FOREST,
};
use crate::ffi::{self, gdal_err};
use crate::py::round_ndigits;
use crate::settings::Settings;
use crate::{Error, Result};

/// A clipped layer and the WKT of its CRS.
struct Layer {
    m: MaskedWindow,
    crs_wkt: String,
}

/// `read_raster(path, reference, reference_transform, reference_crs)`
fn read_raster(
    settings: &Settings,
    layer: &str,
    aoi: &Aoi,
    reference: Option<&Layer>,
) -> Result<Layer> {
    let src = open_layer(settings, layer)?;
    let polygons = aoi_in_raster_crs(aoi, &src)?;
    match mask_crop(&src, &polygons, aoi.is_multi, false) {
        Ok(m) => Ok(Layer {
            m,
            crs_wkt: src.projection(),
        }),
        Err(e)
            if e.kind == "ValueError"
                && e.message.contains("Input shapes do not overlap raster") =>
        {
            let Some(r) = reference else { return Err(e) };
            let n = r.m.width * r.m.height;
            Ok(Layer {
                m: MaskedWindow {
                    data: vec![0.0; n],
                    mask: vec![false; n],
                    ..r.m.clone()
                },
                crs_wkt: r.crs_wkt.clone(),
            })
        }
        Err(e) => Err(e),
    }
}

/// `round(percentage(area), 2)`: the int 0 when the total is 0.
fn pct(area: f64, total: f64) -> Value {
    if total == 0.0 {
        json!(0)
    } else {
        json!(round_ndigits(area / total * 100.0, 2))
    }
}

fn srs(wkt: &str) -> Result<SpatialRef> {
    let mut s = SpatialRef::from_wkt(wkt).map_err(gdal_err)?;
    s.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    Ok(s)
}

/// `mask_to_geometry(mask, transform, crs)`: the value-1 shapes of the mask, dissolved; None when
/// there are none.
fn mask_to_geometry(mask: &MaBool, transform: &Affine) -> Result<Option<Geometry>> {
    let (h, w) = mask.shape;
    let mut image: Vec<u8> = mask.data.iter().map(|b| *b as u8).collect();
    let mut m = image.clone();
    ffi::polygonize_union_u8(&mut image, &mut m, w, h, transform.to_gdal(), 1)
}

/// geopandas `estimate_utm_crs()` of the AOI (ESRI:54034): the AOI bounds to EPSG:4326 (the
/// cylindrical inverse is separable, so pyproj's densified `transform_bounds` reaches its extremes
/// at the corners), the centre point, then the first EPSG WGS 84 "UTM zone" projected CRS whose
/// area of use contains it, in PROJ's database order (`query_utm_crs_info`).
pub fn estimate_utm_epsg(aoi: &Aoi) -> Result<u32> {
    let ct = CoordTransform::new(&srs(REFERENCE_CRS_WKT)?, &{
        let mut s = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
        s.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        s
    })
    .map_err(gdal_err)?;
    let (minx, miny, maxx, maxy) = aoi.bounds;
    let (mut xs, mut ys) = ([minx, maxx, maxx, minx], [miny, miny, maxy, maxy]);
    ct.transform_coords(&mut xs, &mut ys, &mut [])
        .map_err(gdal_err)?;
    let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let x = (lo(&xs) + hi(&xs)) / 2.0;
    let y = (lo(&ys) + hi(&ys)) / 2.0;
    for info in ffi::crs_info_list("EPSG")? {
        if !info.projected || info.deprecated {
            continue;
        }
        let Some([west, south, east, north]) = info.bbox else {
            continue;
        };
        let hit = if west <= east {
            !(x < west || x > east || y < south || y > north)
        } else {
            // shortcut: antimeridian-crossing areas of use; no WGS 84 UTM zone has one
            continue;
        };
        if hit
            && info.name.contains("UTM zone")
            && info.name.split('/').next().unwrap_or("").replace(' ', "") == "WGS84"
        {
            return info
                .code
                .parse()
                .map_err(|_| Error::new("ValueError", format!("bad EPSG code {}", info.code)));
        }
    }
    Err(Error::new("RuntimeError", "Unable to determine UTM CRS"))
}

/// The canal layer as `read_raster(...)` then `drainage == 1` (numpy.ma: False under the mask),
/// reduced while reading: a Byte band is read as u8, not f64. Same values, an eighth of the
/// window: peat_canal.tif has 4.7 m pixels, ~40x the pixel count of the other layers, and its f64
/// window was the whole of peatland's peak memory. Other band types take the shared path.
fn read_canal_mask(
    settings: &Settings,
    aoi: &Aoi,
    reference: &Layer,
) -> Result<(MaBool, Affine, String)> {
    let src = open_layer(settings, THREAT_PEAT_CANAL)?;
    let band_type = src.rasterband(1).map_err(gdal_err)?.band_type();
    if band_type != gdal::raster::GdalDataType::UInt8 {
        let l = read_raster(settings, THREAT_PEAT_CANAL, aoi, Some(reference))?;
        return Ok((l.m.eq(1.0), l.m.transform, l.crs_wkt));
    }
    let polygons = aoi_in_raster_crs(aoi, &src)?;
    match raster_geometry_mask_crop(&src, &polygons, aoi.is_multi, false) {
        Ok((mut mask, transform, w)) => {
            let b = src.rasterband(1).map_err(gdal_err)?;
            let (off, size) = (
                (w.col_off as isize, w.row_off as isize),
                (w.width as usize, w.height as usize),
            );
            let valid = b
                .open_mask_band()
                .map_err(gdal_err)?
                .read_as::<u8>(off, size, size, None)
                .map_err(gdal_err)?
                .into_shape_and_vec()
                .1;
            for (m, v) in mask.iter_mut().zip(&valid) {
                *m = *m || *v == 0;
            }
            drop(valid);
            let data = b
                .read_as::<u8>(off, size, size, None)
                .map_err(gdal_err)?
                .into_shape_and_vec()
                .1;
            let data: Vec<bool> = data
                .into_iter()
                .zip(&mask)
                .map(|(d, m)| !m && d == 1)
                .collect();
            Ok((
                MaBool {
                    data,
                    mask,
                    shape: (size.1, size.0),
                },
                transform,
                src.projection(),
            ))
        }
        Err(e)
            if e.kind == "ValueError"
                && e.message.contains("Input shapes do not overlap raster") =>
        {
            // zeros on the reference grid: `0 == 1` is False everywhere, nothing masked
            let n = reference.m.width * reference.m.height;
            let shape = (reference.m.height, reference.m.width);
            Ok((
                MaBool {
                    data: vec![false; n],
                    mask: vec![false; n],
                    shape,
                },
                reference.m.transform,
                reference.crs_wkt.clone(),
            ))
        }
        Err(e) => Err(e),
    }
}

/// `canal_proximity_level()`, given `drainage == 1` with its grid
fn canal_proximity_level(
    aoi: &Aoi,
    current_peatland: &MaBool,
    eco: &Layer,
    canal: (MaBool, Affine, String),
) -> Result<(&'static str, Option<f64>)> {
    if !current_peatland.any() {
        return Ok(("Not identified", None));
    }
    let (canal_mask, canal_transform, canal_crs) = canal;
    if !canal_mask.any() {
        return Ok(("Low", None));
    }
    let canal_geom = mask_to_geometry(&canal_mask, &canal_transform)?;
    drop(canal_mask);
    let (Some(mut peat), Some(mut canal)) = (
        mask_to_geometry(current_peatland, &eco.m.transform)?,
        canal_geom,
    ) else {
        return Ok(("Low", None));
    };
    let mut utm = SpatialRef::from_epsg(estimate_utm_epsg(aoi)?).map_err(gdal_err)?;
    utm.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    peat.transform_inplace(&CoordTransform::new(&srs(&eco.crs_wkt)?, &utm).map_err(gdal_err)?)
        .map_err(gdal_err)?;
    canal
        .transform_inplace(&CoordTransform::new(&srs(&canal_crs)?, &utm).map_err(gdal_err)?)
        .map_err(gdal_err)?;
    let distance_m = ffi::distance(&peat, &canal);
    let level = if distance_m <= 500.0 {
        "High"
    } else if distance_m <= 1000.0 {
        "Medium"
    } else {
        "Low"
    };
    Ok((level, Some(distance_m)))
}

/// The first label whose code is among `values` (checked in the given order), else `default`.
fn graded(values: &[f64], ladder: &[(f64, &'static str)], default: &'static str) -> &'static str {
    ladder
        .iter()
        .find(|(c, _)| values.contains(c))
        .map_or(default, |(_, l)| l)
}

/// `analyze_peatland`
pub fn analyze_peatland(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    // Same reads in the same order as the Python, but each layer is reduced to the masks it feeds
    // and dropped before the next is read, so at most two clips are held at once (memory only;
    // every value is the same).
    let mut eco = read_raster(settings, THREAT_ECOSYSTEM, aoi, None)?;
    let transform = eco.m.transform;
    let width = eco.m.width;
    let area = |m: &MaBool| area_ha_by_row_loop(&m.data, width, &transform); // np.asarray: raw data
    let peatland_mask = eco.m.eq(THREAT_PEATLAND);
    // the fallback reference only needs the grid: keep the shape, release the pixels
    eco.m.data = Vec::new();
    eco.m.mask = Vec::new();
    let eco_grid = eco;
    let read = |layer: &str| read_raster(settings, layer, aoi, Some(&eco_grid));
    let total_area = area(&peatland_mask);
    let historical = read(THREAT_HISTORICAL)?;
    let remaining_mask = peatland_mask.and(&historical.m.eq(THREAT_REMAINING_FOREST))?;
    let loss_mask = peatland_mask.and(&historical.m.eq(THREAT_FOREST_LOSS))?;
    drop(historical);
    let remaining_area = area(&remaining_mask);
    let forest2024 = read(THREAT_FOREST_2024)?;
    let current_peatland_mask = peatland_mask.and(&forest2024.m.eq(THREAT_CURRENT_FOREST))?;
    drop(forest2024);
    let disturbance = read(THREAT_DISTURBANCE)?;
    let disturbed_mask =
        current_peatland_mask.and(&disturbance.m.gt(THREAT_DISTURBANCE_THRESHOLD))?;
    drop(disturbance);
    let disturbed_area = area(&disturbed_mask);
    let loss_area = area(&loss_mask);
    drop((remaining_mask, disturbed_mask, loss_mask));
    let canal_density = read(THREAT_PEAT_CANALS_DENSITY)?;
    let drainage_values = unique_unmasked_at(&canal_density.m, &peatland_mask.data);
    drop(canal_density);
    let fire_risk = read(THREAT_FIRE_RISK)?;
    let fire_values = unique_unmasked_at(&fire_risk.m, &peatland_mask.data);
    drop(fire_risk);
    let drainage_canals = read_canal_mask(settings, aoi, &eco_grid)?;

    let (mut canal_proximity, canal_distance_m) =
        canal_proximity_level(aoi, &current_peatland_mask, &eco_grid, drainage_canals)?;

    let mut drainage_pressure = graded(
        &drainage_values,
        &[(3.0, "High"), (2.0, "Medium"), (1.0, "Low")],
        "Not identified",
    );
    let mut fire_risk_level = graded(
        &fire_values,
        &[
            (4.0, "High"),
            (3.0, "Medium"),
            (2.0, "Low"),
            (1.0, "Very low"),
        ],
        "No risk",
    );

    if !peatland_mask.any() {
        canal_proximity = "-";
        drainage_pressure = "-";
        fire_risk_level = "-";
    }

    Ok(json!({"peatland": {
        "total_area_ha": round_ndigits(total_area, 2),
        "remaining_forest": {"area_ha": round_ndigits(remaining_area, 2), "percentage": pct(remaining_area, total_area)},
        "disturbed": {"area_ha": round_ndigits(disturbed_area, 2), "percentage": pct(disturbed_area, total_area)},
        "converted_loss": {"area_ha": round_ndigits(loss_area, 2), "percentage": pct(loss_area, total_area)},
        "drivers": {
            "canal_proximity": canal_proximity,
            "canal_distance_m": canal_distance_m.map(|d| round_ndigits(d, 2)),
            "drainage_pressure": drainage_pressure,
            "fire_risk": fire_risk_level,
        },
    }}))
}
