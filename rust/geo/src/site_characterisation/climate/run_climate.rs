//! The seams of `climate/run_climate.py` that change output: `_carbon_shares` (the endpoint's
//! carbon total and three shares over SOIL + AGB + BGB, given 3.1's and 3.2's outputs).

use serde_json::{Map, Value, json};

use crate::Result;
use crate::py::fsum_py;

/// `CARBON_SHARE_FIELDS`: (pool, view field), in order.
pub const CARBON_SHARE_FIELDS: [(&str, &str); 3] = [
    ("above_ground_biomass", "above_ground_biomass_number"),
    ("below_ground_biomass", "below_ground_biomass_number"),
    ("soil_organic_carbon", "soil_organic_carbon_number"),
];

/// Python `sum(values)` of JSON numbers: exact int while every item is an int, then CPython's
/// compensated float path from the int prefix on.
fn py_sum_numbers(values: &[&Value]) -> Value {
    let mut int_total: i64 = 0;
    let mut k = 0;
    while k < values.len() {
        match values[k].as_i64() {
            Some(i) if !values[k].is_f64() => int_total += i,
            _ => break,
        }
        k += 1;
    }
    if k == values.len() {
        return json!(int_total);
    }
    let rest = values[k..].iter().map(|v| v.as_f64().unwrap_or(0.0));
    json!(fsum_py(std::iter::once(int_total as f64).chain(rest)))
}

/// `_carbon_shares(aoi, carbon, soc)`: given 3.1's and 3.2's `(results, view_results)`.
pub fn carbon_shares(carbon: &(Value, Value), soc: &(Value, Value)) -> Result<(Value, Value)> {
    // {**carbon_view, **soc_view}.get(field): the later dict wins
    let get = |field: &str| {
        soc.1
            .get(field)
            .or_else(|| carbon.1.get(field))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let values: Vec<(&str, Value)> = CARBON_SHARE_FIELDS
        .iter()
        .map(|(pool, field)| (*pool, get(field)))
        .collect();
    let present: Vec<&Value> = values
        .iter()
        .map(|(_, v)| v)
        .filter(|v| !v.is_null())
        .collect();
    let total = py_sum_numbers(&present);
    let t = total.as_f64().unwrap_or(0.0);

    let mut view = Map::new();
    for (pool, value) in &values {
        let share = match value.as_f64() {
            Some(v) if t > 0.0 => json!(v / t * 100.0),
            _ => Value::Null,
        };
        view.insert(format!("{pool}_percentage"), share);
    }
    view.insert(
        "total_carbon_storage".into(),
        if present.is_empty() {
            Value::Null
        } else {
            total.clone()
        },
    );

    let missing: Vec<&str> = values
        .iter()
        .filter(|(_, v)| v.is_null())
        .map(|(p, _)| *p)
        .collect();
    let mut flags: Vec<String> = Vec::new();
    if !missing.is_empty() {
        flags.push(format!(
            "carbon shares: {} did not report, so `total_carbon_storage` and the percentages divide by a partial total \
             and do not describe the whole carbon stock.",
            missing.join(", ")
        ));
    }
    let mut vals: Map<String, Value> = values
        .into_iter()
        .map(|(p, v)| (p.to_string(), v))
        .collect();
    vals.insert("share_denominator".into(), total);
    Ok((
        json!({"narrative": "", "tables": {}, "values": vals, "flags": flags}),
        Value::Object(view),
    ))
}
