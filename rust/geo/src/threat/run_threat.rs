//! F02-P3 Threat runner (`threat/run_threat.py`): the five sections shaped into their tabs, run
//! concurrently (one thread per requested section, as the Python's pool), streamed in plan order
//! through `pipeline::stream`.
//!
//! A section that raises ValueError because a layer's footprint does not reach the AOI is an
//! ANSWER (`missing`, zero-filled shape), not a retryable crash (`_guard` / `_uncovered`).

use std::sync::Arc;
use std::thread::JoinHandle;

use serde_json::{Map, Value, json};

use crate::Result;
use crate::common::{Aoi, aoi_in_raster_crs, open_layer};
use crate::config::{
    THREAT_DISTURBANCE, THREAT_ECOSYSTEM, THREAT_FOREST_2024_54034, THREAT_LAND_DEGRADATION,
    THREAT_PEAT_CANAL, THREAT_PEAT_CANALS_DENSITY,
};
use crate::db::MappingKeys;
use crate::ffi::gdal_err;
use crate::pipeline::{Process, Stream, error_status, safe, stream};
use crate::py::round_ndigits;
use crate::settings::Settings;
use crate::threat::{all_ecosystems, dryland_forest, land_degradation, mangrove, peatland};

const PERIOD: (i64, i64) = (2015, 2024);
const LAND_DEGRADATION_PERIOD: (i64, i64) = (2014, 2024);
const LAND_DEGRADATION_CLASSES: [(i64, &str); 3] =
    [(1, "Degraded"), (2, "Stable"), (3, "Improved")];
const CORE_LAYERS: [&str; 2] = [THREAT_ECOSYSTEM, THREAT_DISTURBANCE];
const PEAT_LAYERS: [&str; 4] = [
    THREAT_ECOSYSTEM,
    THREAT_DISTURBANCE,
    THREAT_PEAT_CANALS_DENSITY,
    THREAT_PEAT_CANAL,
];
const DEGRADATION_LAYERS: [&str; 2] = [THREAT_FOREST_2024_54034, THREAT_LAND_DEGRADATION];
const NO_COVERAGE: &str = "The threat layers do not cover this project area, so no disturbance can be reported. The \
     forest disturbance layer reaches only 10 degrees north, which excludes Thailand, Laos, Myanmar, northern Vietnam \
     and the northern Philippines.";

/// `processes`, in emission order.
pub fn processes() -> Vec<Process> {
    [
        ("preparation", 0.1),
        ("all ecosystems", 3.0),
        ("land degradation", 4.0),
        ("mangrove", 5.0),
        ("dryland forest", 8.0),
        ("peatland", 12.0),
        ("end", 0.1),
    ]
    .iter()
    .map(|(n, w)| Process::new(n, *w))
    .collect()
}

fn g0() -> Value {
    json!({"area_ha": 0.0, "percentage": 0})
}
fn no_drivers() -> Value {
    json!({"non_natural": [], "natural": [], "other": []})
}

/// `_uncovered`: the layers whose bounds are disjoint from the AOI's (header reads only).
fn uncovered(settings: &Settings, aoi: &Aoi, layers: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for layer in layers {
        let Ok(src) = open_layer(settings, layer) else {
            continue;
        };
        let Ok(polys) = aoi_in_raster_crs(aoi, &src) else {
            continue;
        };
        let Ok(gt) = src.geo_transform().map_err(gdal_err) else {
            continue;
        };
        let (w, h) = src.raster_size();
        let (mut a0, mut a1, mut a2, mut a3) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for p in polys.iter().flatten().flatten() {
            a0 = a0.min(p[0]);
            a1 = a1.min(p[1]);
            a2 = a2.max(p[0]);
            a3 = a3.max(p[1]);
        }
        let (l, t) = (gt[0], gt[3]);
        let (r, b) = (l + gt[1] * w as f64, t + gt[5] * h as f64);
        let (b0, b1, b2, b3) = (l, b.min(t), r, b.max(t));
        // rasterio.coords.disjoint_bounds
        if a0 > b2 || b0 > a2 || a1 > b3 || b1 > a3 {
            out.push(layer.to_string());
        }
    }
    out
}

/// `_guard`: (raw, covered).
fn guard(
    settings: &Settings,
    aoi: &Aoi,
    r: Result<Value>,
    layers: &[&str],
    zero: Value,
) -> Result<(Value, bool)> {
    match r {
        Ok(v) => Ok((v, true)),
        Err(e) if e.kind == "ValueError" && !uncovered(settings, aoi, layers).is_empty() => {
            Ok((zero, false))
        }
        Err(e) => Err(e),
    }
}

