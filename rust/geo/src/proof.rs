//! The proof harness shared by the integration test and the `run_one` example: run a ported
//! component on a stored AOI and compare its JSON with what the original wrote.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common::{Aoi, prepare_aoi_geojson};
use crate::db::MappingKeys;
use crate::pathway::pathway_distribution;
use crate::settings::Settings;
use crate::site_characterisation::climate::{
    annual_precipitation, annual_temperature, current_carbon_storage, fire_susceptibility,
    historical_burned_area, peat_depth, soil_classification, soil_organic_carbon,
};
use crate::site_characterisation::general::{
    deforestation_risk, ecosystem_type, historical_deforestation, land_cover,
    natural_disaster_risk, run_general, terrain_slope_elevation,
};
use crate::site_characterisation::nature::{
    conservation_significance, endangered_trees, forest_landscape_integrity, habitat_area,
};
use crate::site_characterisation::people::{people_demography, vulnerability_assessment};
use crate::threat::{all_ecosystems, dryland_forest, land_degradation, mangrove, peatland};
use crate::{Error, Result};

/// (settings, prepared AOI, proof AOI name) -> `to_jsonable` of the component's return value.
pub type Component = fn(&Settings, &Aoi, &str) -> Result<Value>;

/// `to_jsonable` of a `(results, view_results)` tuple: a two-item list.
fn pair(r: Result<(Value, Value)>) -> Result<Value> {
    r.map(|(results, view)| json!([results, view]))
}

/// Ported components by the name the Python driver (proof/run_python.py) records them under.
pub const COMPONENTS: &[(&str, Component)] = &[
    ("ecosystem_type", |s, a, _| {
        pair(ecosystem_type::analyze_ecosystem_type(s, a))
    }),
    ("ecosystem_on_site_area", |s, a, _| {
        pair(run_general::ecosystem_on_site_area(s, a))
    }),
    ("historical_deforestation", |s, a, _| {
        pair(historical_deforestation::analyze_historical_deforestation(
            s, a,
        ))
    }),
    ("natural_disaster_risk", |s, a, _| {
        pair(natural_disaster_risk::analyze_natural_risk(s, a))
    }),
    ("land_cover", |s, a, _| {
        pair(land_cover::analyze_land_cover(s, a))
    }),
    ("fire_susceptibility", |s, a, _| {
        pair(fire_susceptibility::analyze_fire_susceptibility(s, a))
    }),
    ("pathway_distribution", |s, a, _| {
        pathway_distribution::analyze_pathway_distribution(s, a)
    }),
    ("endangered_trees", |s, a, _| {
        pair(endangered_trees::analyze_endangered_trees(s, a))
    }),
    ("soil_organic_carbon", |s, a, _| {
        pair(soil_organic_carbon::analyze_soil_organic_carbon(s, a, None))
    }),
    ("current_carbon_storage", |s, a, _| {
        pair(current_carbon_storage::analyze_current_carbon_storage(s, a))
    }),
    ("annual_temperature", |s, a, _| {
        pair(annual_temperature::analyze_annual_temperature(s, a))
    }),
    ("annual_precipitation", |s, a, _| {
        pair(annual_precipitation::analyze_annual_precipitation(s, a))
    }),
    ("people_demography", |s, a, _| {
        pair(people_demography::analyze_people_demography(s, a))
    }),
    ("vulnerability_assessment", |s, a, _| {
        pair(vulnerability_assessment::analyze_vulnerability_assessment(
            s, a,
        ))
    }),
    ("terrain", |s, a, _| {
        pair(terrain_slope_elevation::analyze_terrain(
            s,
            proof_mapping_keys()?,
            a,
        ))
    }),
    ("all_ecosystems", |s, a, _| {
        all_ecosystems::analyze_all_ecosystem(s, a)
    }),
    ("deforestation_risk", |s, a, n| {
        pair(deforestation_risk::analyze_deforestation_risk(
            s,
            a,
            dominant_country(n),
        ))
    }),
    ("dryland_forest", |s, a, _| {
        dryland_forest::analyze_dryland_forest(s, a)
    }),
    ("mangrove", |s, a, _| mangrove::analyze_mangrove(s, a)),
    ("peatland", |s, a, _| peatland::analyze_peatland(s, a)),
    ("land_degradation", |s, a, _| {
        land_degradation::analyze_land_degradation(s, a)
    }),
    ("soil_classification", |s, a, _| {
        pair(soil_classification::analyze_soil_classification(s, a))
    }),
    ("peat_depth", |s, a, _| {
        pair(peat_depth::analyze_peat_depth(s, a))
    }),
    ("historical_burned_area", |s, a, _| {
        pair(historical_burned_area::analyze_historical_burned_area(s, a))
    }),
    ("forest_landscape_integrity", |s, a, _| {
        pair(forest_landscape_integrity::analyze_flii(s, a))
    }),
    ("conservation_significance", |s, a, _| {
        pair(conservation_significance::analyze_conservation_significance(s, a))
    }),
    ("habitat_area", |s, a, _| {
        pair(habitat_area::analyze_habitat_area(s, a))
    }),
];

