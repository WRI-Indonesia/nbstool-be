//! `v3/common.py`: the AOI contract, raster access, zonal tabulation and the shared forest masks.
//!
//! Every GDAL call here is the one rasterio 1.3.10 makes for the Python function of the same name,
//! with the same options (README.md, "rasterio -> GDAL"). Arrays are row-major `Vec`s with an
//! explicit width and height; a masked array is a value `Vec<f64>` plus a `Vec<bool>` mask that
//! is true where numpy's mask is true (masked = excluded).

use gdal::raster::{RasterizeOptions, rasterize};
use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use gdal::vector::Geometry;
use gdal::{Dataset, Metadata};
use gdal_sys::{GDALResampleAlg, OGRwkbGeometryType};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::affine::{Affine, from_bounds};
use crate::config::{
    FC2014_FOREST_CODES, FC2014_RASTER, FC2024_FOREST_CODES, FC2024_RASTER, REFERENCE_CRS_WKT,
};
use crate::ffi::{self, MemDataset, gdal_err};
use crate::py::safe_pct;
use crate::settings::{Settings, gdal_path};
use crate::{Error, Result};

pub const M2_PER_HA: f64 = 10_000.0;

// ============================ AOI CONTRACT ============================

/// A linear ring as GEOS returns it (closed: last point equals the first).
pub type Ring = Vec<[f64; 2]>;

/// The project area, already in REFERENCE_CRS (ESRI:54034), dissolved into one geometry.
#[derive(Clone, Debug)]
pub struct Aoi {
    /// Polygons of the dissolved geometry, each a shell followed by its holes.
    pub polygons: Vec<Vec<Ring>>,
    /// True when the dissolved geometry is a MultiPolygon (shapely's `__geo_interface__` type).
    pub is_multi: bool,
    /// `geometry.bounds`: (minx, miny, maxx, maxy)
    pub bounds: (f64, f64, f64, f64),
    pub area_ha: f64,
    pub source_crs: String,
}

/// `prepare_aoi_from_session`: the session polygon, as `Polygons.get_geometry` reads it.
pub async fn prepare_aoi_from_session(app_db: &PgPool, session_id: &str) -> Result<Aoi> {
    // find_by_session_id(...).first() then get_geometry(...).first(): the same row, unordered.
    // simple protocol with the id inlined as psycopg2 would (crate::db::pg_literal)
    let query = format!(
        "select ST_AsGeoJSON(geom) from polygons where session_id = {} limit 1",
        crate::db::pg_literal(session_id)?
    );
    let row = sqlx::raw_sql(&query)
        .fetch_all(app_db)
        .await
        .and_then(|rows| {
            rows.first()
                .map(|r| sqlx::Row::try_get::<Option<String>, _>(r, 0))
                .transpose()
        })
        .map_err(|_| Error::new("OperationalError", "polygon query failed"))?;
    let Some(geojson) = row else {
        return Err(Error::new(
            "ValueError",
            format!("session id {session_id} has no polygon"),
        ));
    };
    let Some(geojson) = geojson else {
        return Err(Error::new(
            "TypeError",
            "the JSON object must be str, bytes or bytearray, not NoneType",
        ));
    };
    prepare_aoi_geojson(&geojson)
}

/// `prepare_aoi(GeoDataFrame(crs="epsg:4326", geometry=[shape(json.loads(geojson))]))`:
/// reproject to ESRI:54034 (PROJ, traditional GIS axis order, as pyproj `always_xy`), dissolve
/// with GEOS unary union, measure the planar area as GEOS does.
pub fn prepare_aoi_geojson(geojson: &str) -> Result<Aoi> {
    let parsed: Value =
        serde_json::from_str(geojson).map_err(|e| Error::new("JSONDecodeError", e.to_string()))?;
    let mut polygons = parse_polygons(&parsed)?;
    let input_is_multi = parsed["type"] == "MultiPolygon";

    let mut src = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut dst = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let ct = CoordTransform::new(&src, &dst).map_err(gdal_err)?;
    for ring in polygons.iter_mut().flatten() {
        let mut xs: Vec<f64> = ring.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f64> = ring.iter().map(|p| p[1]).collect();
        ct.transform_coords(&mut xs, &mut ys, &mut [])
            .map_err(gdal_err)?;
        for (p, (x, y)) in ring.iter_mut().zip(xs.into_iter().zip(ys)) {
            *p = [x, y];
        }
    }

    // union_all(): GEOSUnaryUnion of a GeometryCollection holding the one input geometry.
    let mut gc = Geometry::empty(OGRwkbGeometryType::wkbGeometryCollection).map_err(gdal_err)?;
    gc.add_geometry(build_geometry(&polygons, input_is_multi)?)
        .map_err(gdal_err)?;
    let dissolved = ffi::unary_union(&gc)?;
    let (polygons, is_multi) = extract_polygons(&dissolved)?;

    let area_ha = geos_area(&polygons) / M2_PER_HA;
    if area_ha <= 0.0 {
        return Err(Error::new(
            "ValueError",
            "AOI has zero area after reprojection.",
        ));
    }
    let mut bounds = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in polygons.iter().flatten().flatten() {
        bounds.0 = bounds.0.min(p[0]);
        bounds.1 = bounds.1.min(p[1]);
        bounds.2 = bounds.2.max(p[0]);
        bounds.3 = bounds.3.max(p[1]);
    }
    Ok(Aoi {
        polygons,
        is_multi,
        bounds,
        area_ha,
        source_crs: "EPSG:4326".into(),
    })
}

fn parse_ring(v: &Value) -> Result<Ring> {
    let bad = || Error::new("ValueError", "malformed GeoJSON coordinates");
    v.as_array()
        .ok_or_else(bad)?
        .iter()
        .map(|pt| {
            let x = pt.get(0).and_then(Value::as_f64).ok_or_else(bad)?;
            let y = pt.get(1).and_then(Value::as_f64).ok_or_else(bad)?;
            Ok([x, y])
        })
        .collect()
}

fn parse_polygon(v: &Value) -> Result<Vec<Ring>> {
    v.as_array()
        .ok_or_else(|| Error::new("ValueError", "malformed GeoJSON coordinates"))?
        .iter()
        .map(parse_ring)
        .collect()
}

