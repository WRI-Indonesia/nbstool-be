//! Component 1.5 Historical Deforestation, 2014 to 2024 (`general/historical_deforestation.py`).
//!
//! Gross loss of 2014 forest (A2 = A1 - loss_ha, gain not netted off) and the annual rate after
//! Puyravaud (2003): rate = (1 / (t2 - t1)) * ln(A2 / A1) * 100. No loss -> a dedicated sentence;
//! total loss (A2 == 0) -> `rate_pct = None` rather than an infinite rate.

use serde_json::{Value, json};

use crate::common::{Aoi, forest_mask_2014, forest_mask_2024};
use crate::config::{DEFOR_PERIOD_YEARS, DEFOR_YEAR_END, DEFOR_YEAR_START};
use crate::py::{fmt_ha, safe_pct};
use crate::settings::Settings;
use crate::{Error, Result};

/// `analyze_historical_deforestation`
pub fn analyze_historical_deforestation(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let f2014 = forest_mask_2014(settings, aoi)?;
    let f2024 = forest_mask_2024(settings, aoi, None)?;

    if f2014.is_empty() {
        let results = json!({
            "narrative": format!(
                "No forest was present in this project area in {DEFOR_YEAR_START}, so a \
                 deforestation rate cannot be calculated."
            ),
            "tables": {},
            "values": {"forest_2014_ha": 0.0, "forest_2024_ha": f2024.area_ha,
                       "loss_ha": 0.0, "rate_pct": null},
            "flags": [],
        });
        let view_results = json!({
            "historical_deforestation_year_start": DEFOR_YEAR_START,
            "historical_deforestation_year_end": DEFOR_YEAR_END,
            "historical_deforestation_percentage": null,
            "historical_deforestation_total_area": 0.0,
        });
        return Ok((results, view_results));
    }

    let a1 = f2014.area_ha;
    // Gross loss of 2014 forest. Gain elsewhere is deliberately not netted off.
    // `f2014.mask & ~f2024.mask` is element-wise: numpy needs equal shapes, not equal grids.
    if (f2014.height, f2014.width) != (f2024.height, f2024.width) {
        return Err(Error::new(
            "ValueError",
            format!(
                "operands could not be broadcast together with shapes ({},{}) ({},{}) ",
                f2014.height, f2014.width, f2024.height, f2024.width
            ),
        ));
    }
    let lost = f2014
        .mask
        .iter()
        .zip(&f2024.mask)
        .filter(|(a, b)| **a && !**b)
        .count();
    let loss_ha = lost as f64 * f2014.pixel_area_ha;
    let a2 = a1 - loss_ha;

    let (rate_pct, narrative) = if loss_ha <= 0.0 {
        (
            Some(0.0),
            format!(
                "No forest loss was detected in this project area between {DEFOR_YEAR_START} and \
                 {DEFOR_YEAR_END}."
            ),
        )
    } else if a2 <= 0.0 {
        (
            None, // ln(0) guard
            format!(
                "Between {DEFOR_YEAR_START} and {DEFOR_YEAR_END}, the project area lost all of its \
                 {} of forest.",
                fmt_ha(a1)
            ),
        )
    } else {
        let rate = ((1.0 / DEFOR_PERIOD_YEARS as f64) * (a2 / a1).ln() * 100.0).abs();
        (
            Some(rate),
            format!(
                "Between {DEFOR_YEAR_START} and {DEFOR_YEAR_END}, the project area lost \
                 {} of forest, an average of {rate:.1}% per year.",
                fmt_ha(loss_ha)
            ),
        )
    };

    let results = json!({
        "narrative": narrative,
        "tables": {},
        "values": {
            "forest_2014_ha": a1,
            "forest_2024_ha": a2,
            "loss_ha": loss_ha,
            "rate_pct": rate_pct,
            "loss_pct_of_2014_forest": safe_pct(loss_ha, a1),
            "forest_2014_pct_of_aoi": safe_pct(a1, aoi.area_ha),
        },
        "flags": [],
    });

    let view_results = json!({
        "historical_deforestation_year_start": DEFOR_YEAR_START,
        "historical_deforestation_year_end": DEFOR_YEAR_END,
        "historical_deforestation_percentage": rate_pct,
        "historical_deforestation_total_area": loss_ha,
    });
    Ok((results, view_results))
}
