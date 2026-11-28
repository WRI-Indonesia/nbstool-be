//! 4.3 Pathway & Activities by Ecosystem (`pathway/by_ecosystem.py`): 4.2 regrouped with
//! ECOSYSTEM as the primary axis. Recomputes nothing; savanna (code 4) is folded into Dryland.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::common::Aoi;
use crate::config::PATHWAY_ECOSYSTEM_CODES;
use crate::py::{fmt_ha, fsum_py, round_ndigits, safe_pct};

const COMPONENT: &str = "4.3 Pathway & Activities by Ecosystem";

/// `ECOSYSTEM_BUCKETS`: display name -> ecosystem codes it absorbs.
pub const ECOSYSTEM_BUCKETS: [(&str, &[i64]); 3] =
    [("Dryland", &[1, 4]), ("Mangrove", &[2]), ("Peatland", &[3])];
const PATHWAY_MIX_ORDER: [&str; 4] = ["Protect", "Manage", "Restore", "Ineligible"];

fn component_result(
    applicable: bool,
    narrative: &str,
    tables: Value,
    values: Value,
    flags: Vec<String>,
    missing: Vec<String>,
) -> Value {
    json!({"component": COMPONENT, "applicable": applicable, "narrative": narrative, "tables": tables,
           "values": values, "flags": flags, "missing": missing})
}

