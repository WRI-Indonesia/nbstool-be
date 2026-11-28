//! 4.1 Pathway Distribution (`pathway/pathway_distribution.py`): area and share of the AOI for
//! each NBS pathway.
//!
//! Band 1 is tabulated (1 Protect, 2 Manage, 3 Restore, 4 Ineligible); code 4 is listed but
//! excluded from the eligible headline. Denominator is the total AOI area, with an Unclassified
//! row for code 0 and nodata, so the table sums to 100. Bands 2 and 3 are not tabulated; their
//! codes-present pass through `values` for the F02-P5 activity join on `(cat_code, ecosystem)`.

use serde_json::{Map, Value, json};

use crate::Result;
use crate::common::{Aoi, Resampling, load_raster_clipped};
use crate::config::{
    PATHWAY_BAND, PATHWAY_CATCODE_BAND, PATHWAY_CATCODE_LABELS, PATHWAY_CODES,
    PATHWAY_ECOSYSTEM_BAND, PATHWAY_ECOSYSTEM_CODES, PATHWAY_ELIGIBLE_CODES, PATHWAY_RASTER,
    PATHWAY_UNCLASSIFIED_WARN_PCT,
};
use crate::py::{fsum_py, safe_pct};
use crate::settings::Settings;

const COMPONENT: &str = "4.1 Pathway Distribution";
const UNCLASSIFIED_LABEL: &str = "Unclassified";

/// One row of the pathway breakdown (`PathwayShare`).
#[derive(Clone)]
struct PathwayShare {
    code: i64,
    label: String,
    area_ha: f64,
    pct: f64,
    is_pathway: bool,
}

impl PathwayShare {
    fn to_json(&self) -> Value {
        json!({"code": self.code, "label": self.label, "area_ha": self.area_ha, "pct": self.pct,
               "is_pathway": self.is_pathway})
    }
}

fn label_of(labels: &[(i64, &str)], c: i64) -> String {
    labels
        .iter()
        .find(|(k, _)| *k == c)
        .map_or_else(|| format!("Unknown code {c}"), |(_, l)| l.to_string())
}

/// `_codes_present`: which codes occur in one pass-through band, with labels.
fn codes_present(
    settings: &Settings,
    aoi: &Aoi,
    band: usize,
    labels: &[(i64, &str)],
) -> Result<(Vec<i64>, Vec<String>)> {
    let raster = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        band,
        None,
    )?;
    let mut codes: Vec<i64> = raster
        .unique_counts()
        .into_iter()
        .map(|(c, _)| c as i64) // int(c)
        .filter(|c| *c != 0) // 0 is mask / none, not a category
        .collect();
    codes.sort();
    codes.dedup();
    let names = codes.iter().map(|c| label_of(labels, *c)).collect();
    Ok((codes, names))
}

/// `ComponentResult` as `to_jsonable` writes it.
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

/// `analyze_pathway_distribution`
pub fn analyze_pathway_distribution(settings: &Settings, aoi: &Aoi) -> Result<Value> {
    let primary = load_raster_clipped(
        settings,
        PATHWAY_RASTER,
        aoi,
        Resampling::Nearest,
        PATHWAY_BAND,
        None,
    )?;

    if primary.valid_area_ha() <= 0.0 {
        let reason =
            "The pathway layer does not cover this project area, so no pathway can be recommended.";
        return Ok(component_result(
            false,
            reason,
            json!({}),
            json!({}),
            vec![],
            vec![reason.into()],
        ));
    }

    // Denominator is the whole site. Code 0 and nodata are absorbed by an Unclassified row.
    let mut rows: Vec<PathwayShare> = Vec::new();
    let mut classified_ha = 0.0;
    for &(code, label) in &PATHWAY_CODES {
        if code == 0 {
            continue; // 0 is folded into Unclassified below, together with nodata
        }
        let area_ha = primary.count_eq(code as f64) as f64 * primary.pixel_area_ha;
        classified_ha += area_ha;
        rows.push(PathwayShare {
            code,
            label: label.into(),
            area_ha,
            pct: safe_pct(area_ha, aoi.area_ha),
            is_pathway: PATHWAY_ELIGIBLE_CODES.contains(&code),
        });
    }
    drop(primary);

    let unclassified_ha = 0.0f64.max(aoi.area_ha - classified_ha);
    rows.push(PathwayShare {
        code: 0,
        label: UNCLASSIFIED_LABEL.into(),
        area_ha: unclassified_ha,
        pct: safe_pct(unclassified_ha, aoi.area_ha),
        is_pathway: false,
    });

    let eligible_ha = fsum_py(rows.iter().filter(|r| r.is_pathway).map(|r| r.area_ha));
    let eligible_pct = safe_pct(eligible_ha, aoi.area_ha);

    let mut flags: Vec<String> = Vec::new();
    let unclassified_pct = safe_pct(unclassified_ha, aoi.area_ha);
    if unclassified_pct > PATHWAY_UNCLASSIFIED_WARN_PCT {
        flags.push(format!(
            "4.1: {unclassified_pct:.0}% of the AOI carries no pathway value. Every share \
             below describes only the remainder of the site."
        ));
    }

    // Bands 2 and 3 pass through untabulated, for the activity generator in F02-P5.
    let (eco_codes, eco_labels) = codes_present(
        settings,
        aoi,
        PATHWAY_ECOSYSTEM_BAND,
        &PATHWAY_ECOSYSTEM_CODES,
    )?;
    let (cat_codes, cat_labels) =
        codes_present(settings, aoi, PATHWAY_CATCODE_BAND, &PATHWAY_CATCODE_LABELS)?;

    let mut pathway_rows: Vec<&PathwayShare> = rows
        .iter()
        .filter(|r| r.is_pathway && r.area_ha > 0.0)
        .collect();
    pathway_rows.sort_by(|a, b| {
        b.area_ha
            .partial_cmp(&a.area_ha)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut pathway_ha = Map::new();
    for r in rows.iter().filter(|r| r.is_pathway) {
        pathway_ha.insert(r.label.clone(), json!(r.area_ha));
    }
    let values = json!({
        "chart_series": "pathway_distribution",
        "chart_unit": "%",
        "chart_axis_label": "Share of project area (%)",
        "eligible_ha": eligible_ha,
        "eligible_pct": eligible_pct,
        "pathway_ha": pathway_ha,
        "dominant_pathway": pathway_rows.first().map(|r| r.label.clone()),
        "unclassified_pct": unclassified_pct,
        "reference_ecosystem_codes": eco_codes,
        "reference_ecosystem_labels": eco_labels,
        "cat_code_codes": cat_codes,
        "cat_code_labels": cat_labels,
    });
    let tables =
        json!({"pathway_distribution": rows.iter().map(PathwayShare::to_json).collect::<Vec<_>>()});
    Ok(component_result(true, "", tables, values, flags, vec![]))
}