fn parse_polygons(g: &Value) -> Result<Vec<Vec<Ring>>> {
    match g["type"].as_str() {
        Some("Polygon") => Ok(vec![parse_polygon(&g["coordinates"])?]),
        Some("MultiPolygon") => g["coordinates"]
            .as_array()
            .ok_or_else(|| Error::new("ValueError", "malformed GeoJSON coordinates"))?
            .iter()
            .map(parse_polygon)
            .collect(),
        // shortcut: sessions store only polygons; other GeoJSON types are refused, not measured
        other => Err(Error::new(
            "GeometryTypeError",
            format!("Unknown geometry type: {other:?}"),
        )),
    }
}

/// An OGR Polygon / MultiPolygon from rings, coordinates copied exactly.
pub fn polygons_geometry(polygons: &[Vec<Ring>], multi: bool) -> Result<Geometry> {
    build_geometry(polygons, multi)
}

fn build_geometry(polygons: &[Vec<Ring>], multi: bool) -> Result<Geometry> {
    let polygon = |rings: &Vec<Ring>| -> Result<Geometry> {
        let mut poly = Geometry::empty(OGRwkbGeometryType::wkbPolygon).map_err(gdal_err)?;
        for ring in rings {
            let mut lr = Geometry::empty(OGRwkbGeometryType::wkbLinearRing).map_err(gdal_err)?;
            for p in ring {
                lr.add_point_2d((p[0], p[1]));
            }
            poly.add_geometry(lr).map_err(gdal_err)?;
        }
        Ok(poly)
    };
    if !multi && polygons.len() == 1 {
        return polygon(&polygons[0]);
    }
    let mut mp = Geometry::empty(OGRwkbGeometryType::wkbMultiPolygon).map_err(gdal_err)?;
    for rings in polygons {
        mp.add_geometry(polygon(rings)?).map_err(gdal_err)?;
    }
    Ok(mp)
}

fn extract_polygons(g: &Geometry) -> Result<(Vec<Vec<Ring>>, bool)> {
    let rings_of = |poly: &Geometry| -> Vec<Ring> {
        (0..poly.geometry_count())
            .map(|i| {
                let ring = poly.get_geometry(i);
                let mut pts = Vec::new();
                ring.get_points(&mut pts);
                pts.into_iter().map(|(x, y, _)| [x, y]).collect()
            })
            .collect()
    };
    let flat = gdal::vector::geometry_type_flatten(g.geometry_type());
    if flat == OGRwkbGeometryType::wkbPolygon {
        Ok((vec![rings_of(g)], false))
    } else if flat == OGRwkbGeometryType::wkbMultiPolygon {
        Ok((
            (0..g.geometry_count())
                .map(|i| rings_of(&g.get_geometry(i)))
                .collect(),
            true,
        ))
    } else {
        Err(Error::new(
            "ValueError",
            "AOI has zero area after reprojection.",
        ))
    }
}

/// GEOS `Area::ofRingSigned` (shoelace anchored at the first x), as shapely's `.area` computes it.
fn geos_ring_area(ring: &Ring) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let x0 = ring[0][0];
    let mut sum = 0.0;
    for i in 1..n - 1 {
        let x = ring[i][0] - x0;
        sum += x * (ring[i - 1][1] - ring[i + 1][1]);
    }
    (sum / 2.0).abs()
}

/// GEOS Polygon::getArea (shell minus holes), summed over parts as GeometryCollection::getArea.
fn geos_area(polygons: &[Vec<Ring>]) -> f64 {
    let mut area = 0.0;
    for rings in polygons {
        let mut a = 0.0;
        for (i, ring) in rings.iter().enumerate() {
            if i == 0 {
                a += geos_ring_area(ring)
            } else {
                a -= geos_ring_area(ring)
            }
        }
        area += a;
    }
    area
}

// ============================ RASTER ACCESS ============================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resampling {
    Nearest,
    Bilinear,
    Average,
    Sum,
}

impl Resampling {
    fn gdal(self) -> GDALResampleAlg::Type {
        match self {
            Resampling::Nearest => GDALResampleAlg::GRA_NearestNeighbour,
            Resampling::Bilinear => GDALResampleAlg::GRA_Bilinear,
            Resampling::Average => GDALResampleAlg::GRA_Average,
            Resampling::Sum => GDALResampleAlg::GRA_Sum,
        }
    }
}

/// A raster clipped and reprojected to the AOI in REFERENCE_CRS (`RasterSlice`).
#[derive(Clone, Debug)]
pub struct RasterSlice {
    pub values: Vec<f64>,
    /// numpy mask: true = masked (nodata or outside the AOI polygon)
    pub mask: Vec<bool>,
    pub width: usize,
    pub height: usize,
    pub pixel_area_ha: f64,
    pub transform: Affine,
}

impl RasterSlice {
    /// `values.count()`
    pub fn valid_count(&self) -> usize {
        self.mask.iter().filter(|m| !**m).count()
    }

    pub fn valid_area_ha(&self) -> f64 {
        self.valid_count() as f64 * self.pixel_area_ha
    }

    /// `np.ma.filled(values == code, False).sum()`
    pub fn count_eq(&self, code: f64) -> usize {
        self.values
            .iter()
            .zip(&self.mask)
            .filter(|(v, m)| !**m && **v == code)
            .count()
    }

    /// `np.isin(values.filled(-1), codes)` as a boolean array.
    pub fn isin_filled(&self, codes: &[f64]) -> Vec<bool> {
        self.values
            .iter()
            .zip(&self.mask)
            .map(|(v, m)| codes.contains(if *m { &-1.0 } else { v }))
            .collect()
    }

    /// `values.filled(0.0).sum()`: numpy's blocked pairwise sum over the whole grid, masked
    /// cells contributing 0.0 in place.
    pub fn filled_sum(&self) -> f64 {
        let filled: Vec<f64> = self
            .values
            .iter()
            .zip(&self.mask)
            .map(|(v, m)| if *m { 0.0 } else { *v })
            .collect();
        crate::py::np_sum(&filled)
    }