/// The dominant_country the Python driver passes to 1.6 for each proof AOI (DOMINANT_COUNTRY).
fn dominant_country(aoi_name: &str) -> Option<&'static str> {
    match aoi_name {
        "id_small" | "id_multi2" | "id_multi12" => Some("Indonesia"),
        "ph_mindanao" => Some("Philippines"),
        "kh_large" => Some("Cambodia"),
        "my_large" => Some("Brunei"),
        _ => None,
    }
}

/// The local database copy the Python driver reads too (proof/run_python.py): read only.
const PROOF_APP_DB: &str = "postgresql://postgres@127.0.0.1:54329/scratch_tpl";

/// tbl_list_mapping_key from the local copy, loaded once per test process.
fn proof_mapping_keys() -> Result<&'static MappingKeys> {
    static KEYS: std::sync::OnceLock<MappingKeys> = std::sync::OnceLock::new();
    if let Some(k) = KEYS.get() {
        return Ok(k);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::new("RuntimeError", e.to_string()))?;
    let keys = rt.block_on(async {
        let pool = crate::settings::pool_from_uri(PROOF_APP_DB)?;
        Ok::<_, Error>(MappingKeys::load(&pool).await)
    })?;
    Ok(KEYS.get_or_init(|| keys))
}

pub fn proof_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("proof")
}

/// The settings row scratch_tpl holds (and staging uses): the public v3 bucket.
pub fn proof_settings() -> Settings {
    Settings::fixed(
        "https://storage.googleapis.com/assets-geo/v3",
        Some("habitat_area_v3"),
    )
}

pub fn aoi_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(proof_dir().join("aois"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    e.file_name()
                        .to_str()?
                        .strip_suffix(".geojson")
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// prepare_aoi + the component, as `[results, view_results]`.
pub fn run(component: &str, aoi_name: &str) -> Result<Value> {
    let f = COMPONENTS
        .iter()
        .find(|(n, _)| *n == component)
        .map(|(_, f)| *f)
        .ok_or_else(|| Error::new("KeyError", component.to_string()))?;
    let text =
        std::fs::read_to_string(proof_dir().join("aois").join(format!("{aoi_name}.geojson")))
            .map_err(|e| Error::new("FileNotFoundError", e.to_string()))?;
    let aoi = prepare_aoi_geojson(&text)?;
    f(&proof_settings(), &aoi, aoi_name)
}

/// Every difference between `expected` and `actual`, as "path: expected != actual (rel diff)".
/// Numbers compare exactly, including int vs float.
pub fn diff(expected: &Value, actual: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk("$", expected, actual, &mut out);
    out
}

/// Largest relative difference over numbers present at the same path in both (0 when equal).
pub fn max_rel_diff(expected: &Value, actual: &Value) -> f64 {
    match (expected, actual) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            if a == b {
                0.0
            } else {
                (a - b).abs() / a.abs().max(b.abs())
            }
        }
        (Value::Array(a), Value::Array(b)) => a
            .iter()
            .zip(b)
            .map(|(x, y)| max_rel_diff(x, y))
            .fold(0.0, f64::max),
        (Value::Object(a), Value::Object(b)) => a
            .iter()
            .filter_map(|(k, x)| b.get(k).map(|y| max_rel_diff(x, y)))
            .fold(0.0, f64::max),
        _ => 0.0,
    }
}

fn walk(path: &str, e: &Value, a: &Value, out: &mut Vec<String>) {
    match (e, a) {
        (Value::Object(eo), Value::Object(ao)) => {
            for (k, ev) in eo {
                match ao.get(k) {
                    Some(av) => walk(&format!("{path}.{k}"), ev, av, out),
                    None => out.push(format!("{path}.{k}: missing in port")),
                }
            }
            for k in ao.keys().filter(|k| !eo.contains_key(*k)) {
                out.push(format!("{path}.{k}: extra in port"));
            }
        }
        (Value::Array(ea), Value::Array(aa)) => {
            if ea.len() != aa.len() {
                out.push(format!("{path}: length {} != {}", ea.len(), aa.len()));
            }
            for (i, (ev, av)) in ea.iter().zip(aa).enumerate() {
                walk(&format!("{path}[{i}]"), ev, av, out);
            }
        }
        _ if e != a => {
            let rel = max_rel_diff(e, a);
            out.push(format!("{path}: {e} != {a} (rel {rel:.3e})"));
        }
        _ => {}
    }
}

/// A module runner's whole NDJSON text on a proof AOI (no retry URL, as a script run has none).
pub fn run_stream(runner: &str, aoi_name: &str) -> Result<String> {
    let text =
        std::fs::read_to_string(proof_dir().join("aois").join(format!("{aoi_name}.geojson")))
            .map_err(|e| Error::new("FileNotFoundError", e.to_string()))?;
    let aoi = std::sync::Arc::new(prepare_aoi_geojson(&text)?);
    let settings = std::sync::Arc::new(proof_settings());
    let keys = std::sync::Arc::new(proof_mapping_keys()?.clone());
    match runner {
        "stream_threat" => Ok(crate::threat::run_threat::stream_threat(
            settings, keys, aoi, None, None,
        )?
        .collect()),
        _ => Err(Error::new("KeyError", runner.to_string())),
    }
}
