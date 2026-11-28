//! POST /geos/map/upload, the parsing half: `routes.py geo_map_upload` from the file checks to
//! `json.loads(results)`, i.e. `GeoLogic.process_zip/kml/kmz_and_get_polygon`
//! (application/apis/geo_apis/utils.py), on the same GDAL/OGR (fiona and pyogrio are OGR) and the
//! same GEOS (shapely's union). Everything happens in memory (/vsimem); nothing touches the disk.
//! The database half of the route (session row, area query, limits) belongs to the handler: see
//! README.md.

mod ffi;

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use gdal::vector::{Geometry, LayerAccess, geometry_type_flatten, geometry_type_has_z};
use gdal::{Dataset, DatasetOptions, DriverManager, GdalOpenFlags, Metadata};
use gdal_sys::OGRwkbGeometryType as T;
use serde_json::{Value, json};

/// What the route answers when parsing fails: 400 for `AppMessageException`, 500 for any other
/// exception; `message` is `str(e)` (the handler redacts 500 messages outside development).
#[derive(Debug, Clone, PartialEq)]
pub struct UploadError {
    pub status: u16,
    pub message: String,
}

type R<T> = Result<T, UploadError>;

fn bad(m: impl Into<String>) -> UploadError {
    UploadError {
        status: 400,
        message: m.into(),
    }
}
fn internal(m: impl Into<String>) -> UploadError {
    UploadError {
        status: 500,
        message: m.into(),
    }
}

/// `routes.py` from `if not file.filename` to `results = json.loads(results)`: the FeatureCollection
/// the handler then extends with session_id / area_size / user_limit / exceed_size.
/// `session_id` is the canonical one the handler settled on; it only shapes file paths inside
/// GDAL error messages, as the original's `Uploaded-File/<session_id>/...` paths.
pub fn parse_upload(filename: &str, data: &[u8], session_id: &str) -> R<Value> {
    if filename.is_empty() {
        return Err(bad("No selected file"));
    }
    if !allowed_file(filename) {
        return Err(bad(
            "Invalid file format, only ZIP, KML or KMZ files are allowed",
        ));
    }
    let extension = extension(filename).unwrap_or_default();
    let mem = MemDir::new(session_id);
    let name = secure_filename(filename);
    match extension.as_str() {
        "zip" => process_zip(&mem, data),
        "kml" => {
            let path = mem.put(&name, data.to_vec())?;
            process_kml(&mem, &path)
        }
        _ => process_kmz(&mem, data),
    }
}

/// `GeoUtils.allowed_file`.
pub fn allowed_file(filename: &str) -> bool {
    matches!(extension(filename).as_deref(), Some("kml" | "kmz" | "zip"))
}

/// `filename.rsplit('.', 1)[1].lower()` (Unicode lower, as Python's str.lower).
fn extension(filename: &str) -> Option<String> {
    filename.rsplit_once('.').map(|(_, e)| e.to_lowercase())
}

/// werkzeug 3 `secure_filename` on POSIX (production). Only reaches error-message paths.
pub fn secure_filename(filename: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let ascii: String = filename
        .nfkd()
        .filter(char::is_ascii)
        .collect::<String>()
        .replace('/', " ");
    let joined = ascii.split_ascii_whitespace().collect::<Vec<_>>().join("_");
    let kept: String = joined
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        .collect();
    kept.trim_matches(|c| c == '.' || c == '_').to_string()
}

// ---------------------------------------------------------------------------------------------
// In-memory upload folder

static NEXT: AtomicU64 = AtomicU64::new(0);

/// `Uploaded-File/<session_id>/` as a /vsimem folder; every file is unlinked on drop (the
/// original's `user_remove_upload_tree_file` in each `finally`).
struct MemDir {
    root: String,
    session_id: String,
    /// (path, buffer): GDAL reads the buffers in place; unlinked in `drop` before they are freed.
    files: std::cell::RefCell<Vec<(String, Vec<u8>)>>,
}

impl MemDir {
    fn new(session_id: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        MemDir {
            root: format!("/vsimem/nbs_upload_{}_{n}", std::process::id()),
            session_id: session_id.to_string(),
            files: Default::default(),
        }
    }

