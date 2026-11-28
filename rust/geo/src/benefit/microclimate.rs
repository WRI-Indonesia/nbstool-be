//! 5.13 Microclimate regulation (`benefit/microclimate.py`), on 5.11's pathway grid.

use serde_json::{Value, json};

use super::biodiversity_uplift::{read_like, restore_grid};
use super::config::{
    BENEFIT_ECOSYSTEM_CLASSES, ECOSYSTEM_NAMES, FOREST_CHANGE_RASTER, FOREST_CHANGE_YEARS,
    GAIN_CODE, lookup,
};
use super::util::na;
use crate::common::Aoi;
use crate::config::THREAT_ECOSYSTEM;
use crate::py::fmt_grouped;
use crate::settings::Settings;
use crate::{Error, Result};

/// Component 5.13.
pub fn analyze_microclimate(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
    ecosystem_class: i64,
) -> Result<(Value, Value)> {
    let rg = restore_grid(settings, aoi)?;
    let grid = &rg.grid;
    let change = read_like(grid, &settings.layer_path(FOREST_CHANGE_RASTER)?)?;
    let ecosystem = read_like(grid, &settings.layer_path(THREAT_ECOSYSTEM)?)?;
    let restore_ecosystem: Vec<bool> = (0..ecosystem.len())
        .map(|i| rg.restore[i] && grid.inside[i] && ecosystem[i] == ecosystem_class as f64)
        .collect();
    let restore_ecosystem_ha = grid.area_where(&restore_ecosystem);
    if restore_ecosystem_ha <= 0.0 {
        return Ok(na(&format!(
            "This project area has no Restore-pathway {} area, so there is no tree-cover gain to project.",
            lookup(&ECOSYSTEM_NAMES, ecosystem_class).unwrap_or("target")
        )));
    }
    let gain: Vec<bool> = (0..change.len())
        .map(|i| restore_ecosystem[i] && change[i] == GAIN_CODE)
        .collect();
    let gain_10yr_ha = grid.area_where(&gain);
    let gain_rate = gain_10yr_ha / restore_ecosystem_ha / FOREST_CHANGE_YEARS;
    let d = duration_years as f64;
    let projected_gain_ha = f64::min(gain_rate * d * restore_ecosystem_ha, restore_ecosystem_ha);
    // `min(x, 100)` keeps the int 100 when it wins; float() of it is 100.0 either way
    let projected_gain_pct = f64::min(gain_rate * d * 100.0, 100.0);
    let label = lookup(&BENEFIT_ECOSYSTEM_CLASSES, ecosystem_class)
        .ok_or_else(|| Error::new("KeyError", ecosystem_class.to_string()))?;
    let narrative = format!(
        "Restoring this {label} ecosystem, with an estimated {} ha ({projected_gain_pct:.1}%) of tree cover increase, \
         could help regulate the local microclimate by moderating temperatures through shading and \
         evapotranspiration and buffering the surrounding area against heat extremes over the project's \
         {duration_years}-year duration.",
        fmt_grouped(projected_gain_ha, 2)
    );
    let values = json!({
        "duration_years": duration_years,
        "ecosystem": label,
        "restore_ecosystem_ha": restore_ecosystem_ha,
        "gain_10yr_ha": gain_10yr_ha,
        "gain_rate_pct_per_year": gain_rate * 100.0,
        "projected_gain_ha": projected_gain_ha,
        "projected_gain_pct": projected_gain_pct,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "ecosystem",
        "duration_years",
        "restore_ecosystem_ha",
        "projected_gain_ha",
        "projected_gain_pct",
    ] {
        view[k] = values[k].clone();
    }
    Ok((
        json!({"narrative": narrative, "tables": {}, "values": values, "flags": []}),
        view,
    ))
}
