//! Component 2.3 Habitat Area, Area of Habitat (`nature/habitat_area.py`).
//!
//! The GeoParquet inventory (one bounding-box footprint per species) prefilters with GEOS
//! `intersects`; each candidate's own raster (uint8, EPSG:4326, DN 1 = habitat) is then read on
//! its native grid (`raster_geometry_mask(crop=True)` + a masked window read) and measured
//! geodesically by row. The candidates run on a pool of AOH_MAX_WORKERS threads, as the Python's
//! ThreadPoolExecutor, and are reduced in input order, so the result does not depend on timing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use gdal::vector::Geometry;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{Map, Value, json};

use crate::common::{
    Aoi, Ring, aoi_4326_union, calculate_mask_area_ha, pixel_area_by_row,
    raster_geometry_mask_crop, read_window_masked,
};
use crate::config::{
    AOH_INVENTORY, AOH_RASTER_ROOT, AOH_TARGET_DN, AOH_TAXON_FIELDS, OVERLAP_THRESHOLD,
};
use crate::ffi::{self, gdal_err};
use crate::py::fmt_grouped;
use crate::settings::{Settings, gdal_path};
use crate::{Error, Result};

const OPTIONAL_COLUMNS: [&str; 5] = ["pattern", "presence", "origin", "seasonality", "Candidate"];
const HABITAT_CLASSES: [&str; 5] = [
    "<20%",
    "20\u{2013}40%",
    "40\u{2013}60%",
    "60\u{2013}80%",
    ">80%",
];
const REQUIRED_COLUMNS: [&str; 6] = [
    "class",
    "geometry",
    "iucn_status",
    "raster_path",
    "redlistCategory",
    "species",
];

/// One inventory row; text columns are `None` where the parquet value is null.
pub(crate) struct Species {
    pub(crate) species: Option<String>,
    pub(crate) class: Option<String>,
    pub(crate) raster_path: String,
    pub(crate) redlist: Option<String>,
    iucn_status: Option<String>,
    optional: Vec<(&'static str, Option<String>)>,
    pub(crate) footprint: Vec<u8>,
}

/// One species with habitat in the AOI.
struct Record {
    sp: usize, // index into the inventory
    pixel_count: usize,
    habitat_area_ha: f64,
    percentage_aoi: Value,
    pct: f64,
    habitat_class: &'static str,
}

/// `habitat_class(pct)`
fn habitat_class(pct: f64) -> &'static str {
    if pct < 20.0 {
        HABITAT_CLASSES[0]
    } else if pct < 40.0 {
        HABITAT_CLASSES[1]
    } else if pct < 60.0 {
        HABITAT_CLASSES[2]
    } else if pct <= 80.0 {
        HABITAT_CLASSES[3]
    } else {
        HABITAT_CLASSES[4]
    }
}

/// `GEOD.geometry_area_perimeter(geometry)[0]`: per polygon the exterior ring's signed area (all
/// vertices, the closing one included), plus each hole's, summed over parts in order.
fn geometry_area_m2(polygons: &[Vec<Ring>]) -> f64 {
    use geographiclib_rs::{Geodesic, PolygonArea, Winding};
    let geod = Geodesic::wgs84();
    let ring_area = |ring: &Ring| {
        let mut pa = PolygonArea::new(&geod, Winding::CounterClockwise);
        for p in ring {
            pa.add_point(p[1], p[0]);
        }
        pa.compute(true).1
    };
    let mut total = 0.0;
    for rings in polygons {
        let mut area = 0.0;
        for (i, ring) in rings.iter().enumerate() {
            area = if i == 0 {
                ring_area(ring)
            } else {
                area + ring_area(ring)
            };
        }
        total += area;
    }
    total
}

