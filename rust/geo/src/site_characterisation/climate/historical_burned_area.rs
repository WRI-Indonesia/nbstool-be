//! Component 7.2 Historical Burned Area (`climate/historical_burned_area.py`): GABAM burned area
//! per year (2014..2023), on each raster's own grid (`mask(crop=True, filled=False)`), measured
//! geodesically row by row (WGS84) for a geographic raster.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, aoi_in_raster_crs, mask_crop, open_layer, row_pixel_area_m2};
use crate::config::{GABAM_BURNED_VALUE, GABAM_YEARS, gabam_raster};
use crate::ffi::gdal_err;
use crate::py::{fmt_grouped, np_sum};
use crate::settings::Settings;

/// `analyze_historical_burned_area`
pub fn analyze_historical_burned_area(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let aoi_area_ha = aoi.area_ha;
    let mut records: Vec<(i64, f64, f64)> = Vec::new();
    for year in GABAM_YEARS {
        let src = open_layer(settings, &gabam_raster(year))?;
        let polygons = aoi_in_raster_crs(aoi, &src)?;
        let clip = mask_crop(&src, &polygons, aoi.is_multi, false)?;
        let burned: Vec<bool> = clip
            .data
            .iter()
            .zip(&clip.mask)
            .map(|(d, m)| !m && *d == GABAM_BURNED_VALUE)
            .collect();
        let t = clip.transform;
        let geographic = src.spatial_ref().map_err(gdal_err)?.is_geographic();
        let burned_ha = if geographic {
            // burned_ha = 0; burned_ha += burned[row].sum() * abs(area) / 10000, every row
            let mut total = 0.0;
            for (row, cells) in burned.chunks(clip.width.max(1)).enumerate() {
                let count = cells.iter().filter(|b| **b).count() as f64;
                total += count * row_pixel_area_m2(&t, row) / 10000.0;
            }
            total
        } else {
            burned.iter().filter(|b| **b).count() as f64 * ((t.a * t.e).abs() / 10000.0)
        };
        records.push((year, burned_ha, burned_ha / aoi_area_ha * 100.0));
    }

    // pandas Series.sum() / .mean(): numpy's pairwise sum; idxmax: the first maximum
    let areas: Vec<f64> = records.iter().map(|r| r.1).collect();
    let cumulative_ha = np_sum(&areas);
    let average_ha = cumulative_ha / areas.len() as f64;
    let peak = records
        .iter()
        .fold(&records[0], |best, r| if r.1 > best.1 { r } else { best });

    let narrative = if cumulative_ha > 0.0 {
        format!(
            "The project area recorded a cumulative burned area of {} ha between 2014 and 2024, with an average annual \
             burned area of {} ha. The highest annual burned area was recorded in {}, when {} ha was affected by fire.",
            fmt_grouped(cumulative_ha, 2),
            fmt_grouped(average_ha, 2),
            peak.0,
            fmt_grouped(peak.1, 2)
        )
    } else {
        "No burned area was detected within the project area between 2014 and 2024.".to_string()
    };
    let burned_any = cumulative_ha > 0.0;
    let peak_year = burned_any.then_some(peak.0);
    let peak_ha = if burned_any { peak.1 } else { 0.0 };
    let rows: Vec<Value> = records
        .iter()
        .map(|(y, a, p)| json!({"year": y, "burned_area_ha": a, "burned_pct_aoi": p}))
        .collect();
    let results = json!({
        "narrative": narrative,
        "tables": {"annual_burned_area": rows},
        "values": {
            "cumulative_ha": cumulative_ha,
            "average_ha": average_ha,
            "peak_year": peak_year,
            "peak_ha": peak_ha,
            "years": records.iter().map(|r| r.0).collect::<Vec<_>>(),
        },
        "flags": [],
    });
    let view_results = json!({
        "total_burned_area": cumulative_ha,
        "average_annual_burned_area": average_ha,
        "peak_burned_year": peak_year,
        "peak_burned_area": peak_ha,
        "historical_burned_areas": records.iter().map(|(y, a, p)| json!({
            "id": y.to_string(), "year": y, "value": a, "percentage": p,
        })).collect::<Vec<_>>(),
    });
    Ok((results, view_results))
}