/// `_absent`
fn absent(label: &str, area_ha: f64) -> Vec<String> {
    if area_ha > 0.0 {
        vec![]
    } else {
        vec![format!(
            "This project area contains no {}, so it has nothing to report.",
            label.to_lowercase()
        )]
    }
}

fn missing_for(covered: bool, label: &str, area: f64) -> Vec<String> {
    if covered {
        absent(label, area)
    } else {
        vec![NO_COVERAGE.to_string()]
    }
}

/// `_graded`
fn graded(raw: &Value, key: &str) -> (Value, Value) {
    let block = raw.get(key);
    (
        block
            .and_then(|b| b.get("area_ha"))
            .cloned()
            .unwrap_or(json!(0.0)),
        block
            .and_then(|b| b.get("percentage"))
            .cloned()
            .unwrap_or(json!(0.0)),
    )
}

/// `_driver_keys`
fn driver_keys(keys: &MappingKeys, ecosystem: &str, drivers: &Value) -> Value {
    let mut out = Map::new();
    for (wire, group) in [
        ("non_natural", "human"),
        ("natural", "natural"),
        ("other", "other"),
    ] {
        let labels = drivers
            .get(wire)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let ks: Vec<Value> = labels
            .iter()
            .map(|l| {
                json!(keys.key(
                    ("threat", Some(ecosystem), Some("driver"), Some(group), None),
                    l.as_str(),
                    false
                ))
            })
            .collect();
        out.insert(wire.into(), json!(ks));
    }
    Value::Object(out)
}

fn period(p: (i64, i64)) -> Value {
    json!({"from": p.0, "to": p.1})
}

