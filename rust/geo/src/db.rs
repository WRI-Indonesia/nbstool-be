//! The parts of `v3/db.py` the ported components read: the label -> frontend key table.

use std::collections::HashMap;

use sqlx::PgPool;

type Scope = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// `load_mapping_keys()`: `{scope: {normalised text: key}}`, in the table's row order (a Python
/// dict keeps first-insertion order, a repeated text keeps its slot and takes the later key).
/// Loaded once per request and passed down, like `Settings`.
#[derive(Clone, Debug, Default)]
pub struct MappingKeys(HashMap<Scope, Vec<(String, Option<String>)>>);

/// `_norm_label`: whitespace collapsed, casefolded.
// shortcut: str::to_lowercase stands in for casefold (equal on the table's ASCII texts); use a
// full case-folding table if non-ASCII labels appear.
fn norm_label(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl MappingKeys {
    /// From the application database. A failure yields an empty map, as the Python logs and
    /// caches `{}` rather than costing a card.
    pub async fn load(app_db: &PgPool) -> Self {
        // simple protocol (no prepared statement), see pg_literal
        let rows = sqlx::raw_sql(
            "select section, sub_section, topic, \"sub topic\", country, data_text, data_key \
             from public.tbl_list_mapping_key",
        )
        .fetch_all(app_db)
        .await
        .and_then(|rows| {
            rows.iter()
                .map(|r| {
                    let t = |i| r.try_get::<Option<String>, _>(i);
                    Ok((t(0)?, t(1)?, t(2)?, t(3)?, t(4)?, t(5)?, t(6)?))
                })
                .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        });
        let mut map: HashMap<Scope, Vec<(String, Option<String>)>> = HashMap::new();
        for (section, sub_section, topic, sub_topic, country, label, key) in
            rows.unwrap_or_default()
        {
            let slot = map
                .entry((section, sub_section, topic, sub_topic, country))
                .or_default();
            let text = norm_label(label.as_deref().unwrap_or("None"));
            match slot.iter_mut().find(|(t, _)| *t == text) {
                Some(entry) => entry.1 = key,
                None => slot.push((text, key)),
            }
        }
        Self(map)
    }

    /// `mapping_key(section, sub_section, topic, sub_topic, country, label, prefix)`
    pub fn key(
        &self,
        scope: (&str, Option<&str>, Option<&str>, Option<&str>, Option<&str>),
        label: Option<&str>,
        prefix: bool,
    ) -> Option<String> {
        let label = label?;
        let s = (
            Some(scope.0.to_string()),
            scope.1.map(str::to_string),
            scope.2.map(str::to_string),
            scope.3.map(str::to_string),
            scope.4.map(str::to_string),
        );
        let entries = self.0.get(&s)?;
        let wanted = norm_label(label);
        let hit = if prefix {
            entries.iter().find(|(t, _)| t.starts_with(&wanted))
        } else {
            entries.iter().find(|(t, _)| *t == wanted)
        };
        hit.and_then(|(_, k)| k.clone())
    }
}

// ============================ GIS / app-database seam (`db.py`) ============================
//
// Pools, the statement timeout, the AOI as a bound parameter, the intersecting loaders and the
// PH region lookup. The SELECT texts are db.py's, with `:name` binds written as `$n`; the AOI
// EWKT text is byte-identical to what shapely 2.0.5 writes (proof/expected/vector_params).

use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use gdal::vector::Geometry;
use gdal_sys::OGRwkbGeometryType;
use serde_json::Value;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{Column, Row, TypeInfo, ValueRef};

use crate::common::{Aoi, M2_PER_HA, aoi_4326_union, polygons_geometry};
use crate::config::REFERENCE_CRS_WKT;
use crate::ffi::{self, gdal_err};
use crate::{Error, Result};

pub const STATEMENT_TIMEOUT_MS: u64 = 30_000;
/// `REFERENCE_SRID = int(REFERENCE_CRS.split(":")[1])`
pub const REFERENCE_SRID: i32 = 54034;

pub const ADMIN_BOUNDARIES_TABLE: &str = "public.adm_boundaries";
pub const WDPA_TABLE: &str = "sea.wdpa_wdoecm";
pub const KBA_TABLE: &str = "sea.key_biodiversity_area";
pub const KEY_SPECIES_TABLE: &str = "sea.key_species_v3";
pub const PLANT_OCCURRENCE_TABLE: &str = "sea.plant_occurrence";
pub const PLANT_SPECIES: &str = "scientificName";
pub const PLANT_CITES: &str = "citesAppendix";
pub const PLANT_YEAR: &str = "year";
pub const SOCIAL_SCHEMA: &str = "se_v3";

// Pool sizing as db.py states it: GIS 8 (+4 overflow), app-database social 6 (+4), 60 s wait.
const POOL_SIZE: u32 = 8;
const SOCIAL_POOL_SIZE: u32 = 6;
const MAX_OVERFLOW: u32 = 4;
const POOL_TIMEOUT_S: u64 = 60;

/// `_get_engine()` / `_get_social_engine()`: a lazily connecting pool on a SQLAlchemy URI
/// (`postgresql+psycopg2://` accepted). Every connection runs `SET statement_timeout` once, which
/// is what db.py's per-checkout SET leaves on a pooled connection. `pool_pre_ping` is sqlx's
/// default test-before-acquire. The URI is never logged.
// shortcut: psycopg2's connect_timeout=15 is folded into the 60 s acquire timeout (sqlx 0.8 has
// no separate connect timeout); add a tokio timeout around connect if a dead host must fail in 15 s.
fn pool(uri: &str, size: u32) -> Result<PgPool> {
    let uri = uri.replacen("postgresql+psycopg2://", "postgresql://", 1);
    PgPoolOptions::new()
        .max_connections(size + MAX_OVERFLOW)
        .acquire_timeout(std::time::Duration::from_secs(POOL_TIMEOUT_S))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // a bare &str runs over the simple query protocol (see load_intersecting)
                sqlx::Executor::execute(conn, "SET statement_timeout = 30000").await?;
                Ok(())
            })
        })
        .connect_lazy(&uri)
        .map_err(|_| Error::new("ArgumentError", "could not parse the database URI"))
}

