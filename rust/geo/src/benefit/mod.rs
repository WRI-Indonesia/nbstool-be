//! F02-P5 Benefit (`application/utils/geos/v3/benefit`): components 5.1-5.13 and
//! `run_benefit.py`'s orchestration (`stream_benefit`, `run_benefit`, `validate`), each proven
//! against the original on the proof AOIs (tests/proof_benefit.rs).
//!
//! The un-emitted `activity_stage` step runs the 4.2 port (`pathway::activity_list`); its
//! `values.by_category` IN ITS KEY ORDER (dominant category first) drives 5.1's lists and 5.2's
//! area sum.

pub mod arr_sequestration;
pub mod avoided_deforestation;
pub mod biodiversity_uplift;
pub mod climate_resilience;
pub mod config;
pub mod ecological_connectivity;
pub mod general_benefit;
pub mod habitat_loss_avoided;
pub mod inventory;
pub mod microclimate;
pub mod net_carbon;
pub mod net_errs;
pub mod threatened_species;
pub mod util;
pub mod watershed_protection;

use std::sync::{Condvar, Mutex};

use serde_json::{Value, json};

use crate::common::Aoi;
use crate::py::fsum_py as py_sum;
use crate::settings::Settings;
use crate::{Error, Result};
use config::{DEFAULT_BUFFER, DEFAULT_LEAKAGE, DEFAULT_UNCERTAINTY};
use util::{na, truthy};

/// 4.2's `values.by_category` as ordered `(key, info)` pairs.
pub type ByCategory = Vec<(String, Value)>;

/// `_activity_stage`: 4.2's `values.by_category` as ordered pairs, `None` when 4.2 carries none
/// (not applicable).
pub fn activity_stage(settings: &Settings, aoi: &Aoi) -> Result<Option<ByCategory>> {
    let result = crate::pathway::activity_list::analyze_activity_list(settings, aoi)?;
    Ok(result["values"]
        .get("by_category")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
}

/// The request POST /feature/benefit passes to `stream_benefit`.
#[derive(Clone, Debug)]
pub struct BenefitRequest {
    pub duration_years: i64,
    pub rate_pct: Option<f64>,
    pub carbon_project: bool,
    pub leakage: Option<f64>,
    pub uncertainty: Option<f64>,
    pub buffer: Option<f64>,
    pub ecosystem_class: i64,
    pub selections: Option<Value>,
    pub wanted: Option<Vec<String>>,
    pub people_benefits: Option<Value>,
}

/// `PEOPLE_BENEFIT_KEYS`
pub const PEOPLE_BENEFIT_KEYS: [&str; 6] = [
    "food_water",
    "livelihood",
    "social",
    "equity",
    "tenure",
    "cultural",
];

const DEACTIVATED: &str =
    "This section is deactivated: the project was not marked as an NbS carbon project.";

/// `_W`, in the `_components` dict order (the order `_order` sorts stably).
pub const WEIGHTS: [(&str, f64); 15] = [
    ("assumptions", 0.1),
    ("activity_stage", 2.0),
    ("general_benefit", 0.1),
    ("avoided_emissions", 4.0),
    ("arr_sequestration", 9.0),
    ("net_emission_reduction", 0.1),
    ("net_carbon_removal", 0.1),
    ("net_errs", 0.1),
    ("ecological_connectivity", 5.0),
    ("watershed_protection", 3.0),
    ("habitat_loss_avoided", 27.0),
    ("threatened_species_habitat", 25.0),
    ("biodiversity_uplift", 30.0),
    ("climate_resilience", 9.0),
    ("microclimate", 6.5),
];

const MAX_WORKERS: usize = 6;

fn deps_of(name: &str) -> &'static [&'static str] {
    match name {
        "general_benefit" | "avoided_emissions" => &["activity_stage"],
        "net_emission_reduction" => &["avoided_emissions"],
        "net_carbon_removal" => &["arr_sequestration"],
        "net_errs" => &["avoided_emissions", "arr_sequestration"],
        _ => &[],
    }
}

