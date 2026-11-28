//! Helpers the benefit components share and the shared modules do not have yet (candidates for
//! merging into common.rs / py.rs, listed in README NOTES): numpy/pandas semantics that reach the
//! output, the native-grid reads of 5.8-5.13 (rasterio `mask` / `geometry_window` + WarpedVRT),
//! and the species thread pool.

use gdal::Dataset;
use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use serde_json::{Value, json};

use crate::affine::Affine;
use crate::common::{
    Aoi, Resampling, Ring, Window, aoi_in_raster_crs, geometry_mask, geometry_window, open_layer,
    pixel_area_by_row, raster_geometry_mask_crop, rasterio_nodata, read_window_masked,
    window_transform,
};
use crate::config::REFERENCE_CRS_WKT;
use crate::ffi::{self, gdal_err};
use crate::settings::{Settings, gdal_path};
use crate::{Error, Result};

pub use crate::py::{nargsort_desc, np_argsort, truthy};

/// The `_na(reason)` pair every benefit component returns when there is nothing to measure.
pub fn na(reason: &str) -> (Value, Value) {
    (
        json!({"narrative": reason, "tables": {}, "values": {}, "flags": [], "missing": [reason]}),
        json!({"applicable": false, "narrative": reason}),
    )
}

/// `round(np.float64(x), n)`: numpy's round, `rint(x * 10**n) / 10**n` (not Python's exact
/// decimal rounding).
pub fn np_round(x: f64, n: i32) -> f64 {
    let f = 10f64.powi(n);
    (x * f).round_ties_even() / f
}

/// Python `a // b` for floats (CPython `float_floor_div`, via fmod).
pub fn py_floordiv(a: f64, b: f64) -> f64 {
    let m = a % b; // fmod
    let mut div = (a - m) / b;
    if m != 0.0 && ((b < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        let mut fl = div.floor();
        if div - fl > 0.5 {
            fl += 1.0;
        }
        fl
    } else {
        (0.0f64).copysign(a / b)
    }
}

/// `ndarray.sum()` of a contiguous float32 array: numpy's float32 pairwise sum in float32, in
/// blocks of 8192 (the float64 rule of `py::np_sum`, in single precision).
pub fn np_sum_f32(a: &[f32]) -> f32 {
    fn pairwise(a: &[f32]) -> f32 {
        let n = a.len();
        if n < 8 {
            let mut res = -0.0f32;
            for &x in a {
                res += x;
            }
            res
        } else if n <= 128 {
            let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
            let mut i = 8;
            while i < n - (n % 8) {
                for k in 0..8 {
                    r[k] += a[i + k];
                }
                i += 8;
            }
            let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
            while i < n {
                res += a[i];
                i += 1;
            }
            res
        } else {
            let mut n2 = n / 2;
            n2 -= n2 % 8;
            pairwise(&a[..n2]) + pairwise(&a[n2..])
        }
    }
    let mut total = 0.0f32;
    for block in a.chunks(8192) {
        total += pairwise(block);
    }
    total
}

/// Python `str(x)` of a JSON scalar (as json.loads would have built it).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => i.to_string(),
            (_, Some(u), _) => u.to_string(),
            (_, _, Some(f)) => crate::benefit::util::py_float_repr(f),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// Python `repr(float)`: `pipeline::py_float` (json.dumps' repr, ties to even) with repr's
/// `nan` / `inf` spellings.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let mut s = String::new();
    crate::pipeline::py_float(f, &mut s);
    s
}

// ============================ native grids ============================

/// The AOI on a raster's own grid: `aoi.geometry.to_crs(src.crs)`, the window
/// (`geometry_window` / `mask(crop=True)`), its transform, `inside_aoi`
/// (`geometry_mask(..., invert=True)`) and the geodesic ha per row.
pub struct NativeGrid {
    /// the source's CRS WKT, geotransform and size (WarpedVRT targets)
    pub wkt: String,
    pub full_gt: [f64; 6],
    pub full_size: (usize, usize),
    pub window: Window,
    pub transform: Affine,
    pub width: usize,
    pub height: usize,
    pub inside: Vec<bool>,
    pub row_ha: Vec<f64>,
}

