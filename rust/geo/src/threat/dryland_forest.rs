//! 3.2 Dryland Forest Ecosystem (`threat/dryland_forest.py`): remaining forest, disturbance, loss
//! and gain, and which drivers are present.
//!
//! Every layer is read on its own EPSG:4326 grid (`mask(crop=True, filled=False)`), and the body
//! works on numpy MASKED arrays: comparisons with `==` are False under the mask, `&` keeps the raw
//! data and ors the masks, `drivers[disturbed]` and `natural_presence |= ...` use the RAW data,
//! `np.any` / `np.ma.filled` honour the mask. `MaskedWindow` / `MaBool` reproduce exactly that.

use serde_json::{Value, json};

use crate::Result;
use crate::common::Aoi;
use crate::common::{MaBool, area_ha_by_row_loop, read_layer_masked, unique_unmasked_at};
use crate::config::{
    THREAT_CURRENT_FOREST, THREAT_DISTURBANCE, THREAT_DRIVERS, THREAT_DRYLAND, THREAT_ECOSYSTEM,
    THREAT_FOREST_2024, THREAT_FOREST_DRIVER_CLASSES, THREAT_FOREST_GAIN, THREAT_FOREST_GAIN_VALUE,
    THREAT_FOREST_LOSS, THREAT_HISTORICAL, THREAT_NATURAL_DRIVERS, THREAT_REMAINING_FOREST,
};
use crate::py::round_ndigits;
use crate::settings::Settings;

/// `round(percentage(area), 2)`: `percentage` returns the int 0 when the total is 0.
fn pct(area: f64, total: f64) -> Value {
    if total == 0.0 {
        json!(0)
    } else {
        json!(round_ndigits(area / total * 100.0, 2))
    }
}

/// `analyze_dryland_forest`
pub fn analyze_dryland_forest(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let read = |layer: &str| read_layer_masked(settings, layer, aoi);
    let eco = read(THREAT_ECOSYSTEM)?;
    let transform = eco.transform;
    let historical = read(THREAT_HISTORICAL)?;
    let forest2024 = read(THREAT_FOREST_2024)?;
    let disturbance = read(THREAT_DISTURBANCE)?;
    let gain = read(THREAT_FOREST_GAIN)?;
    let drivers = read(THREAT_DRIVERS)?;

    let dryland = eco.eq(THREAT_DRYLAND);
    drop(eco);
    let remaining = dryland.and(&historical.eq(THREAT_REMAINING_FOREST))?;
    let forest_loss = dryland.and(&historical.eq(THREAT_FOREST_LOSS))?;
    drop(historical);
    let current_forest = dryland.and(&forest2024.eq(THREAT_CURRENT_FOREST))?;
    drop(forest2024);
    let disturbed = current_forest.and(&disturbance.gt(0.0))?;
    drop(disturbance);
    let forest_gain = dryland.and(&gain.eq(THREAT_FOREST_GAIN_VALUE))?;
    drop(gain);

    let width = dryland.shape.1;
    let area = |m: &MaBool| area_ha_by_row_loop(&m.filled_false(), width, &transform);
    let total_area = area(&dryland);
    let remaining_area = area(&remaining);
    let disturbed_area = area(&disturbed);
    let loss_area = area(&forest_loss);
    let gain_area = area(&forest_gain);

    // Non-natural + explicit other forest drivers
    let (mut non_natural, mut other) = (Vec::<&str>::new(), Vec::<&str>::new());
    for value in unique_unmasked_at(&drivers, &disturbed.data) {
        let value = value as i64; // int(value)
        let Some((_, driver_name)) = THREAT_FOREST_DRIVER_CLASSES
            .iter()
            .find(|(c, _)| *c == value)
        else {
            continue;
        };
        if *driver_name == "Non-productive conversion" {
            other.push(driver_name)
        } else {
            non_natural.push(driver_name)
        }
    }

    // Natural drivers
    let mut natural = Vec::<&str>::new();
    let mut natural_presence = vec![false; disturbed.data.len()];
    for (driver_name, raster, values) in THREAT_NATURAL_DRIVERS {
        let driver_raster = read(raster)?;
        let driver_mask =
            disturbed.and_plain(&driver_raster.isin_raw(values), driver_raster.shape())?;
        if driver_mask.any() {
            natural.push(driver_name);
            for (p, d) in natural_presence.iter_mut().zip(&driver_mask.data) {
                *p |= *d; // `|=` into a plain array: raw data
            }
        }
    }

    // Unknown driver
    let known_codes: Vec<f64> = THREAT_FOREST_DRIVER_CLASSES
        .iter()
        .map(|(c, _)| *c as f64)
        .collect();
    let known_forest_driver =
        disturbed.and_plain(&drivers.isin_raw(&known_codes), drivers.shape())?;
    let not_natural: Vec<bool> = natural_presence.iter().map(|p| !p).collect();
    let unknown = disturbed
        .and(&known_forest_driver.not())?
        .and_plain(&not_natural, disturbed.shape)?;
    if unknown.any() {
        other.push("Unknown");
    }

    Ok(json!({
        "total_area_ha": round_ndigits(total_area, 2),
        "remaining_forest": {"area_ha": round_ndigits(remaining_area, 2), "percentage": pct(remaining_area, total_area)},
        "disturbed": {"area_ha": round_ndigits(disturbed_area, 2), "percentage": pct(disturbed_area, total_area)},
        "forest_loss": {"area_ha": round_ndigits(loss_area, 2), "percentage": pct(loss_area, total_area)},
        "forest_gain": {"area_ha": round_ndigits(gain_area, 2), "percentage": pct(gain_area, total_area)},
        "drivers": {"non_natural": non_natural, "natural": natural, "other": other},
    }))
}
