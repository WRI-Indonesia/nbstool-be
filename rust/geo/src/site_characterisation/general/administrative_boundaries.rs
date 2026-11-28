//! Component 1.2 Administrative Boundaries (`general/administrative_boundaries.py`).
//!
//! `public.adm_boundaries` clipped to the AOI in the database (`db::load_admin_intersecting`),
//! then geopandas' `dissolve(by=group_cols)` (groups in sorted key order, GEOS unary union of each
//! group's rows in row order), the intersection with the AOI, the 1% sliver rule and the
//! hierarchical dominance chain, as the Python.

use std::collections::{BTreeMap, HashMap};

use gdal::vector::Geometry;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Result;
use crate::common::{Aoi, M2_PER_HA};
use crate::db::{Feature, aoi_geometry, intersection_area, load_admin_intersecting, union_all};
use crate::py::{fmt_grouped, safe_pct, sentences};

const ADMIN_SLIVER_PCT: f64 = 1.0;

/// ADMIN_LEVELS: (group columns, name field, parent field)
const COUNTRY: (&[&str], &str, Option<&str>) = (&["COUNTRY"], "COUNTRY", None);
const PROVINCE: (&[&str], &str, Option<&str>) = (&["COUNTRY", "NAME_1"], "NAME_1", Some("COUNTRY"));
const DISTRICT: (&[&str], &str, Option<&str>) =
    (&["COUNTRY", "NAME_1", "NAME_2"], "NAME_2", Some("NAME_1"));
const SUBDISTRICT: (&[&str], &str, Option<&str>) = (
    &["COUNTRY", "NAME_1", "NAME_2", "NAME_3"],
    "NAME_3",
    Some("NAME_2"),
);
const VILLAGE: (&[&str], &str, Option<&str>) = (
    &["COUNTRY", "NAME_1", "NAME_2", "NAME_3", "NAME_4"],
    "NAME_4",
    Some("NAME_3"),
);

/// `AdminUnit` (dataclass equality: every field).
#[derive(Clone, Debug, PartialEq)]
struct AdminUnit {
    name: String,
    area_ha: f64,
    pct: f64,
    parent: Option<String>,
}

impl AdminUnit {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "area_ha": self.area_ha, "pct": self.pct, "parent": self.parent})
    }
}

