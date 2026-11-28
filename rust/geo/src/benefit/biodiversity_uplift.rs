//! 5.11 Potential biodiversity uplift (`benefit/biodiversity_uplift.py`). Works on the pathway
//! raster's own grid (`mask(crop=True)` reference, WarpedVRT for everything else).

use serde_json::{Value, json};

use super::config::{
    DEGRADATION_CODE, FOREST_CHANGE_RASTER, FOREST_CHANGE_YEARS, GAIN_CODE, RESTORE_CODE, iucn_map,
};
use super::inventory::{Species, candidates};
use super::util::{NativeGrid, na, nargsort_desc, np_round, pool_map, read_vrt_filled0};
use crate::Result;
use crate::common::{Aoi, Resampling};
use crate::config::{AOH_RASTER_ROOT, PATHWAY_RASTER};
use crate::py::np_sum;
use crate::settings::Settings;

/// `read_reference(pathway, aoi, band=1)` + `inside_aoi` + `restore`: the grid 5.11 and 5.13 share.
pub struct RestoreGrid {
    pub grid: NativeGrid,
    pub restore: Vec<bool>,
}

pub fn restore_grid(settings: &Settings, aoi: &Aoi) -> Result<RestoreGrid> {
    let (grid, data, mask) = NativeGrid::masked(settings, PATHWAY_RASTER, aoi, 1)?;
    let restore = (0..data.len())
        .map(|i| grid.inside[i] && !mask[i] && data[i] == RESTORE_CODE)
        .collect();
    Ok(RestoreGrid { grid, restore })
}

/// `read_like(path, transform, crs, shape)`: WarpedVRT onto the crop grid, nearest, filled(0).
pub fn read_like(grid: &NativeGrid, path: &str) -> Result<Vec<f64>> {
    Ok(read_vrt_filled0(path, &grid.crop_grid(), 1, None, Resampling::Nearest)?.0)
}

struct Row {
    species: Option<String>,
    iucn: Option<&'static str>,
    habitat_aoi_ha: f64,
    restoration_overlap_ha: f64,
    habitat_gain_ha: f64,
    habitat_gain_pct: f64,
}

impl Row {
    fn to_json(&self) -> Value {
        json!({
            "species": self.species, "iucn": self.iucn, "habitat_aoi_ha": self.habitat_aoi_ha,
            "restoration_overlap_ha": self.restoration_overlap_ha, "habitat_gain_ha": self.habitat_gain_ha,
            "habitat_gain_pct": self.habitat_gain_pct,
        })
    }
}