/// `_load_inventory`: the GeoParquet from wherever V3_BUCKET points, `Class` read as `class`,
/// `raster_path` rebuilt as `<class>/<file name>`, `VN`/`CN` respelled, empty Data Deficient -> DD.
pub(crate) fn load_inventory(settings: &Settings) -> Result<(Vec<Species>, bool)> {
    let bytes = ffi::read_vsi_file(&gdal_path(&settings.layer_path(AOH_INVENTORY)?))?;
    let reader = SerializedFileReader::new(bytes::Bytes::from(bytes))
        .map_err(|e| Error::new("ArrowInvalid", e.to_string()))?;
    let schema = reader.metadata().file_metadata().schema_descr();
    let names: Vec<String> = schema
        .columns()
        .iter()
        .map(|c| {
            if c.name() == "Class" {
                "class".into()
            } else {
                c.name().to_string()
            }
        })
        .collect();
    let mut missing: Vec<&str> = REQUIRED_COLUMNS
        .iter()
        .copied()
        .filter(|c| !names.iter().any(|n| n == c))
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(Error::new(
            "KeyError",
            format!("\"Missing GeoParquet columns: {missing:?}\"").replace('"', "'"),
        ));
    }
    let has_optional: Vec<bool> = OPTIONAL_COLUMNS
        .iter()
        .map(|c| names.iter().any(|n| n == c))
        .collect();
    let mut out = Vec::new();
    for row in reader
        .get_row_iter(None)
        .map_err(|e| Error::new("ArrowInvalid", e.to_string()))?
    {
        let row = row.map_err(|e| Error::new("ArrowInvalid", e.to_string()))?;
        let mut text: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let mut footprint = Vec::new();
        for (name, field) in row.get_column_iter() {
            let name = if name == "Class" {
                "class"
            } else {
                name.as_str()
            };
            match field {
                Field::Str(s) => {
                    text.insert(name.to_string(), Some(s.clone()));
                }
                Field::Bytes(b) if name == "geometry" => footprint = b.data().to_vec(),
                Field::Null => {
                    text.insert(name.to_string(), None);
                }
                _ => {}
            }
        }
        let get = |k: &str| text.get(k).cloned().flatten();
        let class = get("class");
        // class + "/" + raster_path.replace("\\", "/").rsplit("/", 1)[-1]
        let file = get("raster_path").map(|p| {
            p.replace('\\', "/")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        });
        let raster_path = format!(
            "{}/{}",
            class.as_deref().unwrap_or(""),
            file.unwrap_or_default()
        );
        let redlist = get("redlistCategory");
        let iucn_status = match get("iucn_status").as_deref() {
            Some("VN") => Some("VU".to_string()),
            Some("CN") => Some("CR".to_string()),
            None if redlist.as_deref() == Some("Data Deficient") => Some("DD".to_string()),
            other => other.map(str::to_string),
        };
        let optional = OPTIONAL_COLUMNS
            .iter()
            .zip(&has_optional)
            .filter(|(_, has)| **has)
            .map(|(c, _)| (*c, get(c)))
            .collect();
        out.push(Species {
            species: get("species"),
            class,
            raster_path,
            redlist,
            iucn_status,
            optional,
            footprint,
        });
    }
    Ok((out, has_optional.iter().any(|h| *h)))
}

/// `_species_habitat`: Ok(None) where the notebook `continue`s, Err(message) for a raster that
/// cannot be read (RasterioIOError), Ok(Some) otherwise.
fn species_habitat(
    settings: &Settings,
    sp: &Species,
    polygons_4326: &[Vec<Ring>],
    is_multi: bool,
    aoi_area_ha: f64,
) -> Result<std::result::Result<Option<(usize, f64)>, String>> {
    let failure = || Ok(Err(format!("{} (RasterioIOError)", sp.raster_path)));
    let path = format!(
        "{}/{}",
        settings.layer_path(AOH_RASTER_ROOT)?,
        sp.raster_path
    );
    let Ok(src) = gdal::Dataset::open(gdal_path(&path)) else {
        return failure();
    };
    // rasterio `src.crs != "EPSG:4326"` / `src.crs.to_epsg() == 4326`
    if src.spatial_ref().ok().and_then(|s| s.auth_code().ok()) != Some(4326) {
        // shortcut: every AoH raster is EPSG:4326; reprojecting the AOI for another CRS is not ported
        return Err(Error::new(
            "NotImplementedError",
            format!("{} is not EPSG:4326", sp.raster_path),
        ));
    }
    let (outside, transform, window) =
        match raster_geometry_mask_crop(&src, polygons_4326, is_multi, false) {
            Ok(r) => r,
            Err(e) if e.kind == "ValueError" => return Ok(Ok(None)), // footprint intersects, raster does not
            Err(e) => return Err(e),
        };
    let Ok((data, masked)) = read_window_masked(&src, 1, &window) else {
        return failure();
    };
    let presence: Vec<bool> = outside
        .iter()
        .zip(&masked)
        .zip(&data)
        .map(|((o, m), d)| !o && !m && *d == AOH_TARGET_DN)
        .collect();
    let pixel_count = presence.iter().filter(|p| **p).count();
    if pixel_count == 0 {
        return Ok(Ok(None));
    }
    let rows = pixel_area_by_row(&transform, window.height as usize);
    let habitat_area_ha = calculate_mask_area_ha(&presence, window.width as usize, &rows);
    let _ = aoi_area_ha;
    Ok(Ok(Some((pixel_count, habitat_area_ha))))
}

