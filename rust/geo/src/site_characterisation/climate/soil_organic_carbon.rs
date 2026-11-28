//! Component 3.2 Soil Organic Carbon (`climate/soil_organic_carbon.py`).
//!
//! SOC in the top 30 cm, in tC: the sum of the three SoilGrids depth intervals (0-5, 5-15,
//! 15-30 cm, tC/ha) per pixel on one shared grid (`like=` the first), average resampling, nodata
//! as zero carbon, times the pixel area. No carbon fraction (already carbon). The peat caveat has
//! three states from 1.1's present set: peat present, no peat, not known (`None`).

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped, not_applicable};
use crate::config::{CARBON_COVERAGE_WARN_PCT, SOIL_CARBON_0_30_RASTERS, SOIL_CARBON_DEPTH_CM};
use crate::py::{fmt_grouped, np_sum, safe_pct, sentences};
use crate::settings::Settings;

const PEATLAND_CLASS: i64 = 3; // ecosystem code from 1.1

/// `analyze_soil_organic_carbon`; `ecosystem_present` is 1.1's Axis 3 set, None when General has
/// not run.
pub fn analyze_soil_organic_carbon(
    settings: &Settings,
    aoi: &Aoi,
    ecosystem_present: Option<&[i64]>,
) -> Result<(Value, Value)> {
    let first = load_raster_clipped(
        settings,
        SOIL_CARBON_0_30_RASTERS[0],
        aoi,
        Resampling::Average,
        1,
        None,
    )?;
    // `sum(lyr.values.filled(0.0) for lyr in layers)`: 0 + a0 + a1 + a2, element-wise, in order.
    let mut stock: Vec<f64> = first
        .values
        .iter()
        .zip(&first.mask)
        .map(|(v, m)| 0.0 + if *m { 0.0 } else { *v })
        .collect();
    for p in &SOIL_CARBON_0_30_RASTERS[1..] {
        let lyr =
            load_raster_clipped(settings, p, aoi, Resampling::Average, 1, Some(first.grid()))?;
        for ((s, v), m) in stock.iter_mut().zip(&lyr.values).zip(&lyr.mask) {
            *s += if *m { 0.0 } else { *v };
        }
    }
    let raster = &first; // grid, pixel area and coverage reference

    let total_tc = np_sum(&stock) * raster.pixel_area_ha;
    drop(stock);

    if total_tc <= 0.0 {
        let (narrative, flags, missing) = not_applicable(
            "No soil carbon data is available for this project area, so soil organic carbon cannot be estimated.",
        );
        let results = json!({"narrative": narrative, "tables": {}, "values": {}, "flags": flags, "missing": missing});
        return Ok((results, json!({"soil_organic_carbon_number": null})));
    }

    let density_tc_ha = if aoi.area_ha > 0.0 {
        total_tc / aoi.area_ha
    } else {
        0.0
    };
    let coverage_pct = safe_pct(raster.valid_area_ha(), aoi.area_ha);

    let mut flags: Vec<String> = Vec::new();
    if coverage_pct < CARBON_COVERAGE_WARN_PCT {
        flags.push(format!(
            "3.2: the soil carbon raster covers only {coverage_pct:.0}% of the AOI. Nodata is \
             counted as zero carbon, so the headline is an under-estimate by an unknown amount."
        ));
    }

    // Peat caveat. Depends on 1.1, so the three states are: peat present, no peat, unknown.
    let has_peat = ecosystem_present.map(|set| set.contains(&PEATLAND_CLASS));
    let mut peat_clause = String::new();
    match has_peat {
        None => flags.push(
            "3.2: the General stage has not been run, so the AOI could not be checked for \
             peatland. If peat is present, this figure understates the soil stock heavily."
                .into(),
        ),
        Some(true) => {
            peat_clause = format!(
                "Part of this project area is peatland, where peat can extend well below \
                 {SOIL_CARBON_DEPTH_CM} cm, so the real soil carbon stock is likely to be \
                 considerably higher."
            );
            flags.push(format!(
                "3.2: peatland present. The raster represents the top {SOIL_CARBON_DEPTH_CM} cm \
                 only, so this is a lower bound on soil carbon, not an estimate of it."
            ));
        }
        Some(false) => {}
    }

    let narrative = sentences(&[
        &format!(
            "The soil in this project area holds approximately {} tC of organic carbon in the top \
             {SOIL_CARBON_DEPTH_CM} cm, an average of {} tC per hectare.",
            fmt_grouped(total_tc, 0),
            fmt_grouped(density_tc_ha, 0)
        ),
        &peat_clause,
    ]);

    let results = json!({
        "narrative": narrative,
        "tables": {},
        "values": {
            "total_tc": total_tc,
            "density_tc_ha": density_tc_ha,
            "unit": "tC",
            "depth_cm": SOIL_CARBON_DEPTH_CM,
            "coverage_pct": coverage_pct,
            "peatland_present": has_peat,
        },
        "flags": flags,
    });
    Ok((results, json!({"soil_organic_carbon_number": total_tc})))
}