/// The GIS database pool (`GIS_DB_SQLALCHEMY_URI`).
pub fn gis_pool(uri: &str) -> Result<PgPool> {
    pool(uri, POOL_SIZE)
}

/// The app-database pool 6.3 and the mapping keys read through (`DB_SQLALCHEMY_URI`).
pub fn social_pool(uri: &str) -> Result<PgPool> {
    pool(uri, SOCIAL_POOL_SIZE)
}

/// One coordinate as shapely 2.0.5's `.wkt` writes it (GEOS 3.11 WKTWriter, trim on, 16
/// decimals): the shortest round-trip digits in fixed notation, rounded half-even ON THOSE
/// DIGITS to 16 decimals, trailing zeros dropped, a zero result written "0" (no sign).
pub fn wkt_number(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return if v == 0.0 { "0".into() } else { format!("{v}") };
    }
    // Python's repr digits (shortest, ties to even: Rust's `{:e}` rounds a tie up)
    let mut repr = String::new();
    crate::pipeline::py_float(v.abs(), &mut repr);
    let (mantissa, exp) = repr.split_once('e').unwrap_or((&repr, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes())
        .map(|b| b - b'0')
        .collect();
    let mut point = whole.len() as i64 + exp; // digits before the decimal point
    while digits.len() > 1 && digits[0] == 0 {
        digits.remove(0);
        point -= 1;
    }
    let n = digits.len() as i64;
    let (mut int_part, mut frac): (Vec<u8>, Vec<u8>) = if point >= n {
        (
            digits
                .iter()
                .copied()
                .chain(std::iter::repeat_n(0, (point - n) as usize))
                .collect(),
            Vec::new(),
        )
    } else if point > 0 {
        (
            digits[..point as usize].to_vec(),
            digits[point as usize..].to_vec(),
        )
    } else {
        (
            vec![0],
            std::iter::repeat_n(0, (-point) as usize)
                .chain(digits.iter().copied())
                .collect(),
        )
    };
    if frac.len() > 16 {
        let rest = frac.split_off(16);
        let tail_nonzero = rest[1..].iter().any(|d| *d != 0);
        let last_odd = frac.last().is_some_and(|d| d % 2 == 1);
        if rest[0] > 5 || (rest[0] == 5 && (tail_nonzero || last_odd)) {
            let mut carry = true;
            for d in frac.iter_mut().rev().chain(int_part.iter_mut().rev()) {
                if *d == 9 {
                    *d = 0;
                } else {
                    *d += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                int_part.insert(0, 1);
            }
        }
    }
    while frac.last() == Some(&0) {
        frac.pop();
    }
    while int_part.len() > 1 && int_part[0] == 0 {
        int_part.remove(0);
    }
    if frac.is_empty() && int_part == [0] {
        return "0".into();
    }
    let mut s = String::with_capacity(int_part.len() + frac.len() + 2);
    if v < 0.0 {
        s.push('-');
    }
    s.extend(int_part.iter().map(|d| (b'0' + d) as char));
    if !frac.is_empty() {
        s.push('.');
        s.extend(frac.iter().map(|d| (b'0' + d) as char));
    }
    s
}

fn ring_wkt(points: &[(f64, f64, f64)], out: &mut String) {
    out.push('(');
    for (i, (x, y, _)) in points.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&wkt_number(*x));
        out.push(' ');
        out.push_str(&wkt_number(*y));
    }
    out.push(')');
}

