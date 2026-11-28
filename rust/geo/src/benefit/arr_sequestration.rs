//! 5.3 ARR Carbon Removal, ex-ante (`benefit/arr_sequestration.py`): reference-rate / yield-curve
//! sequestration on the Restore area, per (cat_code, ecosystem, dryland zone) group.

use serde_json::{Value, json};

use super::config::*;
use super::util::na;
use crate::common::{Aoi, Resampling, load_raster_bands_clipped, load_raster_clipped};
use crate::config::{
    AGB_RASTER, CARBON_COVERAGE_WARN_PCT, ELEVATION_RASTER, PATHWAY_BAND, PATHWAY_CATCODE_BAND,
    PATHWAY_ECOSYSTEM_BAND, PATHWAY_ECOSYSTEM_CODES, PATHWAY_RASTER, WORLDCLIM_MONTHS,
    WORLDCLIM_PREC_RASTER,
};
use crate::py::fsum_py as py_sum;
use crate::py::{fmt_grouped, fmt_ha, np_sum, safe_pct};
use crate::settings::Settings;
use crate::{Error, Result};

fn arr_params(ecosystem: i64, zone: Option<i64>) -> Result<((f64, f64), f64, f64)> {
    let missing = || Error::new("KeyError", ecosystem.to_string());
    if ecosystem == 1 {
        let z = zone.ok_or_else(missing)?;
        return Ok((
            arr_rate_dm_dryland(z).ok_or_else(missing)?,
            arr_root_to_shoot_dryland(z).ok_or_else(missing)?,
            arr_carbon_fraction(1).ok_or_else(missing)?,
        ));
    }
    Ok((
        arr_rate_dm(ecosystem).ok_or_else(missing)?,
        arr_root_to_shoot(ecosystem).ok_or_else(missing)?,
        arr_carbon_fraction(ecosystem).ok_or_else(missing)?,
    ))
}

/// `_arr_accum_co2e_per_ha`
fn accum_co2e_per_ha(rate: (f64, f64), r: f64, cf: f64, years: i64) -> f64 {
    let young = years.min(ARR_YOUNG_END_YEAR);
    let old = 0.max(years.min(ARR_OLD_END_YEAR) - ARR_YOUNG_END_YEAR);
    let agb_dm = rate.0 * young as f64 + rate.1 * old as f64;
    let tb_dm = agb_dm * (1.0 + r);
    tb_dm * cf * CO2_PER_C
}

/// `_arr_dryland_zone`: per-pixel zone code and validity on the pathway grid.
fn dryland_zone(
    settings: &Settings,
    aoi: &Aoi,
    like: crate::common::Grid,
) -> Result<(Vec<i64>, Vec<bool>)> {
    let elev = load_raster_clipped(
        settings,
        ELEVATION_RASTER,
        aoi,
        Resampling::Bilinear,
        1,
        Some(like),
    )?;
    let bands: Vec<usize> = (1..=WORLDCLIM_MONTHS).collect();
    let prec = load_raster_bands_clipped(
        settings,
        WORLDCLIM_PREC_RASTER,
        aoi,
        Resampling::Average,
        &bands,
        Some(like),
    )?;
    let n = elev.values.len();
    let mut zone = vec![ARR_DRYLAND_DEFAULT_ZONE; n];
    let mut valid = vec![false; n];
    for i in 0..n {
        // np.ma.stack(...).sum(axis=0): months added in order, masked months as 0, masked when all are
        let mut annual = 0.0;
        let mut dry = 0i64;
        let mut all_masked = true;
        for (k, p) in prec.iter().enumerate() {
            if !p.mask[i] {
                all_masked = false;
                annual = if k == 0 {
                    p.values[i]
                } else {
                    annual + p.values[i]
                };
                if p.values[i] < ARR_ZONE_DRY_MONTH_MM {
                    dry += 1;
                }
            } else if k > 0 {
                annual += 0.0;
            }
        }
        let inputs_valid = !elev.mask[i] && !all_masked;
        let elev_f = if elev.mask[i] { 0.0 } else { elev.values[i] };
        let (annual_f, dry_f) = if all_masked {
            (0.0, ARR_ZONE_DRY_SEASON_MONTHS)
        } else {
            (annual, dry)
        };
        let mut z = ARR_DRYLAND_DEFAULT_ZONE;
        if annual_f > ARR_ZONE_WET_ANNUAL_MM && dry_f < ARR_ZONE_DRY_SEASON_MONTHS {
            z = 1;
        }
        if elev_f > ARR_ZONE_ELEV_MONTANE_M {
            z = 3;
        }
        if !inputs_valid {
            z = ARR_DRYLAND_DEFAULT_ZONE;
        }
        zone[i] = z;
        valid[i] = inputs_valid;
    }
    Ok((zone, valid))
}

