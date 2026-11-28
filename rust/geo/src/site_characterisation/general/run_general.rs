//! The component seams of `general/run_general.py` that change a component's output.
//! (The NDJSON streaming and thread pool belong to the HTTP layer, not to this crate.)

use serde_json::Value;

use super::ecosystem_type::analyze_ecosystem_type;
use crate::Result;
use crate::common::Aoi;
use crate::py::fsum_py;
use crate::settings::Settings;

/// `_ecosystem_on_site_area`: 1.1 with the CARD areas rescaled to sum to the site area when the
/// pixels overcount it. `results` is untouched.
pub fn ecosystem_on_site_area(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let (results, mut view) = analyze_ecosystem_type(settings, aoi)?;
    if let Some(cards) = view["ecosystems"].as_array_mut() {
        let total = fsum_py(cards.iter().map(|c| c["area"].as_f64().unwrap_or(0.0)));
        if total > aoi.area_ha {
            let scale = aoi.area_ha / total;
            for c in cards.iter_mut() {
                let area = c["area"].as_f64().unwrap_or(0.0) * scale;
                c["area"] = area.into();
                // min: a sole class scales to 100.00000000000001, and the card colours `> 100` red.
                let pct = 100.0f64.min(c["percentage"].as_f64().unwrap_or(0.0) * scale);
                c["percentage"] = pct.into();
            }
        }
    }
    Ok((results, view))
}