/// Component 5.11.
pub fn analyze_biodiversity_uplift(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
    workers: usize,
) -> Result<(Value, Value)> {
    let RestoreGrid { grid, restore } = restore_grid(settings, aoi)?;
    let restore_ha = grid.area_where(&restore);
    if restore_ha <= 0.0 {
        return Ok(na(
            "No area of this project falls under the Restore pathway, so there is no restoration to project a \
             biodiversity uplift from.",
        ));
    }
    let change = read_like(&grid, &settings.layer_path(FOREST_CHANGE_RASTER)?)?;
    let gain: Vec<bool> = (0..change.len())
        .map(|i| restore[i] && change[i] == GAIN_CODE)
        .collect();
    let degradation: Vec<bool> = (0..change.len())
        .map(|i| restore[i] && change[i] == DEGRADATION_CODE)
        .collect();
    let gain_10yr_ha = grid.area_where(&gain);
    let degradation_10yr_ha = grid.area_where(&degradation);
    let gain_rate = gain_10yr_ha / restore_ha / FOREST_CHANGE_YEARS;
    let degradation_rate = degradation_10yr_ha / restore_ha / FOREST_CHANGE_YEARS;
    let condition_uplift = f64::min(1.0, (gain_rate + degradation_rate) * duration_years as f64);
    let biodiversity_uplift_ha = restore_ha * condition_uplift;

    let species = candidates(settings, aoi)?;
    let root = settings.layer_path(AOH_RASTER_ROOT)?;
    let rows: Vec<Option<Row>> = pool_map(&species, workers, |sp: &Species| {
        let habitat = read_like(&grid, &format!("{root}/{}", sp.raster_path))?;
        let habitat_aoi: Vec<bool> = (0..habitat.len())
            .map(|i| grid.inside[i] && habitat[i] == 1.0)
            .collect();
        let habitat_ha = grid.area_where(&habitat_aoi);
        if habitat_ha <= 0.0 {
            return Ok(None);
        }
        let restore_habitat: Vec<bool> = (0..habitat.len())
            .map(|i| habitat_aoi[i] && restore[i])
            .collect();
        let restore_habitat_ha = grid.area_where(&restore_habitat);
        let habitat_gain_ha = restore_habitat_ha * condition_uplift;
        let habitat_gain_pct = habitat_gain_ha / habitat_ha * 100.0;
        let status = sp
            .red_list_category
            .clone()
            .unwrap_or_else(|| "None".into());
        Ok(Some(Row {
            species: sp.species.clone(),
            iucn: iucn_map(&status.trim().to_uppercase()),
            habitat_aoi_ha: np_round(habitat_ha, 2),
            restoration_overlap_ha: np_round(restore_habitat_ha, 2),
            habitat_gain_ha: np_round(habitat_gain_ha, 2),
            habitat_gain_pct: np_round(habitat_gain_pct, 2),
        }))
    })?;
    let benefiting: Vec<Row> = rows
        .into_iter()
        .flatten()
        .filter(|r| r.habitat_gain_ha > 0.0)
        .collect();
    let species_count = benefiting.len();
    let threatened: Vec<&Row> = benefiting
        .iter()
        .filter(|r| matches!(r.iucn, Some("CR" | "EN" | "VU")))
        .collect();
    let threatened_count = threatened.len();
    let threatened_pct = if species_count > 0 {
        threatened_count as f64 / species_count as f64 * 100.0
    } else {
        0.0
    };
    let count = |s: &str| threatened.iter().filter(|r| r.iucn == Some(s)).count();
    let (cr, en, vu) = (count("CR"), count("EN"), count("VU"));
    let col = |f: fn(&Row) -> f64| benefiting.iter().map(f).collect::<Vec<f64>>();
    let mean_habitat_gain_pct = if species_count > 0 {
        np_sum(&col(|r| r.habitat_gain_pct)) / species_count as f64
    } else {
        0.0
    };
    let weighted_habitat_gain_pct = if species_count > 0 {
        np_sum(&col(|r| r.habitat_gain_ha)) / np_sum(&col(|r| r.habitat_aoi_ha)) * 100.0
    } else {
        0.0
    };
    let narrative = format!(
        "By restoring degraded areas, this project may expand habitat for {species_count} species, including {threatened_pct:.1}% \
         threatened species: {cr} CR, {en} EN, and {vu} VU. Over the project\u{2019}s {duration_years} year \
         duration, the intervention may increase suitable habitat by {mean_habitat_gain_pct:.2}% , with an area-weighted habitat gain of {weighted_habitat_gain_pct:.2}%",
    );
    let order = nargsort_desc(&col(|r| r.habitat_gain_pct));
    let species_rows: Vec<Value> = order.iter().map(|i| benefiting[*i].to_json()).collect();
    let values = json!({
        "duration_years": duration_years,
        "restore_ha": restore_ha,
        "gain_10yr_ha": gain_10yr_ha,
        "degradation_10yr_ha": degradation_10yr_ha,
        "gain_rate_pct_per_year": gain_rate * 100.0,
        "degradation_rate_pct_per_year": degradation_rate * 100.0,
        "condition_uplift_pct": condition_uplift * 100.0,
        "biodiversity_uplift_ha": biodiversity_uplift_ha,
        "species_benefiting": species_count,
        "threatened_count": threatened_count,
        "threatened_pct": threatened_pct,
        "cr": cr, "en": en, "vu": vu,
        "mean_habitat_gain_pct": mean_habitat_gain_pct,
        "weighted_habitat_gain_pct": weighted_habitat_gain_pct,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "duration_years",
        "restore_ha",
        "condition_uplift_pct",
        "biodiversity_uplift_ha",
        "species_benefiting",
        "threatened_count",
        "threatened_pct",
        "cr",
        "en",
        "vu",
        "mean_habitat_gain_pct",
        "weighted_habitat_gain_pct",
    ] {
        view[k] = values[k].clone();
    }
    view["species"] = json!(species_rows); // after the values, as the dict literal
    Ok((
        json!({"narrative": narrative, "tables": {"species": species_rows}, "values": values, "flags": []}),
        view,
    ))
}