impl NativeGrid {
    /// `geom = aoi.geometry.to_crs(src.crs).union_all(); window = geometry_window(src, [geom])`
    /// (5.9, 5.10, 5.12). An empty window raises rasterio's WindowError.
    pub fn window(settings: &Settings, layer: &str, aoi: &Aoi) -> Result<(Self, Dataset)> {
        let src = open_layer(settings, layer)?;
        let polygons = aoi_in_raster_crs(aoi, &src)?;
        let Some(window) = geometry_window(&src, &polygons)? else {
            return Err(Error::new("WindowError", "Intersection is empty"));
        };
        let t = window_transform(
            &Affine::from_gdal(src.geo_transform().map_err(gdal_err)?),
            &window,
        );
        let (w, h) = (window.width as usize, window.height as usize);
        let shape = Aoi {
            polygons,
            is_multi: aoi.is_multi,
            bounds: (0.0, 0.0, 0.0, 0.0),
            area_ha: 0.0,
            source_crs: String::new(),
        };
        let inside = geometry_mask(&shape, w, h, &t, false)?
            .into_iter()
            .map(|o| !o)
            .collect();
        let grid = Self::new(&src, window, t, inside)?;
        Ok((grid, src))
    }

    fn new(src: &Dataset, window: Window, t: Affine, inside: Vec<bool>) -> Result<Self> {
        let (w, h) = (window.width as usize, window.height as usize);
        Ok(Self {
            wkt: src.projection(),
            full_gt: src.geo_transform().map_err(gdal_err)?,
            full_size: src.raster_size(),
            row_ha: pixel_area_by_row(&t, h),
            window,
            transform: t,
            width: w,
            height: h,
            inside,
        })
    }

    /// `rasterio.mask.mask(src, aoi.to_crs(src.crs).geometry, crop=True, filled=False,
    /// indexes=band)` (5.8, 5.11, 5.13): the grid plus band `band`'s values and numpy mask.
    pub fn masked(
        settings: &Settings,
        layer: &str,
        aoi: &Aoi,
        band: usize,
    ) -> Result<(Self, Vec<f64>, Vec<bool>)> {
        let src = open_layer(settings, layer)?;
        let polygons = aoi_in_raster_crs(aoi, &src)?;
        let (outside, t, window) = raster_geometry_mask_crop(&src, &polygons, aoi.is_multi, false)?;
        let (data, read_mask) = read_window_masked(&src, band, &window)?;
        let mask = read_mask
            .iter()
            .zip(&outside)
            .map(|(a, b)| *a || *b)
            .collect();
        let inside = outside.into_iter().map(|o| !o).collect();
        Ok((Self::new(&src, window, t, inside)?, data, mask))
    }

    /// `src.read(band, window=window, masked=True).filled(0)`
    pub fn read_filled0(&self, src: &Dataset, band: usize) -> Result<Vec<f64>> {
        let (data, mask) = read_window_masked(src, band, &self.window)?;
        Ok(filled0(data, &mask))
    }

    /// `pixel_ha[mask].sum()`
    pub fn area_where(&self, mask: &[bool]) -> f64 {
        let picked: Vec<f64> = mask
            .chunks(self.width.max(1))
            .zip(&self.row_ha)
            .flat_map(|(row, a)| row.iter().filter(|m| **m).map(move |_| *a))
            .collect();
        crate::py::np_sum(&picked)
    }

    /// `(pixel_ha * weight * mask).sum()` over the whole grid.
    pub fn weighted_sum(&self, weight: impl Fn(usize) -> f64, mask: &[bool]) -> f64 {
        let w = self.width.max(1);
        let all: Vec<f64> = (0..mask.len())
            .map(|i| self.row_ha[i / w] * weight(i) * if mask[i] { 1.0 } else { 0.0 })
            .collect();
        crate::py::np_sum(&all)
    }

    /// `(crs wkt, full transform, full width, full height)` of the source: what WarpedVRT
    /// targets in the windowed reads (5.9, 5.10, 5.12).
    pub fn full_grid(&self) -> VrtGrid {
        VrtGrid {
            wkt: self.wkt.clone(),
            gt: self.full_gt,
            width: self.full_size.0,
            height: self.full_size.1,
        }
    }

    /// The cropped grid itself (5.11, 5.13 `read_like(path, transform, crs, shape)`).
    pub fn crop_grid(&self) -> VrtGrid {
        VrtGrid {
            wkt: self.wkt.clone(),
            gt: self.transform.to_gdal(),
            width: self.width,
            height: self.height,
        }
    }
}

pub fn filled0(data: Vec<f64>, mask: &[bool]) -> Vec<f64> {
    data.into_iter()
        .zip(mask)
        .map(|(d, m)| if *m { 0.0 } else { d })
        .collect()
}