/// `analyze_biodiversity` up to the species table, then the component's payload.
fn analyze(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    let (polygons, is_multi) = aoi_4326_union(aoi)?;
    let aoi_area_ha = geometry_area_m2(&polygons).abs() / 10000.0;
    let (inventory, _) = load_inventory(settings)?;
    let aoi_geom = crate::common::polygons_geometry(&polygons, is_multi)?;
    let mut candidates = Vec::new();
    for (i, sp) in inventory.iter().enumerate() {
        let g = Geometry::from_wkb(&sp.footprint).map_err(gdal_err)?;
        if g.intersects(&aoi_geom) {
            candidates.push(i);
        }
    }

    let workers = settings.aoh_max_workers()?.min(candidates.len()).max(1);
    let next = AtomicUsize::new(0);
    #[expect(
        clippy::type_complexity,
        reason = "one result slot per candidate, written by the worker pool"
    )]
    let slots: Mutex<Vec<Option<Result<std::result::Result<Option<(usize, f64)>, String>>>>> =
        Mutex::new((0..candidates.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // AOH_GDAL_OPTIONS, scoped to these threads as rasterio.Env scopes them
                let _ = gdal::config::set_thread_local_config_option(
                    "CPL_VSIL_CURL_ALLOWED_EXTENSIONS",
                    ".tif",
                );
                let _ = gdal::config::set_thread_local_config_option(
                    "GDAL_DISABLE_READDIR_ON_OPEN",
                    "EMPTY_DIR",
                );
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = candidates.get(k) else { break };
                    let r =
                        species_habitat(settings, &inventory[i], &polygons, is_multi, aoi_area_ha);
                    if let Ok(mut s) = slots.lock() {
                        s[k] = Some(r);
                    }
                }
            });
        }
    });
    let slots = slots
        .into_inner()
        .map_err(|_| Error::new("RuntimeError", "worker panicked"))?;
    let mut records = Vec::new();
    let mut failures = Vec::new();
    for (k, slot) in slots.into_iter().enumerate() {
        match slot.ok_or_else(|| Error::new("RuntimeError", "species worker did not finish"))?? {
            Ok(Some((pixel_count, habitat_area_ha))) => {
                let (pct, percentage_aoi) = if aoi_area_ha > 0.0 {
                    let p = habitat_area_ha / aoi_area_ha * 100.0;
                    (p, json!(p))
                } else {
                    (0.0, json!(0))
                };
                records.push(Record {
                    sp: candidates[k],
                    pixel_count,
                    habitat_area_ha,
                    percentage_aoi,
                    pct,
                    habitat_class: habitat_class(pct),
                });
            }
            Ok(None) => {}
            Err(f) => failures.push(f),
        }
    }
    // sort_values(["class", "species"]): stable
    records.sort_by(|a, b| {
        let (x, y) = (&inventory[a.sp], &inventory[b.sp]);
        (&x.class, &x.species).cmp(&(&y.class, &y.species))
    });

    let published: Vec<&str> = {
        let mut v: Vec<&str> = inventory
            .iter()
            .filter_map(|s| s.class.as_deref())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let count_where = |f: &dyn Fn(&Record) -> bool| records.iter().filter(|r| f(r)).count();
    let class_summary: Map<String, Value> = if records.is_empty() {
        Map::new()
    } else {
        ["Amphibian", "Mammal", "Bird", "Reptile"]
            .iter()
            .map(|c| {
                (
                    c.to_string(),
                    json!(count_where(
                        &|r| inventory[r.sp].class.as_deref() == Some(*c)
                    )),
                )
            })
            .collect()
    };
    // value_counts(): count descending, ties in first-seen order, nulls dropped
    let mut iucn: Vec<(String, usize)> = Vec::new();
    for r in &records {
        if let Some(s) = &inventory[r.sp].iucn_status {
            match iucn.iter_mut().find(|(k, _)| k == s) {
                Some((_, n)) => *n += 1,
                None => iucn.push((s.clone(), 1)),
            }
        }
    }
    #[expect(
        clippy::unnecessary_sort_by,
        reason = "descending, stable, as the Python's sort; kept as proven"
    )]
    iucn.sort_by(|a, b| b.1.cmp(&a.1));
    let iucn_summary: Map<String, Value> =
        iucn.iter().map(|(k, n)| (k.clone(), json!(n))).collect();
    let habitat_summary: Map<String, Value> = HABITAT_CLASSES
        .iter()
        .map(|c| {
            (
                c.to_string(),
                json!(count_where(&|r| r.habitat_class == *c)),
            )
        })
        .collect();
    let over_90: Vec<&Record> = records
        .iter()
        .filter(|r| r.pct > OVERLAP_THRESHOLD)
        .collect();
    let status_count = |s: &str| {
        over_90
            .iter()
            .filter(|r| inventory[r.sp].iucn_status.as_deref() == Some(s))
            .count()
    };
    let (over_90_count, en_count, vu_count, cr_count) = (
        over_90.len(),
        status_count("EN"),
        status_count("VU"),
        status_count("CR"),
    );

    let mut flags: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let retryable = !failures.is_empty();
    if !failures.is_empty() {
        let shown = failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if failures.len() > 3 {
            format!(" and {} more", failures.len() - 3)
        } else {
            String::new()
        };
        flags.push(format!(
            "2.3: {} of {} candidate species rasters could not be read ({shown}{more}), so their species are missing from the counts.",
            failures.len(),
            candidates.len()
        ));
    }
    let mut view = Map::new();
    for (taxon, field) in AOH_TAXON_FIELDS {
        let v = if published.contains(&taxon) {
            json!(
                class_summary
                    .get(taxon)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            )
        } else {
            Value::Null
        };
        view.insert(field.to_string(), v);
    }
    let mut unpublished: Vec<&str> = AOH_TAXON_FIELDS
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| !published.contains(t))
        .collect();
    unpublished.sort();
    if !unpublished.is_empty() {
        let fields: Vec<&str> = unpublished
            .iter()
            .filter_map(|t| {
                AOH_TAXON_FIELDS
                    .iter()
                    .find(|(k, _)| k == t)
                    .map(|(_, f)| *f)
            })
            .collect();
        missing.push(format!(
            "2.3: the species inventory publishes nothing for {}, so {} is null rather than a count.",
            unpublished.join(", "),
            fields.join(", ")
        ));
    }
    let name_of = |r: &Record| {
        inventory[r.sp]
            .species
            .clone()
            .unwrap_or_default()
            .replace('_', " ")
    };
    let mut species_names: Vec<String> = records.iter().map(name_of).collect();
    species_names.sort();
    let opt = |sp: &Species, lower: bool| -> Map<String, Value> {
        sp.optional
            .iter()
            .map(|(c, v)| {
                (
                    if lower {
                        c.to_lowercase()
                    } else {
                        c.to_string()
                    },
                    json!(v),
                )
            })
            .collect()
    };
    let species_rows: Vec<Value> = records
        .iter()
        .map(|r| {
            let sp = &inventory[r.sp];
            let mut m = Map::new();
            m.insert("species".into(), json!(sp.species));
            m.insert("class".into(), json!(sp.class));
            m.insert("redlistCategory".into(), json!(sp.redlist));
            m.insert("iucn_status".into(), json!(sp.iucn_status));
            m.extend(opt(sp, false));
            m.insert("pixel_count".into(), json!(r.pixel_count));
            m.insert("habitat_area_ha".into(), json!(r.habitat_area_ha));
            m.insert("percentage_aoi".into(), r.percentage_aoi.clone());
            m.insert("habitat_class".into(), json!(r.habitat_class));
            Value::Object(m)
        })
        .collect();
    let species_list: Vec<Value> = records
        .iter()
        .map(|r| {
            let sp = &inventory[r.sp];
            let mut m = Map::new();
            m.insert("scientific_name".into(), json!(name_of(r)));
            m.insert("taxon_class".into(), json!(sp.class));
            m.insert("iucn_status".into(), json!(sp.iucn_status));
            m.insert("redlist_category".into(), json!(sp.redlist));
            m.insert("habitat_area_ha".into(), json!(r.habitat_area_ha));
            m.insert("percentage_aoi".into(), r.percentage_aoi.clone());
            m.insert("habitat_class".into(), json!(r.habitat_class));
            m.extend(opt(sp, true));
            Value::Object(m)
        })
        .collect();
    let over_90_species =
        json!({"count": over_90_count, "en": en_count, "vu": vu_count, "cr": cr_count});
    let values = json!({
        "aoi_area_ha": crate::py::round_ndigits(aoi_area_ha, 2),
        "inventory_species_count": inventory.len(),
        "candidate_species_count": candidates.len(),
        "species_count": records.len(),
        "class_summary": class_summary,
        "iucn_summary": iucn_summary,
        "habitat_summary": habitat_summary,
        "over_90_count": over_90_count,
        "en_count": en_count,
        "vu_count": vu_count,
        "cr_count": cr_count,
    });
    view.insert("total_wildlife_species".into(), json!(records.len()));
    let threshold = |view: &mut Map<String, Value>| {
        view.insert(
            "habitat_summary".into(),
            Value::Object(habitat_summary.clone()),
        );
        view.insert("over_90_species".into(), over_90_species.clone());
    };
    if species_names.is_empty() {
        view.insert("habitat_species_name".into(), json!([]));
        view.insert("species_list".into(), json!([]));
        view.insert("iucn_summary".into(), json!({}));
        threshold(&mut view);
        let results = json!({
            "narrative": "No species in the Area of Habitat inventory have suitable habitat in this project area.",
            "tables": {"species": []},
            "values": values, "flags": flags, "missing": missing, "retryable": retryable,
        });
        return Ok((results, Value::Object(view)));
    }
    let g = |n: usize| fmt_grouped(n as f64, 0);
    let narrative = if over_90_count > 0 {
        format!(
            "Project area supports the area of habitat for {} species. A subset of {} species has more than 90% habitat \
             overlapped within the project, including {} EN, {} VU, and {} CR species.",
            g(records.len()),
            g(over_90_count),
            g(en_count),
            g(vu_count),
            g(cr_count)
        )
    } else {
        format!(
            "Project area supports the area of habitat for {} species. Most of the species have less than 90% habitat \
             overlapped within the project.",
            g(records.len())
        )
    };
    view.insert("habitat_species_name".into(), json!(species_names));
    view.insert("species_list".into(), json!(species_list));
    view.insert("iucn_summary".into(), Value::Object(iucn_summary));
    threshold(&mut view);
    let results = json!({
        "narrative": narrative,
        "tables": {"species": species_rows},
        "values": values, "flags": flags, "missing": missing, "retryable": retryable,
    });
    Ok((results, Value::Object(view)))
}

