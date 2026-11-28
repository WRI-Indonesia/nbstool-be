//! Endangered tree species richness (`nature/endangered_trees.py`).
//!
//! Not a notebook component: v2's `get_nature_richness_data` logic. The mean richness of the
//! AOI's valid pixels of `tree_species_v3.tif` (read with average resampling), reported as
//! `round(mean)`. The number is a typical per-point richness, not a distinct-species count.

use serde_json::{Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped};
use crate::config::TREE_SPECIES_RASTER;
use crate::py::round_int;
use crate::settings::Settings;

/// `analyze_endangered_trees`
pub fn analyze_endangered_trees(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let raster = load_raster_clipped(
        settings,
        TREE_SPECIES_RASTER,
        aoi,
        Resampling::Average,
        1,
        None,
    )?;

    let Some(mean_richness) = raster.ma_mean() else {
        let results = json!({
            "narrative": "No endangered tree species data covers this project area.",
            "tables": {},
            "values": {"endangered_tree_richness": null, "covered_ha": 0.0},
            "flags": [],
        });
        return Ok((results, json!({"endangered_tree_number_of_species": null})));
    };
    let reported = round_int(mean_richness);

    let results = json!({
        // v2's wording, kept so the two backends do not describe the same number differently.
        "narrative": format!("The project area is home to {reported} of endangered trees."),
        "tables": {},
        "values": {
            "endangered_tree_richness": mean_richness,
            "endangered_tree_reported": reported,
            "covered_ha": raster.valid_area_ha(),
        },
        "flags": [],
    });
    Ok((
        results,
        json!({"endangered_tree_number_of_species": reported}),
    ))
}