fn area(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// `_overview`
fn overview(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let zero_eco = |_l: &str| json!({"area_ha": 0.0, "percentage_total": 0, "disturbed_area_ha": 0.0, "disturbed_percentage": 0});
    let zero = json!({
        "total_ecosystem_area_ha": 0.0, "total_disturbed_area_ha": 0.0, "total_disturbed_percentage": 0,
        "ecosystems": {"Other": zero_eco("Other"), "Dryland forest": zero_eco("Dryland forest"),
                       "Mangrove": zero_eco("Mangrove"), "Peatland": zero_eco("Peatland")},
    });
    let (raw, covered) = guard(
        settings,
        aoi,
        all_ecosystems::analyze_all_ecosystem(settings, aoi),
        &CORE_LAYERS,
        zero,
    )?;
    let cards: Vec<Value> = [("Dryland", "Dryland forest", "Dryland forest"), ("Mangrove", "Mangrove", "Mangrove"),
                             ("Peatland", "Peatland", "Peatland"), ("Other", "Other", "Other")]
        .iter()
        .map(|(key, label, display)| {
            let row = raw["ecosystems"].get(*label);
            json!({
                "ecosystem": key,
                "label": display,
                "area_ha": row.and_then(|r| r.get("area_ha")).cloned().unwrap_or(json!(0.0)),
                "percentage_total": row.and_then(|r| r.get("percentage_total")).cloned().unwrap_or(json!(0.0)),
            })
        })
        .collect();
    let total = raw["total_ecosystem_area_ha"].clone();
    let pct = if aoi.area_ha != 0.0 {
        json!(round_ndigits(100.0 * area(&total) / aoi.area_ha, 2))
    } else {
        json!(0.0)
    };
    let view = json!({"total_ecosystem_area_ha": total, "total_ecosystem_percentage": pct, "ecosystems": cards});
    let mut missing = Vec::new();
    if !covered {
        missing.push(NO_COVERAGE.to_string());
    } else if area(&total) <= 0.0 {
        missing.push(
            "The ecosystem layer does not cover this project area, so no threat can be reported."
                .into(),
        );
    }
    Ok((
        json!({"narrative": "", "tables": {}, "values": raw, "flags": [], "missing": missing}),
        view,
    ))
}

/// `_dryland`
fn dryland(settings: &Settings, keys: &MappingKeys, aoi: &Aoi) -> Result<(Value, Value)> {
    let zero = json!({"total_area_ha": 0.0, "remaining_forest": g0(), "disturbed": g0(), "forest_loss": g0(),
                      "forest_gain": g0(), "drivers": no_drivers()});
    let (raw, covered) = guard(
        settings,
        aoi,
        dryland_forest::analyze_dryland_forest(settings, aoi),
        &CORE_LAYERS,
        zero,
    )?;
    let (rh, rp) = graded(&raw, "remaining_forest");
    let (dh, dp) = graded(&raw, "disturbed");
    let (lh, lp) = graded(&raw, "forest_loss");
    let (gh, gp) = graded(&raw, "forest_gain");
    let view = json!({
        "total_area_ha": raw["total_area_ha"],
        "remaining_forest_ha": rh, "remaining_forest_percentage": rp,
        "disturbed_area_ha": dh, "disturbed_percentage": dp,
        "forest_loss_ha": lh, "forest_loss_percentage": lp,
        "forest_gain_ha": gh, "forest_gain_percentage": gp,
        "period": period(PERIOD),
        "drivers": raw["drivers"],
        "driver_keys": driver_keys(keys, "dryland", &raw["drivers"]),
    });
    let notes = [
        "3.2: the drivers are presence only -- no area and no ranking is computed, so the narrative cannot \
                  name a dominant driver. Drought and Typhoon can never appear at all: no layer supplies them.",
    ];
    let missing = missing_for(covered, "Dryland forest", area(&raw["total_area_ha"]));
    Ok((
        json!({"narrative": "", "tables": {}, "values": raw, "flags": [], "missing": missing, "notes": notes}),
        view,
    ))
}

/// `_mangrove`
fn mangrove_tab(settings: &Settings, keys: &MappingKeys, aoi: &Aoi) -> Result<(Value, Value)> {
    let zero = json!({"mangrove": {"total_area_ha": 0.0, "remaining_forest": g0(), "disturbed": g0(),
                                   "main_pressure": "Not identified", "drivers": no_drivers()}});
    let (wrapped, covered) = guard(
        settings,
        aoi,
        mangrove::analyze_mangrove(settings, aoi),
        &CORE_LAYERS,
        zero,
    )?;
    let raw = wrapped["mangrove"].clone();
    let (rh, rp) = graded(&raw, "remaining_forest");
    let (dh, dp) = graded(&raw, "disturbed");
    let view = json!({
        "total_area_ha": raw["total_area_ha"],
        "remaining_forest_ha": rh, "remaining_forest_percentage": rp,
        "disturbed_area_ha": dh, "disturbed_percentage": dp,
        "main_pressure": raw["main_pressure"],
        "main_pressure_key": keys.key(("threat", Some("mangrove"), Some("driver"), Some("human"), None), raw["main_pressure"].as_str(), false),
        "period": period(PERIOD),
        "drivers": raw["drivers"],
        "driver_keys": driver_keys(keys, "mangrove", &raw["drivers"]),
    });
    let missing = missing_for(covered, "Mangrove", area(&raw["total_area_ha"]));
    Ok((
        json!({"narrative": "", "tables": {}, "values": raw, "flags": [], "missing": missing}),
        view,
    ))
}

/// `_peatland`
fn peatland_tab(settings: &Settings, keys: &MappingKeys, aoi: &Aoi) -> Result<(Value, Value)> {
    let zero = json!({"peatland": {"total_area_ha": 0.0, "remaining_forest": g0(), "disturbed": g0(), "converted_loss": g0(),
        "drivers": {"canal_proximity": "-", "canal_distance_m": null, "drainage_pressure": "-", "fire_risk": "-"}}});
    let (wrapped, covered) = guard(
        settings,
        aoi,
        peatland::analyze_peatland(settings, aoi),
        &PEAT_LAYERS,
        zero,
    )?;
    let raw = wrapped["peatland"].clone();
    let (rh, rp) = graded(&raw, "remaining_forest");
    let (dh, dp) = graded(&raw, "disturbed");
    let (ch, cp) = graded(&raw, "converted_loss");
    let d = &raw["drivers"];
    let badge = |label: &str| {
        json!(keys.key(
            ("threat", Some("peatland"), Some("badge"), None, None),
            Some(label),
            false
        ))
    };
    let view = json!({
        "total_area_ha": raw["total_area_ha"],
        "remaining_forest_ha": rh, "remaining_forest_percentage": rp,
        "disturbed_area_ha": dh, "disturbed_percentage": dp,
        "converted_loss_ha": ch, "converted_loss_percentage": cp,
        "canal_proximity": d["canal_proximity"],
        "canal_distance_m": d["canal_distance_m"],
        "drainage_pressure": d["drainage_pressure"],
        "fire_risk": d["fire_risk"],
        "badge_keys": {"canal_proximity": badge("Canal proximity"), "drainage_pressure": badge("Drainage pressure"),
                       "fire_risk": badge("Fire risk")},
    });
    let notes = [
        "3.4: canal proximity and drainage pressure deliberately consider canals OUTSIDE the project boundary \
                  -- canals affect the water table up to 500 m away and biomass growth up to 1 km, which is where the \
                  High and Medium thresholds come from.",
    ];
    let missing = missing_for(covered, "Peatland", area(&raw["total_area_ha"]));
    Ok((
        json!({"narrative": "", "tables": {}, "values": raw, "flags": [], "missing": missing, "notes": notes}),
        view,
    ))
}

/// `_land_degradation`
fn land_degradation_tab(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let zero = json!({
        "degraded_ha": 0.0, "stable_ha": 0.0, "improved_ha": 0.0,
        "degraded_inside_forest_ha": 0.0, "degraded_outside_forest_ha": 0.0,
        "table": LAND_DEGRADATION_CLASSES.iter().map(|(c, _)| json!({"class": c, "inside_forest_pixels": 0,
            "inside_forest_ha": 0.0, "outside_forest_pixels": 0, "outside_forest_ha": 0.0})).collect::<Vec<_>>(),
        "narrative": "",
    });
    let (raw, covered, published) = match guard(
        settings,
        aoi,
        land_degradation::analyze_land_degradation(settings, aoi),
        &DEGRADATION_LAYERS,
        zero.clone(),
    ) {
        Ok((raw, covered)) => (raw, covered, true),
        Err(e) if e.kind == "RasterioIOError" && e.message.contains("404") => (zero, true, false),
        Err(e) => return Err(e),
    };
    let classes: Vec<Value> = raw["table"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| {
            let c = row["class"].as_i64().unwrap_or(0);
            json!({"class": row["class"], "label": LAND_DEGRADATION_CLASSES.iter().find(|(k, _)| *k == c).map(|(_, l)| *l),
                   "inside_forest_ha": row["inside_forest_ha"], "outside_forest_ha": row["outside_forest_ha"]})
        })
        .collect();
    let view = json!({
        "degraded_ha": raw["degraded_ha"], "stable_ha": raw["stable_ha"], "improved_ha": raw["improved_ha"],
        "degraded_inside_forest_ha": raw["degraded_inside_forest_ha"],
        "degraded_outside_forest_ha": raw["degraded_outside_forest_ha"],
        "period": period(LAND_DEGRADATION_PERIOD),
        "classes": classes,
    });
    let mut missing: Vec<&str> = Vec::new();
    if !published {
        missing.push("The land degradation layers are not published yet, so land degradation cannot be reported.");
    } else if !covered {
        missing.push("The land degradation layers do not cover this project area, so land degradation cannot be reported.");
    }
    let values: Map<String, Value> = raw
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| *k != "table" && *k != "narrative")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok((
        json!({"narrative": raw["narrative"], "tables": {"classes": raw["table"]}, "values": values,
               "flags": [], "missing": missing}),
        view,
    ))
}

