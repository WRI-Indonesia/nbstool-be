//! Component 2.2 Key Biodiversity Areas (`nature/key_biodiversity_areas.py`).
//!
//! `sea.key_biodiversity_area` through `db::load_kba_intersecting`; union overlap for the
//! headline, per-site overlap for the list, as the Python.

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Result;
use crate::common::Aoi;
use crate::db::{Feature, load_kba_intersecting, per_feature_overlap_ha, union_overlap_ha};
use crate::py::{fmt_ha, fmt_pct, oxford_join, safe_pct, sentences};

const KBA_DEFINITION: &str = "Key Biodiversity Areas are sites that contribute significantly to the global persistence of biodiversity.";

/// `analyze_kba(aoi)`
pub async fn analyze_kba(gis: &PgPool, aoi: &Aoi) -> Result<(Value, Value)> {
    let rows = load_kba_intersecting(gis, aoi).await?;
    kba_from(rows, aoi)
}

/// The body after the load.
pub fn kba_from(gdf: Vec<Feature>, aoi: &Aoi) -> Result<(Value, Value)> {
    if gdf.is_empty() {
        let results = json!({
            "narrative": "This project area does not overlap any Key Biodiversity Areas.",
            "tables": {"sites": []},
            "values": {"kba_ha": 0.0, "kba_pct": 0.0, "kba_site_count": 0},
            "flags": [],
        });
        let view = json!({
            "overlapping_key_biodiversity_areas": [],
            "overlapping_key_biodiversity_area_total_size": 0.0,
            "overlapping_key_biodiversity_area_percentage": 0.0,
        });
        return Ok((results, view));
    }

    let kba_ha = union_overlap_ha(aoi, &gdf)?;
    let kba_pct = safe_pct(kba_ha, aoi.area_ha);

    let areas = per_feature_overlap_ha(aoi, &gdf)?;
    let mut sites: Vec<(String, f64)> = gdf
        .iter()
        .zip(areas)
        .map(|(f, a)| (f.str_of("IntName"), a))
        .collect();
    sites.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let head = if sites.len() == 1 {
        format!(
            "This project area overlaps {} ({}) of a Key Biodiversity Area, {}.",
            fmt_ha(kba_ha),
            fmt_pct(kba_pct, 0),
            sites[0].0
        )
    } else {
        let (largest, rest) = (&sites[0], &sites[1..]);
        let rest_text = oxford_join(
            &rest
                .iter()
                .map(|(n, a)| format!("{n} ({})", fmt_ha(*a)))
                .collect::<Vec<_>>(),
        );
        format!(
            "This project area overlaps {} ({}) of Key Biodiversity Areas, across {} sites. The largest is {} ({}), followed by {rest_text}.",
            fmt_ha(kba_ha),
            fmt_pct(kba_pct, 0),
            sites.len(),
            largest.0,
            fmt_ha(largest.1)
        )
    };

    let table: Vec<Value> = sites
        .iter()
        .map(|(n, a)| json!({"name": n, "area_ha": a}))
        .collect();
    let results = json!({
        "narrative": sentences(&[&head, KBA_DEFINITION]),
        "tables": {"sites": table},
        "values": {"kba_ha": kba_ha, "kba_pct": kba_pct, "kba_site_count": sites.len()},
        "flags": [],
    });
    let view = json!({
        "overlapping_key_biodiversity_areas": sites.iter().map(|(n, a)| json!({
            "overlapping_key_biodiversity_name": n,
            "overlapping_key_biodiversity_area_size": a,
        })).collect::<Vec<_>>(),
        "overlapping_key_biodiversity_area_total_size": kba_ha,
        "overlapping_key_biodiversity_area_percentage": kba_pct,
    });
    Ok((results, view))
}