fn polygon_body_wkt(poly: &Geometry, out: &mut String) {
    out.push('(');
    for i in 0..poly.geometry_count() {
        if i > 0 {
            out.push_str(", ");
        }
        let mut pts = Vec::new();
        poly.get_geometry(i).get_points(&mut pts);
        ring_wkt(&pts, out);
    }
    out.push(')');
}

/// shapely `.wkt` of a 2D Polygon / MultiPolygon (the AOI is always one of the two).
pub fn polygonal_wkt(g: &Geometry) -> Result<String> {
    let flat = gdal::vector::geometry_type_flatten(g.geometry_type());
    let mut out = String::new();
    if flat == OGRwkbGeometryType::wkbPolygon {
        out.push_str("POLYGON ");
        if g.is_empty() {
            out.push_str("EMPTY");
        } else {
            polygon_body_wkt(g, &mut out);
        }
    } else if flat == OGRwkbGeometryType::wkbMultiPolygon {
        out.push_str("MULTIPOLYGON ");
        if g.is_empty() {
            out.push_str("EMPTY");
        } else {
            out.push('(');
            for i in 0..g.geometry_count() {
                if i > 0 {
                    out.push_str(", ");
                }
                polygon_body_wkt(&g.get_geometry(i), &mut out);
            }
            out.push(')');
        }
    } else {
        return Err(Error::new("TypeError", "AOI is not polygonal"));
    }
    Ok(out)
}

/// The two bind values `_load_intersecting` sends:
/// `SRID=4326;{aoi.geometry.to_crs(4326).union_all().wkt}` and
/// `SRID=54034;{aoi.geometry.union_all().wkt}` (a second union of the already dissolved AOI).
pub fn aoi_ewkt_params(aoi: &Aoi) -> Result<(String, String)> {
    let (p4326, multi) = aoi_4326_union(aoi)?;
    let g4326 = polygons_geometry(&p4326, multi)?;
    let gref = union_all([&aoi_geometry(aoi)?])?;
    Ok((
        format!("SRID=4326;{}", polygonal_wkt(&g4326)?),
        format!("SRID={REFERENCE_SRID};{}", polygonal_wkt(&gref)?),
    ))
}

/// One row of a loader: the attribute columns in SELECT order (text, integers and floats as
/// JSON values, NULL as Null) and the geometry, in REFERENCE_CRS. `geom` is None for a NULL.
pub struct Feature {
    pub attrs: Vec<(String, Value)>,
    pub geom: Option<Geometry>,
}

impl Feature {
    /// The attribute by column name (`row["NAME"]`); Null when absent.
    pub fn get(&self, name: &str) -> &Value {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .unwrap_or(&Value::Null)
    }

    /// Python `str(row[name])`.
    pub fn str_of(&self, name: &str) -> String {
        py_str(self.get(name))
    }
}