/// `analyze_habitat_area`: any failure before the species table becomes the "inventory could not
/// be read" payload, as the Python's broad `except Exception` does.
pub fn analyze_habitat_area(settings: &Settings, aoi: &Aoi) -> Result<(Value, Value)> {
    match analyze(settings, aoi) {
        Ok(r) => Ok(r),
        Err(e) => {
            let flags = vec![format!(
                "2.3: the species inventory could not be read ({}), so no habitat assessment was made and every species count is null, not zero.",
                e.kind
            )];
            let results = json!({"narrative": "Habitat data is not available for this project area.",
                                 "tables": {"species": []}, "values": {}, "flags": flags, "retryable": true});
            let mut view = Map::new();
            for (_, field) in AOH_TAXON_FIELDS {
                view.insert(field.to_string(), Value::Null);
            }
            view.insert("total_wildlife_species".into(), Value::Null);
            view.insert("habitat_species_name".into(), json!([]));
            view.insert("species_list".into(), json!([]));
            for k in ["iucn_summary", "habitat_summary", "over_90_species"] {
                view.insert(k.to_string(), Value::Null);
            }
            Ok((results, Value::Object(view)))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bins() {
        assert_eq!(super::habitat_class(19.999), "<20%");
        assert_eq!(super::habitat_class(80.0), "60\u{2013}80%");
        assert_eq!(super::habitat_class(80.0001), ">80%");
    }
}