fn weight(name: &str) -> f64 {
    WEIGHTS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(0.0, |(_, w)| *w)
}

/// `_order`: cheapest expected finish first, dependencies ahead of dependents (stable sort).
pub fn order() -> Vec<&'static str> {
    fn finishes_at(name: &str) -> f64 {
        weight(name)
            + deps_of(name)
                .iter()
                .map(|d| finishes_at(d))
                .fold(0.0, |m: f64, x| if x > m { x } else { m })
    }
    let mut names: Vec<&'static str> = WEIGHTS.iter().map(|(n, _)| *n).collect();
    names.sort_by(|a, b| finishes_at(a).total_cmp(&finishes_at(b)));
    names
}

/// `validate`: the GUI's per-field ranges plus the 4.4 sum rule.
pub fn validate(duration_years: i64, leakage: f64, uncertainty: f64, buffer: f64) -> Result<()> {
    let _ = duration_years;
    for (name, value) in [
        ("leakage", leakage),
        ("uncertainty", uncertainty),
        ("buffer", buffer),
    ] {
        if !(1.0..=100.0).contains(&value) {
            return Err(Error::new(
                "ValueError",
                format!("{name} must be between 1 and 100 percent."),
            ));
        }
    }
    if leakage + uncertainty + buffer > 100.0 {
        return Err(Error::new(
            "ValueError",
            "Total deductions cannot exceed 100%.",
        ));
    }
    Ok(())
}

/// `_filter_stage`: 4.2's by_category reduced to the pathway screen's selections (5.1 only).
pub fn filter_stage(by_category: &ByCategory, selections: Option<&Value>) -> ByCategory {
    let Some(selections) = selections.filter(|s| truthy(s)) else {
        return by_category.clone();
    };
    let mut out = Vec::new();
    for (key, info) in by_category {
        let label = key.rsplit('|').next().unwrap_or(key).trim();
        let selection_key = match label {
            "Dryland forest" | "Savanna" => Some("forest"),
            "Mangrove" => Some("mangrove"),
            "Peatland" => Some("peatland"),
            _ => None,
        };
        let Some(selection_key) = selection_key else {
            out.push((key.clone(), info.clone()));
            continue;
        };
        let pathway = info
            .get("pathway")
            .map(util::py_str)
            .unwrap_or_default()
            .to_lowercase();
        let chosen = selections.get(selection_key);
        let Some(chosen) = chosen.filter(|c| c.is_object()) else {
            continue;
        };
        if !chosen.get(&pathway).is_some_and(truthy) {
            continue;
        }
        let ids: Vec<String> = match chosen.get("activities") {
            Some(Value::Array(a)) => a.iter().map(util::py_str).collect(),
            _ => Vec::new(),
        };
        let mut info = info.clone();
        let activities: Vec<Value> = info
            .get("activities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let activities = if ids.is_empty() {
            activities
        } else {
            activities
                .into_iter()
                .filter(|a| {
                    ids.contains(&util::py_str(a.get("activity_id").unwrap_or(&Value::Null)))
                })
                .collect()
        };
        if let Some(o) = info.as_object_mut() {
            o.insert("activities".into(), Value::Array(activities));
        }
        out.push((key.clone(), info));
    }
    out
}

/// One component's `(results, view_results)`, plus the activity stage when it is that step.
#[derive(Clone, Debug)]
struct Out {
    results: Value,
    view: Value,
    stage: Option<ByCategory>,
}

impl Out {
    fn pair((results, view): (Value, Value)) -> Self {
        Self {
            results,
            view,
            stage: None,
        }
    }
    fn failed(&self) -> bool {
        self.results.get("failed").is_some_and(truthy)
    }
    /// `_gross_of`
    fn gross(&self) -> Option<f64> {
        if self.view.get("applicable").is_some_and(truthy) {
            self.view.get("total_tco2e").and_then(Value::as_f64)
        } else {
            None
        }
    }
}

/// `pipeline.error_status`, `null` when clean.
pub fn error_status(results: &Value) -> Value {
    crate::pipeline::error_status(results).unwrap_or(Value::Null)
}

