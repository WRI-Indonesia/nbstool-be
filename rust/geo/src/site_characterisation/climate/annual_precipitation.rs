//! Component 3.4 Annual Precipitation (`climate/annual_precipitation.py`): the monthly rainfall
//! profile and the annual total. `results` carries the SPATIAL spread of the annual total; the
//! card fields carry the SEASONAL spread (driest and wettest month) and the annual total, plus
//! the count of months whose AOI mean is below DRY_MONTH_MM.

use serde_json::{Value, json};

use super::climate_stack::{Annual, MonthlyValue, read_monthly_stack};
use crate::Result;
use crate::common::{Aoi, not_applicable};
use crate::config::{
    DRY_MONTH_MM, WORLDCLIM_PERIOD, WORLDCLIM_PREC_RASTER, WORLDCLIM_RESOLUTION, WORLDCLIM_VERSION,
};
use crate::py::fmt_grouped;
use crate::settings::Settings;

/// `analyze_annual_precipitation`
pub fn analyze_annual_precipitation(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    // "sum": twelve months are added per pixel BEFORE any spatial statistic is taken.
    let Some(stack) = read_monthly_stack(settings, WORLDCLIM_PREC_RASTER, aoi, Annual::Sum)? else {
        let (narrative, flags, missing) =
            not_applicable("No precipitation data is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"monthly_precipitation": []},
                             "values": {}, "flags": flags, "missing": missing});
        let view = json!({"historical_precipitations": [], "precipitation_avg_number": null,
                          "precipitation_min_number": null, "precipitation_max_number": null,
                          "precipitation_total_number": null, "number_of_dry_months": null});
        return Ok((results, view));
    };

    let (mean_mm, min_mm, max_mm) = (stack.mean(), stack.min(), stack.max());
    let mut flags: Vec<String> = Vec::new();
    let narrative = if stack.has_range() {
        format!(
            "Annual precipitation in the selected area ranges from {} to {} mm with an average of {} mm.",
            fmt_grouped(min_mm, 1),
            fmt_grouped(max_mm, 1),
            fmt_grouped(mean_mm, 1)
        )
    } else {
        flags.push(format!(
            "3.4: only {} climate grid cells fall inside the AOI, so no spatial range is reported.",
            stack.n_pixels
        ));
        format!(
            "Annual precipitation in the selected area is {} mm.",
            fmt_grouped(mean_mm, 1)
        )
    };

    let results = json!({
        "narrative": narrative,
        "tables": {"monthly_precipitation": stack.monthly.iter().map(MonthlyValue::to_json).collect::<Vec<_>>()},
        "values": {
            "chart_series": "monthly_precipitation",
            "chart_unit": "mm",
            "chart_axis_label": "Precipitation (mm)",
            "annual_mean_mm": mean_mm,
            "annual_min_mm": min_mm,
            "annual_max_mm": max_mm,
            "n_pixels": stack.n_pixels,
            "source": format!("{WORLDCLIM_VERSION} {WORLDCLIM_RESOLUTION}"),
            "period": WORLDCLIM_PERIOD,
        },
        "flags": flags,
    });

    let dry_months = stack
        .monthly
        .iter()
        .filter(|m| m.value < DRY_MONTH_MM)
        .count();
    let month_min_mm = stack
        .monthly
        .iter()
        .map(|m| m.value)
        .fold(f64::INFINITY, f64::min);
    let month_max_mm = stack
        .monthly
        .iter()
        .map(|m| m.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let total_mm = mean_mm;

    let view = json!({
        "precipitation_min_number": month_min_mm,
        "precipitation_max_number": month_max_mm,
        "precipitation_avg_number": total_mm / 12.0,
        "precipitation_total_number": total_mm,
        "number_of_dry_months": dry_months,
        "historical_precipitations": stack.monthly.iter()
            .map(|m| json!({"precipitation": m.value, "month": m.month})).collect::<Vec<_>>(),
    });
    Ok((results, view))
}
