//! Component 2.1 Forest Landscape Integrity (`nature/forest_landscape_integrity.py`): the mean
//! FLII score over forest cells (bilinear warp) and the High / Medium / Low shares from the class
//! raster (nearest, `like=` the score grid), which is also the forest mask.

use serde_json::{Value, json};

use crate::common::{
    Aoi, ClassShare, RasterSlice, Resampling, dominant, forest_mask_2024, load_raster_clipped,
    not_applicable, tabulate_classes,
};
use crate::py::{fmt_ha, fmt_pct, sentences};
use crate::settings::Settings;
use crate::{Error, Result};

const FLII_FOREST_RASTER: &str = "flii_v3.tif";
const FLII_CLASS_RASTER: &str = "flii_class_v3.tif";
const FLII_CLASSES: [(i64, &str); 3] = [(1, "Low"), (2, "Medium"), (3, "High")];
const FLII_NONFOREST_FLAG_PCT: f64 = 5.0;
const FLII_LOW: i64 = 1;
const FLII_MEDIUM: i64 = 2;
const FLII_HIGH: i64 = 3;

fn gloss(code: i64) -> &'static str {
    match code {
        FLII_HIGH => "indicating largely intact and well-connected forest under low human pressure",
        FLII_MEDIUM => {
            "indicating moderately modified forest with some fragmentation or human pressure"
        }
        _ => "indicating heavily modified and fragmented forest under high human pressure",
    }
}

fn empty(reason: &str) -> (Value, Value) {
    let (narrative, flags, missing) = not_applicable(reason);
    (
        json!({"narrative": narrative, "tables": {"integrity": []}, "values": {}, "flags": flags, "missing": missing}),
        json!({"flii_score": null, "high_integrity_percentage": null, "medium_integrity_percentage": null,
               "low_integrity_percentage": null, "dominant_integrity_class": null}),
    )
}

/// `analyze_flii`
pub fn analyze_flii(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    if forest_mask_2024(settings, aoi, None)?.is_empty() {
        return Ok(empty(
            "No forest is present in this project area, so landscape integrity cannot be assessed.",
        ));
    }
    let score = load_raster_clipped(
        settings,
        FLII_FOREST_RASTER,
        aoi,
        Resampling::Bilinear,
        1,
        None,
    )?;
    let classes = load_raster_clipped(
        settings,
        FLII_CLASS_RASTER,
        aoi,
        Resampling::Nearest,
        1,
        Some(score.grid()),
    )?;
    let forest_score = RasterSlice {
        mask: score
            .mask
            .iter()
            .zip(&classes.mask)
            .map(|(a, b)| *a || *b)
            .collect(),
        ..score.clone()
    };
    let forest_area_ha = classes.valid_area_ha();
    if forest_area_ha <= 0.0 || forest_score.valid_count() == 0 {
        return Ok(empty(
            "The forest integrity layer does not cover the forest in this project area.",
        ));
    }
    let mean_flii = forest_score.ma_mean().unwrap_or(f64::NAN);
    drop(forest_score);
    let mean_flii_aoi = score.ma_mean().unwrap_or(f64::NAN);
    let rows = tabulate_classes(&classes, &FLII_CLASSES, forest_area_ha);
    let pct_of = |code: i64| {
        rows.iter()
            .find(|r| r.code == crate::common::Code::Int(code))
            .map_or(0.0, |r| r.pct)
    };
    let Some(dom) = dominant(&rows) else {
        return Err(Error::new(
            "AttributeError",
            "'NoneType' object has no attribute 'label'",
        ));
    };
    let dom_code = match dom.code {
        crate::common::Code::Int(c) => c,
        crate::common::Code::Str(_) => 0,
    };
    let narrative = sentences(&[
        &format!(
            "Of the forest in this area, {} has high landscape integrity, {} medium, and {} low.",
            fmt_pct(pct_of(FLII_HIGH), 0),
            fmt_pct(pct_of(FLII_MEDIUM), 0),
            fmt_pct(pct_of(FLII_LOW), 0)
        ),
        &format!(
            "The forest is predominantly {} integrity, {}.",
            dom.label.to_lowercase(),
            gloss(dom_code)
        ),
    ]);
    let mut flags = Vec::new();
    let nonforest_ha = score.valid_area_ha() - forest_area_ha;
    if nonforest_ha > forest_area_ha * FLII_NONFOREST_FLAG_PCT / 100.0 {
        flags.push(format!(
            "The FLII score raster carries {} of non-forest inside this AOI, stored as 0. Those cells are excluded from \
             the headline mean. Including them would give {mean_flii_aoi:.2} instead of {mean_flii:.2}.",
            fmt_ha(nonforest_ha)
        ));
    }
    let results = json!({
        "narrative": narrative,
        "tables": {"integrity": rows.iter().map(ClassShare::to_json).collect::<Vec<_>>()},
        "values": {
            "mean_flii": mean_flii,
            "dominant_class": dom.code.to_json(),
            "pct_high": pct_of(FLII_HIGH),
            "forest_area_ha": forest_area_ha,
            "mean_flii_score_extent": mean_flii_aoi,
            "score_extent_ha": score.valid_area_ha(),
        },
        "flags": flags,
    });
    let view_results = json!({
        "flii_score": mean_flii,
        "high_integrity_percentage": pct_of(FLII_HIGH),
        "medium_integrity_percentage": pct_of(FLII_MEDIUM),
        "low_integrity_percentage": pct_of(FLII_LOW),
        "dominant_integrity_class": dom.label,
    });
    Ok((results, view_results))
}