    fn put(&self, rel: &str, data: Vec<u8>) -> R<String> {
        let path = format!("{}/{rel}", self.root);
        // A re-put path (duplicate zip entry) is replaced by GDAL; the old buffer stays until drop.
        ffi::mem_file_over(&path, &data).map_err(internal)?;
        self.files.borrow_mut().push((path.clone(), data));
        Ok(path)
    }

    /// A GDAL message with our /vsimem paths turned into the original's relative paths.
    fn msg(&self, m: String) -> String {
        m.replace(&self.root, &format!("Uploaded-File/{}", self.session_id))
    }
}

impl Drop for MemDir {
    fn drop(&mut self) {
        for (f, _) in self.files.borrow().iter() {
            let _ = gdal::vsi::unlink_mem_file(f);
        }
    }
}

fn gdal_msg(e: gdal::errors::GdalError) -> String {
    use gdal::errors::GdalError::*;
    match e {
        NullPointer { msg, .. } | CplError { msg, .. } => msg,
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// zip / kmz: zipfile.extractall + os.walk, the last match wins

/// `ZipFile(path).extractall(<root>/temp_zip_extraction)` then the `os.walk` search for the last
/// file named `*<suffix>` not starting with '.'. Returns the /vsimem path of the match.
fn extract_and_find(mem: &MemDir, data: &[u8], suffix: &str) -> R<Option<String>> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(data)).map_err(|_| internal("File is not a zip file"))?;
    // CPython `_extract_member` on POSIX: drop '', '.', '..' components; later entries overwrite.
    let mut files: BTreeMap<Vec<String>, Vec<u8>> = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| internal(e.to_string()))?;
        let parts: Vec<String> = f
            .name()
            .split('/')
            .filter(|p| !matches!(*p, "" | "." | ".."))
            .map(str::to_string)
            .collect();
        let is_dir = f.is_dir();
        let mut buf = Vec::with_capacity(f.size().min(1 << 26) as usize);
        f.read_to_end(&mut buf).map_err(|e| {
            if e.to_string().contains("checksum") {
                internal(format!("Bad CRC-32 for file '{}'", f.name()))
            } else {
                internal(e.to_string())
            }
        })?;
        if !is_dir && !parts.is_empty() {
            files.insert(parts, buf);
        }
    }
    // os.walk top-down: a folder's files, then its sub-folders. Listing order is the file
    // system's (unspecified on Linux ext4); here it is sorted by name, as on NTFS for ASCII names.
    let mut found: Option<Vec<String>> = None;
    walk(&files, &[], suffix, &mut found);
    for (parts, buf) in files {
        mem.put(&format!("temp_zip_extraction/{}", parts.join("/")), buf)?;
    }
    Ok(found.map(|p| format!("{}/temp_zip_extraction/{}", mem.root, p.join("/"))))
}

fn walk(
    files: &BTreeMap<Vec<String>, Vec<u8>>,
    dir: &[String],
    suffix: &str,
    found: &mut Option<Vec<String>>,
) {
    let mut subdirs: Vec<&String> = Vec::new();
    for parts in files
        .keys()
        .filter(|p| p.len() > dir.len() && p[..dir.len()] == *dir)
    {
        let name = &parts[dir.len()];
        if parts.len() == dir.len() + 1 {
            if !name.starts_with('.') && name.ends_with(suffix) {
                *found = Some(parts.clone());
            }
        } else if subdirs.last() != Some(&name) {
            subdirs.push(name);
        }
    }
    for s in subdirs {
        let mut d = dir.to_vec();
        d.push(s.clone());
        walk(files, &d, suffix, found);
    }
}

fn process_kmz(mem: &MemDir, data: &[u8]) -> R<Value> {
    let path = extract_and_find(mem, data, ".kml")?
        .ok_or_else(|| bad("No .kml file found in the ZIP file."))?;
    process_kml(mem, &path)
}