struct Params<'a> {
    settings: &'a Settings,
    aoi: &'a Aoi,
    duration: i64,
    rate_pct: Option<f64>,
    carbon_project: bool,
    leakage: f64,
    uncertainty: f64,
    buffer: f64,
    ecosystem_class: i64,
    selections: Option<&'a Value>,
    people_benefits: Option<&'a Value>,
    workers: usize,
}

/// The component body of `_components`, given its dependencies' outputs.
fn compute(p: &Params, name: &str, deps: &[&Out]) -> Result<Out> {
    let stage = || deps.first().and_then(|d| d.stage.as_ref());
    Ok(match name {
        "assumptions" => {
            let mut view = json!({
                "duration_years": p.duration,
                "carbon_project": p.carbon_project,
                "carbon_risk": {"leakage_percentage": p.leakage, "uncertainty_percentage": p.uncertainty, "buffer_percentage": p.buffer},
            });
            if let Some(s) = p.selections {
                view["selections"] = s.clone();
            }
            let pb = p.people_benefits.filter(|v| truthy(v));
            let slots: serde_json::Map<String, Value> = PEOPLE_BENEFIT_KEYS
                .iter()
                .map(|k| {
                    (
                        k.to_string(),
                        pb.and_then(|v| v.get(*k)).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect();
            view["people_benefits"] = Value::Object(slots);
            Out::pair((json!({"flags": [], "missing": []}), view))
        }
        "activity_stage" => {
            let by_category = activity_stage(p.settings, p.aoi)?;
            Out {
                results: json!({}),
                view: json!({}),
                stage: by_category,
            }
        }
        "general_benefit" => {
            let filtered = stage().map(|b| filter_stage(b, p.selections));
            Out::pair(general_benefit::analyze_general_benefit(filtered.as_ref()))
        }
        "avoided_emissions" => Out::pair(
            avoided_deforestation::analyze_avoided_deforestation_emissions(
                p.settings,
                p.aoi,
                p.duration,
                p.rate_pct,
                stage(),
            )?,
        ),
        "arr_sequestration" => Out::pair(arr_sequestration::analyze_arr_sequestration(
            p.settings, p.aoi, p.duration,
        )?),
        "net_emission_reduction" => Out::pair(if !p.carbon_project {
            na(DEACTIVATED)
        } else {
            match deps[0].gross() {
                None => na(
                    "5.2 is not applicable on this site, so there is no gross emission reduction to deduct from.",
                ),
                Some(g) => {
                    net_carbon::net_emission_reduction(g, p.leakage, p.uncertainty, p.buffer)
                }
            }
        }),
        "net_carbon_removal" => Out::pair(if !p.carbon_project {
            na(DEACTIVATED)
        } else {
            match deps[0].gross() {
                None => na(
                    "5.3 is not applicable on this site, so there is no gross removal to deduct from.",
                ),
                Some(g) => net_carbon::net_carbon_removal(g, p.leakage, p.uncertainty, p.buffer),
            }
        }),
        "net_errs" => Out::pair(if !p.carbon_project {
            na(DEACTIVATED)
        } else {
            // `_gross_of(x) or 0.0`
            let er = deps[0].gross().filter(|g| *g != 0.0).unwrap_or(0.0);
            let removal = deps[1].gross().filter(|g| *g != 0.0).unwrap_or(0.0);
            if er + removal <= 0.0 {
                na(
                    "Neither 5.2 nor 5.3 is applicable on this site, so there are no gross ERRs to deduct from.",
                )
            } else {
                net_errs::net_errs(er, removal, p.leakage, p.uncertainty, p.buffer, p.duration)
            }
        }),
        "ecological_connectivity" => {
            Out::pair(ecological_connectivity::analyze_ecological_connectivity(
                p.settings, p.aoi, p.duration,
            )?)
        }
        "watershed_protection" => Out::pair(watershed_protection::analyze_watershed_protection(
            p.settings, p.aoi,
        )?),
        "habitat_loss_avoided" => Out::pair(habitat_loss_avoided::analyze_habitat_loss_avoided(
            p.settings,
            p.aoi,
            p.duration,
            p.rate_pct,
            p.ecosystem_class,
            p.workers,
        )?),
        "threatened_species_habitat" => {
            Out::pair(threatened_species::analyze_threatened_species_habitat(
                p.settings,
                p.aoi,
                p.duration,
                p.rate_pct,
                p.ecosystem_class,
                p.workers,
            )?)
        }
        "biodiversity_uplift" => Out::pair(biodiversity_uplift::analyze_biodiversity_uplift(
            p.settings, p.aoi, p.duration, p.workers,
        )?),
        "climate_resilience" => Out::pair(climate_resilience::analyze_climate_resilience(
            p.settings, p.aoi, p.duration,
        )?),
        "microclimate" => Out::pair(microclimate::analyze_microclimate(
            p.settings,
            p.aoi,
            p.duration,
            p.ecosystem_class,
        )?),
        other => return Err(Error::new("KeyError", other.to_string())),
    })
}

/// `pipeline.safe` / `after`: a raising component becomes `failed` with only its exception type
/// on the wire; a dependent of a failed component is marked retryable.
fn run_safe(p: &Params, name: &str, deps: &[&Out]) -> Out {
    let mut out = match compute(p, name, deps) {
        Ok(o) => o,
        Err(e) => Out {
            results: json!({"failed": true, "flags": [format!("{name} could not be computed ({}).", e.kind)]}),
            view: json!({}),
            stage: None,
        },
    };
    if deps.iter().any(|d| d.failed())
        && let Some(o) = out.results.as_object_mut()
    {
        o.insert("retryable".into(), json!(true));
    }
    out
}

/// `stream_benefit`: the NDJSON lines (as JSON values, in order) handed to `emit` as soon as
/// each is ready, cheapest first. Inputs are validated before any line, as the route does.
/// `workers` is the species pool width (AOH_MAX_WORKERS).
pub fn stream_benefit(
    settings: &Settings,
    aoi: &Aoi,
    req: &BenefitRequest,
    workers: usize,
    mut emit: impl FnMut(Value),
) -> Result<()> {
    let leakage = req.leakage.unwrap_or(DEFAULT_LEAKAGE);
    let uncertainty = req.uncertainty.unwrap_or(DEFAULT_UNCERTAINTY);
    let buffer = req.buffer.unwrap_or(DEFAULT_BUFFER);
    validate(req.duration_years, leakage, uncertainty, buffer)?;
    let p = Params {
        settings,
        aoi,
        duration: req.duration_years,
        rate_pct: req.rate_pct,
        carbon_project: req.carbon_project,
        leakage,
        uncertainty,
        buffer,
        ecosystem_class: req.ecosystem_class,
        selections: req.selections.as_ref(),
        people_benefits: req.people_benefits.as_ref(),
        workers,
    };
    let order = order();
    let wanted = req.wanted.as_ref();
    let emitted: Vec<&str> = order
        .iter()
        .copied()
        .filter(|n| *n != "activity_stage" && wanted.is_none_or(|w| w.iter().any(|x| x == n)))
        .collect();
    // dependencies of the emitted set, closed transitively: run, not emitted
    let mut running: Vec<&str> = Vec::new();
    let mut stack: Vec<&str> = emitted.clone();
    while let Some(n) = stack.pop() {
        if running.contains(&n) {
            continue;
        }
        running.push(n);
        stack.extend(deps_of(n).iter().copied());
    }
    let run_order: Vec<&str> = order
        .iter()
        .copied()
        .filter(|n| running.contains(n))
        .collect();

    // plan_for
    let mut plan = vec![json!({"name": "preparation", "w": 0.1})];
    plan.extend(emitted.iter().map(|n| json!({"name": n, "w": weight(n)})));
    plan.push(json!({"name": "end", "w": 0.1}));
    let total_w = crate::py::round_ndigits(py_sum(plan.iter().filter_map(|x| x["w"].as_f64())), 2);
    let last = plan.len() - 1;
    let line = |step: usize, data: Value, status: Value| {
        json!({
            "process": plan[step]["name"], "data": data, "error_status": status, "w": total_w, "a": plan[step]["w"],
            "next": if step < last { plan[step + 1]["name"].clone() } else { Value::Null },
        })
    };
    emit(line(0, json!({"processes": plan}), Value::Null));

    let done: Mutex<Vec<(&str, Out)>> = Mutex::new(Vec::new());
    let ready = Condvar::new();
    let next_job = Mutex::new(0usize);
    let wait_for = |names: &[&str]| -> Vec<Out> {
        let mut g = done.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let found: Vec<Out> = names
                .iter()
                .filter_map(|n| g.iter().find(|(k, _)| k == n).map(|(_, o)| o.clone()))
                .collect();
            if found.len() == names.len() {
                return found;
            }
            g = ready.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    };
    std::thread::scope(|s| {
        for _ in 0..run_order.len().clamp(1, MAX_WORKERS) {
            s.spawn(|| {
                loop {
                    let job = {
                        let mut j = next_job.lock().unwrap_or_else(|e| e.into_inner());
                        let k = *j;
                        *j += 1;
                        k
                    };
                    let Some(name) = run_order.get(job).copied() else {
                        break;
                    };
                    let deps = wait_for(deps_of(name));
                    let out = run_safe(&p, name, &deps.iter().collect::<Vec<_>>());
                    done.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((name, out));
                    ready.notify_all();
                }
            });
        }
        for (step, name) in emitted.iter().enumerate() {
            let out = wait_for(&[name]).remove(0);
            emit(line(step + 1, out.view.clone(), error_status(&out.results)));
        }
    });
    emit(line(last, json!({}), Value::Null));
    Ok(())
}

/// `stream_benefit` for the route: inputs validated first (an `Err` is the route's 400), then the
/// NDJSON lines (`pipeline::pack`) over a one-slot channel from a producer thread, as
/// `pipeline::spawn_lines`. No retry URL: the endpoint is POST.
pub fn stream_benefit_lines(
    settings: std::sync::Arc<Settings>,
    aoi: std::sync::Arc<Aoi>,
    req: BenefitRequest,
    workers: usize,
) -> Result<std::sync::mpsc::Receiver<String>> {
    validate(
        req.duration_years,
        req.leakage.unwrap_or(DEFAULT_LEAKAGE),
        req.uncertainty.unwrap_or(DEFAULT_UNCERTAINTY),
        req.buffer.unwrap_or(DEFAULT_BUFFER),
    )?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = stream_benefit(&settings, &aoi, &req, workers, |v| {
            let _ = tx.send(crate::pipeline::pack(&v));
        });
    });
    Ok(rx)
}

/// `run_benefit`: every emitted component's view as one `{process: data}` object (scripts and
/// offline checks; the endpoint serves `stream_benefit`). The original runs it without
/// `pipeline.safe`, so the first failing component's error is returned.
pub fn run_benefit(
    settings: &Settings,
    aoi: &Aoi,
    req: &BenefitRequest,
    workers: usize,
) -> Result<Value> {
    let req = BenefitRequest {
        wanted: None,
        selections: None,
        people_benefits: None,
        ..req.clone()
    };
    let mut out = serde_json::Map::new();
    let mut lines = Vec::new();
    stream_benefit(settings, aoi, &req, workers, |l| lines.push(l))?;
    for l in lines {
        let name = l["process"].as_str().unwrap_or_default().to_string();
        if name == "preparation" || name == "end" {
            continue;
        }
        if l["error_status"]["state"] == "partial" && l["data"] == json!({}) {
            let msg = l["error_status"]["messages"][0]
                .as_str()
                .unwrap_or_default()
                .to_string();
            return Err(Error::new("RuntimeError", msg));
        }
        out.insert(name, l["data"].clone());
    }
    Ok(Value::Object(out))
}