    /// `np.ma.mean(values)` = `values.sum() * 1. / values.count()`, with `sum` = `filled(0).sum()`.
    /// None when every cell is masked (numpy returns the masked constant).
    pub fn ma_mean(&self) -> Option<f64> {
        let count = self.valid_count();
        if count == 0 {
            None
        } else {
            Some(self.filled_sum() * 1.0 / count as f64)
        }
    }

    /// `np.unique(values.compressed(), return_counts=True)`: ascending values (NaN last, -0.0
    /// merged with 0.0) with their counts.
    pub fn unique_counts(&self) -> Vec<(f64, usize)> {
        let mut counts: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (v, m) in self.values.iter().zip(&self.mask) {
            if !*m {
                let v = if *v == 0.0 { 0.0f64 } else { *v };
                *counts.entry(v.to_bits()).or_default() += 1;
            }
        }
        let mut out: Vec<(f64, usize)> = counts
            .into_iter()
            .map(|(b, n)| (f64::from_bits(b), n))
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }
}

/// rasterio's `dataset.nodata`: band 1's GDAL nodata, or None when unset or outside the band
/// type's range (`get_nodatavals` + `dtypes.in_dtype_range`). A u8 band declaring 256 has none.
pub fn rasterio_nodata(ds: &Dataset) -> Result<Option<f64>> {
    use gdal::raster::GdalDataType as T;
    let band = ds.rasterband(1).map_err(gdal_err)?;
    let Some(v) = band.no_data_value() else {
        return Ok(None);
    };
    let range = match band.band_type() {
        T::UInt8 => Some((0.0, 255.0)),
        T::UInt16 => Some((0.0, 65535.0)),
        T::Int16 => Some((-32768.0, 32767.0)),
        T::UInt32 => Some((0.0, 4294967295.0)),
        T::Int32 => Some((-2147483648.0, 2147483647.0)),
        T::Float32 if v.is_nan() || v.is_infinite() => None,
        T::Float32 => Some((f32::MIN as f64, f32::MAX as f64)),
        _ => None, // float64 always in range; other types are not range-checked by rasterio
    };
    Ok(match range {
        Some((lo, hi)) if !(lo <= v && v <= hi) => None,
        _ => Some(v),
    })
}

/// A destination grid: what `like=` copies from an earlier slice.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub transform: Affine,
    pub width: usize,
    pub height: usize,
}

impl RasterSlice {
    pub fn grid(&self) -> Grid {
        Grid {
            transform: self.transform,
            width: self.width,
            height: self.height,
        }
    }
}

pub fn open_layer(settings: &Settings, layer: &str) -> Result<Dataset> {
    let path = gdal_path(&settings.layer_path(layer)?);
    Dataset::open(&path).map_err(|e| Error::new("RasterioIOError", e.to_string()))
}