/// Python `str()` of a value read from the database: None -> "None", text as is.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// PostGIS EWKB (geometry_send) -> OGR, with the SRID header removed (ISO/OGC WKB is what OGR
/// reads); the coordinates are the stored doubles, as shapely gets them from the hex EWKB.
fn ewkb_to_geometry(b: &[u8]) -> Result<Geometry> {
    const SRID_FLAG: u32 = 0x2000_0000;
    let short = || Error::new("WKBReadingError", "short EWKB");
    let head: [u8; 4] = b
        .get(1..5)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(short)?;
    let le = b[0] == 1;
    let ty = if le {
        u32::from_le_bytes(head)
    } else {
        u32::from_be_bytes(head)
    };
    if ty & SRID_FLAG == 0 {
        return Geometry::from_wkb(b).map_err(gdal_err);
    }
    let body = b.get(9..).ok_or_else(short)?;
    let ty = ty & !SRID_FLAG;
    let mut wkb = Vec::with_capacity(b.len() - 4);
    wkb.push(b[0]);
    wkb.extend_from_slice(&if le {
        ty.to_le_bytes()
    } else {
        ty.to_be_bytes()
    });
    wkb.extend_from_slice(body);
    Geometry::from_wkb(&wkb).map_err(gdal_err)
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    let bad = || Error::new("WKBReadingError", "bad hex EWKB");
    if !s.len().is_multiple_of(2) {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).ok_or_else(bad)?, 16).map_err(|_| bad()))
        .collect()
}

/// psycopg2's adaptation of a Python str under standard_conforming_strings: `'...'`, quotes
/// doubled; a NUL raises as psycopg2 does. The one way rust/geo puts a value into SQL: every query
/// runs over the simple protocol (`sqlx::raw_sql`), so no prepared statement meets a pooler that
/// shares server connections.
pub(crate) fn pg_literal(s: &str) -> Result<String> {
    if s.contains('\0') {
        return Err(Error::new(
            "ValueError",
            "A string literal cannot contain NUL (0x00) characters.",
        ));
    }
    Ok(format!("'{}'", s.replace('\'', "''")))
}

fn decode_row(row: &PgRow, to_ref: Option<&CoordTransform>) -> Result<Feature> {
    let bad = |_| Error::new("DataError", "unexpected column type");
    let mut attrs = Vec::new();
    let mut geom = None;
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(bad)?;
        if col.name() == "geom" {
            if !raw.is_null() {
                // simple-query results are text: geometry_out's hex EWKB, as psycopg2 receives it
                let hex = raw
                    .as_str()
                    .map_err(|_| Error::new("DataError", "geometry column"))?;
                let bytes = decode_hex(hex)?;
                let mut g = ewkb_to_geometry(&bytes)?;
                if let Some(ct) = to_ref {
                    g.transform_inplace(ct).map_err(gdal_err)?;
                }
                geom = Some(g);
            }
            continue;
        }
        let v = if raw.is_null() {
            Value::Null
        } else {
            match raw.type_info().name() {
                "INT2" => Value::from(row.try_get::<i16, _>(i).map_err(bad)?),
                "INT4" => Value::from(row.try_get::<i32, _>(i).map_err(bad)?),
                "INT8" => Value::from(row.try_get::<i64, _>(i).map_err(bad)?),
                "FLOAT4" => Value::from(row.try_get::<f32, _>(i).map_err(bad)? as f64),
                "FLOAT8" => Value::from(row.try_get::<f64, _>(i).map_err(bad)?),
                _ => Value::String(row.try_get::<String, _>(i).map_err(bad)?),
            }
        };
        attrs.push((col.name().to_string(), v));
    }
    Ok(Feature { attrs, geom })
}

/// EPSG:4326 -> REFERENCE_CRS, traditional GIS order (geopandas `to_crs`, pyproj always_xy).
fn to_reference() -> Result<CoordTransform> {
    let mut src = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut dst = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    CoordTransform::new(&src, &dst).map_err(gdal_err)
}