/// The sections, in emission order.
pub const SECTIONS: [&str; 5] = [
    "all ecosystems",
    "land degradation",
    "mangrove",
    "dryland forest",
    "peatland",
];

pub fn run_section(
    name: &str,
    settings: &Settings,
    keys: &MappingKeys,
    aoi: &Aoi,
) -> (Value, Value) {
    let r = match name {
        "all ecosystems" => overview(settings, aoi),
        "land degradation" => land_degradation_tab(settings, aoi),
        "mangrove" => mangrove_tab(settings, keys, aoi),
        "dryland forest" => dryland(settings, keys, aoi),
        _ => peatland_tab(settings, keys, aoi),
    };
    safe(name, r)
}

/// `stream_threat(aoi, wanted, retry_url)`: every requested section starts at once on its own
/// thread; lines are yielded in plan order as each one lands.
pub fn stream_threat(
    settings: Arc<Settings>,
    keys: Arc<MappingKeys>,
    aoi: Arc<Aoi>,
    wanted: Option<&[String]>,
    retry_url: Option<crate::pipeline::RetryUrl>,
) -> Result<Stream<impl Iterator<Item = (Value, Option<Value>)>>> {
    let running: Vec<&'static str> = SECTIONS
        .iter()
        .copied()
        .filter(|s| wanted.is_none_or(|w| w.iter().any(|x| x == s)))
        .collect();
    let all = processes();
    let plan: Vec<Process> = match wanted {
        None => all,
        Some(_) => {
            let mut p = vec![all[0].clone()];
            p.extend(
                all[1..all.len() - 1]
                    .iter()
                    .filter(|p| running.contains(&p.name.as_str()))
                    .cloned(),
            );
            p.push(all[all.len() - 1].clone());
            p
        }
    };
    let handles: Vec<(&'static str, JoinHandle<(Value, Value)>)> = running
        .iter()
        .map(|&name| {
            let (s, k, a) = (settings.clone(), keys.clone(), aoi.clone());
            (
                name,
                std::thread::spawn(move || run_section(name, &s, &k, &a)),
            )
        })
        .collect();
    let views = handles.into_iter().map(|(name, h)| {
        let (results, view) = h.join().unwrap_or_else(|_| {
            safe(
                name,
                Err(crate::Error::new("PanicException", "section panicked")),
            )
        });
        let status = error_status(&results);
        (view, status)
    });
    stream(plan, views, retry_url)
}