struct Group {
    cat_code: i64,
    ecosystem: i64,
    ecosystem_label: String,
    zone_label: String,
    activity_mode: &'static str,
    stocking_factor: f64,
    area_ha: f64,
    net_tco2e: f64,
    net_tco2e_per_ha: f64,
}

/// `np.maximum(0.0, a - b).sum()`
fn clamped_sum(a: f64, b: &[f64]) -> f64 {
    np_sum(&b.iter().map(|x| f64::max(0.0, a - x)).collect::<Vec<_>>())
}

/// Component 5.3.
pub fn analyze_arr_sequestration(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
) -> Result<(Value, Value)> {
    if duration_years < 1 {
        return Err(Error::new(
            "ValueError",
            "PROJECT_DURATION_YEARS must be a whole number of years, at least 1.",
        ));
    }
    let pathway = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_BAND,
        None,
    )?;
    let like = pathway.grid();
    let eco = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_ECOSYSTEM_BAND,
        Some(like),
    )?;
    let cat = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_CATCODE_BAND,
        Some(like),
    )?;
    let agb = load_raster_clipped(
        settings,
        AGB_RASTER,
        aoi,
        Resampling::Average,
        1,
        Some(like),
    )?;
    let pix = pathway.pixel_area_ha;
    let n_px = pathway.values.len();
    let restore: Vec<bool> = (0..n_px)
        .map(|i| !pathway.mask[i] && pathway.values[i] == RESTORE_CODE)
        .collect();
    if !restore.iter().any(|x| *x) {
        return Ok(na(
            "No area of this project falls under the Restore pathway, so ARR carbon removal cannot be estimated.",
        ));
    }
    let filled_int = |s: &crate::common::RasterSlice| -> Vec<i64> {
        s.values
            .iter()
            .zip(&s.mask)
            .map(|(v, m)| if *m { 0 } else { *v as i64 })
            .collect()
    };
    let catv = filled_int(&cat);
    let ecov = filled_int(&eco);
    let agbv: Vec<f64> = agb
        .values
        .iter()
        .zip(&agb.mask)
        .map(|(v, m)| if *m { 0.0 } else { *v })
        .collect();
    let agb_valid: Vec<bool> = agb.mask.iter().map(|m| !m).collect();

    let dryland_restore = (0..n_px).any(|i| restore[i] && ecov[i] == 1);
    let (zone_arr, zone_valid) = if dryland_restore {
        dryland_zone(settings, aoi, like)?
    } else {
        (vec![ARR_DRYLAND_DEFAULT_ZONE; n_px], vec![true; n_px])
    };

    let mut groups: Vec<Group> = Vec::new();
    let mut quantified = vec![false; n_px];
    let mut annual = vec![0.0f64; duration_years as usize];
    let mut agb_valid_quant_ha = 0.0;
    let mut gross_tco2e = 0.0;
    let mut clamped_ha = 0.0;
    let mut net_classbaseline_tco2e = 0.0;
    let mut net_gedi_tco2e = 0.0;
    for (cc, ec) in ARR_SEQ_PAIRS_SORTED {
        if ARR_CARBON_DEFERRED_ECO.contains(&ec) {
            continue;
        }
        let base_mask: Vec<bool> = (0..n_px)
            .map(|i| restore[i] && catv[i] == cc && ecov[i] == ec)
            .collect();
        if !base_mask.iter().any(|x| *x) {
            continue;
        }
        let subgroups: Vec<(Option<i64>, Vec<bool>)> = if ec == 1 {
            ARR_DRYLAND_ZONES
                .iter()
                .map(|(z, _)| {
                    (
                        Some(*z),
                        (0..n_px)
                            .map(|i| base_mask[i] && zone_arr[i] == *z)
                            .collect(),
                    )
                })
                .collect()
        } else {
            vec![(None, base_mask)]
        };
        for (zone_code, gmask) in subgroups {
            let n = gmask.iter().filter(|x| **x).count();
            if n == 0 {
                continue;
            }
            for (q, g) in quantified.iter_mut().zip(&gmask) {
                *q |= *g;
            }
            let area = n as f64 * pix;
            let (rate, r, cf) = arr_params(ec, zone_code)?;
            let conv = (1.0 + r) * cf * CO2_PER_C;
            let anr = ARR_ANR_PAIRS.contains(&(cc, ec));
            let stocking = if anr {
                ARR_STOCKING_ANR
            } else {
                ARR_STOCKING_PLANTING
            };
            let mode = if anr { "ANR/EMR" } else { "planting" };
            let accum = accum_co2e_per_ha(rate, r, cf, duration_years);
            let base_gedi: Vec<f64> = (0..n_px)
                .filter(|i| gmask[*i])
                .map(|i| agbv[i] * conv)
                .collect();
            let base_class = vec![arr_baseline_class_mgha(cc) * conv; n];
            // ARR_BASELINE_MODE == "class"
            let base_primary = &base_class;
            let net = clamped_sum(accum, base_primary) * stocking * pix;
            gross_tco2e += accum * stocking * area;
            net_gedi_tco2e += clamped_sum(accum, &base_gedi) * stocking * pix;
            net_classbaseline_tco2e += clamped_sum(accum, &base_class) * stocking * pix;
            clamped_ha += base_gedi.iter().filter(|b| **b >= accum).count() as f64 * pix;
            groups.push(Group {
                cat_code: cc,
                ecosystem: ec,
                ecosystem_label: crate::benefit::config::lookup(&PATHWAY_ECOSYSTEM_CODES, ec)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("eco {ec}")),
                zone_label: if ec == 1 {
                    zone_code
                        .and_then(|z| lookup(&ARR_DRYLAND_ZONES, z))
                        .unwrap_or("")
                        .to_string()
                } else {
                    String::new()
                },
                activity_mode: mode,
                stocking_factor: stocking,
                area_ha: area,
                net_tco2e: net,
                net_tco2e_per_ha: if area != 0.0 { net / area } else { 0.0 },
            });
            for (i, t) in (1..=duration_years).enumerate() {
                let accum_t = accum_co2e_per_ha(rate, r, cf, t);
                annual[i] += clamped_sum(accum_t, base_primary) * stocking * pix;
            }
            agb_valid_quant_ha +=
                (0..n_px).filter(|i| gmask[*i] && agb_valid[*i]).count() as f64 * pix;
        }
    }
    if groups.is_empty() {
        return Ok(na(
            "No Restore area carries an ARR activity eligible for carbon sequestration (planting or ANR on dryland or \
             mangrove), so ex-ante carbon removal cannot be estimated.",
        ));
    }
    let total = py_sum(groups.iter().map(|g| g.net_tco2e));
    let quantified_ha = py_sum(groups.iter().map(|g| g.area_ha));
    let deferred: Vec<bool> = (0..n_px).map(|i| restore[i] && !quantified[i]).collect();
    let savanna_ha = (0..n_px).filter(|i| deferred[*i] && ecov[*i] == 4).count() as f64 * pix;
    let peat_ha = (0..n_px)
        .filter(|i| deferred[*i] && ecov[*i] == PATHWAY_ECOSYSTEM_PEATLAND)
        .count() as f64
        * pix;
    let other_deferred_ha =
        deferred.iter().filter(|x| **x).count() as f64 * pix - savanna_ha - peat_ha;
    let zone_defaulted_ha = (0..n_px)
        .filter(|i| quantified[*i] && ecov[*i] == 1 && !zone_valid[*i])
        .count() as f64
        * pix;

    let mut notes: Vec<String> = Vec::new();
    let cov = safe_pct(agb_valid_quant_ha, quantified_ha);
    // ARR_BASELINE_MODE is "class": the per-pixel coverage note does not fire
    let _ = CARBON_COVERAGE_WARN_PCT;
    notes.push(format!(
        "5.3: baseline mode is '{ARR_BASELINE_MODE}'. The class-based values are PLACEHOLDERS (C4 {}, C5 {}, C6 {} \
         Mg/ha) pending references, so the total is indicative. For comparison, the per-pixel GEDI baseline gives {} \
         tCO2e and gross (no baseline) {}.",
        fmt_g(ARR_BASELINE_C4),
        fmt_g(ARR_BASELINE_C5),
        fmt_g(ARR_BASELINE_C6),
        fmt_grouped(net_gedi_tco2e, 0),
        fmt_grouped(gross_tco2e, 0)
    ));
    if savanna_ha > 0.0 {
        notes.push(format!(
            "5.3: {} of Restore area is savanna, whose carbon is deferred (recovery is mainly soil and roots). \
             Activity and benefits still apply.",
            fmt_ha(savanna_ha)
        ));
    }
    if peat_ha > 0.0 {
        notes.push(format!(
            "5.3: {} of Restore area is peatland, temporarily excluded from carbon quantification by team decision \
             (the biomass method and rates exist; remove peatland from ARR_CARBON_DEFERRED_ECO to re-enable). Activity \
             and benefits still apply.",
            fmt_ha(peat_ha)
        ));
    }
    if zone_defaulted_ha > 0.0 {
        notes.push(format!(
            "5.3: {} of dryland could not be zoned (missing elevation or precipitation) and defaulted to the \
             seasonal-lowland rate.",
            fmt_ha(zone_defaulted_ha)
        ));
    }
    if duration_years > ARR_OLD_END_YEAR {
        notes.push(format!(
            "5.3: the accumulation curve is defined only to year {ARR_OLD_END_YEAR}; the {} years beyond it add no \
             further removal.",
            duration_years - ARR_OLD_END_YEAR
        ));
    }
    notes.push(
        "5.3: the ANR/EMR stocking factor (0.8) and the planting-vs-ANR split are uncalibrated. Dryland zones use \
         elevation and 12-band monthly precipitation (units to verify), with a dry month defined as below 100 mm \
         (Walsh 1996)."
            .to_string(),
    );
    let total_low = total * ARR_UNCERTAINTY_LOW;
    let total_high = total * ARR_UNCERTAINTY_HIGH;
    let mut net_by_ecosystem: Vec<(String, f64)> = Vec::new();
    let mut area_by_ecosystem: Vec<(String, f64)> = Vec::new();
    let mut area_by_zone: Vec<(String, f64)> = Vec::new();
    let add = |m: &mut Vec<(String, f64)>, k: &str, v: f64| match m.iter_mut().find(|(n, _)| n == k)
    {
        Some(e) => e.1 += v,
        None => m.push((k.to_string(), 0.0 + v)),
    };
    for g in &groups {
        add(&mut net_by_ecosystem, &g.ecosystem_label, g.net_tco2e);
        add(&mut area_by_ecosystem, &g.ecosystem_label, g.area_ha);
        if !g.zone_label.is_empty() {
            add(&mut area_by_zone, &g.zone_label, g.area_ha);
        }
    }
    let to_obj = |m: Vec<(String, f64)>| -> Value {
        Value::Object(m.into_iter().map(|(k, v)| (k, json!(v))).collect())
    };
    let narrative = format!(
        "Restoring the eligible areas of this project could remove an estimated {} tCO2e over {duration_years} \
         years, with an indicative range of {} to {} tCO2e.",
        fmt_grouped(total, 0),
        fmt_grouped(total_low, 0),
        fmt_grouped(total_high, 0)
    );
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|a, b| (-groups[*a].net_tco2e).total_cmp(&-groups[*b].net_tco2e)); // stable
    let groups_sorted: Vec<Value> = order
        .iter()
        .map(|i| {
            let g = &groups[*i];
            json!({
                "cat_code": g.cat_code, "ecosystem": g.ecosystem, "ecosystem_label": g.ecosystem_label,
                "zone_label": g.zone_label, "activity_mode": g.activity_mode, "stocking_factor": g.stocking_factor,
                "area_ha": g.area_ha, "net_tco2e": g.net_tco2e, "net_tco2e_per_ha": g.net_tco2e_per_ha,
            })
        })
        .collect();
    let annual_rows: Vec<Value> = annual
        .iter()
        .enumerate()
        .map(|(t, v)| json!({"year": t as i64 + 1, "cumulative_tco2e": v}))
        .collect();
    let values = json!({
        "total_tco2e": total,
        "total_low_tco2e": total_low,
        "total_high_tco2e": total_high,
        "duration_years": duration_years,
        "quantified_ha": quantified_ha,
        "baseline_mode": ARR_BASELINE_MODE,
        "total_gross_tco2e": gross_tco2e,
        "net_perpixel_gedi_tco2e": net_gedi_tco2e,
        "net_classbaseline_tco2e": net_classbaseline_tco2e,
        "gedi_baseline_zeroed_ha": clamped_ha,
        "net_by_ecosystem_tco2e": to_obj(net_by_ecosystem),
        "area_by_ecosystem_ha": to_obj(area_by_ecosystem),
        "area_by_dryland_zone_ha": to_obj(area_by_zone),
        "agb_coverage_pct": cov,
        "zone_defaulted_ha": zone_defaulted_ha,
        "deferred_savanna_ha": savanna_ha,
        "deferred_peat_ha": peat_ha,
        "peat_excluded": true,
        "deferred_other_ha": other_deferred_ha,
        "method": "reference-rate / yield-curve, NBS-v3-ANX-B v2",
        "pools_included": ["aboveground biomass", "belowground biomass"],
        "pools_excluded": ["soil organic carbon", "peat soil", "dead wood", "litter", "avoided emissions"],
        "uncertainty": {"low": ARR_UNCERTAINTY_LOW, "high": ARR_UNCERTAINTY_HIGH},
    });
    // key order as the dict literal: applicable, narrative, the five values, projection, notes
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "total_tco2e",
        "total_low_tco2e",
        "total_high_tco2e",
        "duration_years",
        "quantified_ha",
    ] {
        view[k] = values[k].clone();
    }
    view["annual_projection"] = json!(annual_rows);
    view["notes"] = json!(notes);
    let results = json!({
        "narrative": narrative,
        "tables": {"annual_projection": annual_rows, "groups": groups_sorted},
        "values": values,
        "flags": [],
        "notes": notes,
    });
    Ok((results, view))
}

/// `f"{x:g}"` for the three class baselines (whole numbers).
fn fmt_g(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}