// ---------------------------------------------------------------------------------------------
// Opening, as fiona 1.9.6 / pyogrio 0.9 do: GDALOpenEx(OF_VECTOR) with every driver. fiona's GDAL
// (3.6.4) has LIBKML, which wins for .kml as in osgeo's 3.8.4; pyogrio's (3.8.5) has no LIBKML,
// so the shapefile path excludes it (only matters for a *.shp that is really KML).

fn vector_drivers_without_libkml() -> &'static [String] {
    static D: OnceLock<Vec<String>> = OnceLock::new();
    D.get_or_init(|| {
        DriverManager::all()
            .filter(|d| d.metadata_item("DCAP_VECTOR", "").as_deref() == Some("YES"))
            .map(|d| d.short_name())
            .filter(|n| n != "LIBKML")
            .collect()
    })
}

fn open_vector(mem: &MemDir, path: &str, libkml: bool) -> R<Dataset> {
    let names: Vec<&str> = vector_drivers_without_libkml()
        .iter()
        .map(String::as_str)
        .collect();
    let opts = DatasetOptions {
        open_flags: GdalOpenFlags::GDAL_OF_VECTOR,
        allowed_drivers: (!libkml).then_some(names.as_slice()),
        ..Default::default()
    };
    Dataset::open_ex(path, opts).map_err(|e| internal(mem.msg(gdal_msg(e))))
}

// ---------------------------------------------------------------------------------------------
// zip: process_zip_and_get_polygon

fn process_zip(mem: &MemDir, data: &[u8]) -> R<Value> {
    let path = extract_and_find(mem, data, ".shp")?
        .ok_or_else(|| bad("No .shp file found in the ZIP file."))?;
    // aoi = gpd.read_file(filename)   (pyogrio: first layer, every feature)
    let ds = open_vector(mem, &path, false)?;
    let mut layer = ds.layer(0).map_err(|e| internal(mem.msg(gdal_msg(e))))?;
    let srs = layer.spatial_ref();
    let geoms: Vec<Geometry> = layer
        .features()
        .filter_map(|f| f.geometry().cloned())
        .collect();
    // aoi = aoi.to_crs(4326)
    let srs = srs.ok_or_else(|| {
        internal("Cannot transform naive geometries.  Please set a crs on the object first.")
    })?;
    let mut collection =
        Geometry::empty(T::wkbGeometryCollection).map_err(|e| internal(e.to_string()))?;
    // pyogrio hands pyproj "EPSG:<code>" when the layer CRS carries an EPSG authority, else WKT;
    // to_crs is a no-op when that CRS is exactly EPSG:4326.
    let epsg = (srs.auth_name().as_deref() == Some("EPSG"))
        .then(|| srs.auth_code().ok())
        .flatten();
    let transform = if epsg == Some(4326) {
        None
    } else {
        let mut src = match epsg {
            Some(code) => gdal::spatial_ref::SpatialRef::from_epsg(code as u32)
                .map_err(|e| internal(e.to_string()))?,
            None => srs,
        };
        let mut dst =
            gdal::spatial_ref::SpatialRef::from_epsg(4326).map_err(|e| internal(e.to_string()))?;
        src.set_axis_mapping_strategy(gdal::spatial_ref::AxisMappingStrategy::TraditionalGisOrder);
        dst.set_axis_mapping_strategy(gdal::spatial_ref::AxisMappingStrategy::TraditionalGisOrder);
        // OGR has a closed-form shortcut for Web Mercator -> WGS84 that differs from PROJ in the
        // last bits; pyproj runs PROJ's pipeline, so it is forced here (axisswap: OGR expects the
        // operation's output in EPSG:4326's lat,lon order, then applies the traditional order).
        let web_mercator =
            gdal::spatial_ref::SpatialRef::from_epsg(3857).map_err(|e| internal(e.to_string()))?;
        let mut options =
            gdal::spatial_ref::CoordTransformOptions::new().map_err(|e| internal(e.to_string()))?;
        if src == web_mercator {
            options
                .set_coordinate_operation(
                    "+proj=pipeline +step +inv +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84 \
                     +step +proj=unitconvert +xy_in=rad +xy_out=deg +step +proj=axisswap +order=2,1",
                    false,
                )
                .map_err(|e| internal(gdal_msg(e)))?;
        }
        Some(
            gdal::spatial_ref::CoordTransform::new_with_options(&src, &dst, &options)
                .map_err(|e| internal(gdal_msg(e)))?,
        )
    };
    for mut g in geoms {
        if let Some(ct) = &transform {
            g.transform_inplace(ct).map_err(|e| internal(gdal_msg(e)))?;
        }
        collection
            .add_geometry(g)
            .map_err(|e| internal(e.to_string()))?;
    }
    // aoi.unary_union -> GeoDataFrame(geometry=[union]) -> drop z -> to_json()
    let union = ffi::unary_union(&collection).map_err(internal)?;
    Ok(feature_collection(vec![geometry_or_null(&union)]))
}

