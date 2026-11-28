//! 3.3 Mangrove Ecosystem (`threat/mangrove.py`): remaining mangrove, disturbance, the collapsed
//! drivers (Commodities 1-4, Settlement 5) and the main pressure.
//!
//! Unlike 3.2 the body works on plain arrays: `~np.ma.getmaskarray(x)` validity and `x.data`
//! comparisons, so no masked-array semantics leak. Every layer is checked against the ecosystem
//! grid by name.

use serde_json::{Value, json};

use crate::common::{
    Aoi, MaskedWindow, area_ha_by_row_loop, read_layer_masked, transforms_allclose,
};
use crate::config::{
    THREAT_COMMODITY_CLASSES, THREAT_CURRENT_FOREST, THREAT_DISTURBANCE,
    THREAT_DISTURBANCE_THRESHOLD, THREAT_DRIVERS, THREAT_ECOSYSTEM, THREAT_FOREST_2024,
    THREAT_HISTORICAL, THREAT_MANGROVE, THREAT_REMAINING_FOREST, THREAT_SETTLEMENT_CLASS,
    THREAT_STORM_RISK, THREAT_STORM_RISK_CLASSES,
};
use crate::py::round_ndigits;
use crate::settings::Settings;
use crate::{Error, Result};

/// `check_grid`
fn check_grid(reference: &MaskedWindow, data: &MaskedWindow, name: &str) -> Result<()> {
    if reference.shape() != data.shape() {
        return Err(Error::new(
            "ValueError",
            format!("Grid size mismatch: {name}"),
        ));
    }
    if !transforms_allclose(&reference.transform, &data.transform) {
        return Err(Error::new(
            "ValueError",
            format!("Grid alignment mismatch: {name}"),
        ));
    }
    Ok(())
}

/// `valid & pred(data)` per cell.
fn valid_where(m: &MaskedWindow, pred: impl Fn(f64) -> bool) -> Vec<bool> {
    m.data
        .iter()
        .zip(&m.mask)
        .map(|(d, masked)| !masked && pred(*d))
        .collect()
}

fn and(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b).map(|(x, y)| *x && *y).collect()
}

/// `x / total * 100 if total > 0 else 0`, rounded to 2.
fn pct(part: f64, total: f64) -> Value {
    if total > 0.0 {
        json!(round_ndigits(part / total * 100.0, 2))
    } else {
        json!(0)
    }
}

fn or_none_detected(list: Vec<&str>) -> Vec<&str> {
    if list.is_empty() {
        vec!["None detected"]
    } else {
        list
    }
}

/// `analyze_mangrove`
pub fn analyze_mangrove(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let read = |layer: &str| read_layer_masked(settings, layer, aoi);
    let ecosystem = read(THREAT_ECOSYSTEM)?;
    let historical = read(THREAT_HISTORICAL)?;
    let forest2024 = read(THREAT_FOREST_2024)?;
    let disturbance = read(THREAT_DISTURBANCE)?;
    let drivers = read(THREAT_DRIVERS)?;
    let storm = read(THREAT_STORM_RISK)?;
    check_grid(&ecosystem, &historical, "historical_deforestation_v3.tif")?;
    check_grid(&ecosystem, &forest2024, "forest_2024_v3.tif")?;
    check_grid(&ecosystem, &disturbance, "forest_disturbance_v3.tif")?;
    check_grid(&ecosystem, &drivers, "drivers_disturbance_v3.tif")?;
    check_grid(&ecosystem, &storm, "risk_storm_v3.tif")?;

    let (transform, width) = (ecosystem.transform, ecosystem.width);
    let area = |m: &[bool]| area_ha_by_row_loop(m, width, &transform);

    let mangrove_mask = valid_where(&ecosystem, |d| d == THREAT_MANGROVE);
    let total_mangrove_area = area(&mangrove_mask);
    let remaining_mask = and(
        &mangrove_mask,
        &valid_where(&historical, |d| d == THREAT_REMAINING_FOREST),
    );
    let remaining_area = area(&remaining_mask);
    let current_mangrove_mask = and(
        &mangrove_mask,
        &valid_where(&forest2024, |d| d == THREAT_CURRENT_FOREST),
    );
    let disturbed_mangrove_mask = and(
        &current_mangrove_mask,
        &valid_where(&disturbance, |d| d > THREAT_DISTURBANCE_THRESHOLD),
    );
    let disturbed_area = area(&disturbed_mangrove_mask);

    let commodities_mask = and(
        &disturbed_mangrove_mask,
        &valid_where(&drivers, |d| THREAT_COMMODITY_CLASSES.contains(&d)),
    );
    let settlement_mask = and(
        &disturbed_mangrove_mask,
        &valid_where(&drivers, |d| d == THREAT_SETTLEMENT_CLASS),
    );
    let commodities_area = area(&commodities_mask);
    let settlement_area = area(&settlement_mask);
    let any = |m: &[bool]| m.iter().any(|x| *x);

    let mut non_natural = Vec::new();
    if any(&commodities_mask) {
        non_natural.push("Commodities");
    }
    if any(&settlement_mask) {
        non_natural.push("Settlement");
    }
    let main_pressure = if commodities_area > settlement_area {
        "Commodities"
    } else if settlement_area > commodities_area {
        "Settlement"
    } else if commodities_area > 0.0 && settlement_area > 0.0 {
        "Commodities and Settlement"
    } else {
        "Not identified"
    };

    let storm_mask = and(
        &disturbed_mangrove_mask,
        &valid_where(&storm, |d| THREAT_STORM_RISK_CLASSES.contains(&d)),
    );
    let mut natural = Vec::new();
    if any(&storm_mask) {
        natural.push("Extreme climate event");
    }
    let known_driver_mask = and(
        &disturbed_mangrove_mask,
        &valid_where(&drivers, |d| (1..=5).any(|c| c as f64 == d)),
    );
    let other_mask: Vec<bool> = disturbed_mangrove_mask
        .iter()
        .zip(&known_driver_mask)
        .map(|(a, k)| *a && !k)
        .collect();
    let mut other = Vec::new();
    if any(&other_mask) {
        other.push("Other");
    }

    Ok(json!({"mangrove": {
        "total_area_ha": round_ndigits(total_mangrove_area, 2),
        "remaining_forest": {"area_ha": round_ndigits(remaining_area, 2), "percentage": pct(remaining_area, total_mangrove_area)},
        "disturbed": {"area_ha": round_ndigits(disturbed_area, 2), "percentage": pct(disturbed_area, total_mangrove_area)},
        "main_pressure": main_pressure,
        "drivers": {
            "non_natural": or_none_detected(non_natural),
            "natural": or_none_detected(natural),
            "other": or_none_detected(other),
        },
    }}))
}
