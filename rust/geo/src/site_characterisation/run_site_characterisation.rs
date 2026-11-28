//! `site_characterisation/run_site_characterisation.py`: the whole of F02-P2 (General, Nature,
//! Climate, People) as ONE NDJSON stream. One pool of 6 workers, submission in the fixed
//! cheapest-first order (`_ORDER`), four dependents waiting on their dependencies on a worker
//! (`pipeline.after`), emission in plan order.
//!
//! Six components are not ported in this crate yet (vector / database: 1.2, 1.3, 2.2, 2.5,
//! plants, 6.3). The caller supplies each through `Provided`; one it does not supply fails like a
//! raising component (`NotImplementedError`), costing only its card.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::climate::run_climate::carbon_shares;
use super::climate::{
    annual_precipitation, annual_temperature, current_carbon_storage, fire_susceptibility,
    historical_burned_area, peat_depth, soil_classification, soil_organic_carbon,
};
use super::general::{
    deforestation_risk, historical_deforestation, land_cover, natural_disaster_risk, run_general,
    terrain_slope_elevation,
};
use super::nature::{
    conservation_significance, endangered_trees, forest_landscape_integrity, habitat_area,
};
use super::people::{people_demography, vulnerability_assessment};
use crate::common::Aoi;
use crate::db::MappingKeys;
use crate::pipeline::{
    Job, Pair, PoolViews, Process, Stream, cheapest_first, plan, resolve, run_pool, stream,
};
use crate::settings::Settings;
use crate::{Error, Result};

/// A component the caller supplies: `(settings, aoi, dependency outputs) -> (results, view)`.
pub type ComponentFn = Arc<dyn Fn(&Settings, &Aoi, &[&Pair]) -> Result<Pair> + Send + Sync>;

/// Supplied components by process name.
pub type Provided = HashMap<String, ComponentFn>;

/// The components this crate does not port; `Provided` fills them.
pub const PROVIDED: [&str; 6] = [
    "administrative boundaries",
    "protected areas",
    "key biodiversity areas",
    "key species presence",
    "plant species occurrence",
    "social statistics",
];

/// The database-backed ports (agent geo-vector: 1.2, 1.3, 2.2, 2.5, plants, 6.3) as `Provided`
/// entries: `gis` = `db::gis_pool`, `social` = `db::social_pool` (the APP database: 6.3 reads
/// its `se_v3` foreign tables), `rt` = a handle of the server's tokio runtime. The pool threads
/// are plain threads, so `Handle::block_on` is legal there.
pub fn db_components(
    gis: sqlx::PgPool,
    social: sqlx::PgPool,
    keys: Arc<MappingKeys>,
    rt: tokio::runtime::Handle,
) -> Provided {
    use super::general::{administrative_boundaries, protected_areas_wdpa};
    use super::nature::{key_biodiversity_areas, key_species_presence, plant_species_occurrence};
    use super::people::social_statistics;
    let mut p = Provided::new();
    let mut add = |name: &str, f: ComponentFn| {
        p.insert(name.to_string(), f);
    };
    let (g, h) = (gis.clone(), rt.clone());
    add(
        "administrative boundaries",
        Arc::new(move |_, a, _| {
            h.block_on(administrative_boundaries::analyze_admin_boundaries(&g, a))
        }),
    );
    let (g, h) = (gis.clone(), rt.clone());
    add(
        "protected areas",
        Arc::new(move |_, a, _| h.block_on(protected_areas_wdpa::analyze_protected_areas(&g, a))),
    );
    let (g, h) = (gis.clone(), rt.clone());
    add(
        "key biodiversity areas",
        Arc::new(move |_, a, _| h.block_on(key_biodiversity_areas::analyze_kba(&g, a))),
    );
    let (g, h) = (gis.clone(), rt.clone());
    add(
        "key species presence",
        Arc::new(move |_, a, _| h.block_on(key_species_presence::analyze_key_species(&g, a))),
    );
    let (g, h) = (gis.clone(), rt.clone());
    add(
        "plant species occurrence",
        Arc::new(move |_, a, _| h.block_on(plant_species_occurrence::analyze_plant_species(&g, a))),
    );
    // _social_after_admin: 1.2's `values` ({} when 1.2 failed)
    add(
        "social statistics",
        Arc::new(move |_, a, deps| {
            let admin = deps.first().and_then(|d| d.0.get("values"));
            rt.block_on(social_statistics::analyze_social_statistics(
                &gis, &social, &keys, a, admin,
            ))
        }),
    );
    p
}