/// rasterio `calculate_default_transform(src.crs, REFERENCE_CRS, src.width, src.height,
/// *src.bounds)`: a proxy VRT carrying `from_bounds(...)` and the source WKT, an exact
/// GenImgProj transformer and GDALSuggestedWarpOutput2.
fn calculate_default_transform(src: &Dataset) -> Result<Affine> {
    let (w, h) = src.raster_size();
    let t = Affine::from_gdal(src.geo_transform().map_err(gdal_err)?);
    if t.b != 0.0 || t.d != 0.0 {
        // shortcut: no v3 layer is rotated; rasterio's rotated-bounds branch is not ported
        return Err(Error::new(
            "NotImplementedError",
            "rotated source rasters are not supported",
        ));
    }
    let (left, bottom, right, top) = (t.c, t.f + t.e * h as f64, t.c + t.a * w as f64, t.f);
    let proxy = from_bounds(left, bottom, right, top, w, h);
    let gt = proxy.to_gdal().map(|v| v.to_string()).join(",");
    let srs = xml_escape(&src.projection());
    let doc = format!(
        "<VRTDataset rasterYSize=\"{h}\" rasterXSize=\"{w}\"><VRTRasterBand /><SRS>{srs}</SRS>\
         <GeoTransform>{gt}</GeoTransform></VRTDataset>"
    );
    let vrt = Dataset::open_ex(
        &doc,
        gdal::DatasetOptions {
            open_flags: gdal::GdalOpenFlags::GDAL_OF_RASTER,
            allowed_drivers: Some(&["VRT"]),
            ..Default::default()
        },
    )
    .map_err(gdal_err)?;
    let (gt, _, _) = ffi::suggested_warp_output(&vrt, REFERENCE_CRS_WKT)?;
    Ok(Affine::from_gdal(gt))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `load_raster_clipped`
pub fn load_raster_clipped(
    settings: &Settings,
    layer: &str,
    aoi: &Aoi,
    resampling: Resampling,
    band: usize,
    like: Option<Grid>,
) -> Result<RasterSlice> {
    let mut slices = load_raster_bands_clipped(settings, layer, aoi, resampling, &[band], like)?;
    Ok(slices.remove(0))
}

/// `load_raster_bands_clipped`: several bands of one raster on one grid, one open of the file.
pub fn load_raster_bands_clipped(
    settings: &Settings,
    layer: &str,
    aoi: &Aoi,
    resampling: Resampling,
    bands: &[usize],
    like: Option<Grid>,
) -> Result<Vec<RasterSlice>> {
    let src = open_layer(settings, layer)?;
    let (dst_transform, dst_w, dst_h) = match like {
        // Force the exact grid of an earlier slice, so pixels line up one to one.
        Some(g) => (g.transform, g.width, g.height),
        None => {
            // Destination grid = source resolution carried into REFERENCE_CRS, cropped to the AOI
            // bounding box so only the AOI window is warped, not the whole SEA raster.
            let base = calculate_default_transform(&src)?;
            let (minx, miny, maxx, maxy) = aoi.bounds;
            let inv = base.inverse();
            let (c0, r0) = inv.apply(minx, maxy);
            let (c1, r1) = inv.apply(maxx, miny);
            let (col_off, row_off) = (c0.floor() as i64, r0.floor() as i64);
            let dst_w = 1.max(c1.ceil() as i64 - col_off) as usize;
            let dst_h = 1.max(r1.ceil() as i64 - row_off) as usize;
            (
                base.compose(&Affine::translation(col_off as f64, row_off as f64)),
                dst_w,
                dst_h,
            )
        }
    };

    // NaN fill separates "no source coverage" from a genuine 0 in a categorical raster.
    let src_nodata = rasterio_nodata(&src)?;
    let fill = src_nodata.unwrap_or(f64::NAN);
    let mut grids = Vec::with_capacity(bands.len());
    for &band in bands {
        let mut dst = vec![fill; dst_w * dst_h];
        let mem = MemDataset::over(
            &mut dst,
            dst_w,
            dst_h,
            dst_transform.to_gdal(),
            Some(REFERENCE_CRS_WKT),
        )?;
        ffi::reproject_band(
            &src,
            band as i32,
            &mem,
            resampling.gdal(),
            src_nodata,
            Some(fill),
        )?;
        drop(mem); // close (and flush) before reading the buffer
        grids.push(dst);
    }
    drop(src);

    // Mask everything outside the AOI polygon, then everything with no source value.
    let outside = geometry_mask(aoi, dst_w, dst_h, &dst_transform, false)?;
    let pixel_area_ha = (dst_transform.a * dst_transform.e).abs() / M2_PER_HA;
    Ok(grids
        .into_iter()
        .map(|values| {
            let mask = values
                .iter()
                .zip(&outside)
                .map(|(v, out)| {
                    *out || if src_nodata.is_none() {
                        v.is_nan()
                    } else {
                        Some(*v) == src_nodata
                    }
                })
                .collect();
            RasterSlice {
                values,
                mask,
                width: dst_w,
                height: dst_h,
                pixel_area_ha,
                transform: dst_transform,
            }
        })
        .collect())
}

/// rasterio `geometry_mask([aoi], out_shape, transform, invert=False, all_touched=...)`: true
/// where a pixel is OUTSIDE the polygon. GDALRasterizeGeometries on a MEM dataset with the
/// transform and no transformer, MERGE_ALG=REPLACE.
pub fn geometry_mask(
    aoi: &Aoi,
    width: usize,
    height: usize,
    transform: &Affine,
    all_touched: bool,
) -> Result<Vec<bool>> {
    let mut burned = vec![0u8; width * height];
    {
        let mut mem = MemDataset::over(&mut burned, width, height, transform.to_gdal(), None)?;
        let geom = build_geometry(&aoi.polygons, aoi.is_multi)?;
        let options = RasterizeOptions {
            all_touched,
            ..Default::default()
        };
        rasterize(&mut mem.ds, &[1], &[geom], &[1.0], Some(options)).map_err(gdal_err)?;
    }
    Ok(burned.into_iter().map(|b| b == 0).collect())
}

/// `aoi.geometry.centroid.to_crs(4326)`: the GEOS centroid in ESRI:54034, moved back to
/// EPSG:4326 (lon, lat) through PROJ.
pub fn centroid_4326(aoi: &Aoi) -> Result<(f64, f64)> {
    let (x, y) = ffi::centroid(&build_geometry(&aoi.polygons, aoi.is_multi)?)?;
    let mut src = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut dst = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let (mut xs, mut ys) = ([x], [y]);
    CoordTransform::new(&src, &dst)
        .map_err(gdal_err)?
        .transform_coords(&mut xs, &mut ys, &mut [])
        .map_err(gdal_err)?;
    Ok((xs[0], ys[0]))
}

impl RasterSlice {
    /// `values.min()` of the masked array (None when all masked)
    pub fn ma_min(&self) -> Option<f64> {
        self.values
            .iter()
            .zip(&self.mask)
            .filter(|(_, m)| !**m)
            .map(|(v, _)| *v)
            .reduce(f64::min)
    }

    /// `values.max()` of the masked array (None when all masked)
    pub fn ma_max(&self) -> Option<f64> {
        self.values
            .iter()
            .zip(&self.mask)
            .filter(|(_, m)| !**m)
            .map(|(v, _)| *v)
            .reduce(f64::max)
    }
}

/// `slope_percent_from_dem`: slope in percent from a clipped DEM, the run corrected for latitude
/// at the AOI centroid (east-west run x cos(lat), north-south run / cos(lat)), np.gradient with
/// second-order central differences inside and first-order one-sided differences at the edges.
pub fn slope_percent_from_dem(elev: &RasterSlice, aoi: &Aoi) -> Result<RasterSlice> {
    let (w, h) = (elev.width, elev.height);
    let z: Vec<f64> = elev
        .values
        .iter()
        .zip(&elev.mask)
        .map(|(v, m)| if *m { f64::NAN } else { *v })
        .collect();
    if w.min(h) < 2 {
        // Too few pixels for a gradient; report flat rather than error.
        return Ok(RasterSlice {
            values: vec![0.0; w * h],
            mask: elev.mask.clone(),
            ..elev.clone()
        });
    }
    let lat = centroid_4326(aoi)?.1.to_radians();
    let cos_lat = lat.cos().max(1e-6);
    let run_x = elev.transform.a.abs() * cos_lat;
    let run_y = elev.transform.e.abs() / cos_lat;

    let at = |r: usize, c: usize| z[r * w + c];
    let mut values = Vec::with_capacity(w * h);
    for r in 0..h {
        for c in 0..w {
            let gy = if r == 0 {
                (at(1, c) - at(0, c)) / run_y
            } else if r == h - 1 {
                (at(h - 1, c) - at(h - 2, c)) / run_y
            } else {
                (at(r + 1, c) - at(r - 1, c)) / (2.0 * run_y)
            };
            let gx = if c == 0 {
                (at(r, 1) - at(r, 0)) / run_x
            } else if c == w - 1 {
                (at(r, w - 1) - at(r, w - 2)) / run_x
            } else {
                (at(r, c + 1) - at(r, c - 1)) / (2.0 * run_x)
            };
            values.push((gx * gx + gy * gy).sqrt() * 100.0);
        }
    }
    let mask = values
        .iter()
        .zip(&elev.mask)
        .map(|(v, m)| v.is_nan() || *m)
        .collect();
    Ok(RasterSlice {
        values,
        mask,
        width: w,
        height: h,
        pixel_area_ha: elev.pixel_area_ha,
        transform: elev.transform,
    })
}

/// `classify_continuous`: `np.digitize(values.filled(nan), breaks, right=False) + 1`, mask kept.
/// A value's class is the number of breaks <= it, plus one; NaN lands in the top class (masked).
pub fn classify_continuous(raster: &RasterSlice, breaks: &[f64]) -> RasterSlice {
    let values = raster
        .values
        .iter()
        .zip(&raster.mask)
        .map(|(v, m)| {
            let v = if *m { f64::NAN } else { *v };
            let n = if v.is_nan() {
                breaks.len()
            } else {
                breaks.iter().filter(|b| **b <= v).count()
            };
            (n + 1) as f64
        })
        .collect();
    RasterSlice {
        values,
        mask: raster.mask.clone(),
        ..raster.clone()
    }
}

/// `pd.read_csv(layer_path(layer))` for the small reference CSVs beside the rasters: the header
/// row and the data rows as text fields. Read through GDAL's VSI (/vsicurl for the bucket).
pub fn read_layer_csv(settings: &Settings, layer: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let bytes = ffi::read_vsi_file(&gdal_path(&settings.layer_path(layer)?))?;
    let mut records = parse_csv(&String::from_utf8_lossy(&bytes)).into_iter();
    let header = records.next().unwrap_or_default();
    Ok((header, records.collect()))
}

/// pandas' default C parser on the record level: `,` separates, `"` quotes (a doubled `""` inside
/// quotes is one quote, quoted fields may hold commas and newlines), `\r\n` or `\n` ends a record,
/// blank lines are skipped.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut records, mut record, mut field) = (Vec::new(), Vec::new(), String::new());
    let (mut in_quotes, mut chars) = (false, text.chars().peekable());
    let end_record =
        |records: &mut Vec<Vec<String>>, record: &mut Vec<String>, field: &mut String| {
            if field.ends_with('\r') {
                field.pop();
            }
            record.push(std::mem::take(field));
            let r = std::mem::take(record);
            if !(r.len() == 1 && r[0].is_empty()) {
                records.push(r);
            }
        };
    while let Some(c) = chars.next() {
        match (in_quotes, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => in_quotes = false,
            (true, c) => field.push(c),
            (false, '"') if field.is_empty() => in_quotes = true,
            (false, ',') => record.push(std::mem::take(&mut field)),
            (false, '\n') => end_record(&mut records, &mut record, &mut field),
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        end_record(&mut records, &mut record, &mut field);
    }
    records
}

#[cfg(test)]
mod csv_tests {
    #[test]
    fn quoted_fields() {
        let r = super::parse_csv("a,b\r\n1,\"x, \"\"y\"\"\nz\"\n\n2,\n");
        assert_eq!(
            r,
            vec![vec!["a", "b"], vec!["1", "x, \"y\"\nz"], vec!["2", ""]]
        );
    }
}

/// `load_national_forest_risk_percentiles(country)`: {10: v, ..., 90: v} for the country, None
/// when the country is not in the reference.
pub fn load_national_forest_risk_percentiles(
    settings: &Settings,
    country: &str,
) -> Result<Option<Vec<(i64, f64)>>> {
    let (header, rows) = read_layer_csv(settings, crate::config::NATIONAL_FOREST_RISK_CSV)?;
    let cols: Vec<String> = header.iter().map(|c| c.trim().to_lowercase()).collect();
    let Some(ci) = cols.iter().position(|c| c == "country") else {
        return Err(Error::new("KeyError", "'country'"));
    };
    let wanted = country.trim().to_lowercase();
    let Some(row) = rows
        .iter()
        .find(|r| r.get(ci).is_some_and(|v| v.trim().to_lowercase() == wanted))
    else {
        return Ok(None);
    };
    let mut breakpoints = Vec::new();
    for pct in (10..100).step_by(10) {
        if let Some(i) = cols.iter().position(|c| *c == format!("p{pct}")) {
            let cell = row.get(i).map(|s| s.trim()).unwrap_or("");
            let v = if cell.is_empty() {
                f64::NAN
            } else {
                cell.parse::<f64>().map_err(|_| {
                    Error::new(
                        "ValueError",
                        format!("could not convert string to float: '{cell}'"),
                    )
                })?
            };
            breakpoints.push((pct, v));
        }
    }
    Ok(if breakpoints.is_empty() {
        None
    } else {
        Some(breakpoints)
    })
}

// ============================ NATIVE GRID (no warp) ============================
// The threat, burned-area, habitat and benefit-VRT modules read rasters on their own grid
// (`rasterio.mask` / `raster_geometry_mask` + a windowed read) and measure area geodesically per
// row, because those rasters stay in EPSG:4326. These are the rasterio 1.3.10 rules they use.

/// A rasterio `Window` (integer offsets and sizes after `geometry_window`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub col_off: i64,
    pub row_off: i64,
    pub width: i64,
    pub height: i64,
}

