//! `pathway/run_pathway.py`: F02-P4 as ONE JSON document for the Pathway Selection screen.
//! 4.1 -> 4.2 -> 4.3 in sequence; a failure is the whole answer's (`Err`), no per-card channel.

use serde_json::{Map, Value, json};

use super::by_ecosystem::{ECOSYSTEM_BUCKETS, analyze_by_ecosystem};
use super::pathway_distribution::analyze_pathway_distribution;
use crate::Result;
use crate::common::Aoi;
use crate::py::{fsum_py, safe_pct};
use crate::settings::Settings;

/// `INTERVENTIONS`: `PATHWAY_CODES` of `PATHWAY_ELIGIBLE_CODES`, in card order.
const INTERVENTIONS: [&str; 3] = ["Protect", "Manage", "Restore"];

/// `config.ECOSYSTEM_DISPLAY_NAMES`
fn display_name(name: &str) -> &str {
    match name {
        "Dryland" => "Forest",
        other => other,
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// `_activities_for`: distinct activities under one intervention, largest category first.
fn activities_for(categories: &Map<String, Value>, pathway: &str) -> Vec<Value> {
    let mut items: Vec<(&String, &Value)> = categories.iter().collect();
    items.sort_by(|a, b| f(&b.1["area_ha"]).total_cmp(&f(&a.1["area_ha"])));
    let mut seen: Vec<Value> = Vec::new();
    let mut out = Vec::new();
    for (key, info) in items {
        if info["pathway"].as_str() != Some(pathway) {
            continue;
        }
        for a in info
            .get("activities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = &a["activity_id"];
            if seen.contains(id) {
                continue;
            }
            seen.push(id.clone());
            let label = key.rsplit('|').next().unwrap_or(key).trim();
            out.push(
                json!({"activity_id": id, "activity": a["activity"], "ecosystem_label": label}),
            );
        }
    }
    out
}

/// `_ecosystem_card`
fn ecosystem_card(name: &str, info: &Value, project_area_ha: f64) -> Value {
    let area_ha = info["area_ha"].clone();
    let a = f(&area_ha);
    let empty = Map::new();
    let categories = info["categories"].as_object().unwrap_or(&empty);
    let mix = |k: &str| info["pathway_mix"].get(k).cloned().unwrap_or(json!(0.0));
    let interventions: Vec<Value> = INTERVENTIONS
        .iter()
        .map(|pw| {
            let m = mix(pw);
            json!({"intervention": pw, "area_ha": m, "percentage": safe_pct(f(&m), a), "eligible": f(&m) > 0.0,
                   "activities": activities_for(categories, pw)})
        })
        .collect();
    let inel = mix("Ineligible");
    json!({
        "ecosystem": name,
        "label": display_name(name),
        "present": a > 0.0,
        "total_area_ha": area_ha,
        "total_area_percentage": safe_pct(a, project_area_ha),
        "disturbed_area_ha": 0.0,
        "disturbed_percentage": 0.0,
        "is_disturbed": false,
        "interventions": interventions,
        "ineligible_area_ha": inel,
        "ineligible_percentage": safe_pct(f(&inel), a),
    })
}

fn scale_key(v: &mut Value, key: &str, scale: f64) {
    let x = f(&v[key]) * scale;
    v[key] = json!(x);
}

/// `min(100.0, x)`
fn cap100(v: &mut Value, key: &str) {
    let x = f(&v[key]);
    v[key] = json!(if x < 100.0 { x } else { 100.0 });
}

/// `_fit_to_site`: every area scaled by one factor when the cards plus Unclassified overcount.
fn fit_to_site(mut payload: Value) -> Value {
    let cards_total = fsum_py(
        payload["ecosystems"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| f(&c["total_area_ha"])),
    );
    let total = cards_total + f(&payload["unclassified_area_ha"]);
    if total <= f(&payload["project_area_ha"]) {
        return payload;
    }
    let scale = f(&payload["project_area_ha"]) / total;
    for key in [
        "eligible_area_ha",
        "eligible_percentage",
        "unclassified_area_ha",
        "unclassified_percentage",
    ] {
        scale_key(&mut payload, key, scale);
    }
    cap100(&mut payload, "eligible_percentage");
    for card in payload["ecosystems"].as_array_mut().into_iter().flatten() {
        for key in [
            "total_area_ha",
            "total_area_percentage",
            "ineligible_area_ha",
        ] {
            scale_key(card, key, scale);
        }
        cap100(card, "total_area_percentage");
        for i in card["interventions"].as_array_mut().into_iter().flatten() {
            scale_key(i, "area_ha", scale);
        }
    }
    payload
}

/// The three component results of one run (4.1, 4.2, 4.3), as JSON.
pub struct PathwayRun {
    pub distribution: Value,
    pub activity_list: Value,
    pub by_ecosystem: Value,
}

/// `run_pathway(aoi)` plus the 4.2 result it computed (the benefit runner's activity stage
/// reads the same 4.2).
pub fn run_pathway_components(settings: &Settings, aoi: &Aoi) -> Result<PathwayRun> {
    let distribution = analyze_pathway_distribution(settings, aoi)?;
    let activity_list = super::activity_list::analyze_activity_list(settings, aoi)?;
    let by_ecosystem = analyze_by_ecosystem(aoi, Some(&activity_list));
    Ok(PathwayRun {
        distribution,
        activity_list,
        by_ecosystem,
    })
}

/// `run_pathway(aoi)`: the Pathway Selection payload (`to_jsonable` of it).
pub fn run_pathway(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let run = run_pathway_components(settings, aoi)?;
    Ok(pathway_payload(aoi, &run))
}

/// The payload of one `PathwayRun`.
pub fn pathway_payload(aoi: &Aoi, run: &PathwayRun) -> Value {
    let (distribution, by_ecosystem) = (&run.distribution, &run.by_ecosystem);
    let empty = Map::new();
    let buckets = by_ecosystem["values"]
        .get("by_ecosystem")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut messages: Vec<Value> = Vec::new();
    for c in [&run.distribution, &run.activity_list, &run.by_ecosystem] {
        for k in ["flags", "missing"] {
            messages.extend(c[k].as_array().cloned().unwrap_or_default());
        }
    }
    let get = |v: &Value, k: &str| v["values"].get(k).cloned().unwrap_or(json!(0.0));
    let default_bucket = json!({"area_ha": 0.0, "pathway_mix": {}, "categories": {}});
    let cards: Vec<Value> = ECOSYSTEM_BUCKETS
        .iter()
        .map(|(name, _)| {
            ecosystem_card(
                name,
                buckets.get(*name).unwrap_or(&default_bucket),
                aoi.area_ha,
            )
        })
        .collect();
    fit_to_site(json!({
        "project_area_ha": aoi.area_ha,
        "applicable": by_ecosystem["applicable"],
        "eligible_area_ha": get(distribution, "eligible_ha"),
        "eligible_percentage": get(distribution, "eligible_pct"),
        "unclassified_area_ha": get(by_ecosystem, "unclassified_ha"),
        "unclassified_percentage": get(by_ecosystem, "unclassified_pct"),
        "ecosystems": cards,
        "duration_years": {"default": 30, "min": 10, "max": 70, "step": 10},
        "carbon_risk": {"leakage_percentage": 15.0, "uncertainty_percentage": 10.0, "buffer_percentage": 12.0},
        "messages": messages,
    }))
}