// ---------------------------------------------------------------------------------------------
// kml: process_kml_and_get_polygon

/// SUPPORTED: fiona's `supported_drivers` plus the KML entries the function adds.
const FIONA_DRIVERS: &[&str] = &[
    "DXF",
    "CSV",
    "OpenFileGDB",
    "ESRIJSON",
    "ESRI Shapefile",
    "FlatGeobuf",
    "GeoJSON",
    "GeoJSONSeq",
    "GPKG",
    "GML",
    "OGR_GMT",
    "GPX",
    "Idrisi",
    "MapInfo File",
    "DGN",
    "PCIDSK",
    "OGR_PDS",
    "S57",
    "SQLite",
    "TopoJSON",
    "kml",
    "KML",
    "libkml",
    "LIBKML",
];

fn process_kml(mem: &MemDir, path: &str) -> R<Value> {
    // collection = list(fiona.open(filepath, 'r'))
    let ds = open_vector(mem, path, true)?;
    let driver = ds.driver().short_name();
    if !FIONA_DRIVERS.contains(&driver.as_str()) {
        return Err(internal(format!("unsupported driver: '{driver}'")));
    }
    if ds.layer_count() == 0 {
        return Err(internal("Null layer: ''"));
    }
    let mut layer = ds.layer(0).map_err(|e| internal(mem.msg(gdal_msg(e))))?;
    // A type fiona has no GeoJSON for (LIBKML gives "Unknown" for a Placemark without geometry)
    // never passes isvalid, like a null geometry.
    let raw: Vec<Option<G>> = layer
        .features()
        .map(|f| f.geometry().and_then(|g| read_g(g).ok()))
        .collect();
    // df = pd.DataFrame(collection); df["is_valid"] = df['geometry'].apply(...)
    if raw.is_empty() {
        return Err(internal("'geometry'"));
    }
    // isvalid: shapely shape() of the fiona geometry succeeds (None never does)
    let valid: Vec<G> = raw
        .into_iter()
        .flatten()
        .filter(|g| shapely(g).is_ok())
        .collect();
    if valid.is_empty() {
        return Err(internal(
            "Assigning CRS to a GeoDataFrame without a geometry column is not supported. Supply geometry using \
             the 'geometry=' keyword argument, or by providing a DataFrame with column name 'geometry'",
        ));
    }
    // df_valid.to_json(orient='records') -> json.loads -> from_features: the geometry is fiona's
    // Geometry object serialised by pandas' ujson (public attributes, floats at 10 decimals), so
    // a GeometryCollection arrives with "coordinates": None and shape() rejects it.
    let mut collection =
        Geometry::empty(T::wkbGeometryCollection).map_err(|e| internal(e.to_string()))?;
    for g in &valid {
        if matches!(g, G::Collection(_)) {
            return Err(internal("Unknown geometry type: 'geometrycollection'"));
        }
        let geom = shapely(&g.map(pandas_round)).map_err(internal)?;
        collection
            .add_geometry(geom)
            .map_err(|e| internal(e.to_string()))?;
    }
    // gdf.dissolve() -> explode(index_parts=False) -> to_crs(4326) no-op -> drop z -> to_json()
    // properties are always {}: pandas serialises fiona's Properties object as an empty object.
    let union = ffi::unary_union(&collection).map_err(internal)?;
    let parts: Vec<Value> = if union.is_empty() {
        Vec::new()
    } else if is_multi(&union) {
        (0..union.geometry_count())
            .map(|i| geometry_or_null(&union.get_geometry(i)))
            .collect()
    } else {
        vec![geometry_or_null(&union)]
    };
    Ok(feature_collection(parts))
}

