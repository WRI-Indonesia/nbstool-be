//! Component 1.3 Protected Areas, WDPA (`general/protected_areas_wdpa.py`).
//!
//! `sea.wdpa_wdoecm` from the GIS database through `db::load_wdpa_intersecting`; the filters, the
//! union overlap and the narrative are the Python's line for line.

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Result;
use crate::common::Aoi;
use crate::db::{Feature, load_wdpa_intersecting, per_feature_overlap_ha, union_overlap_ha};
use crate::py::{fmt_grouped, oxford_join, safe_pct, sentences};

const WDPA_KEEP_STATUS: [&str; 3] = ["Designated", "Inscribed", "Established"];
const WDPA_DROP_REALM: &str = "Marine";
const WDPA_STRICT_IUCN: [&str; 3] = ["Ia", "Ib", "II"];
const WDPA_NO_DESIG: [&str; 5] = ["", "none", "not reported", "not applicable", "nan"];

struct ProtectedSite {
    name: String,
    designation: String,
    iucn_category: String,
    status: String,
    area_ha: f64,
}

/// `_unique_designations`: distinct DESIG_ENG by summed overlap, descending (stable).
fn unique_designations(sites: &[ProtectedSite]) -> Vec<String> {
    let mut totals: Vec<(String, f64)> = Vec::new();
    for s in sites {
        // shortcut: str.lower() as to_lowercase (same on the table's designations)
        if WDPA_NO_DESIG.contains(&s.designation.trim().to_lowercase().as_str()) {
            continue;
        }
        match totals.iter_mut().find(|(d, _)| *d == s.designation) {
            Some(t) => t.1 += s.area_ha,
            None => totals.push((s.designation.clone(), 0.0 + s.area_ha)),
        }
    }
    totals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    totals.into_iter().map(|(d, _)| d).collect()
}

/// `analyze_protected_areas(aoi)`
pub async fn analyze_protected_areas(gis: &PgPool, aoi: &Aoi) -> Result<(Value, Value)> {
    let rows = load_wdpa_intersecting(gis, aoi).await?;
    protected_areas_from(rows, aoi)
}

/// The body after the load: filters, overlap, narrative.
pub fn protected_areas_from(rows: Vec<Feature>, aoi: &Aoi) -> Result<(Value, Value)> {
    let gdf: Vec<Feature> = rows
        .into_iter()
        .filter(|f| {
            f.get("STATUS")
                .as_str()
                .is_some_and(|s| WDPA_KEEP_STATUS.contains(&s))
        })
        .filter(|f| f.str_of("REALM").trim() != WDPA_DROP_REALM)
        .filter(|f| {
            f.geom.as_ref().is_some_and(|g| {
                let t = gdal::vector::geometry_type_flatten(g.geometry_type());
                t == gdal_sys::OGRwkbGeometryType::wkbPolygon
                    || t == gdal_sys::OGRwkbGeometryType::wkbMultiPolygon
            })
        })
        .collect();

    if gdf.is_empty() {
        let results = json!({
            "narrative": "",
            "tables": {"sites": []},
            "values": {"protected_ha": 0.0, "protected_pct": 0.0, "in_strict_pa": false},
            "flags": [],
        });
        let view = json!({
            "overlapping_protected_area": 0.0,
            "overlapping_protected_name": null,
            "overlapping_protected_area_percentage": 0.0,
        });
        return Ok((results, view));
    }

    let protected_ha = union_overlap_ha(aoi, &gdf)?;
    let protected_pct = safe_pct(protected_ha, aoi.area_ha);

    let areas = per_feature_overlap_ha(aoi, &gdf)?;
    let mut sites: Vec<ProtectedSite> = gdf
        .iter()
        .zip(&areas)
        .map(|(f, a)| ProtectedSite {
            name: f.str_of("NAME"),
            designation: f.str_of("DESIG_ENG"),
            iucn_category: f.str_of("IUCN_CAT"),
            status: f.str_of("STATUS"),
            area_ha: *a,
        })
        .collect();
    // sort_by_area: sorted(reverse=True) is stable
    sites.sort_by(|a, b| {
        b.area_ha
            .partial_cmp(&a.area_ha)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let overlap_sentence = format!(
        "Besides, this project area overlaps with {} hectares of protected areas.",
        fmt_grouped(protected_ha, 0)
    );

    let mut missing: Vec<String> = Vec::new();
    let designations = unique_designations(&sites);
    let designation_sentence = if designations.is_empty() {
        missing.push(
            "1.3: overlapping WDPA sites carry no usable DESIG_ENG. The designation sentence is omitted.".into(),
        );
        String::new()
    } else {
        format!(
            "The protected area within the polygon is designated for {}.",
            oxford_join(&designations)
        )
    };

    let in_strict_pa = sites
        .iter()
        .any(|s| WDPA_STRICT_IUCN.contains(&s.iucn_category.as_str()));

    let site_rows: Vec<Value> = sites
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "designation": s.designation,
                "iucn_category": s.iucn_category,
                "status": s.status,
                "area_ha": s.area_ha,
            })
        })
        .collect();
    let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();

    let results = json!({
        "narrative": sentences(&[&overlap_sentence, &designation_sentence]),
        "tables": {"sites": site_rows},
        "values": {
            "protected_ha": protected_ha,
            "protected_pct": protected_pct,
            "in_strict_pa": in_strict_pa,
            "designations": designations,
        },
        "flags": [],
        "missing": missing,
    });
    let view = json!({
        "overlapping_protected_area": protected_ha,
        "overlapping_protected_name": names.join(", "),
        "overlapping_protected_area_percentage": protected_pct,
    });
    Ok((results, view))
}