/// A WarpedVRT destination grid.
#[derive(Clone, Debug)]
pub struct VrtGrid {
    pub wkt: String,
    pub gt: [f64; 6],
    pub width: usize,
    pub height: usize,
}

/// `with rasterio.open(path) as src, WarpedVRT(src, crs, transform, width, height,
/// resampling) as vrt: vrt.read(band, window=window, masked=True).filled(0)`; `window` None reads
/// the whole VRT. Returned as f64 (exact for every v3 dtype) with the band's GDAL type.
pub fn read_vrt_filled0(
    path: &str,
    grid: &VrtGrid,
    band: usize,
    window: Option<&Window>,
    resampling: Resampling,
) -> Result<(Vec<f64>, gdal::raster::GdalDataType)> {
    let src =
        Dataset::open(gdal_path(path)).map_err(|e| Error::new("RasterioIOError", e.to_string()))?;
    let nodata = rasterio_nodata(&src)?;
    let alg = match resampling {
        Resampling::Nearest => gdal_sys::GDALResampleAlg::GRA_NearestNeighbour,
        Resampling::Bilinear => gdal_sys::GDALResampleAlg::GRA_Bilinear,
        Resampling::Average => gdal_sys::GDALResampleAlg::GRA_Average,
        Resampling::Sum => gdal_sys::GDALResampleAlg::GRA_Sum,
    };
    let vrt = ffi::warped_vrt(
        src,
        nodata,
        &grid.wkt,
        grid.gt,
        grid.width,
        grid.height,
        alg,
    )?;
    let whole = Window {
        col_off: 0,
        row_off: 0,
        width: grid.width as i64,
        height: grid.height as i64,
    };
    let w = window.unwrap_or(&whole);
    let dtype = vrt.vrt.rasterband(band).map_err(gdal_err)?.band_type();
    let (data, mask) = read_window_masked(&vrt.vrt, band, w)?;
    Ok((filled0(data, &mask), dtype))
}

/// `ThreadPoolExecutor(max_workers).map(f, items)` collected in input order; the first error in
/// input order is raised, as iterating `map` does. Each worker enters AOH_GDAL_OPTIONS (GDAL
/// configuration is thread local, rasterio.Env per worker).
pub fn pool_map<T: Sync, R: Send>(
    items: &[T],
    workers: usize,
    f: impl Fn(&T) -> Result<R> + Sync,
) -> Result<Vec<R>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<Result<R>>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..workers.max(1).min(items.len().max(1)) {
            s.spawn(|| {
                for (k, v) in crate::benefit::config::AOH_GDAL_OPTIONS {
                    let _ = gdal::config::set_thread_local_config_option(k, v);
                }
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() {
                        break;
                    }
                    let r = f(&items[i]);
                    if let Ok(mut g) = slots.lock() {
                        g[i] = Some(r);
                    }
                }
            });
        }
    });
    let slots = slots
        .into_inner()
        .map_err(|_| Error::new("RuntimeError", "worker panicked"))?;
    slots
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err(Error::new("RuntimeError", "worker panicked"))))
        .collect()
}

/// `aoi.geometry.to_crs(EPSG:4326)` (pyproj always_xy) as polygons.
pub fn aoi_polygons_4326(aoi: &Aoi) -> Result<Vec<Vec<Ring>>> {
    let mut src = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gdal_err)?;
    let mut dst = SpatialRef::from_epsg(4326).map_err(gdal_err)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
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
    Ok(polygons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floordiv_and_round_like_python_numpy() {
        assert_eq!(py_floordiv(7.0, 2.0), 3.0);
        assert_eq!(py_floordiv(-7.0, 2.0), -4.0);
        assert_eq!(py_floordiv(0.3, 0.1), 2.0); // Python: 0.3 // 0.1 == 2.0
        assert_eq!(np_round(2.675, 2), 2.68); // numpy (Python's round gives 2.67)
        assert_eq!(py_float_repr(5.0), "5.0");
        assert_eq!(py_float_repr(1e16), "1e+16");
    }

    #[test]
    fn argsort_small_is_insertion_and_stable() {
        let v = [3.0, 1.0, 2.0, 1.0, 3.0];
        assert_eq!(np_argsort(&v), vec![1, 3, 2, 0, 4]);
        // numpy 1.26 (MSVC): np.argsort(np.array([i % 5 for i in range(17)], float))
        let v: Vec<f64> = (0..40).map(|i| (i % 5) as f64).collect();
        let o = np_argsort(&v);
        assert!(o.windows(2).all(|w| v[w[0]] <= v[w[1]]));
    }
}
