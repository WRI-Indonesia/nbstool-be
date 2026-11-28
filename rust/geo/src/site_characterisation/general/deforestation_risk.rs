//! Component 1.6 Deforestation Risk (`general/deforestation_risk.py`).
//!
//! The AOI's median modelled deforestation risk (forest-masked `def_risk_v3.tif`, 0..65535 as
//! 0..100, read nearest) positioned inside the dominant country's percentile breakpoints of
//! national forest risk by linear interpolation (clamped to p10..p90). The country is 1.2's
//! `dominant_country`.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_national_forest_risk_percentiles, load_raster_clipped};
use crate::config::{PROB_RASTER, PROB_SCALE_MAX, RISK_HIGHER_PCTL, RISK_LOWER_PCTL};
use crate::py::fmt_pct;
use crate::settings::Settings;

/// `np.interp(value, xs, fp)` for one value (numpy `arr_interp`): clamped at both ends, `j` the
/// last breakpoint <= value, `slope * (x - xp[j]) + fp[j]` with the NaN fallbacks numpy applies.
fn np_interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    if x.is_nan() {
        return x;
    }
    if x < xp[0] {
        return fp[0];
    }
    if x >= xp[n - 1] {
        return fp[n - 1];
    }
    let j = xp.iter().take_while(|v| **v <= x).count() - 1;
    if x == xp[j] {
        return fp[j];
    }
    let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j]);
    let mut r = slope * (x - xp[j]) + fp[j];
    if r.is_nan() {
        r = slope * (x - xp[j + 1]) + fp[j + 1];
        if r.is_nan() && fp[j] == fp[j + 1] {
            r = fp[j];
        }
    }
    r
}

/// `_national_percentile`: breakpoints sorted by percentile, interpolated.
fn national_percentile(value: f64, breakpoints: &[(i64, f64)]) -> f64 {
    let mut bp = breakpoints.to_vec();
    bp.sort_by_key(|(p, _)| *p);
    let xs: Vec<f64> = bp.iter().map(|(_, v)| *v).collect();
    let pcts: Vec<f64> = bp.iter().map(|(p, _)| *p as f64).collect();
    np_interp(value, &xs, &pcts)
}

/// `np.median` of a 1-D array (NaN if any NaN, as numpy's NaN check returns).
fn np_median(mut v: Vec<f64>) -> f64 {
    if v.iter().any(|x| x.is_nan()) {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        // np.mean of the two middle values: (-0.0 + a + b) / 2
        (-0.0 + v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn view_results(top_share: Option<f64>) -> Value {
    json!({"deforestation_risk_share_percentage": top_share})
}

/// `analyze_deforestation_risk(aoi, dominant_country)`
pub fn analyze_deforestation_risk(
    settings: &Settings,
    aoi: &Aoi,
    dominant_country: Option<&str>,
) -> Result<(Value, Value)> {
    // The risk raster is already masked to forest upstream, so its valid pixels are the forest.
    let prob = load_raster_clipped(settings, PROB_RASTER, aoi, Resampling::Nearest, 1, None)?;
    let valid: Vec<f64> = prob
        .values
        .iter()
        .zip(&prob.mask)
        .filter(|(_, m)| !**m)
        .map(|(v, _)| *v)
        .collect();
    let pixel_area_ha = prob.pixel_area_ha;
    drop(prob);
    if valid.is_empty() {
        let results = json!({
            "narrative": "No forest covered by the deforestation risk model is present in this project \
                          area, so deforestation risk cannot be assessed.",
            "tables": {},
            "values": {"aoi_risk": null, "assessed_ha": 0.0, "national_percentile": null},
            "flags": [],
            "missing": ["1.6: no forest covered by the deforestation risk model is present in \
                         this project area, so risk cannot be assessed."],
        });
        return Ok((results, view_results(None)));
    }

    // 0 to PROB_SCALE_MAX encodes a 0 to 100 relative risk score.
    let size = valid.len();
    let forest_risk: Vec<f64> = valid
        .into_iter()
        .map(|v| v / PROB_SCALE_MAX * 100.0)
        .collect();
    let aoi_risk = np_median(forest_risk);
    let assessed_ha = size as f64 * pixel_area_ha;
    let base_values = json!({
        "aoi_risk": aoi_risk,
        "assessed_ha": assessed_ha,
        "assessed_pct_of_aoi": if aoi.area_ha != 0.0 { assessed_ha / aoi.area_ha * 100.0 } else { 0.0 },
        "national_percentile": null,
    });

    let Some(country) = dominant_country.filter(|c| !c.is_empty()) else {
        let results = json!({
            "narrative": "No country could be determined, so risk cannot be compared nationally.",
            "tables": {},
            "values": base_values,
            "missing": ["1.6: no dominant country from 1.2, so the national comparison is skipped."],
        });
        return Ok((results, view_results(None)));
    };

    let Some(breakpoints) = load_national_forest_risk_percentiles(settings, country)? else {
        let results = json!({
            "narrative": format!(
                "No national forest risk reference is available for {country}, so risk cannot be \
                 compared nationally."
            ),
            "tables": {},
            "values": base_values,
            "missing": [format!("1.6: no national risk reference is published for {country}, so the percentile is null.")],
        });
        return Ok((results, view_results(None)));
    };

    let percentile = national_percentile(aoi_risk, &breakpoints);
    let top_share = 100.0 - percentile;
    let (comparison, narrative) = if percentile > RISK_HIGHER_PCTL {
        (
            "higher than",
            format!(
                "Forest in this area is at higher deforestation risk than the national average, \
                 ranking in the top {} of the country's forest for deforestation risk.",
                fmt_pct(top_share, 0)
            ),
        )
    } else if percentile < RISK_LOWER_PCTL {
        (
            "lower than",
            format!(
                "Forest in this area is at lower deforestation risk than the national average, in \
                 the bottom {} of the country's forest.",
                fmt_pct(percentile, 0)
            ),
        )
    } else {
        (
            "similar to",
            "Forest in this area is at deforestation risk similar to the national average, around \
             the national median."
                .to_string(),
        )
    };

    let mut values = base_values;
    values["national_percentile"] = json!(percentile);
    values["top_share_pct"] = json!(top_share);
    values["comparison"] = json!(comparison);
    let results = json!({"narrative": narrative, "tables": {}, "values": values, "flags": []});
    Ok((results, view_results(Some(top_share))))
}