struct Bucket {
    area_ha: f64,
    pathway_mix: Map<String, Value>,
    rows: Vec<Value>,
    categories: Map<String, Value>,
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// `analyze_by_ecosystem(aoi, results)`, given 4.2's result JSON (None when absent).
pub fn analyze_by_ecosystem(aoi: &Aoi, activity_list: Option<&Value>) -> Value {
    let Some(src) = activity_list.filter(|s| s["applicable"] == json!(true)) else {
        let reason = "The activity list (4.2) is not available for this project area, so it cannot be grouped by ecosystem.";
        return component_result(
            false,
            reason,
            json!({}),
            json!({}),
            vec![],
            vec![reason.into()],
        );
    };
    let empty = Map::new();
    let by_category = src["values"]
        .get("by_category")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let mut buckets: Vec<Bucket> = ECOSYSTEM_BUCKETS
        .iter()
        .map(|_| Bucket {
            area_ha: 0.0,
            pathway_mix: Map::new(),
            rows: vec![],
            categories: Map::new(),
        })
        .collect();
    let mut savanna_ha = 0.0;
    let mut savanna_labels: BTreeSet<String> = BTreeSet::new();
    let mut off_bucket_ha = 0.0;
    let mut flags: Vec<String> = Vec::new();

    for (key, info) in by_category {
        let parts: Vec<&str> = key.split('|').map(str::trim).collect();
        let (cat_label, eco_label) = (
            parts.first().copied().unwrap_or(""),
            parts.get(1).copied().unwrap_or(""),
        );
        // `{label: code}`: the last code of a repeated label wins
        let eco_code = PATHWAY_ECOSYSTEM_CODES
            .iter()
            .rev()
            .find(|(_, l)| *l == eco_label)
            .map(|(c, _)| *c);
        let bucket = eco_code.and_then(|c| {
            ECOSYSTEM_BUCKETS
                .iter()
                .position(|(_, codes)| codes.contains(&c))
        });
        let area_ha = f(&info["area_ha"]);
        let pathway = info["pathway"].as_str().unwrap_or("").to_string();
        let acts: Vec<Value> = info
            .get("activities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let Some(bi) = bucket else {
            off_bucket_ha += area_ha;
            continue;
        };
        if eco_code == Some(4) {
            savanna_ha += area_ha;
            for a in &acts {
                let text = a.get("activity").and_then(Value::as_str).unwrap_or("");
                if text.to_lowercase().contains("savanna") {
                    savanna_labels.insert(text.to_string());
                }
            }
        }
        let name = ECOSYSTEM_BUCKETS[bi].0;
        let b = &mut buckets[bi];
        b.area_ha += area_ha;
        let prev = b.pathway_mix.get(&pathway).map_or(0.0, f);
        b.pathway_mix.insert(pathway.clone(), json!(prev + area_ha));
        b.categories.insert(
            format!("{cat_label} | {eco_label}"),
            json!({"pathway": pathway, "area_ha": area_ha, "activities": acts}),
        );
        let rounded = round_ndigits(area_ha, 1);
        if acts.is_empty() {
            let note = if pathway == "Ineligible" {
                "(ineligible, no activity)"
            } else {
                "(no catalog match)"
            };
            b.rows.push(json!({"ecosystem": name, "pathway": pathway, "category": cat_label, "area_ha": rounded,
                               "activity_id": "", "activity": note}));
        } else {
            for a in &acts {
                b.rows.push(json!({"ecosystem": name, "pathway": pathway, "category": cat_label, "area_ha": rounded,
                                   "activity_id": a["activity_id"], "activity": a["activity"]}));
            }
        }
    }

    let mut summary_rows = Vec::new();
    let mut activity_rows: Vec<Value> = Vec::new();
    let mut by_ecosystem = Map::new();
    for ((name, _), b) in ECOSYSTEM_BUCKETS.iter().zip(&buckets) {
        let mix = |k: &str| b.pathway_mix.get(k).map_or(0.0, f);
        let pct = safe_pct(b.area_ha, aoi.area_ha);
        summary_rows.push(json!({
            "ecosystem": name, "area_ha": b.area_ha, "pct": pct,
            "protect_ha": mix("Protect"), "manage_ha": mix("Manage"), "restore_ha": mix("Restore"),
            "ineligible_ha": mix("Ineligible"), "present": b.area_ha > 0.0,
        }));
        // sorted(rows, key=area_ha, reverse=True): stable, equal keys keep their order
        let mut rows = b.rows.clone();
        rows.sort_by(|x, y| f(&y["area_ha"]).total_cmp(&f(&x["area_ha"])));
        activity_rows.extend(rows);
        let ordered_mix: Map<String, Value> = PATHWAY_MIX_ORDER
            .iter()
            .filter_map(|pw| b.pathway_mix.get(*pw).map(|v| (pw.to_string(), v.clone())))
            .collect();
        by_ecosystem.insert(
            name.to_string(),
            json!({"area_ha": b.area_ha, "pct": pct, "pathway_mix": ordered_mix,
                                                     "categories": b.categories}),
        );
    }

    if off_bucket_ha > 0.0 {
        flags.push(format!(
            "4.3: {} has a category but no reference ecosystem; counted as Unclassified here, not under any ecosystem.",
            fmt_ha(off_bucket_ha)
        ));
    }
    let classified_ha = fsum_py(buckets.iter().map(|b| b.area_ha));
    let rest = aoi.area_ha - classified_ha - off_bucket_ha;
    let unclassified_ha = (if rest > 0.0 { rest } else { 0.0 }) + off_bucket_ha;
    let unclassified_pct = safe_pct(unclassified_ha, aoi.area_ha);

    if savanna_ha > 0.0 {
        flags.push(format!(
            "4.3: {} of savanna reference is reported under Dryland. cat_code and pathway are unchanged.",
            fmt_ha(savanna_ha)
        ));
        if !savanna_labels.is_empty() {
            flags.push(format!(
                "4.3: these activity labels still read 'savanna' under the Dryland group: {}",
                savanna_labels.into_iter().collect::<Vec<_>>().join("; ")
            ));
        }
    }

    let present: Vec<&str> = ECOSYSTEM_BUCKETS
        .iter()
        .zip(&buckets)
        .filter(|(_, b)| b.area_ha > 0.0)
        .map(|((n, _), _)| *n)
        .collect();
    component_result(
        true,
        "",
        json!({"ecosystem_summary": summary_rows, "ecosystem_activities": activity_rows}),
        json!({"by_ecosystem": by_ecosystem, "ecosystems_present": present, "unclassified_ha": unclassified_ha,
               "unclassified_pct": unclassified_pct}),
        flags,
        vec![],
    )
}