/// A query failure as the original names it: the SQLAlchemy exception type only (its text can
/// carry connection details, and `pipeline.safe` puts the text on the wire). SQLSTATE class 42
/// (undefined table / column, syntax) is psycopg2's ProgrammingError.
fn db_err(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::Database(d) if d.code().is_some_and(|c| c.starts_with("42")) => {
            Error::new("ProgrammingError", "database query failed")
        }
        sqlx::Error::Database(d) => Error::new(
            "OperationalError",
            format!(
                "database query failed (SQLSTATE {})",
                d.code().unwrap_or_default()
            ),
        ),
        sqlx::Error::PoolTimedOut => {
            Error::new("OperationalError", "database query failed (pool timed out)")
        }
        sqlx::Error::Io(_) => {
            Error::new("OperationalError", "database query failed (connection i/o)")
        }
        sqlx::Error::Protocol(_) => {
            Error::new("OperationalError", "database query failed (protocol)")
        }
        _ => Error::new("OperationalError", "database query failed"),
    }
}

/// `_load_intersecting(select_sql, aoi, clipped)`: rows of one SELECT that reads the `aoi` CTE.
/// `clipped` geometries are already in REFERENCE_CRS (geopandas skips an exact-same `to_crs`);
/// the others arrive in 4326 and are moved to REFERENCE_CRS.
pub async fn load_intersecting(
    gis: &PgPool,
    select_sql: &str,
    aoi: &Aoi,
    clipped: bool,
) -> Result<Vec<Feature>> {
    let (aoi_ewkt, aoi_ref_ewkt) = aoi_ewkt_params(aoi)?;
    // The simple query protocol with the values inlined as psycopg2 inlines `:aoi_geom`: the
    // statement text, plan and row order are the original's, and no prepared statement has to
    // survive between round trips (the GIS database answers 26000 "prepared statement does not
    // exist" to the extended protocol when two clients share it: a transaction-mode pooler).
    // The EWKT is our own WKT writer's output (digits, signs, points, keywords), never user text.
    let query = format!(
        "\n    with aoi as (\n        select ST_MakeValid(CAST({} AS geometry)) as geom,\n               \
         ST_MakeValid(CAST({} AS geometry)) as geom_ref\n    )\n    {select_sql}\n    ",
        pg_literal(&aoi_ewkt)?,
        pg_literal(&aoi_ref_ewkt)?,
    );
    let rows = sqlx::raw_sql(&query).fetch_all(gis).await.map_err(db_err)?;
    let ct = if clipped { None } else { Some(to_reference()?) };
    rows.iter().map(|r| decode_row(r, ct.as_ref())).collect()
}

/// `load_admin_intersecting` (public.adm_boundaries columns, clipped to the AOI in 54034).
pub async fn load_admin_intersecting(gis: &PgPool, aoi: &Aoi) -> Result<Vec<Feature>> {
    let columns = if ADMIN_BOUNDARIES_TABLE.ends_with("adm_boundaries")
        && ADMIN_BOUNDARIES_TABLE.contains("public.")
    {
        "\n            adm.country::text      as \"COUNTRY\",\n            adm.province::text     as \"NAME_1\",\n            \
         adm.district::text     as \"NAME_2\",\n            adm.sub_district::text as \"NAME_3\",\n            \
         adm.village::text      as \"NAME_4\",\n        "
    } else {
        "\n            adm.country::text as \"COUNTRY\",\n            adm.name_1::text  as \"NAME_1\",\n            \
         adm.name_2::text  as \"NAME_2\",\n            null::text        as \"NAME_3\",\n            \
         null::text        as \"NAME_4\",\n        "
    };
    let sql = format!(
        "\n        select\n            {columns}\n            ST_CollectionExtract(\n                \
         ST_Intersection(ST_Transform(adm.geom, {REFERENCE_SRID}), aoi.geom_ref), 3\n            ) as geom\n        \
         from {ADMIN_BOUNDARIES_TABLE} adm, aoi\n        where adm.geom && aoi.geom and ST_Intersects(adm.geom, aoi.geom)\n        "
    );
    load_intersecting(gis, &sql, aoi, true).await
}