/// `gpd.GeoSeries([aoi_geometry], crs=aoi_crs).to_crs(src.crs)` when the CRSs differ
/// (rasterio `src.crs != aoi_crs`, an OSRIsSame test), else the geometry unchanged.
pub fn aoi_in_raster_crs(aoi: &Aoi, src: &Dataset) -> Result<Vec<Vec<Ring>>> {
    let Ok(mut dst) = src.spatial_ref() else {
        return Err(Error::new(
            "ValueError",
            format!(
                "Raster has no CRS: {}",
                src.description().unwrap_or_default()
            ),
        ));
    };
    let mut reference = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    if dst == reference {
        return Ok(aoi.polygons.clone());
    }
    reference.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let ct = CoordTransform::new(&reference, &dst).map_err(gdal_err)?;
    let mut polygons = aoi.polygons.clone();
    for ring in polygons.iter_mut().flatten() {
        let mut xs: Vec<f64> = ring.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f64> = ring.iter().map(|p| p[1]).collect();
        ct.transform_coords(&mut xs, &mut ys, &mut [])
            .map_err(gdal_err)?;
        for (p, (x, y)) in ring.iter_mut().zip(xs.into_iter().zip(ys)) {
            *p = [x, y];
        }
    }
    Ok(polygons)
}

/// `aoi.geometry.to_crs(4326).union_all()`: the AOI moved back to EPSG:4326 (PROJ, traditional GIS
/// order) and dissolved again (GEOS unary union), as (polygons, is_multi).
pub fn aoi_4326_union(aoi: &Aoi) -> Result<(Vec<Vec<Ring>>, bool)> {
    let mut src = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut dst = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let ct = CoordTransform::new(&src, &dst).map_err(gdal_err)?;
    let mut polygons = aoi.polygons.clone();
    for ring in polygons.iter_mut().flatten() {
        let mut xs: Vec<f64> = ring.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f64> = ring.iter().map(|p| p[1]).collect();
        ct.transform_coords(&mut xs, &mut ys, &mut [])
            .map_err(gdal_err)?;
        for (p, (x, y)) in ring.iter_mut().zip(xs.into_iter().zip(ys)) {
            *p = [x, y];
        }
    }
    let mut gc = Geometry::empty(OGRwkbGeometryType::wkbGeometryCollection).map_err(gdal_err)?;
    gc.add_geometry(build_geometry(&polygons, aoi.is_multi)?)
        .map_err(gdal_err)?;
    extract_polygons(&ffi::unary_union(&gc)?)
}

