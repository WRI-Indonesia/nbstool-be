//! 5.2 Avoided emissions from unplanned deforestation (`benefit/avoided_deforestation.py`):
//! ranked-risk allocation of a compounding baseline loss over the Protect area, on the
//! deforestation-risk grid (ESRI:54034 warp).

use serde_json::{Value, json};

use super::ByCategory;
use super::config::*;
use super::util::{na, np_argsort, py_floordiv};
use crate::common::{Aoi, Resampling, load_raster_clipped};
use crate::config::{
    AGB_RASTER, CARBON_COVERAGE_WARN_PCT, CARBON_FRACTION, DEFOR_PERIOD_YEARS, PATHWAY_BAND,
    PATHWAY_ECOSYSTEM_BAND, PATHWAY_RASTER, PROB_RASTER, ROOT_TO_SHOOT_RATIO,
};
use crate::py::fsum_py as py_sum;
use crate::py::{fmt_grouped, fmt_ha, oxford_join, safe_pct};
use crate::settings::Settings;
use crate::{Error, Result};

/// `_protect_qb_avoided_ha`: Protect categories of 4.2 with a QB Avoided activity.
fn protect_qb_avoided_ha(by_category: Option<&ByCategory>) -> f64 {
    let Some(by_category) = by_category else {
        return 0.0;
    };
    let mut area = 0.0;
    for (_, info) in by_category {
        if info.get("pathway").and_then(Value::as_str) != Some("Protect") {
            continue;
        }
        let acts = info
            .get("activities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if acts
            .iter()
            .any(|a| a.get("qb_avoided").is_some_and(super::util::truthy))
        {
            area += info.get("area_ha").and_then(Value::as_f64).unwrap_or(0.0);
        }
    }
    area
}

/// `_carbon_on_area`
fn carbon_on_area(cumulative: &[f64], area_ha: f64, pixel_area_ha: f64) -> f64 {
    if area_ha <= 0.0 || cumulative.is_empty() {
        return 0.0;
    }
    let total_ha = cumulative.len() as f64 * pixel_area_ha;
    if area_ha >= total_ha {
        return cumulative[cumulative.len() - 1];
    }
    let whole = py_floordiv(area_ha, pixel_area_ha) as i64;
    let mut carbon = if whole > 0 {
        cumulative[(whole - 1) as usize]
    } else {
        0.0
    };
    let remainder_ha = area_ha - whole as f64 * pixel_area_ha;
    if remainder_ha > 0.0 && (whole as usize) < cumulative.len() {
        let prev = if whole > 0 {
            cumulative[(whole - 1) as usize]
        } else {
            0.0
        };
        let next_pixel_carbon = cumulative[whole as usize] - prev;
        carbon += next_pixel_carbon * (remainder_ha / pixel_area_ha);
    }
    carbon
}

/// Component 5.2. `by_category` = 4.2's (unfiltered) by_category, None when 4.2 is absent.
pub fn analyze_avoided_deforestation_emissions(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
    rate_pct: Option<f64>,
    by_category: Option<&ByCategory>,
) -> Result<(Value, Value)> {
    if duration_years < 1 {
        return Err(Error::new(
            "ValueError",
            "PROJECT_DURATION_YEARS must be a whole number of years, at least 1.",
        ));
    }
    let qb_avoided_ha = protect_qb_avoided_ha(by_category);
    if qb_avoided_ha <= 0.0 {
        return Ok(na(
            "No Protect-pathway activity in this project area is flagged for avoided-emissions quantification (QB \
             Avoided Emissions = Yes), so avoided deforestation emissions are not quantified for this site.",
        ));
    }
    let risk = load_raster_clipped(settings, PROB_RASTER, aoi, Resampling::Nearest, 1, None)?;
    let like = risk.grid();
    let pathway = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_BAND,
        Some(like),
    )?;
    let pix = risk.pixel_area_ha;
    let n_px = risk.values.len();
    let protect_all: Vec<bool> = (0..n_px)
        .map(|i| !pathway.mask[i] && pathway.values[i] == PROTECT_CODE)
        .collect();
    let pool: Vec<bool> = (0..n_px).map(|i| protect_all[i] && !risk.mask[i]).collect();
    let protect_all_ha = protect_all.iter().filter(|x| **x).count() as f64 * pix;
    let protect_ha = pool.iter().filter(|x| **x).count() as f64 * pix;
    if protect_ha <= 0.0 {
        return Ok(na(
            "No forest in this project area falls under the Protect pathway, so avoided emissions from deforestation \
             cannot be estimated.",
        ));
    }
    let Some(rate_pct) = rate_pct else {
        return Ok(na(
            "No historical deforestation rate is available for this project area, so a baseline for avoided \
             emissions cannot be projected.",
        ));
    };
    let mut notes: Vec<String> = Vec::new();
    let risk_coverage_pct = safe_pct(protect_ha, protect_all_ha);
    if risk_coverage_pct < PROTECT_RISK_COVERAGE_WARN_PCT {
        notes.push(format!(
            "5.2: the risk layer covers only {risk_coverage_pct:.0}% of the Protect area. The remainder carries no \
             projected loss and no avoided emissions."
        ));
    }
    if duration_years > BASELINE_RATE_MAX_YEARS {
        notes.push(format!(
            "5.2: the deforestation rate was measured over {DEFOR_PERIOD_YEARS} years and is projected over \
             {duration_years}. A rate that far outside its measurement window is an assumption, not an observation."
        ));
    }
    let ecosystem = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_ECOSYSTEM_BAND,
        Some(like),
    )?;
    let mut counts: Vec<(i64, usize)> = Vec::new();
    for i in (0..n_px).filter(|i| pool[*i]) {
        let c = if ecosystem.mask[i] {
            0
        } else {
            ecosystem.values[i] as i64
        };
        match counts.iter_mut().find(|(k, _)| *k == c) {
            Some(e) => e.1 += 1,
            None => counts.push((c, 1)),
        }
    }
    counts.sort_by_key(|(c, _)| *c);
    let ecosystem_ha: Vec<(i64, f64)> = counts.iter().map(|(c, n)| (*c, *n as f64 * pix)).collect();
    let mut by_area = ecosystem_ha.clone();
    by_area.sort_by(|a, b| b.1.total_cmp(&a.1)); // sorted(key=get, reverse=True), stable
    let words: Vec<String> = by_area
        .iter()
        .filter_map(|(c, _)| lookup(&PROTECT_ECOSYSTEM_WORDS, *c))
        .map(str::to_string)
        .collect();
    let ecosystem_label = match oxford_join(&words) {
        w if w.is_empty() => "natural".to_string(),
        w => w,
    };
    let unmapped: Vec<f64> = ecosystem_ha
        .iter()
        .filter(|(c, _)| lookup(&PROTECT_ECOSYSTEM_WORDS, *c).is_none())
        .map(|(_, h)| *h)
        .collect();
    let unmapped_ha = py_sum(unmapped);
    if unmapped_ha > 0.0 {
        notes.push(format!(
            "5.2: {} of the Protect area carries a risk value but a reference ecosystem that is not forest, mangrove \
             or peatland. The risk layer and pathway band 3 disagree about what is forest.",
            fmt_ha(unmapped_ha)
        ));
    }
    let peat_ha = ecosystem_ha
        .iter()
        .find(|(c, _)| *c == PATHWAY_ECOSYSTEM_PEATLAND)
        .map_or(0.0, |(_, h)| *h);
    if peat_ha > 0.0 {
        notes.push(format!(
            "5.2: {} of the Protect area sits on peatland. Only aboveground and belowground biomass is counted, and \
             on peat the avoided emission is dominated by peat oxidation, so this figure is a large under-estimate \
             there.",
            fmt_ha(peat_ha)
        ));
    }
    let agb = load_raster_clipped(
        settings,
        AGB_RASTER,
        aoi,
        Resampling::Average,
        1,
        Some(like),
    )?;
    let density: Vec<f64> = agb
        .values
        .iter()
        .zip(&agb.mask)
        .map(|(v, m)| {
            (if *m { 0.0 } else { *v }) * (1.0 + ROOT_TO_SHOOT_RATIO) * CARBON_FRACTION * CO2_PER_C
        })
        .collect();
    let biomass_coverage_pct = safe_pct(
        (0..n_px).filter(|i| pool[*i] && !agb.mask[*i]).count() as f64 * pix,
        protect_ha,
    );
    if biomass_coverage_pct < CARBON_COVERAGE_WARN_PCT {
        notes.push(format!(
            "5.2: the biomass raster covers only {biomass_coverage_pct:.0}% of the Protect area. Nodata counts as zero \
             carbon, so the estimate is an under-estimate by an unknown amount."
        ));
    }
    let pool_density: Vec<f64> = (0..n_px).filter(|i| pool[*i]).map(|i| density[i]).collect();
    let pool_risk: Vec<f64> = (0..n_px)
        .filter(|i| pool[*i])
        .map(|i| if risk.mask[i] { 0.0 } else { risk.values[i] })
        .collect();
    // np.cumsum(density[np.argsort(risk)[::-1]]) * pixel_area_ha
    let order = np_argsort(&pool_risk);
    let mut acc = 0.0;
    let cumulative: Vec<f64> = order
        .iter()
        .rev()
        .map(|i| {
            acc += pool_density[*i];
            acc * pix
        })
        .collect();
    let standing_tco2e = cumulative[cumulative.len() - 1];

    let rate_frac = rate_pct / 100.0;
    let mut rows: Vec<Value> = Vec::new();
    let mut previous = 0.0;
    let (mut total_tco2e, mut projected_loss_ha) = (0.0, 0.0);
    for t in 1..=duration_years {
        let cumulative_loss_ha = protect_ha * (1.0 - (-rate_frac * t as f64).exp());
        let capped = if protect_ha < cumulative_loss_ha {
            protect_ha
        } else {
            cumulative_loss_ha
        }; // min()
        let carbon = carbon_on_area(&cumulative, capped, pix);
        rows.push(json!({
            "year": t, "cumulative_loss_ha": capped, "annual_avoided_tco2e": carbon - previous,
            "cumulative_avoided_tco2e": carbon,
        }));
        previous = carbon;
        total_tco2e = carbon;
        projected_loss_ha = capped;
    }
    let annual_mean_tco2e = total_tco2e / duration_years as f64;
    let uniform_tco2e = standing_tco2e * safe_pct(projected_loss_ha, protect_ha) / 100.0;
    let narrative = if rate_pct <= 0.0 {
        "No forest loss was recorded in this project area between 2014 and 2024, so the baseline projects no further \
         loss and no avoided emissions can be claimed from protecting the standing forest."
            .to_string()
    } else {
        format!(
            "Protecting this {ecosystem_label} ecosystem can avoid an estimated {} tonnes of CO2eq emissions over the \
             project's {duration_years} year duration.",
            fmt_grouped(total_tco2e, 0)
        )
    };
    let eco_obj: serde_json::Map<String, Value> = ecosystem_ha
        .iter()
        .map(|(c, h)| (c.to_string(), json!(h)))
        .collect();
    let values = json!({
        "chart_series": "annual_projection",
        "chart_unit": "tCO2e",
        "chart_axis_label": "Cumulative avoided emissions (tCO2e)",
        "total_tco2e": total_tco2e,
        "annual_mean_tco2e": annual_mean_tco2e,
        "duration_years": duration_years,
        "protect_ha": protect_ha,
        "qb_avoided_protect_ha": qb_avoided_ha,
        "protect_risk_coverage_pct": risk_coverage_pct,
        "projected_loss_ha": projected_loss_ha,
        "standing_tco2e": standing_tco2e,
        "baseline_rate_pct": rate_pct,
        "baseline_rate_source": "AOI forest 2014 to 2024, component 1.5",
        "allocation": "descending deforestation risk",
        "uniform_allocation_tco2e": uniform_tco2e,
        "peat_ha": peat_ha,
        "biomass_coverage_pct": biomass_coverage_pct,
        "pools_included": BIOMASS_POOLS,
        "ecosystem_ha": eco_obj,
        "ecosystem_label": ecosystem_label,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "total_tco2e",
        "annual_mean_tco2e",
        "duration_years",
        "ecosystem_label",
        "protect_ha",
        "projected_loss_ha",
        "baseline_rate_pct",
        "chart_unit",
        "chart_axis_label",
    ] {
        view[k] = values[k].clone();
    }
    view["annual_projection"] = json!(rows); // after the values, as the dict literal
    if !notes.is_empty() {
        view["notes"] = json!(notes);
    }
    let results = json!({
        "narrative": narrative,
        "tables": {"annual_projection": rows},
        "values": values,
        "flags": [],
        "notes": notes,
    });
    Ok((results, view))
}
