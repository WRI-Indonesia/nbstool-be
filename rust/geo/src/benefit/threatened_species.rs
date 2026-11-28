//! 5.10 Threatened species habitat (`benefit/threatened_species.py`): same grid discipline as
//! 5.9; per CR/EN/VU species, its AOI habitat and the share projected deforestation would take.

use serde_json::{Value, json};

use super::config::iucn_map;
use super::habitat_loss_avoided::RiskGrid;
use super::inventory::{Species, candidates};
use super::util::{na, nargsort_desc, np_round, pool_map};
use crate::Result;
use crate::common::Aoi;
use crate::config::{AOH_RASTER_ROOT, THREAT_ECOSYSTEM};
use crate::py::np_sum;
use crate::settings::Settings;

struct Row {
    species: Option<String>,
    iucn: &'static str,
    habitat_aoi_ha: f64,
    habitat_loss_avoided_ha: f64,
    avoided_habitat_loss_pct: f64,
}

enum Outcome {
    NoHabitat,
    Counted,
    Row(Row),
}

/// Component 5.10. `workers` = AOH_MAX_WORKERS.
pub fn analyze_threatened_species_habitat(
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
    let risk_pool: Vec<bool> = (0..ecosystem.len())
        .map(|i| g.inside[i] && ecosystem[i] == ecosystem_class as f64 && rg.risk[i] > 1.0)
        .collect();
    let risk_area = g.area_where(&risk_pool);
    let projected_loss = risk_area * (1.0 - (-(rate_pct / 100.0) * duration_years as f64).exp());
    let loss = rg.allocate_loss(&risk_pool, projected_loss);

    let species = candidates(settings, aoi)?;
    let root = settings.layer_path(AOH_RASTER_ROOT)?;
    let outcomes: Vec<Outcome> = pool_map(&species, workers, |sp: &Species| {
        let h = rg.read(&format!("{root}/{}", sp.raster_path))?;
        let habitat: Vec<bool> = (0..h.len()).map(|i| g.inside[i] && h[i] == 1.0).collect();
        let habitat_area = g.area_where(&habitat);
        if habitat_area <= 0.0 {
            return Ok(Outcome::NoHabitat);
        }
        let status_raw = sp
            .red_list_category
            .clone()
            .unwrap_or_else(|| "None".into())
            .trim()
            .to_uppercase();
        let Some(status) = iucn_map(&status_raw) else {
            return Ok(Outcome::Counted);
        };
        let avoided_ha = rg.loss_area(&loss, &habitat);
        let pct = avoided_ha / habitat_area * 100.0;
        Ok(Outcome::Row(Row {
            species: sp.species.clone(),
            iucn: status,
            habitat_aoi_ha: np_round(habitat_area, 2),
            habitat_loss_avoided_ha: np_round(avoided_ha, 2),
            avoided_habitat_loss_pct: np_round(pct, 2),
        }))
    })?;
    let mut total_species = 0usize;
    let mut rows: Vec<Row> = Vec::new();
    for o in outcomes {
        match o {
            Outcome::NoHabitat => {}
            Outcome::Counted => total_species += 1,
            Outcome::Row(r) => {
                total_species += 1;
                rows.push(r);
            }
        }
    }
    // species_df.sort_values("avoided_habitat_loss_pct", ascending=False)
    let order = nargsort_desc(
        &rows
            .iter()
            .map(|r| r.avoided_habitat_loss_pct)
            .collect::<Vec<_>>(),
    );
    let sorted: Vec<&Row> = order.iter().map(|i| &rows[*i]).collect();
    let threatened_count = sorted.len();
    let count = |s: &str| sorted.iter().filter(|r| r.iucn == s).count();
    let (cr, en, vu) = (count("CR"), count("EN"), count("VU"));
    let threatened_pct = if total_species > 0 {
        threatened_count as f64 / total_species as f64 * 100.0
    } else {
        0.0
    };
    let benefiting_species = sorted
        .iter()
        .filter(|r| r.habitat_loss_avoided_ha > 0.0)
        .count();
    let avoided_habitat_loss_pct = if threatened_count > 0 {
        let total_threatened_habitat =
            np_sum(&sorted.iter().map(|r| r.habitat_aoi_ha).collect::<Vec<_>>());
        let total_avoided_habitat = np_sum(
            &sorted
                .iter()
                .map(|r| r.habitat_loss_avoided_ha)
                .collect::<Vec<_>>(),
        );
        if total_threatened_habitat > 0.0 {
            total_avoided_habitat / total_threatened_habitat * 100.0
        } else {
            0.0
        }
    } else {
        0.0
    };
    let narrative = format!(
        "By protecting at-risk habitat, this project supports habitat for {total_species} species, including \
         {threatened_pct:.1}% threatened species: {cr} CR, {en} EN, and {vu} VU. Over the project's \
         {duration_years}-year duration, the intervention may reduce threat levels by protecting at least \
         {avoided_habitat_loss_pct:.1}% of habitatfor {benefiting_species} threatened species."
    );
    let species_rows: Vec<Value> = sorted
        .iter()
        .map(|r| {
            json!({
                "species": r.species, "iucn": r.iucn, "habitat_aoi_ha": r.habitat_aoi_ha,
                "habitat_loss_avoided_ha": r.habitat_loss_avoided_ha,
                "avoided_habitat_loss_pct": r.avoided_habitat_loss_pct,
            })
        })
        .collect();
    let values = json!({
        "duration_years": duration_years,
        "rate_pct": rate_pct,
        "projected_loss_ha": projected_loss,
        "species_with_habitat": total_species,
        "threatened_count": threatened_count,
        "threatened_pct": threatened_pct,
        "cr": cr, "en": en, "vu": vu,
        "avoided_habitat_loss_pct": avoided_habitat_loss_pct,
        "benefiting_species": benefiting_species,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "duration_years",
        "species_with_habitat",
        "threatened_count",
        "threatened_pct",
        "cr",
        "en",
        "vu",
        "avoided_habitat_loss_pct",
        "benefiting_species",
    ] {
        view[k] = values[k].clone();
    }
    view["species"] = json!(species_rows); // after the values, as the dict literal
    Ok((
        json!({"narrative": narrative, "tables": {"species": species_rows}, "values": values, "flags": []}),
        view,
    ))
}