/// `rasterio.features.geometry_window(dataset, [geom])` (no padding, not boundless): every
/// vertex through `~dataset.transform`, floor/ceil of the extremes, intersected with the raster.
/// None when the window is empty (rasterio raises WindowError).
pub fn geometry_window(src: &Dataset, polygons: &[Vec<Ring>]) -> Result<Option<Window>> {
    let inv = Affine::from_gdal(src.geo_transform().map_err(gdal_err)?).inverse();
    let (mut minc, mut maxc, mut minr, mut maxr) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in polygons.iter().flatten().flatten() {
        let (c, r) = inv.apply(p[0], p[1]);
        minc = minc.min(c);
        maxc = maxc.max(c);
        minr = minr.min(r);
        maxr = maxr.max(r);
    }
    let (row_start, row_stop) = (minr.floor() as i64, maxr.ceil() as i64);
    let (col_start, col_stop) = (minc.floor() as i64, maxc.ceil() as i64);
    let w = Window {
        col_off: col_start,
        row_off: row_start,
        width: (col_stop - col_start).max(0),
        height: (row_stop - row_start).max(0),
    };
    // Window.intersection with Window(0, 0, width, height)
    let (sw, sh) = src.raster_size();
    let col_off = w.col_off.max(0);
    let row_off = w.row_off.max(0);
    let width = (w.col_off + w.width).min(sw as i64) - col_off;
    let height = (w.row_off + w.height).min(sh as i64) - row_off;
    Ok(if width > 0 && height > 0 {
        Some(Window {
            col_off,
            row_off,
            width,
            height,
        })
    } else {
        None
    })
}

/// `dataset.window_transform(window)`: `translation(x - T.c, y - T.f) * T`, with
/// `(x, y) = T * (col_off, row_off)`.
pub fn window_transform(t: &Affine, w: &Window) -> Affine {
    let (x, y) = t.apply(w.col_off as f64, w.row_off as f64);
    Affine::translation(x - t.c, y - t.f).compose(t)
}

/// `rasterio.mask.raster_geometry_mask(src, [geom], crop=True, all_touched=...)`: the
/// outside-mask on the cropped window, its transform and the window. Raises ValueError('Input
/// shapes do not overlap raster.') when they do not overlap.
pub fn raster_geometry_mask_crop(
    src: &Dataset,
    polygons: &[Vec<Ring>],
    is_multi: bool,
    all_touched: bool,
) -> Result<(Vec<bool>, Affine, Window)> {
    let Some(window) = geometry_window(src, polygons)? else {
        return Err(Error::new(
            "ValueError",
            "Input shapes do not overlap raster.",
        ));
    };
    let t = window_transform(
        &Affine::from_gdal(src.geo_transform().map_err(gdal_err)?),
        &window,
    );
    let shape = Aoi {
        polygons: polygons.to_vec(),
        is_multi,
        bounds: (0.0, 0.0, 0.0, 0.0),
        area_ha: 0.0,
        source_crs: String::new(),
    };
    let outside = geometry_mask(
        &shape,
        window.width as usize,
        window.height as usize,
        &t,
        all_touched,
    )?;
    Ok((outside, t, window))
}

/// `src.read(band, window=window, masked=True)`: the values (as f64, exact for every v3 dtype)
/// and numpy's mask, which rasterio takes from GDAL's mask band (true where the mask is 0).
pub fn read_window_masked(src: &Dataset, band: usize, w: &Window) -> Result<(Vec<f64>, Vec<bool>)> {
    let b = src.rasterband(band).map_err(gdal_err)?;
    let (off, size) = (
        (w.col_off as isize, w.row_off as isize),
        (w.width as usize, w.height as usize),
    );
    let data = b
        .read_as::<f64>(off, size, size, None)
        .map_err(gdal_err)?
        .into_shape_and_vec()
        .1;
    let mask_band = b.open_mask_band().map_err(gdal_err)?;
    let mask = mask_band
        .read_as::<u8>(off, size, size, None)
        .map_err(gdal_err)?
        .into_shape_and_vec()
        .1;
    Ok((data, mask.into_iter().map(|m| m == 0).collect()))
}

/// `pixel_area_by_row`: ha per pixel for each row, from the geodesic area (WGS84, Karney, as
/// pyproj `Geod.polygon_area_perimeter`) of the row's first pixel.
pub fn pixel_area_by_row(t: &Affine, height: usize) -> Vec<f64> {
    use geographiclib_rs::{Geodesic, PolygonArea, Winding};
    let geod = Geodesic::wgs84();
    let pixel_width = t.a.abs();
    (0..height)
        .map(|row| {
            let north = t.f + row as f64 * t.e;
            let south = north + t.e;
            let west = t.c;
            let east = west + pixel_width;
            let mut pa = PolygonArea::new(&geod, Winding::CounterClockwise);
            for (lat, lon) in [(north, west), (north, east), (south, east), (south, west)] {
                pa.add_point(lat, lon);
            }
            let (_perimeter, area_m2, _) = pa.compute(true);
            area_m2.abs() / 10000.0
        })
        .collect()
}

/// `rasterio.mask.mask(src, shapes, crop=True, filled=False)`, band 1 (`data[0]`; the other
/// bands rasterio also reads are discarded by every caller): the window's raw values and numpy's
/// mask = the read mask | outside the shapes.
#[derive(Clone, Debug)]
pub struct MaskedWindow {
    pub data: Vec<f64>,
    pub mask: Vec<bool>,
    pub transform: Affine,
    pub width: usize,
    pub height: usize,
}

