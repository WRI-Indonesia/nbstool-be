//! 5.9 Habitat loss avoided (`benefit/habitat_loss_avoided.py`): on the deforestation-risk
//! raster's native grid, projected loss allocated highest risk first, over the union of the
//! candidate species' habitats (~400 species rasters read on a thread pool).

use serde_json::{Value, json};

use super::config::{ECOSYSTEM_NAMES, lookup};
use super::inventory::{Species, candidates};
use super::util::{NativeGrid, VrtGrid, na, pool_map, read_vrt_filled0};
use crate::common::{Aoi, Resampling};
use crate::config::{AOH_RASTER_ROOT, PROB_RASTER, THREAT_ECOSYSTEM};
use crate::py::{fmt_grouped, np_sum};
use crate::settings::Settings;
use crate::{Error, Result};

/// The state 5.9 and 5.10 build before their species loops.
pub struct RiskGrid {
    pub grid: NativeGrid,
    pub full: VrtGrid,
    pub risk: Vec<f64>,
}

impl RiskGrid {
    pub fn load(settings: &Settings, aoi: &Aoi) -> Result<Self> {
        let (grid, src) = NativeGrid::window(settings, PROB_RASTER, aoi)?;
        let risk = grid.read_filled0(&src, 1)?;
        let full = grid.full_grid();
        Ok(Self { grid, full, risk })
    }

    /// `read(path)`: WarpedVRT onto the full risk grid, nearest, the AOI window, filled(0).
    pub fn read(&self, path: &str) -> Result<Vec<f64>> {
        Ok(read_vrt_filled0(
            path,
            &self.full,
            1,
            Some(&self.grid.window),
            Resampling::Nearest,
        )?
        .0)
    }

    /// `allocate_loss(mask, target_ha)`: float32 allocation, highest risk first (stable on the
    /// flat index), whole pixels then the fraction of the next.
    pub fn allocate_loss(&self, mask: &[bool], target_ha: f64) -> Vec<f32> {
        let w = self.grid.width.max(1);
        let mut allocation = vec![0f32; mask.len()];
        let mut idx: Vec<usize> = (0..mask.len()).filter(|i| mask[*i]).collect();
        // np.argsort(-risk[idx], kind="stable"): descending risk, ties in index order
        idx.sort_by(|a, b| self.risk[*b].total_cmp(&self.risk[*a]));
        let areas: Vec<f64> = idx.iter().map(|i| self.grid.row_ha[i / w]).collect();
        let mut cumulative = 0.0;
        let mut n = areas.len();
        for (k, a) in areas.iter().enumerate() {
            cumulative += a;
            if cumulative > target_ha {
                n = k; // searchsorted(cumulative, target, side="right")
                break;
            }
        }
        for i in &idx[..n] {
            allocation[*i] = 1.0;
        }
        let used = np_sum(&areas[..n]);
        if n < idx.len() {
            allocation[idx[n]] = f64::min((target_ha - used) / areas[n], 1.0) as f32;
        }
        allocation
    }

    /// `(pixel_ha * loss * habitat).sum()`
    pub fn loss_area(&self, loss: &[f32], habitat: &[bool]) -> f64 {
        self.grid.weighted_sum(|i| loss[i] as f64, habitat)
    }
}

/// Component 5.9. `workers` = AOH_MAX_WORKERS.
pub fn analyze_habitat_loss_avoided(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
    rate_pct: Option<f64>,
    ecosystem_class: i64,
    workers: usize,
) -> Result<(Value, Value)> {
    let Some(rate_pct) = rate_pct else {
        return Ok(na(
            "No historical deforestation rate is available for this project area, so projected habitat loss cannot \
             be estimated.",
        ));
    };
    let rg = RiskGrid::load(settings, aoi)?;
    let g = &rg.grid;
    let ecosystem = rg.read(&settings.layer_path(THREAT_ECOSYSTEM)?)?;
    let ecosystem_mask: Vec<bool> = (0..ecosystem.len())
        .map(|i| g.inside[i] && ecosystem[i] == ecosystem_class as f64)
        .collect();
    let risk_pool: Vec<bool> = (0..ecosystem.len())
        .map(|i| ecosystem_mask[i] && rg.risk[i] > 1.0)
        .collect();
    let ecosystem_area = g.area_where(&ecosystem_mask);
    let risk_area = g.area_where(&risk_pool);
    if ecosystem_area <= 0.0 {
        return Ok(na(&format!(
            "This project area contains no {} ecosystem, so there is no habitat to assess.",
            lookup(&ECOSYSTEM_NAMES, ecosystem_class).unwrap_or("target")
        )));
    }
    let projected_loss = risk_area * (1.0 - (-(rate_pct / 100.0) * duration_years as f64).exp());
    let loss = rg.allocate_loss(&risk_pool, projected_loss);

    let species = candidates(settings, aoi)?;
    let root = settings.layer_path(AOH_RASTER_ROOT)?;
    // Union and count are order-free, so each worker folds its species into the shared union
    // (holding ~400 window-sized masks at once would not fit a large AOI).
    let union = std::sync::Mutex::new(vec![false; loss.len()]);
    let affected: Vec<bool> = pool_map(&species, workers, |sp: &Species| {
        let h = rg.read(&format!("{root}/{}", sp.raster_path))?;
        let habitat: Vec<bool> = (0..h.len())
            .map(|i| ecosystem_mask[i] && h[i] == 1.0)
            .collect();
        let mut u = union
            .lock()
            .map_err(|_| Error::new("RuntimeError", "worker panicked"))?;
        for (x, y) in u.iter_mut().zip(&habitat) {
            *x |= *y;
        }
        Ok(habitat.iter().zip(&loss).any(|(x, l)| *x && *l > 0.0))
    })?;
    let species_count = affected.iter().filter(|a| **a).count();
    let union = union
        .into_inner()
        .map_err(|_| Error::new("RuntimeError", "worker panicked"))?;
    let current_habitat = g.area_where(&union);
    let habitat_avoided = rg.loss_area(&loss, &union);
    let name = lookup(&ECOSYSTEM_NAMES, ecosystem_class)
        .ok_or_else(|| Error::new("KeyError", ecosystem_class.to_string()))?;
    let narrative = format!(
        "Conserving this {name} ecosystem is estimated to avoid the loss of {} hectares of suitable habitat over the \
         project's {duration_years}-year duration.",
        fmt_grouped(habitat_avoided, 2)
    );
    let values = json!({
        "ecosystem": name,
        "duration_years": duration_years,
        "rate_pct": rate_pct,
        "ecosystem_area_ha": ecosystem_area,
        "risk_area_ha": risk_area,
        "projected_loss_ha": projected_loss,
        "current_habitat_ha": current_habitat,
        "habitat_loss_avoided_ha": habitat_avoided,
        "species_affected": species_count,
        "candidate_species": species.len(),
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "ecosystem",
        "duration_years",
        "current_habitat_ha",
        "projected_loss_ha",
        "habitat_loss_avoided_ha",
        "species_affected",
    ] {
        view[k] = values[k].clone();
    }
    Ok((
        json!({"narrative": narrative, "tables": {}, "values": values, "flags": []}),
        view,
    ))
}
