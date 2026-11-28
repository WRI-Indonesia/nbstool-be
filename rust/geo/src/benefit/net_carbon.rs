//! 5.4 Net carbon emission reduction and 5.5 Net carbon sequestration (`benefit/net_carbon.py`).

use serde_json::{Value, json};

use crate::py::fmt_grouped;

fn deductions(gross: f64, leakage: f64, uncertainty: f64, buffer: f64) -> (f64, f64, f64, f64) {
    let leakage_t = gross * leakage / 100.0;
    let uncertainty_t = gross * uncertainty / 100.0;
    let buffer_t = gross * buffer / 100.0;
    let net = gross - leakage_t - uncertainty_t - buffer_t;
    (leakage_t, uncertainty_t, buffer_t, net)
}

fn pair(
    gross: f64,
    leakage: f64,
    uncertainty: f64,
    buffer: f64,
    narrative: String,
    d: (f64, f64, f64, f64),
) -> (Value, Value) {
    let values = json!({
        "gross_tco2e": gross,
        "leakage_pct": leakage,
        "leakage_tco2e": d.0,
        "uncertainty_pct": uncertainty,
        "uncertainty_tco2e": d.1,
        "buffer_pct": buffer,
        "buffer_tco2e": d.2,
        "net_tco2e": d.3,
    });
    let mut view = json!({"applicable": true, "narrative": narrative});
    if let (Some(v), Some(src)) = (view.as_object_mut(), values.as_object()) {
        v.extend(src.clone());
    }
    (
        json!({"narrative": narrative, "tables": {}, "values": values, "flags": []}),
        view,
    )
}

/// Component 5.4.
pub fn net_emission_reduction(
    gross_er: f64,
    leakage: f64,
    uncertainty: f64,
    buffer: f64,
) -> (Value, Value) {
    let d = deductions(gross_er, leakage, uncertainty, buffer);
    let narrative = format!(
        "Net carbon emissions reduction is estimated at {} tCO2e after applying deductions to the total estimated \
         carbon reduction of {} tCO2e.",
        fmt_grouped(d.3, 2),
        fmt_grouped(gross_er, 2)
    );
    pair(gross_er, leakage, uncertainty, buffer, narrative, d)
}

/// Component 5.5.
pub fn net_carbon_removal(
    gross_removal: f64,
    leakage: f64,
    uncertainty: f64,
    buffer: f64,
) -> (Value, Value) {
    let d = deductions(gross_removal, leakage, uncertainty, buffer);
    let narrative = format!(
        "Net carbon sequestration is estimated at {} tCO2e after applying deductions to the total estimated carbon \
         sequestration of {} tCO2e.",
        fmt_grouped(d.3, 2),
        fmt_grouped(gross_removal, 2)
    );
    pair(gross_removal, leakage, uncertainty, buffer, narrative, d)
}