/// `load_wdpa_intersecting`, columns renamed to the shapefile's (NAME, DESIG_ENG, IUCN_CAT,
/// STATUS, REALM).
pub async fn load_wdpa_intersecting(gis: &PgPool, aoi: &Aoi) -> Result<Vec<Feature>> {
    let sql = format!(
        "\n        select\n            w.name, w.design_eng, w.iucn_categ, w.status,\n            \
         case when w.ecosystem = 'Marine Protected Areas' then 'Marine' else 'Terrestrial' end\n                as realm,\n            \
         w.geom\n        from {WDPA_TABLE} w, aoi\n        where w.geom && aoi.geom and ST_Intersects(w.geom, aoi.geom)\n        "
    );
    let mut rows = load_intersecting(gis, &sql, aoi, false).await?;
    rename(
        &mut rows,
        &[
            ("name", "NAME"),
            ("design_eng", "DESIG_ENG"),
            ("iucn_categ", "IUCN_CAT"),
            ("status", "STATUS"),
            ("realm", "REALM"),
        ],
    );
    Ok(rows)
}

/// `load_kba_intersecting`, `intname`/`natname` renamed to `IntName`/`NatName`.
pub async fn load_kba_intersecting(gis: &PgPool, aoi: &Aoi) -> Result<Vec<Feature>> {
    let sql = format!(
        "\n        select k.intname, k.natname, k.geom\n        from {KBA_TABLE} k, aoi\n        \
         where k.geom && aoi.geom and ST_Intersects(k.geom, aoi.geom)\n        "
    );
    let mut rows = load_intersecting(gis, &sql, aoi, false).await?;
    rename(&mut rows, &[("intname", "IntName"), ("natname", "NatName")]);
    Ok(rows)
}

/// `load_key_species_intersecting`: GBIF points inside the AOI, Darwin Core columns.
pub async fn load_key_species_intersecting(gis: &PgPool, aoi: &Aoi) -> Result<Vec<Feature>> {
    let sql = format!(
        "\n        select\n            k.species,\n            k.\"class\"           as taxon_class,\n            \
         k.family            as family,\n            k.\"individualCount\" as individual_count,\n            \
         k.\"eventDate\"       as event_date,\n            k.\"basisOfRecord\"   as basis_of_record,\n            \
         k.\"Shape\"           as geom\n        from {KEY_SPECIES_TABLE} k, aoi\n        \
         where k.\"Shape\" && aoi.geom and ST_Intersects(k.\"Shape\", aoi.geom)\n        "
    );
    load_intersecting(gis, &sql, aoi, false).await
}

/// `load_plants_intersecting`: GBIF plant points inside the AOI, ordered by OBJECTID_1.
pub async fn load_plants_intersecting(gis: &PgPool, aoi: &Aoi) -> Result<Vec<Feature>> {
    let sql = format!(
        "\n        select\n            p.\"{PLANT_SPECIES}\",\n            p.\"{PLANT_CITES}\",\n            \
         p.\"{PLANT_YEAR}\",\n            p.\"Shape\" as geom\n        from {PLANT_OCCURRENCE_TABLE} p, aoi\n        \
         where p.\"Shape\" && aoi.geom and ST_Intersects(p.\"Shape\", aoi.geom)\n        order by p.\"OBJECTID_1\"\n        "
    );
    load_intersecting(gis, &sql, aoi, false).await
}

fn rename(rows: &mut [Feature], map: &[(&str, &str)]) {
    for row in rows {
        for (k, _) in row.attrs.iter_mut() {
            if let Some((_, to)) = map.iter().find(|(from, _)| from == k) {
                *k = (*to).to_string();
            }
        }
    }
}

/// `load_statistical_area(aoi, table, column)`: the unit of `table` holding the largest share
/// of the AOI, or None. `table`/`column` come from SOCIAL_LEVEL_SOURCES (constants).
pub async fn load_statistical_area(
    gis: &PgPool,
    aoi: &Aoi,
    table: &str,
    column: &str,
) -> Result<Option<String>> {
    let (aoi_ewkt, _) = aoi_ewkt_params(aoi)?;
    let query = format!(
        "\n    with aoi as (\n        select ST_MakeValid(CAST({} AS geometry)) as geom\n    )\n    \
         select t.{column}::text\n    from {table} t, aoi\n    where t.geom && aoi.geom and ST_Intersects(t.geom, aoi.geom)\n    \
         order by ST_Area(ST_Intersection(t.geom, aoi.geom)) desc\n    limit 1\n    ",
        pg_literal(&aoi_ewkt)?,
    );
    let rows = sqlx::raw_sql(&query).fetch_all(gis).await.map_err(db_err)?;
    match rows.first() {
        None => Ok(None),
        Some(r) => r
            .try_get::<Option<String>, _>(0)
            .map_err(|_| Error::new("DataError", "unexpected column type")),
    }
}

