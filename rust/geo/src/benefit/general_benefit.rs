//! 5.1 General Benefit (`benefit/general_benefit.py`): per pathway, its activities and their
//! benefits split into nature / climate / people. Reads 4.2's `by_category` (ordered).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::ByCategory;
use super::util::na;

const PILLAR_KEYS: [(&str, &str); 3] = [
    ("benefit_nature", "nature"),
    ("benefit_climate", "climate"),
    ("benefit_people", "people"),
];
const PATHWAY_REPORT_ORDER: [&str; 3] = ["Protect", "Manage", "Restore"];

/// `_split_benefit_phrases`
fn split_benefit_phrases(cell: &str) -> Vec<String> {
    cell.split(';')
        .map(|part| {
            part.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .trim_end_matches('.')
                .trim()
                .to_string()
        })
        .filter(|p| !p.is_empty())
        .collect()
}

struct Slot {
    activities: Vec<(Value, Value)>, // dict activity_id -> text, insertion order
    benefits: [BTreeSet<String>; 3],
}

/// Component 5.1. `by_category` None = 4.2 absent (the KeyError branch).
pub fn analyze_general_benefit(by_category: Option<&ByCategory>) -> (Value, Value) {
    let Some(by_category) = by_category else {
        return na(
            "The activity list (4.2) is not available for this project area. Run F02-P4 for this AOI first.",
        );
    };
    if by_category.is_empty() {
        return na(
            "The activity list (4.2) found no categories in this project area, so there are no activities or \
             benefits to list.",
        );
    }
    let mut collected: Vec<(String, Slot)> = Vec::new();
    for (_, info) in by_category {
        let activities = info
            .get("activities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if activities.is_empty() {
            continue;
        }
        let pathway = match info.get("pathway") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => "Unknown".to_string(),
        };
        let idx = match collected.iter().position(|(p, _)| *p == pathway) {
            Some(i) => i,
            None => {
                collected.push((
                    pathway,
                    Slot {
                        activities: Vec::new(),
                        benefits: Default::default(),
                    },
                ));
                collected.len() - 1
            }
        };
        let slot = &mut collected[idx].1;
        for activity in &activities {
            let id = activity.get("activity_id").cloned().unwrap_or(json!(""));
            if !slot.activities.iter().any(|(k, _)| *k == id) {
                slot.activities
                    .push((id, activity.get("activity").cloned().unwrap_or(json!(""))));
            }
            for (k, (col, _)) in PILLAR_KEYS.iter().enumerate() {
                let cell = match activity.get(*col) {
                    Some(Value::String(s)) => s.as_str(),
                    _ => "", // `activity.get(col, "") or ""`
                };
                slot.benefits[k].extend(split_benefit_phrases(cell));
            }
        }
    }
    if collected.is_empty() {
        return na(
            "No activity in this project area declares a benefit, so no benefits can be listed.",
        );
    }
    let mut by_pathway = serde_json::Map::new();
    let mut present = Vec::new();
    let mut activity_rows = Vec::new();
    let mut benefit_rows = Vec::new();
    for pw in PATHWAY_REPORT_ORDER {
        let Some((_, slot)) = collected.iter().find(|(p, _)| p == pw) else {
            continue;
        };
        let activities: Vec<Value> = slot
            .activities
            .iter()
            .map(|(id, text)| json!({"activity_id": id, "activity": text}))
            .collect();
        let mut benefits = serde_json::Map::new();
        for (k, (_, pillar)) in PILLAR_KEYS.iter().enumerate() {
            benefits.insert(
                pillar.to_string(),
                json!(slot.benefits[k].iter().collect::<Vec<_>>()),
            );
        }
        for (id, text) in &slot.activities {
            activity_rows.push(json!({"pathway": pw, "activity_id": id, "activity": text}));
        }
        for (k, (_, pillar)) in PILLAR_KEYS.iter().enumerate() {
            for b in &slot.benefits[k] {
                benefit_rows.push(json!({"pathway": pw, "pillar": pillar, "benefit": b}));
            }
        }
        by_pathway.insert(
            pw.to_string(),
            json!({"activities": activities, "benefits": benefits}),
        );
        present.push(pw);
    }
    let results = json!({
        "narrative": "",
        "tables": {"activities": activity_rows, "benefits": benefit_rows},
        "values": {"by_pathway": by_pathway, "pathways_present": present},
        "flags": [],
    });
    let view = json!({
        "applicable": true,
        "narrative": "",
        "pathways_present": present,
        "activities": activity_rows,
        "benefits": benefit_rows,
    });
    (results, view)
}