impl MaskedWindow {
    pub fn shape(&self) -> (usize, usize) {
        (self.height, self.width)
    }

    /// `ma == v`: numpy.ma `_comparison`: masked cells compare as False.
    pub fn eq(&self, v: f64) -> MaBool {
        let data = self
            .data
            .iter()
            .zip(&self.mask)
            .map(|(d, m)| !m && *d == v)
            .collect();
        MaBool {
            data,
            mask: self.mask.clone(),
            shape: self.shape(),
        }
    }

    /// `ma > v`: raw comparison, mask kept.
    pub fn gt(&self, v: f64) -> MaBool {
        MaBool {
            data: self.data.iter().map(|d| *d > v).collect(),
            mask: self.mask.clone(),
            shape: self.shape(),
        }
    }

    /// `np.isin(ma, values)`: a plain array over the RAW data (isin drops the mask).
    pub fn isin_raw(&self, values: &[f64]) -> Vec<bool> {
        self.data.iter().map(|d| values.contains(d)).collect()
    }

    /// `mask(..., filled=True)` with `nodata`: masked cells replaced.
    pub fn filled(&self, nodata: f64) -> Vec<f64> {
        self.data
            .iter()
            .zip(&self.mask)
            .map(|(d, m)| if *m { nodata } else { *d })
            .collect()
    }
}

/// A boolean numpy masked array, kept with its raw data, because the v3 threat code indexes and
/// `|=`-accumulates with the raw data while `np.any` / `np.ma.filled` honour the mask.
#[derive(Clone, Debug)]
pub struct MaBool {
    pub data: Vec<bool>,
    pub mask: Vec<bool>,
    pub shape: (usize, usize),
}

/// numpy's broadcast error for two different 2-D shapes.
// shortcut: equal shapes only; a (1, w) against (h, w) pair would broadcast in numpy. Co-registered
// threat layers always clip to one window; port broadcasting if a layer ever differs.
fn broadcast_check(a: (usize, usize), b: (usize, usize)) -> Result<()> {
    if a == b {
        Ok(())
    } else {
        Err(Error::new(
            "ValueError",
            format!(
                "operands could not be broadcast together with shapes ({},{}) ({},{}) ",
                a.0, a.1, b.0, b.1
            ),
        ))
    }
}

impl MaBool {
    /// `a & b` of two masked arrays: raw data and-ed, masks or-ed.
    pub fn and(&self, o: &MaBool) -> Result<MaBool> {
        broadcast_check(self.shape, o.shape)?;
        Ok(MaBool {
            data: self
                .data
                .iter()
                .zip(&o.data)
                .map(|(a, b)| *a && *b)
                .collect(),
            mask: self
                .mask
                .iter()
                .zip(&o.mask)
                .map(|(a, b)| *a || *b)
                .collect(),
            shape: self.shape,
        })
    }

    /// `a & plain`: mask of `a` kept.
    pub fn and_plain(&self, o: &[bool], o_shape: (usize, usize)) -> Result<MaBool> {
        broadcast_check(self.shape, o_shape)?;
        Ok(MaBool {
            data: self.data.iter().zip(o).map(|(a, b)| *a && *b).collect(),
            mask: self.mask.clone(),
            shape: self.shape,
        })
    }

    /// `~a`: raw data inverted, mask kept.
    pub fn not(&self) -> MaBool {
        MaBool {
            data: self.data.iter().map(|a| !a).collect(),
            mask: self.mask.clone(),
            shape: self.shape,
        }
    }

    /// `np.ma.filled(a, False)`
    pub fn filled_false(&self) -> Vec<bool> {
        self.data
            .iter()
            .zip(&self.mask)
            .map(|(d, m)| *d && !m)
            .collect()
    }

    /// `np.any(a)`: masked cells count as False.
    pub fn any(&self) -> bool {
        self.data.iter().zip(&self.mask).any(|(d, m)| *d && !m)
    }
}