/// `_admin_units(gdf, aoi, group_cols, name_field, parent_field)`
fn admin_units(
    gdf: &[Feature],
    aoi_geom: &Geometry,
    aoi_area_ha: f64,
    (group_cols, name_field, parent_field): (&[&str], &str, Option<&str>),
) -> Result<Vec<AdminUnit>> {
    // blank keys dropped: notna and str(x).strip() != ""
    let keep = |f: &Feature| {
        group_cols.iter().all(|c| match f.get(c) {
            Value::Null => false,
            v => !crate::db::py_str(v).trim().is_empty(),
        })
    };
    // dissolve: groupby(sort=True) on the raw key values, geometries in row order
    let mut groups: BTreeMap<Vec<String>, Vec<&Geometry>> = BTreeMap::new();
    for f in gdf.iter().filter(|f| keep(f)) {
        let key: Vec<String> = group_cols.iter().map(|c| f.str_of(c)).collect();
        let slot = groups.entry(key).or_default();
        if let Some(g) = &f.geom {
            slot.push(g);
        }
    }
    let col = |name: &str| group_cols.iter().position(|c| *c == name);
    let mut units = Vec::new();
    for (key, geoms) in &groups {
        let merged = union_all(geoms.iter().copied())?;
        let area_ha = intersection_area(&merged, aoi_geom) / M2_PER_HA;
        units.push(AdminUnit {
            name: col(name_field).map(|i| key[i].clone()).unwrap_or_default(),
            area_ha,
            pct: safe_pct(area_ha, aoi_area_ha),
            parent: parent_field.and_then(|p| col(p).map(|i| key[i].clone())),
        });
    }
    let mut kept: Vec<AdminUnit> = units
        .into_iter()
        .filter(|u| u.pct >= ADMIN_SLIVER_PCT)
        .collect();
    kept.sort_by(|a, b| {
        b.area_ha
            .partial_cmp(&a.area_ha)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(kept)
}

/// `_ancestors(gdf)`: (district, sub-district) -> (province, country), later rows win.
fn ancestors(gdf: &[Feature]) -> HashMap<(String, String), (String, String)> {
    gdf.iter()
        .map(|f| {
            (
                (f.str_of("NAME_2"), f.str_of("NAME_3")),
                (f.str_of("NAME_1"), f.str_of("COUNTRY")),
            )
        })
        .collect()
}

/// `analyze_admin_boundaries(aoi)`
pub async fn analyze_admin_boundaries(gis: &PgPool, aoi: &Aoi) -> Result<(Value, Value)> {
    let gdf = load_admin_intersecting(gis, aoi).await?;
    admin_boundaries_from(&gdf, aoi)
}

/// The body after the load.
pub fn admin_boundaries_from(gdf: &[Feature], aoi: &Aoi) -> Result<(Value, Value)> {
    let aoi_geom = aoi_geometry(aoi)?;
    let level = |spec| admin_units(gdf, &aoi_geom, aoi.area_ha, spec);
    let countries = level(COUNTRY)?;
    let provinces = level(PROVINCE)?;
    let districts = level(DISTRICT)?;
    let subdistricts = level(SUBDISTRICT)?;
    let villages = level(VILLAGE)?;

    let area_text = fmt_grouped(aoi.area_ha, 0);
    let mut flags: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    let dominant_country = countries.first().map(|u| u.name.clone());
    let mut in_country: Vec<&AdminUnit> = provinces
        .iter()
        .filter(|u| u.parent == dominant_country)
        .collect();
    if in_country.is_empty() {
        in_country = provinces.iter().collect();
    }
    let dominant_province = in_country.first().map(|u| u.name.clone());
    let in_province: Vec<&AdminUnit> = districts
        .iter()
        .filter(|u| u.parent == dominant_province)
        .collect();
    let main = in_province.first().copied();

    let where_ = if let Some(main) = main {
        Some(match &dominant_province {
            Some(p) => format!("{}, {p}", main.name),
            None => main.name.clone(),
        })
    } else if let Some(p) = &dominant_province {
        flags.push(
            "1.2: no GADM L2 district within the dominant province above the 1% sliver threshold. \
             The narrative falls back to province level."
                .into(),
        );
        Some(p.clone())
    } else {
        missing.push(
            "1.2: AOI does not intersect any GADM unit above the 1% sliver threshold. \
             The national comparison in 1.6 will report as not applicable."
                .into(),
        );
        None
    };

    let opening = match &where_ {
        Some(w) => format!(
            "This project area is majorly located in {w} with an approximate total area of {area_text} hectares."
        ),
        None => format!("This project area has an approximate total area of {area_text} hectares."),
    };
    let follow = if in_province.len() > 1 {
        "Within this province, it also overlaps with the following districts:"
    } else {
        ""
    };

    let main_subdistrict = subdistricts
        .iter()
        .find(|u| main.is_some_and(|m| u.parent.as_deref() == Some(m.name.as_str())));
    let main_village = villages
        .iter()
        .find(|u| main_subdistrict.is_some_and(|s| u.parent.as_deref() == Some(s.name.as_str())));
    let dominant_unit = main_village
        .or(main_subdistrict)
        .or(main)
        .or(in_country.first().copied());

    if provinces.len() > 1 {
        flags.push(format!(
            "1.2: AOI spans {} provinces, but the narrative says \"Within this province\". Accepted by the team; \
             recorded here so the mismatch is visible.",
            provinces.len()
        ));
    }
    if countries.len() > 1 {
        flags.push(
            "1.2: AOI is transboundary. The narrative does not mention it, and the national risk comparison in 1.6 \
             uses the dominant country only."
                .into(),
        );
    }

    let units_json = |v: &[AdminUnit]| v.iter().map(AdminUnit::to_json).collect::<Vec<_>>();
    let values = json!({
        "dominant_country": dominant_country,
        "dominant_province": dominant_province,
        "dominant_district": main.map(|m| m.name.clone()),
        "dominant_subdistrict": main_subdistrict.map(|u| u.name.clone()),
        "dominant_village": main_village.map(|u| u.name.clone()),
        "dominant_area_ha": dominant_unit.map(|u| u.area_ha),
        "transboundary": countries.len() > 1,
        "provinces": provinces.iter().map(|u| u.name.clone()).collect::<Vec<_>>(),
    });
    let results = json!({
        "narrative": sentences(&[&opening, follow]),
        "tables": {
            "district_table": units_json(&districts),
            "country": units_json(&countries),
            "province": units_json(&provinces),
            "subdistrict": units_json(&subdistricts),
            "village": units_json(&villages),
        },
        "values": values,
        "flags": flags,
        "missing": missing,
    });

    let overlapping: Vec<Value> = if !subdistricts.is_empty() {
        let anc = ancestors(gdf);
        subdistricts
            .iter()
            .filter(|u| Some(*u) != main_subdistrict)
            .map(|u| {
                let found = anc.get(&(
                    u.parent.clone().unwrap_or_else(|| "None".into()),
                    u.name.clone(),
                ));
                json!({
                    "subdistrict": u.name,
                    "district": u.parent,
                    "province": found.map(|a| a.0.clone()),
                    "country": found.map(|a| a.1.clone()),
                    "area": u.area_ha,
                })
            })
            .collect()
    } else {
        // {u.name: u.parent for u in provinces}: a later duplicate name wins
        let mut province_country: HashMap<&str, &Option<String>> = HashMap::new();
        for u in &provinces {
            province_country.insert(u.name.as_str(), &u.parent);
        }
        districts
            .iter()
            .filter(|u| Some(*u) != main)
            .map(|u| {
                let country = u
                    .parent
                    .as_deref()
                    .and_then(|p| province_country.get(p))
                    .and_then(|c| (*c).clone());
                json!({
                    "subdistrict": null,
                    "district": u.name,
                    "province": u.parent,
                    "country": country,
                    "area": u.area_ha,
                })
            })
            .collect()
    };

    let v = &results["values"];
    let view = json!({
        "village": v["dominant_village"],
        "subdistrict": v["dominant_subdistrict"],
        "district": v["dominant_district"],
        "province": v["dominant_province"],
        "country": v["dominant_country"],
        "area": v["dominant_area_ha"],
        "overlapping_administration": overlapping,
    });
    Ok((results, view))
}