/// `_UNORDERED` with each component's `COST` (the module `processes` weights), in module order.
pub fn unordered() -> Vec<(&'static str, f64, Vec<&'static str>)> {
    vec![
        // General, 1.1 to 1.8
        ("ecosystem type", 2.9, vec![]),
        ("administrative boundaries", 1.9, vec![]),
        ("protected areas", 1.2, vec![]),
        ("terrain", 3.7, vec![]),
        ("historical deforestation", 3.5, vec![]),
        ("deforestation risk", 2.2, vec!["administrative boundaries"]),
        ("natural disaster risks", 3.7, vec![]),
        ("land cover", 3.7, vec![]),
        // Nature
        ("forest landscape integrity", 3.7, vec![]),
        ("key biodiversity areas", 1.2, vec![]),
        ("habitat area", 21.9, vec![]),
        ("key species presence", 1.5, vec![]),
        ("conservation significance", 1.4, vec![]),
        ("endangered trees", 0.9, vec![]),
        ("plant species occurrence", 1.5, vec![]),
        // Climate
        ("current carbon storage", 1.6, vec![]),
        ("soil organic carbon", 2.1, vec!["ecosystem type"]),
        (
            "carbon shares",
            0.1,
            vec!["current carbon storage", "soil organic carbon"],
        ),
        ("annual temperature", 3.6, vec![]),
        ("annual precipitation", 3.4, vec![]),
        ("fire susceptibility", 1.1, vec![]),
        ("burned area", 6.7, vec![]),
        ("soil classification", 1.6, vec![]),
        ("peat depth", 0.8, vec![]),
        // People
        ("people demography", 10.5, vec![]),
        ("vulnerability assessment", 5.2, vec![]),
        ("social statistics", 1.5, vec!["administrative boundaries"]),
    ]
}

/// `COMPONENTS`: `(name, cost, deps)` in `_ORDER` (submission and emission order).
pub fn components() -> Vec<(String, f64, Vec<String>)> {
    let table = unordered();
    cheapest_first(&table)
        .into_iter()
        .map(|n| {
            let (_, c, d) =
                table
                    .iter()
                    .find(|(m, _, _)| *m == n)
                    .cloned()
                    .unwrap_or(("", 0.0, vec![]));
            (n, c, d.into_iter().map(str::to_string).collect())
        })
        .collect()
}

/// `processes`: preparation, every component cheapest first, end.
pub fn processes() -> Vec<Process> {
    let mut p = vec![Process::new("preparation", 0.1)];
    p.extend(
        components()
            .into_iter()
            .map(|(n, c, _)| Process { name: n, w: c }),
    );
    p.push(Process::new("end", 0.1));
    p
}

