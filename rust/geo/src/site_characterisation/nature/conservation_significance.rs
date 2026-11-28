//! Component 2.6 Conservation Significance (`nature/conservation_significance.py`): where the AOI
//! sits in the NatureMap global priority ranks (three scenarios, top 10% / 30% budgets).
//!
//! The rasters stay in EPSG:4326 and are clipped with `mask(crop=True, all_touched=True,
//! nodata=src.nodata, filled=True)`; each cell's ground area is a SPHERE latitude band
//! (R = 6371.0088 km), not the geodesic rows of the threat sections.

use serde_json::{Value, json};

use crate::Result;
use crate::affine::Affine;
use crate::common::{Aoi, aoi_4326_union, mask_crop, not_applicable, open_layer, rasterio_nodata};
use crate::py::{fmt_grouped, np_sum};
use crate::settings::Settings;

const NATUREMAP_RASTERS: [(&str, &str); 3] = [
    ("Biodiversity", "biodiversity_only_v3.tif"),
    ("Carbon", "biodiversity_carbon_v3.tif"),
    ("Water", "biodiversity_water_v3.tif"),
];
const NATUREMAP_BUDGETS: [i64; 2] = [10, 30];
const EARTH_RADIUS_KM: f64 = 6371.0088;
const NATUREMAP_MIN_PIXELS: usize = 20;

/// `pixel_area_km2` for one row: `R**2 * radians(|a|) * (sin(radians(top)) - sin(radians(bot)))`.
// NOTE: numpy's float64 sin is the platform libm on Windows and glibc builds without SVML; this
// uses Rust's `sin`, which is the same libm call. A numpy build that dispatches SVML would differ
// in the last bits.
fn row_area_km2(t: &Affine, row: usize) -> f64 {
    let (d_lon, d_lat) = (t.a.abs(), t.e.abs());
    let row_top = t.f - row as f64 * d_lat;
    let row_bot = row_top - d_lat;
    let band = row_top.to_radians().sin() - row_bot.to_radians().sin();
    EARTH_RADIUS_KM * EARTH_RADIUS_KM * d_lon.to_radians() * band
}

/// `clip(path, geoms)`: values with NaN for nodata, and each cell's area (NaN where the value is).
fn clip(
    settings: &Settings,
    layer: &str,
    polygons: &[Vec<crate::common::Ring>],
    is_multi: bool,
) -> Result<(Vec<f64>, Vec<f64>)> {
    let src = open_layer(settings, layer)?;
    let nodata = rasterio_nodata(&src)?;
    let m = mask_crop(&src, polygons, is_multi, true)?;
    let mut vals = m.filled(nodata.unwrap_or(0.0));
    if let Some(nd) = nodata {
        for v in vals.iter_mut().filter(|v| **v == nd) {
            *v = f64::NAN;
        }
    }
    let width = m.width.max(1);
    let area = vals
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if v.is_nan() {
                f64::NAN
            } else {
                row_area_km2(&m.transform, i / width)
            }
        })
        .collect();
    Ok((vals, area))
}

/// `np.nansum`
fn nansum(v: impl Iterator<Item = f64>) -> f64 {
    np_sum(
        &v.map(|x| if x.is_nan() { 0.0 } else { x })
            .collect::<Vec<_>>(),
    )
}

