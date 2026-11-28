//! Component 3.1 Current Carbon Storage (`climate/current_carbon_storage.py`).
//!
//! Biomass carbon stored in the project area, in tC, split into aboveground and belowground
//! pools. `agbd_v3.tif` is dry biomass density (Mg/ha), read with average resampling; density
//! times pixel area gives Mg, times CARBON_FRACTION (0.47) gives tC. Nodata inside the AOI counts
//! as zero biomass (team decision); coverage is flagged below CARBON_COVERAGE_WARN_PCT. BGB is
//! DERIVED as AGB x ROOT_TO_SHOOT_RATIO (0.28), so the pool split is constant by construction.

use serde_json::{Map, Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped, not_applicable};
use crate::config::{AGB_RASTER, CARBON_COVERAGE_WARN_PCT, CARBON_FRACTION, ROOT_TO_SHOOT_RATIO};
use crate::py::{fmt_grouped, fmt_pct, fsum_py, oxford_join, safe_pct};
use crate::settings::Settings;

const CARBON_POOLS: [&str; 2] = ["Aboveground biomass", "Belowground biomass"];

/// One biomass pool integrated over the AOI (`CarbonPool`).
#[derive(Clone)]
struct CarbonPool {
    name: &'static str,
    biomass_mg: f64,
    storage_tc: f64,
    coverage_pct: f64,
    pct: f64,
}

impl CarbonPool {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "biomass_mg": self.biomass_mg, "storage_tc": self.storage_tc,
               "coverage_pct": self.coverage_pct, "pct": self.pct})
    }
}

/// `_integrate_pool`: density times area, so the pixel area cancels the per hectare unit.
fn integrate_pool(
    settings: &Settings,
    name: &'static str,
    path: &str,
    aoi: &Aoi,
) -> Result<CarbonPool> {
    let raster = load_raster_clipped(settings, path, aoi, Resampling::Average, 1, None)?;
    // Team decision: nodata inside the AOI counts as zero biomass.
    let biomass_mg = raster.filled_sum() * raster.pixel_area_ha;
    let storage_tc = biomass_mg * CARBON_FRACTION;
    Ok(CarbonPool {
        name,
        biomass_mg,
        storage_tc,
        coverage_pct: safe_pct(raster.valid_area_ha(), aoi.area_ha),
        pct: 0.0,
    })
}

/// `analyze_current_carbon_storage`
pub fn analyze_current_carbon_storage(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let agb_pool = integrate_pool(settings, CARBON_POOLS[0], AGB_RASTER, aoi)?;
    let bgb_pool = CarbonPool {
        name: CARBON_POOLS[1],
        biomass_mg: agb_pool.biomass_mg * ROOT_TO_SHOOT_RATIO,
        storage_tc: agb_pool.storage_tc * ROOT_TO_SHOOT_RATIO,
        coverage_pct: agb_pool.coverage_pct,
        pct: 0.0,
    };
    let mut pools = vec![agb_pool, bgb_pool];
    let total_tc = fsum_py(pools.iter().map(|p| p.storage_tc));

    if total_tc <= 0.0 {
        let (narrative, flags, missing) = not_applicable(
            "No biomass data is available for this project area, so current carbon storage cannot be estimated.",
        );
        let results = json!({"narrative": narrative, "tables": {"pools": []}, "values": {},
                             "flags": flags, "missing": missing});
        return Ok((
            results,
            json!({"above_ground_biomass_number": null, "below_ground_biomass_number": null}),
        ));
    }

    for p in pools.iter_mut() {
        p.pct = safe_pct(p.storage_tc, total_tc);
    }
    let density_tc_ha = if aoi.area_ha > 0.0 {
        total_tc / aoi.area_ha
    } else {
        0.0
    };
    let coverage_pct = pools
        .iter()
        .map(|p| p.coverage_pct)
        .fold(f64::NEG_INFINITY, f64::max);

    let mut flags: Vec<String> = Vec::new();
    if coverage_pct < CARBON_COVERAGE_WARN_PCT {
        flags.push(format!(
            "3.1: the biomass rasters cover only {coverage_pct:.0}% of the AOI. Nodata is \
             counted as zero biomass, so the headline is an under-estimate by an unknown amount."
        ));
    }
    // `{ROOT_TO_SHOOT_RATIO:g}`: 0.28 prints the same under %g and Rust's shortest form.
    let notes = vec![format!(
        "3.1: belowground biomass is derived as {ROOT_TO_SHOOT_RATIO} x aboveground, not \
         mapped, so the aboveground / belowground split is constant by construction and is not a \
         site-specific result."
    )];

    let parts: Vec<String> = pools
        .iter()
        .map(|p| {
            format!(
                "{} holds {} tC ({})",
                p.name.to_lowercase(),
                fmt_grouped(p.storage_tc, 0),
                fmt_pct(p.pct, 0)
            )
        })
        .collect();
    let narrative = format!(
        "This project area currently stores approximately {} tC in aboveground and belowground \
         biomass, an average of {} tC per hectare. Of this, {}. Soil organic carbon is not included.",
        fmt_grouped(total_tc, 0),
        fmt_grouped(density_tc_ha, 0),
        oxford_join(&parts)
    );

    let mut pool_tc = Map::new();
    let mut pool_pct = Map::new();
    for p in &pools {
        pool_tc.insert(p.name.into(), json!(p.storage_tc));
        pool_pct.insert(p.name.into(), json!(p.pct));
    }
    let results = json!({
        "narrative": narrative,
        "tables": {"pools": pools.iter().map(CarbonPool::to_json).collect::<Vec<_>>()},
        "values": {
            "total_tc": total_tc,
            "density_tc_ha": density_tc_ha,
            "unit": "tC",
            "coverage_pct": coverage_pct,
            "pool_tc": pool_tc,
            "pool_pct": pool_pct,
            "pools_included": CARBON_POOLS,
            "pools_excluded": ["deadwood", "litter", "soil organic carbon"],
            "bgb_derived_root_to_shoot": ROOT_TO_SHOOT_RATIO,
        },
        "flags": flags,
        "notes": notes,
    });
    let view_results = json!({
        "above_ground_biomass_number": pools[0].storage_tc,
        "below_ground_biomass_number": pools[1].storage_tc,
    });
    Ok((results, view_results))
}