/// `np.unique(values[index])` over a masked array indexed by a boolean array's RAW data, masked
/// elements dropped (the callers skip `np.ma.is_masked` values): ascending distinct values.
pub fn unique_unmasked_at(values: &MaskedWindow, index: &[bool]) -> Vec<f64> {
    let mut out: Vec<f64> = values
        .data
        .iter()
        .zip(&values.mask)
        .zip(index)
        .filter(|((_, m), i)| **i && !**m)
        .map(|((d, _), _)| if *d == 0.0 { 0.0 } else { *d })
        .collect();
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// `mask(src, shapes, crop=True, filled=False)` band 1, on the source's own grid.
pub fn mask_crop(
    src: &Dataset,
    polygons: &[Vec<Ring>],
    is_multi: bool,
    all_touched: bool,
) -> Result<MaskedWindow> {
    let (outside, transform, window) =
        raster_geometry_mask_crop(src, polygons, is_multi, all_touched)?;
    let (data, read_mask) = read_window_masked(src, 1, &window)?;
    let mask = read_mask
        .iter()
        .zip(&outside)
        .map(|(a, b)| *a || *b)
        .collect();
    Ok(MaskedWindow {
        data,
        mask,
        transform,
        width: window.width as usize,
        height: window.height as usize,
    })
}

/// open + `aoi.to_crs(src.crs)` when they differ + `mask_crop`: the threat sections' `read`.
pub fn read_layer_masked(settings: &Settings, layer: &str, aoi: &Aoi) -> Result<MaskedWindow> {
    let src = open_layer(settings, layer)?;
    let polygons = aoi_in_raster_crs(aoi, &src)?;
    mask_crop(&src, &polygons, aoi.is_multi, false)
}

/// `np.allclose(t1, t2)` of two affine transforms (all nine coefficients).
pub fn transforms_allclose(a: &Affine, b: &Affine) -> bool {
    let (x, y) = (
        [a.a, a.b, a.c, a.d, a.e, a.f, 0.0, 0.0, 1.0],
        [b.a, b.b, b.c, b.d, b.e, b.f, 0.0, 0.0, 1.0],
    );
    x.iter()
        .zip(&y)
        .all(|(p, q)| (p - q).abs() <= 1e-8 + 1e-5 * q.abs())
}

/// The geodesic area (m², positive) of one pixel of row `row`: pyproj
/// `Geod(ellps="WGS84").polygon_area_perimeter([w, e, e, w], [n, n, s, s])`, abs.
pub fn row_pixel_area_m2(t: &Affine, row: usize) -> f64 {
    use geographiclib_rs::{Geodesic, PolygonArea, Winding};
    let geod = Geodesic::wgs84();
    let north = t.f + row as f64 * t.e;
    let south = north + t.e;
    let west = t.c;
    let east = west + t.a.abs();
    let mut pa = PolygonArea::new(&geod, Winding::CounterClockwise);
    for (lat, lon) in [(north, west), (north, east), (south, east), (south, west)] {
        pa.add_point(lat, lon);
    }
    let (_perimeter, area_m2, _) = pa.compute(true);
    area_m2.abs()
}

/// The threat sections' `area_ha(mask, transform)`: a Python loop over rows,
/// `total += count * abs(pixel_m2)` (plain float addition, empty rows skipped), then `/ 10000`.
pub fn area_ha_by_row_loop(mask: &[bool], width: usize, t: &Affine) -> f64 {
    let mut total = 0.0;
    for (row, cells) in mask.chunks(width.max(1)).enumerate() {
        let count = cells.iter().filter(|m| **m).count();
        if count == 0 {
            continue;
        }
        total += count as f64 * row_pixel_area_m2(t, row);
    }
    total / 10000.0
}

/// `calculate_mask_area_ha`: `np.sum(mask.sum(axis=1) * row_areas)`.
pub fn calculate_mask_area_ha(mask: &[bool], width: usize, row_areas: &[f64]) -> f64 {
    let per_row: Vec<f64> = mask
        .chunks(width.max(1))
        .zip(row_areas)
        .map(|(row, a)| row.iter().filter(|m| **m).count() as f64 * a)
        .collect();
    crate::py::np_sum(&per_row)
}

// ============================ ZONAL TABULATION ============================

#[derive(Clone, Debug, PartialEq)]
pub enum Code {
    Int(i64),
    Str(&'static str),
}

impl Code {
    pub fn to_json(&self) -> Value {
        match self {
            Code::Int(i) => json!(i),
            Code::Str(s) => json!(s),
        }
    }

    /// `str(code)`
    pub fn as_string(&self) -> String {
        match self {
            Code::Int(i) => i.to_string(),
            Code::Str(s) => s.to_string(),
        }
    }
}

/// One row of a class distribution.
#[derive(Clone, Debug)]
pub struct ClassShare {
    pub code: Code,
    pub label: String,
    pub area_ha: f64,
    pub pct: f64,
}

impl ClassShare {
    /// `to_jsonable(ClassShare)`
    pub fn to_json(&self) -> Value {
        json!({"code": self.code.to_json(), "label": self.label, "area_ha": self.area_ha, "pct": self.pct})
    }
}

/// `tabulate_classes`
pub fn tabulate_classes(
    raster: &RasterSlice,
    class_labels: &[(i64, &str)],
    denominator_ha: f64,
) -> Vec<ClassShare> {
    class_labels
        .iter()
        .map(|&(code, label)| {
            let count = raster.count_eq(code as f64);
            let area_ha = count as f64 * raster.pixel_area_ha;
            ClassShare {
                code: Code::Int(code),
                label: label.to_string(),
                area_ha,
                pct: safe_pct(area_ha, denominator_ha),
            }
        })
        .collect()
}

/// `dominant`: the first class with the largest area among those above zero (Python `max`
/// keeps the first maximal element).
pub fn dominant(rows: &[ClassShare]) -> Option<&ClassShare> {
    rows.iter()
        .filter(|r| r.area_ha > 0.0)
        .fold(None, |best: Option<&ClassShare>, r| match best {
            Some(b) if r.area_ha <= b.area_ha => Some(b),
            _ => Some(r),
        })
}

/// `sort_by_area`: descending, stable (Python's `sorted(reverse=True)` keeps ties in order).
pub fn sort_by_area(mut rows: Vec<ClassShare>) -> Vec<ClassShare> {
    rows.sort_by(|a, b| {
        b.area_ha
            .partial_cmp(&a.area_ha)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

// ============================ SHARED MASKS ============================

/// The AOI forest at a single date (`ForestMask`).
#[derive(Clone, Debug)]
pub struct ForestMask {
    pub mask: Vec<bool>,
    pub width: usize,
    pub height: usize,
    pub pixel_area_ha: f64,
    pub area_ha: f64,
    pub grid: Grid,
}

impl ForestMask {
    pub fn is_empty(&self) -> bool {
        self.area_ha <= 0.0
    }
}

fn forest_mask(
    settings: &Settings,
    aoi: &Aoi,
    layer: &str,
    codes: &[f64],
    like: Option<Grid>,
) -> Result<ForestMask> {
    let fc = load_raster_clipped(settings, layer, aoi, Resampling::Nearest, 1, like)?;
    let mask = fc.isin_filled(codes);
    let count = mask.iter().filter(|m| **m).count();
    Ok(ForestMask {
        area_ha: count as f64 * fc.pixel_area_ha,
        mask,
        width: fc.width,
        height: fc.height,
        pixel_area_ha: fc.pixel_area_ha,
        grid: fc.grid(),
    })
}

/// `forest_mask_2014`
pub fn forest_mask_2014(settings: &Settings, aoi: &Aoi) -> Result<ForestMask> {
    forest_mask(settings, aoi, FC2014_RASTER, &FC2014_FOREST_CODES, None)
}

/// `forest_mask_2024`
pub fn forest_mask_2024(settings: &Settings, aoi: &Aoi, like: Option<Grid>) -> Result<ForestMask> {
    forest_mask(settings, aoi, FC2024_RASTER, &FC2024_FOREST_CODES, like)
}

// ============================ RESULT BASE ============================

/// `not_applicable(component, reason)` reduced to what callers read: (narrative, flags, missing).
pub fn not_applicable(reason: &str) -> (String, Vec<String>, Vec<String>) {
    (reason.to_string(), vec![], vec![reason.to_string()])
}