fn is_multi(g: &Geometry) -> bool {
    matches!(
        geometry_type_flatten(g.geometry_type()),
        T::wkbMultiPoint | T::wkbMultiLineString | T::wkbMultiPolygon | T::wkbGeometryCollection
    )
}

/// pandas 2.2 ujson `Buffer_AppendDoubleUnchecked` at the default double_precision=10, read back
/// by Python's float(). Outside (1e-15, 1e16-1) ujson writes the shortest repr: value unchanged.
pub fn pandas_round(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let neg = value < 0.0;
    let v = value.abs();
    if v > 1e16 - 1.0 || v < 1e-15 {
        return value;
    }
    let pow10 = 1e10_f64;
    let mut whole = v as u64;
    let tmp = (v - whole as f64) * pow10;
    let mut frac = tmp as u64;
    let diff = tmp - frac as f64;
    if diff > 0.5 || (diff == 0.5 && (frac == 0 || frac & 1 == 1)) {
        frac += 1;
    }
    if frac as f64 >= pow10 {
        frac = 0;
        whole += 1;
    }
    let text = format!("{}{whole}.{frac:010}", if neg { "-" } else { "" });
    text.parse().unwrap_or(value)
}

// ---------------------------------------------------------------------------------------------
// Geometry as fiona hands it to Python, and shapely's shape() on it

type C = Vec<f64>;

/// A fiona geometry: coordinates as fiona builds them (x, y[, z]).
#[derive(Debug, Clone)]
enum G {
    Point(C),
    Line(Vec<C>),
    Poly(Vec<Vec<C>>),
    MultiPoint(Vec<C>),
    MultiLine(Vec<Vec<C>>),
    MultiPoly(Vec<Vec<Vec<C>>>),
    Collection(Vec<G>),
}

impl G {
    fn map(&self, f: fn(f64) -> f64) -> G {
        let c = |c: &C| c.iter().map(|&v| f(v)).collect::<C>();
        let l = |l: &Vec<C>| l.iter().map(c).collect::<Vec<C>>();
        let p = |p: &Vec<Vec<C>>| p.iter().map(l).collect::<Vec<_>>();
        match self {
            G::Point(x) => G::Point(c(x)),
            G::Line(x) => G::Line(l(x)),
            G::Poly(x) => G::Poly(p(x)),
            G::MultiPoint(x) => G::MultiPoint(l(x)),
            G::MultiLine(x) => G::MultiLine(p(x)),
            G::MultiPoly(x) => G::MultiPoly(x.iter().map(p).collect()),
            G::Collection(x) => G::Collection(x.iter().map(|g| g.map(f)).collect()),
        }
    }
}

fn coords(g: &Geometry) -> Vec<C> {
    let z = geometry_type_has_z(g.geometry_type());
    let mut pts = Vec::new();
    g.get_points(&mut pts);
    pts.into_iter()
        .map(|(x, y, zz)| if z { vec![x, y, zz] } else { vec![x, y] })
        .collect()
}

fn read_g(g: &Geometry) -> R<G> {
    let sub = |i: usize| g.get_geometry(i);
    let n = g.geometry_count();
    Ok(match geometry_type_flatten(g.geometry_type()) {
        T::wkbPoint => G::Point(coords(g).into_iter().next().unwrap_or_default()),
        T::wkbLineString | T::wkbLinearRing => G::Line(coords(g)),
        T::wkbPolygon => G::Poly((0..n).map(|i| coords(&sub(i))).collect()),
        T::wkbMultiPoint => G::MultiPoint(
            (0..n)
                .map(|i| coords(&sub(i)).into_iter().next().unwrap_or_default())
                .collect(),
        ),
        T::wkbMultiLineString => G::MultiLine((0..n).map(|i| coords(&sub(i))).collect()),
        T::wkbMultiPolygon => G::MultiPoly(
            (0..n)
                .map(|i| {
                    let p = sub(i);
                    (0..p.geometry_count())
                        .map(|j| coords(&p.get_geometry(j)))
                        .collect()
                })
                .collect(),
        ),
        T::wkbGeometryCollection => {
            G::Collection((0..n).map(|i| read_g(&sub(i))).collect::<R<_>>()?)
        }
        other => {
            return Err(internal(format!(
                "Unsupported geometry type {}",
                gdal::vector::geometry_type_to_name(other)
            )));
        }
    })
}