/// One site-characterisation component's body, given its dependencies' outputs (in the order
/// `unordered` lists them). Also used by the union stream.
pub fn run_component(
    name: &str,
    s: &Settings,
    keys: &MappingKeys,
    aoi: &Aoi,
    deps: &[&Pair],
    provided: &Provided,
) -> Result<Pair> {
    if let Some(f) = provided.get(name) {
        return f(s, aoi, deps);
    }
    match name {
        "ecosystem type" => run_general::ecosystem_on_site_area(s, aoi),
        "terrain" => terrain_slope_elevation::analyze_terrain(s, keys, aoi),
        "historical deforestation" => {
            historical_deforestation::analyze_historical_deforestation(s, aoi)
        }
        // _risk_after_admin
        "deforestation risk" => {
            let country = deps
                .first()
                .and_then(|d| d.0.get("values"))
                .and_then(|v| v.get("dominant_country"))
                .and_then(Value::as_str);
            deforestation_risk::analyze_deforestation_risk(s, aoi, country)
        }
        "natural disaster risks" => natural_disaster_risk::analyze_natural_risk(s, aoi),
        "land cover" => land_cover::analyze_land_cover(s, aoi),
        "forest landscape integrity" => forest_landscape_integrity::analyze_flii(s, aoi),
        "habitat area" => habitat_area::analyze_habitat_area(s, aoi),
        "conservation significance" => {
            conservation_significance::analyze_conservation_significance(s, aoi)
        }
        "endangered trees" => endangered_trees::analyze_endangered_trees(s, aoi),
        "current carbon storage" => current_carbon_storage::analyze_current_carbon_storage(s, aoi),
        // _soc_after_ecosystem: 1.1's present_set, None when 1.1 failed
        "soil organic carbon" => {
            let present: Option<Vec<i64>> = deps
                .first()
                .and_then(|d| d.0.get("values"))
                .and_then(|v| v.get("present_set"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect());
            soil_organic_carbon::analyze_soil_organic_carbon(s, aoi, present.as_deref())
        }
        "carbon shares" => match deps {
            [carbon, soc] => carbon_shares(carbon, soc),
            _ => Err(Error::new("TypeError", "carbon shares takes 3.1 and 3.2")),
        },
        "annual temperature" => annual_temperature::analyze_annual_temperature(s, aoi),
        "annual precipitation" => annual_precipitation::analyze_annual_precipitation(s, aoi),
        "fire susceptibility" => fire_susceptibility::analyze_fire_susceptibility(s, aoi),
        "burned area" => historical_burned_area::analyze_historical_burned_area(s, aoi),
        "soil classification" => soil_classification::analyze_soil_classification(s, aoi),
        "peat depth" => peat_depth::analyze_peat_depth(s, aoi),
        "people demography" => people_demography::analyze_people_demography(s, aoi),
        "vulnerability assessment" => {
            vulnerability_assessment::analyze_vulnerability_assessment(s, aoi)
        }
        other => Err(Error::new(
            "NotImplementedError",
            format!("{other} is not ported to Rust yet"),
        )),
    }
}

/// `_MAX_WORKERS`
pub const MAX_WORKERS: usize = 6;

/// The shared inputs of one run.
#[derive(Clone)]
pub struct RunInputs {
    pub settings: Arc<Settings>,
    pub keys: Arc<MappingKeys>,
    pub aoi: Arc<Aoi>,
    pub provided: Arc<Provided>,
}

/// `stream_site_characterisation(aoi, wanted, retry_url)`: a lazy iterator of NDJSON lines.
/// `wanted` names must be known (the route answers 400 first).
pub fn stream_site_characterisation(
    inputs: RunInputs,
    wanted: Option<&[String]>,
    retry_url: Option<crate::pipeline::RetryUrl>,
) -> Result<Stream<PoolViews>> {
    let comps = components();
    let order: Vec<String> = comps.iter().map(|c| c.0.clone()).collect();
    let deps_of = |n: &str| {
        comps
            .iter()
            .find(|c| c.0 == n)
            .map(|c| c.2.clone())
            .unwrap_or_default()
    };
    let running = resolve(&order, deps_of, wanted);
    let plan = plan(&processes(), wanted);
    let jobs: Vec<Job> = running
        .iter()
        .map(|name| {
            let deps: Vec<usize> = deps_of(name)
                .iter()
                .filter_map(|d| running.iter().position(|r| r == d))
                .collect();
            let (inp, n) = (inputs.clone(), name.clone());
            Job {
                name: name.clone(),
                deps,
                run: Box::new(move |d| {
                    run_component(&n, &inp.settings, &inp.keys, &inp.aoi, d, &inp.provided)
                }),
            }
        })
        .collect();
    let emit: Vec<usize> = plan[1..plan.len() - 1]
        .iter()
        .filter_map(|p| running.iter().position(|r| *r == p.name))
        .collect();
    let workers = running.len().min(MAX_WORKERS);
    stream(plan, run_pool(jobs, emit, workers), retry_url)
}
