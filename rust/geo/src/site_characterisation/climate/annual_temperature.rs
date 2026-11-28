//! Component 3.3 Annual Temperature (`climate/annual_temperature.py`): the monthly temperature
//! profile and the annual mean across the AOI (spatial min and max of the per-pixel annual mean).

use serde_json::{Value, json};

use super::climate_stack::{Annual, MonthlyValue, read_monthly_stack};
use crate::Result;
use crate::common::{Aoi, not_applicable};
use crate::config::{
    WORLDCLIM_PERIOD, WORLDCLIM_RESOLUTION, WORLDCLIM_TAVG_RASTER, WORLDCLIM_VERSION,
};
use crate::settings::Settings;

/// `analyze_annual_temperature`
pub fn analyze_annual_temperature(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let Some(stack) = read_monthly_stack(settings, WORLDCLIM_TAVG_RASTER, aoi, Annual::Mean)?
    else {
        let (narrative, flags, missing) =
            not_applicable("No temperature data is available for this project area.");
        let results = json!({"narrative": narrative, "tables": {"monthly_temperature": []},
                             "values": {}, "flags": flags, "missing": missing});
        let view = json!({"historical_temperatures": [], "temperature_avg_number": null,
                          "temperature_min_number": null, "temperature_max_number": null});
        return Ok((results, view));
    };

    let (mean_c, min_c, max_c) = (stack.mean(), stack.min(), stack.max());
    let mut flags: Vec<String> = Vec::new();
    let narrative = if stack.has_range() {
        format!(
            "Annual mean temperature in the selected area ranges from {min_c:.1} to \
             {max_c:.1} degree Celsius with an average of {mean_c:.1} degree Celsius."
        )
    } else {
        flags.push(format!(
            "3.3: only {} climate grid cells fall inside the AOI, so no spatial range is reported.",
            stack.n_pixels
        ));
        format!("Annual mean temperature in the selected area is {mean_c:.1} degree Celsius.")
    };

    let results = json!({
        "narrative": narrative,
        "tables": {"monthly_temperature": stack.monthly.iter().map(MonthlyValue::to_json).collect::<Vec<_>>()},
        "values": {
            "chart_series": "monthly_temperature",
            "chart_unit": "C",
            "chart_axis_label": "Temperature (C)",
            "annual_mean_c": mean_c,
            "annual_min_c": min_c,
            "annual_max_c": max_c,
            "n_pixels": stack.n_pixels,
            "source": format!("{WORLDCLIM_VERSION} {WORLDCLIM_RESOLUTION}"),
            "period": WORLDCLIM_PERIOD,
        },
        "flags": flags,
    });
    let view = json!({
        "temperature_min_number": min_c,
        "temperature_max_number": max_c,
        "temperature_avg_number": mean_c,
        "historical_temperatures": stack.monthly.iter()
            .map(|m| json!({"temperature": m.value, "month": m.month})).collect::<Vec<_>>(),
    });
    Ok((results, view))
}