/// shapely 2.0.5 `_is_coordinates_empty` of a GeoJSON "coordinates" member.
fn coords_empty(g: &G) -> bool {
    fn l(x: &[C]) -> bool {
        x.is_empty() || x.iter().all(Vec::is_empty)
    }
    fn p(x: &[Vec<C>]) -> bool {
        x.is_empty() || x.iter().all(|r| l(r))
    }
    match g {
        G::Point(c) => c.is_empty(),
        G::Line(x) | G::MultiPoint(x) => l(x),
        G::Poly(x) | G::MultiLine(x) => p(x),
        G::MultiPoly(x) => x.is_empty() || x.iter().all(|q| p(q)),
        G::Collection(_) => false,
    }
}

fn empty(t: T::Type) -> Result<Geometry, String> {
    Geometry::empty(t).map_err(|e| e.to_string())
}

fn add(g: &mut Geometry, c: &C) {
    match c.as_slice() {
        [x, y] => g.add_point_2d((*x, *y)),
        [x, y, z, ..] => g.add_point((*x, *y, *z)),
        _ => {}
    }
}

fn point(c: &C) -> Result<Geometry, String> {
    let mut g = empty(T::wkbPoint)?;
    add(&mut g, c);
    Ok(g)
}

fn line(cs: &[C]) -> Result<Geometry, String> {
    if cs.is_empty() {
        return empty(T::wkbLineString);
    }
    if cs.len() < 2 {
        return Err("LineStrings must have at least 2 coordinate tuples".into());
    }
    let mut g = empty(T::wkbLineString)?;
    cs.iter().for_each(|c| add(&mut g, c));
    Ok(g)
}

/// shapely.linearrings: closes an open ring (all dimensions compared), then needs 4 coordinates.
fn ring(cs: &[C]) -> Result<Geometry, String> {
    let mut g = empty(T::wkbLinearRing)?;
    if cs.is_empty() {
        return Ok(g);
    }
    let closed = cs.first() == cs.last();
    if cs.len() + usize::from(!closed) < 4 {
        return Err("A linearring requires at least 4 coordinates.".into());
    }
    cs.iter().for_each(|c| add(&mut g, c));
    if !closed {
        add(&mut g, &cs[0]);
    }
    Ok(g)
}

fn polygon(rings: &[Vec<C>]) -> Result<Geometry, String> {
    let mut g = empty(T::wkbPolygon)?;
    if rings.first().is_none_or(|r| r.is_empty()) {
        return Ok(g);
    }
    for r in rings {
        g.add_geometry(ring(r)?).map_err(|e| e.to_string())?;
    }
    Ok(g)
}