/// One row of `load_social_rows`: (category, unit, value).
pub type SocialRow = (Option<String>, Option<String>, f64);

/// `load_social_rows(iso3, table, level, area_name, where)`: `(year, [(category, unit, value)])`
/// at the newest year that has the filtered rows. `filters`: Some(text) binds `= value`, None
/// means `IS NULL`. Only `category` / `subgroup` may be filtered on.
pub async fn load_social_rows(
    social: &PgPool,
    iso3: &str,
    table: &str,
    level: i64,
    area_name: Option<&str>,
    filters: &[(String, Option<String>)],
) -> Result<(Option<i64>, Vec<SocialRow>)> {
    let column = match level {
        0 => None,
        1 => Some("name_1"),
        2 => Some("name_2"),
        3 => Some("name_3"),
        other => return Err(Error::new("KeyError", other.to_string())),
    };
    let mut conditions = vec!["value is not null".to_string()];
    // values inlined as psycopg2 inlines them (simple query protocol, see load_intersecting)
    if let Some(column) = column {
        let Some(area_name) = area_name else {
            return Ok((None, Vec::new()));
        };
        conditions.push(format!("{column} = {}", pg_literal(area_name)?));
    }
    for (key, value) in filters {
        if key != "category" && key != "subgroup" {
            return Err(Error::new(
                "ValueError",
                format!("{iso3}_{table}: cannot filter on '{key}'"),
            ));
        }
        match value {
            None => conditions.push(format!("{key} is null")),
            Some(v) => conditions.push(format!("{key} = {}", pg_literal(v)?)),
        }
    }
    let where_sql = conditions.join("\n              and ");
    let query = format!(
        "\n        with matched as (\n            select year, category, unit, value\n            \
         from {SOCIAL_SCHEMA}.{iso3}_{table}\n            where {where_sql}\n        )\n        \
         select year, category, min(unit) as unit, sum(value) as value\n        \
         from matched\n        where year = (select max(year) from matched)\n        group by year, category\n        order by category\n    "
    );
    let rows = sqlx::raw_sql(&query)
        .fetch_all(social)
        .await
        .map_err(db_err)?;
    // Text results: `int(year)`, `float(value)` (Decimal -> nearest double, as Rust's parse).
    // shortcut: category / unit are taken as text; a non-text category column would reach the
    // Python payload as a number. Unproven against the real se_v3 tables (no local credentials).
    let text = |r: &PgRow, i: usize| -> Option<String> {
        r.try_get_raw(i)
            .ok()
            .filter(|v| !v.is_null())
            .and_then(|v| v.as_str().ok().map(str::to_string))
    };
    let Some(first) = rows.first() else {
        return Ok((None, Vec::new()));
    };
    let year = text(first, 0)
        .and_then(|y| y.parse::<f64>().ok())
        .map(|y| y.trunc() as i64);
    Ok((
        year,
        rows.iter()
            .map(|r| {
                (
                    text(r, 1),
                    text(r, 2),
                    text(r, 3).and_then(|v| v.parse().ok()).unwrap_or(f64::NAN),
                )
            })
            .collect(),
    ))
}

// ============================ vector overlay (`common.py`) ============================