/// `analyze_conservation_significance`
pub fn analyze_conservation_significance(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let (polygons, is_multi) = aoi_4326_union(aoi)?;
    let true_km2 = aoi.area_ha / 100.0;
    let reference = match clip(settings, NATUREMAP_RASTERS[0].1, &polygons, is_multi) {
        Ok(r) => Some(r),
        Err(e) if e.kind == "ValueError" => None,
        Err(e) => return Err(e),
    };
    let envelope = reference
        .as_ref()
        .map_or(0.0, |(_, a)| nansum(a.iter().copied()));
    let n_px = reference
        .as_ref()
        .map_or(0, |(v, _)| v.iter().filter(|x| !x.is_nan()).count());
    drop(reference);
    if n_px == 0 || envelope <= 0.0 {
        let (narrative, flags, missing) =
            not_applicable("No global priority ranking is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"priority_shares": []}, "values": {}, "flags": flags, "missing": missing});
        return Ok((
            results,
            json!({"conservation_significances": [], "conservation_priority_rank": null}),
        ));
    }
    let mut flags = Vec::new();
    if n_px < NATUREMAP_MIN_PIXELS {
        flags.push(format!(
            "2.6: the AOI covers {n_px} priority cells and the source warns below {NATUREMAP_MIN_PIXELS}. At 10 km a cell is \
             about 100 km2, so these shares move in large steps and should be read as landscape context, not as a site figure."
        ));
    }
    let mut notes = Vec::new();
    if envelope > true_km2 {
        notes.push(format!(
            "2.6: percentages are shares of the {} km2 pixel envelope, {:+.0}% larger than the {} km2 project area, because \
             a 10 km cell is kept whole wherever the polygon touches it.",
            fmt_grouped(envelope, 0),
            100.0 * envelope / true_km2 - 100.0,
            fmt_grouped(true_km2, 0)
        ));
    }

    // (scenario, budget, area_km2, pct_of_envelope, min_rank, valid_px)
    #[expect(
        clippy::type_complexity,
        reason = "the six columns named in the comment above"
    )]
    let mut rows: Vec<(&str, i64, f64, f64, Option<f64>, usize)> = Vec::new();
    let mut min_ranks: Vec<(&str, Option<f64>)> = Vec::new();
    for (label, layer) in NATUREMAP_RASTERS {
        let (rank, area) = clip(settings, layer, &polygons, is_multi)?;
        let valid = rank.iter().filter(|x| !x.is_nan()).count();
        let rmin = (valid > 0).then(|| {
            rank.iter()
                .copied()
                .filter(|x| !x.is_nan())
                .fold(f64::INFINITY, f64::min)
        });
        min_ranks.push((label, rmin));
        for b in NATUREMAP_BUDGETS {
            let km2 = nansum(
                rank.iter()
                    .zip(&area)
                    .filter(|(r, _)| **r <= b as f64)
                    .map(|(_, a)| *a),
            );
            rows.push((label, b, km2, 100.0 * km2 / envelope, rmin, valid));
        }
    }
    let best_rank = min_ranks
        .iter()
        .filter_map(|(_, r)| *r)
        .reduce(|a, b| if b < a { b } else { a });
    let narrative = match best_rank {
        Some(best) => {
            let top30 = rows
                .iter()
                .filter(|r| r.1 == 30)
                .map(|r| r.3)
                .reduce(|a, b| if b > a { b } else { a })
                .unwrap_or(f64::NAN);
            format!(
                "Part of this project area falls within the top {}% of global conservation priority. Across the three \
                 weighting scenarios, {}% of the assessed area lies inside the global top 30%.",
                fmt_grouped(best, 0),
                fmt_grouped(top30, 0)
            )
        }
        None => "No global priority ranking is available for this project area.".to_string(),
    };
    let min_ranks_json: serde_json::Map<String, Value> = min_ranks
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    let results = json!({
        "narrative": narrative,
        "tables": {"priority_shares": rows.iter().map(|(s, b, km2, pct, rmin, valid)| json!({
            "scenario": s, "budget_pct": b, "area_km2": km2, "area_ha": km2 * 100.0,
            "pct_of_envelope": pct, "min_rank": rmin, "valid_px": valid,
        })).collect::<Vec<_>>()},
        "values": {
            "chart_series": "priority_shares",
            "chart_unit": "%",
            "chart_axis_label": "Share of assessed area (%)",
            "min_ranks": min_ranks_json,
            "best_rank": best_rank,
            "valid_pixels": n_px,
            "envelope_km2": envelope,
            "true_km2": true_km2,
        },
        "flags": flags,
        "notes": notes,
    });
    let view_results = json!({
        "conservation_significances": rows.iter().map(|(s, b, km2, pct, _, _)| json!({
            "id": format!("{s}_top{b}"),
            "name": format!("{s}, global top {b}%"),
            "scenario_id": s,
            "budget_id": b.to_string(),
            "area": km2 * 100.0,
            "percentage": pct,
        })).collect::<Vec<_>>(),
        "conservation_priority_rank": best_rank,
    });
    Ok((results, view_results))
}