/// shapely 2.0.5 `shape(geojson)`: the geometry, or the error text it raises.
fn shapely(g: &G) -> Result<Geometry, String> {
    let multi = |t: T::Type, parts: Vec<Geometry>, kind: &str| -> Result<Geometry, String> {
        let mut m = empty(t)?;
        for p in parts {
            if p.is_empty() {
                return Err(format!("Can't create {kind} with empty component"));
            }
            m.add_geometry(p).map_err(|e| e.to_string())?;
        }
        Ok(m)
    };
    if coords_empty(g) {
        return empty(match g {
            G::Point(_) => T::wkbPoint,
            G::Line(_) => T::wkbLineString,
            G::Poly(_) => T::wkbPolygon,
            G::MultiPoint(_) => T::wkbMultiPoint,
            G::MultiLine(_) => T::wkbMultiLineString,
            G::MultiPoly(_) => T::wkbMultiPolygon,
            G::Collection(_) => T::wkbGeometryCollection,
        });
    }
    match g {
        G::Point(c) => point(c),
        G::Line(cs) => line(cs),
        G::Poly(rs) => polygon(rs),
        G::MultiPoint(ps) => multi(
            T::wkbMultiPoint,
            ps.iter().map(point).collect::<Result<_, _>>()?,
            "MultiPoint",
        ),
        G::MultiLine(ls) => multi(
            T::wkbMultiLineString,
            ls.iter().map(|l| line(l)).collect::<Result<_, _>>()?,
            "MultiLineString",
        ),
        G::MultiPoly(ps) => multi(
            T::wkbMultiPolygon,
            ps.iter().map(|p| polygon(p)).collect::<Result<_, _>>()?,
            "MultiPolygon",
        ),
        G::Collection(gs) => {
            let mut m = empty(T::wkbGeometryCollection)?;
            for x in gs {
                m.add_geometry(shapely(x)?).map_err(|e| e.to_string())?;
            }
            Ok(m)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// GeoDataFrame.to_json()

fn feature_collection(geometries: Vec<Value>) -> Value {
    let features: Vec<Value> = geometries
        .into_iter()
        .map(|g| json!({"id": "0", "type": "Feature", "properties": {}, "geometry": g}))
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

/// `mapping(geom) if geom else None`, after the WKB round trip that drops Z.
fn geometry_or_null(g: &Geometry) -> Value {
    if g.is_empty() {
        Value::Null
    } else {
        mapping(g)
    }
}

fn num(v: f64) -> Value {
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

fn xy(g: &Geometry) -> Vec<Value> {
    let mut pts = Vec::new();
    g.get_points(&mut pts);
    pts.into_iter()
        .map(|(x, y, _)| json!([num(x), num(y)]))
        .collect()
}

fn mapping(g: &Geometry) -> Value {
    let n = g.geometry_count();
    let subs = |f: &dyn Fn(&Geometry) -> Value| -> Vec<Value> {
        (0..n).map(|i| f(&g.get_geometry(i))).collect()
    };
    let rings = |p: &Geometry| -> Value {
        Value::Array(
            (0..p.geometry_count())
                .map(|i| Value::Array(xy(&p.get_geometry(i))))
                .collect(),
        )
    };
    let (kind, coordinates) = match geometry_type_flatten(g.geometry_type()) {
        T::wkbPoint => ("Point", xy(g).into_iter().next().unwrap_or(json!([]))),
        T::wkbLineString | T::wkbLinearRing => ("LineString", Value::Array(xy(g))),
        T::wkbPolygon => ("Polygon", rings(g)),
        T::wkbMultiPoint => (
            "MultiPoint",
            Value::Array(subs(&|p| xy(p).into_iter().next().unwrap_or(json!([])))),
        ),
        T::wkbMultiLineString => (
            "MultiLineString",
            Value::Array(subs(&|l| Value::Array(xy(l)))),
        ),
        T::wkbMultiPolygon => ("MultiPolygon", Value::Array(subs(&rings))),
        _ => return json!({"type": "GeometryCollection", "geometries": subs(&mapping)}),
    };
    json!({"type": kind, "coordinates": coordinates})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pandas_rounding() {
        assert_eq!(pandas_round(0.00000000005), 1e-10); // frac 0 on a tie rounds up
        assert_eq!(pandas_round(0.99999999995), 1.0);
        assert_eq!(pandas_round(-6.987654321098765), -6.9876543211);
        assert_eq!(pandas_round(1e-12), 0.0);
    }

    #[test]
    fn secure_names() {
        assert_eq!(secure_filename("My cool movie.mov"), "My_cool_movie.mov");
        assert_eq!(secure_filename("../../../etc/passwd"), "etc_passwd");
        assert_eq!(
            secure_filename("i contain cool \u{fc}ml\u{e4}uts.txt"),
            "i_contain_cool_umlauts.txt"
        );
    }
}
