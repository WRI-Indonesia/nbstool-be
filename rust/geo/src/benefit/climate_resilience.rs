//! 5.12 Enhance resilience to climate hazards (`benefit/climate_resilience.py`): hazard-exposed
//! area and FuturePop population at the project's end year, on the flood risk layer's grid.

use gdal::raster::GdalDataType;
use serde_json::{Value, json};

use super::config::{FUTUREPOP_RASTER, POP_BASE_YEAR, futurepop_years};
use super::util::{NativeGrid, np_sum_f32, read_vrt_filled0};
use crate::Result;
use crate::common::{Aoi, Resampling};
use crate::config::{
    THREAT_FIRE_RISK, THREAT_FLOOD_RISK, THREAT_LANDSLIDE_RISK, THREAT_STORM_RISK,
};
use crate::py::{fmt_grouped, np_sum};
use crate::settings::Settings;

/// `population[mask].sum()` in the population array's own dtype, as a Python float.
fn pop_sum(pop: &[f64], dtype: GdalDataType, mask: &[bool]) -> f64 {
    let picked = pop.iter().zip(mask).filter(|(_, m)| **m).map(|(v, _)| *v);
    match dtype {
        GdalDataType::Float32 => np_sum_f32(&picked.map(|v| v as f32).collect::<Vec<_>>()) as f64,
        GdalDataType::Float64 => np_sum(&picked.collect::<Vec<_>>()),
        _ => picked.map(|v| v as i64).sum::<i64>() as f64, // integer dtypes sum exactly
    }
}

/// Component 5.12 (`pop_base_year` = POP_BASE_YEAR as the runner calls it).
pub fn analyze_climate_resilience(
    settings: &Settings,
    aoi: &Aoi,
    duration_years: i64,
) -> Result<(Value, Value)> {
    let hazards = [
        ("Flood", settings.layer_path(THREAT_FLOOD_RISK)?),
        ("Fire", settings.layer_path(THREAT_FIRE_RISK)?),
        ("Landslide", settings.layer_path(THREAT_LANDSLIDE_RISK)?),
        ("Typhoon", settings.layer_path(THREAT_STORM_RISK)?),
    ];
    let pop_base_year = POP_BASE_YEAR;
    let project_year = pop_base_year + duration_years;
    let years = futurepop_years();
    let mut population_year = years[0];
    for y in &years {
        if (y - project_year).abs() < (population_year - project_year).abs() {
            population_year = *y;
        }
    }
    let population_band = years
        .iter()
        .position(|y| *y == population_year)
        .unwrap_or(0)
        + 1;

    let (grid, _ref) = NativeGrid::window(settings, THREAT_FLOOD_RISK, aoi)?;
    let full = grid.full_grid();
    let (population, dtype) = read_vrt_filled0(
        &settings.layer_path(FUTUREPOP_RASTER)?,
        &full,
        population_band,
        Some(&grid.window),
        Resampling::Sum,
    )?;
    // np.where(inside_aoi, population, 0)
    let population: Vec<f64> = population
        .iter()
        .zip(&grid.inside)
        .map(|(p, i)| if *i { *p } else { 0.0 })
        .collect();

    let mut multi = vec![false; population.len()];
    let mut by_hazard = serde_json::Map::new();
    let mut rows = Vec::new();
    for (name, path) in &hazards {
        let (hazard, _) =
            read_vrt_filled0(path, &full, 1, Some(&grid.window), Resampling::Nearest)?;
        let exposed: Vec<bool> = (0..hazard.len())
            .map(|i| grid.inside[i] && (hazard[i] == 4.0 || hazard[i] == 5.0))
            .collect();
        for (m, e) in multi.iter_mut().zip(&exposed) {
            *m |= *e;
        }
        let entry = json!({"area_ha": grid.area_where(&exposed), "population": pop_sum(&population, dtype, &exposed)});
        rows.push(
            json!({"hazard": name, "area_ha": entry["area_ha"], "population": entry["population"]}),
        );
        by_hazard.insert(name.to_string(), entry);
    }
    let unique_area_ha = grid.area_where(&multi);
    let unique_population = pop_sum(&population, dtype, &multi);
    let narrative = format!(
        "Implementing NbS in this ecosystem can reduce disaster exposure across an estimated {} ha, helping to lower \
         risk for an estimated {} people over the project's {duration_years}-year duration.",
        fmt_grouped(unique_area_ha, 2),
        fmt_grouped(unique_population, 0)
    );
    let values = json!({
        "duration_years": duration_years,
        "population_base_year": pop_base_year,
        "population_year": population_year,
        "population_band": population_band,
        "unique_hazard_area_ha": unique_area_ha,
        "projected_population": unique_population,
        "by_hazard": by_hazard,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    for k in [
        "duration_years",
        "population_year",
        "unique_hazard_area_ha",
        "projected_population",
    ] {
        view[k] = values[k].clone();
    }
    view["hazards"] = json!(rows); // after the values, as the dict literal
    Ok((
        json!({"narrative": narrative, "tables": {"hazards": rows}, "values": values, "flags": []}),
        view,
    ))
}