/// GEOS `getArea` of any geometry: rings by `Area::ofRingSigned` (abs), shell minus holes,
/// collections summed in order; points and lines 0.
pub fn geos_area(g: &Geometry) -> f64 {
    let flat = gdal::vector::geometry_type_flatten(g.geometry_type());
    if flat == OGRwkbGeometryType::wkbPolygon {
        let mut area = 0.0;
        for i in 0..g.geometry_count() {
            let mut pts = Vec::new();
            g.get_geometry(i).get_points(&mut pts);
            let a = ring_area(&pts);
            if i == 0 { area += a } else { area -= a }
        }
        area
    } else if flat == OGRwkbGeometryType::wkbMultiPolygon
        || flat == OGRwkbGeometryType::wkbGeometryCollection
    {
        (0..g.geometry_count())
            .map(|i| geos_area(&g.get_geometry(i)))
            .fold(0.0, |acc, a| acc + a)
    } else {
        0.0
    }
}

fn ring_area(r: &[(f64, f64, f64)]) -> f64 {
    let n = r.len();
    if n < 3 {
        return 0.0;
    }
    let x0 = r[0].0;
    let mut sum = 0.0;
    for i in 1..n - 1 {
        sum += (r[i].0 - x0) * (r[i - 1].1 - r[i + 1].1);
    }
    (sum / 2.0).abs()
}

/// shapely `a.intersection(b).area` (GEOSIntersection through OGR). A null result is 0.
pub fn intersection_area(a: &Geometry, b: &Geometry) -> f64 {
    a.intersection(b).map(|g| geos_area(&g)).unwrap_or(0.0)
}

/// The AOI's own geometry, `aoi.geometry.iloc[0]`.
pub fn aoi_geometry(aoi: &Aoi) -> Result<Geometry> {
    polygons_geometry(&aoi.polygons, aoi.is_multi)
}

/// shapely `union_all` over geometries (a missing one is skipped), GEOS unary union of a
/// GeometryCollection holding them in order.
pub fn union_all<'a>(geoms: impl IntoIterator<Item = &'a Geometry>) -> Result<Geometry> {
    let mut gc = Geometry::empty(OGRwkbGeometryType::wkbGeometryCollection).map_err(gdal_err)?;
    for g in geoms {
        gc.add_geometry(g.clone()).map_err(gdal_err)?;
    }
    ffi::unary_union(&gc)
}

/// `union_overlap_ha(aoi, features)`: `aoi.geometry.intersection(features.union_all()).area / 1e4`.
pub fn union_overlap_ha(aoi: &Aoi, features: &[Feature]) -> Result<f64> {
    if features.is_empty() {
        return Ok(0.0);
    }
    let merged = union_all(features.iter().filter_map(|f| f.geom.as_ref()))?;
    Ok(intersection_area(&aoi_geometry(aoi)?, &merged) / M2_PER_HA)
}

/// `per_feature_overlap_ha(aoi, features)`: `geom.intersection(aoi).area / 1e4` per row, in order.
pub fn per_feature_overlap_ha(aoi: &Aoi, features: &[Feature]) -> Result<Vec<f64>> {
    let a = aoi_geometry(aoi)?;
    Ok(features
        .iter()
        // a NULL geometry: shapely's intersection gives None, whose area is NaN
        .map(|f| {
            f.geom
                .as_ref()
                .map_or(f64::NAN, |g| intersection_area(g, &a) / M2_PER_HA)
        })
        .collect())
}

#[cfg(test)]
mod vector_tests {
    use super::{pg_literal, wkt_number};

    #[test]
    fn literals_are_psycopg2s() {
        assert_eq!(pg_literal("V3_BUCKET").unwrap(), "'V3_BUCKET'");
        assert_eq!(pg_literal("a'b\\c").unwrap(), "'a''b\\c'");
        assert_eq!(pg_literal("a\0b").unwrap_err().kind, "ValueError");
    }

    /// Every line of proof/expected/wkt_numbers.tsv: repr(x) TAB shapely's text for x.
    #[test]
    fn wkt_numbers_match_shapely() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/proof/expected/wkt_numbers.tsv"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut bad = Vec::new();
        for line in text.lines() {
            let (repr, want) = line.split_once('\t').unwrap_or((line, ""));
            let got = wkt_number(repr.parse().unwrap_or(f64::NAN));
            if got != want {
                bad.push(format!("{repr}: {got} != {want}"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} mismatches, first: {:?}",
            bad.len(),
            &bad[..bad.len().min(5)]
        );
    }
}
