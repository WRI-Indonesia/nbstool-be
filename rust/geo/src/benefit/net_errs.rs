//! 5.6 Estimated net emission reduction and removals (`benefit/net_errs.py`).

use serde_json::{Value, json};

use crate::py::fmt_grouped;

/// Component 5.6, the notebook cell's arithmetic in its order.
pub fn net_errs(
    gross_er: f64,
    gross_removal: f64,
    leakage: f64,
    uncertainty: f64,
    buffer: f64,
    project_duration: i64,
) -> (Value, Value) {
    let leakage_er = gross_er * leakage / 100.0;
    let uncertainty_er = gross_er * uncertainty / 100.0;
    let leakage_removal = gross_removal * leakage / 100.0;
    let uncertainty_removal = gross_removal * uncertainty / 100.0;
    let gross_err = gross_er + gross_removal;
    let adjusted_err = (gross_er - leakage_er - uncertainty_er)
        + (gross_removal - leakage_removal - uncertainty_removal);
    let buffer_err = adjusted_err * buffer / 100.0;
    let net_err = adjusted_err - buffer_err;
    let annual_err = net_err / project_duration as f64;
    let narrative = format!(
        "Over a {project_duration} year crediting period, the project area could generate an estimated {} tCO2e in \
         emission reductions and removals, equivalent to an average of {} tCO2e per year. This figure already \
         subtracts leakage and uncertainty deductions from the grosspotential of {} tCO2e. It does not yet subtract \
         the buffer pool contribution, which is held to cover non-permanence risk.",
        fmt_grouped(net_err, 0),
        fmt_grouped(annual_err, 0),
        fmt_grouped(gross_er, 0)
    );
    let values = json!({
        "gross_tco2e": gross_err,
        "gross_emission_reduction_tco2e": gross_er,
        "gross_removal_tco2e": gross_removal,
        "leakage_pct": leakage,
        "leakage_tco2e": leakage_er + leakage_removal,
        "uncertainty_pct": uncertainty,
        "uncertainty_tco2e": uncertainty_er + uncertainty_removal,
        "adjusted_tco2e": adjusted_err,
        "buffer_pct": buffer,
        "buffer_tco2e": buffer_err,
        "net_tco2e": net_err,
        "annual_mean_tco2e": annual_err,
        "duration_years": project_duration,
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
