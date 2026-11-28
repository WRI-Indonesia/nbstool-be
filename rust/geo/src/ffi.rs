//! Every `unsafe` call of the crate, behind safe functions.
//!
//! The safe `gdal` crate has no suggested-warp-output, no warp operation with options and no
//! unary union, and it cannot open a MEM dataset over a caller's buffer. rasterio makes exactly
//! these calls (rasterio 1.3.10 `_warp.pyx`, `_io.pyx` `MemoryDataset`), so they are made here the
//! same way. Each GDAL handle is owned by a guard whose `Drop` releases it, so an early `?` return
//! cannot leak one.

use std::ffi::{CStr, CString, c_void};
use std::marker::PhantomData;
use std::ptr;

use gdal::Dataset;
use gdal::cpl::CslStringList;
use gdal::vector::Geometry;
use gdal_sys::{CPLErr, GDALResampleAlg, OGRGeometryH};

use crate::Error;

/// The message GDAL left for the failed call, as rasterio's `exc_wrap_*` raises it.
fn last_error(context: &str) -> Error {
    // SAFETY: CPLGetLastErrorMsg returns a pointer to a thread-local, NUL-terminated buffer owned
    // by GDAL (never null); it is copied out before any other GDAL call can change it.
    let msg = unsafe { CStr::from_ptr(gdal_sys::CPLGetLastErrorMsg()) }
        .to_string_lossy()
        .into_owned();
    Error::new(
        "CPLE_AppDefinedError",
        if msg.is_empty() {
            context.to_string()
        } else {
            msg
        },
    )
}

fn cstring(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::new("ValueError", "embedded NUL in a GDAL string"))
}

/// `GDALCreateGenImgProjTransformer2` result, destroyed on drop.
struct GenImgTransformer(*mut c_void);

impl GenImgTransformer {
    fn new(src: &Dataset, dst: Option<&Dataset>, options: &CslStringList) -> Result<Self, Error> {
        let dst_handle = dst.map_or(ptr::null_mut(), |d| d.c_dataset());
        // SAFETY: both dataset handles are live for this call (borrowed `Dataset`s, or null for
        // "no destination", which the function accepts); the option list is a valid CSL owned by
        // `options`. GDAL copies what it keeps.
        let arg = unsafe {
            gdal_sys::GDALCreateGenImgProjTransformer2(
                src.c_dataset(),
                dst_handle,
                options.as_ptr(),
            )
        };
        if arg.is_null() {
            Err(last_error("GDALCreateGenImgProjTransformer2 failed"))
        } else {
            Ok(Self(arg))
        }
    }
}

impl Drop for GenImgTransformer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: non-null handle created by GDALCreateGenImgProjTransformer2 and not yet
            // destroyed (ownership is cleared to null when handed to an approximate transformer).
            unsafe { gdal_sys::GDALDestroyGenImgProjTransformer(self.0) }
        }
    }
}

/// `GDALCreateApproxTransformer` result that owns its sub-transformer, destroyed on drop.
struct ApproxTransformer(*mut c_void);

impl ApproxTransformer {
    fn new(mut exact: GenImgTransformer, max_error: f64) -> Result<Self, Error> {
        // SAFETY: `exact.0` is a live GenImgProj transformer and GDALGenImgProjTransform is its
        // matching function.
        let arg = unsafe {
            gdal_sys::GDALCreateApproxTransformer(
                Some(gdal_sys::GDALGenImgProjTransform),
                exact.0,
                max_error,
            )
        };
        if arg.is_null() {
            return Err(last_error("GDALCreateApproxTransformer failed")); // `exact` drops itself
        }
        // SAFETY: `arg` is the live approximate transformer just created; after this call it owns
        // `exact.0` and destroys it with itself, so `exact` must not destroy it again.
        unsafe { gdal_sys::GDALApproxTransformerOwnsSubtransformer(arg, 1) };
        exact.0 = ptr::null_mut();
        Ok(Self(arg))
    }
}

impl Drop for ApproxTransformer {
    fn drop(&mut self) {
        // SAFETY: live handle from GDALCreateApproxTransformer, destroyed exactly once here.
        unsafe { gdal_sys::GDALDestroyApproxTransformer(self.0) }
    }
}

/// `GDALWarpOptions`, destroyed (with every array it owns) on drop.
struct WarpOptions(*mut gdal_sys::GDALWarpOptions);

impl Drop for WarpOptions {
    fn drop(&mut self) {
        // SAFETY: live struct from GDALCreateWarpOptions; every pointer stored in it was allocated
        // with CPLMalloc / CSL functions, which is what GDALDestroyWarpOptions frees.
        unsafe { gdal_sys::GDALDestroyWarpOptions(self.0) }
    }
}

/// `GDALWarpOperationH`, destroyed on drop.
struct WarpOperation(gdal_sys::GDALWarpOperationH);

impl Drop for WarpOperation {
    fn drop(&mut self) {
        // SAFETY: live handle from GDALCreateWarpOperation, destroyed exactly once.
        unsafe { gdal_sys::GDALDestroyWarpOperation(self.0) }
    }
}

/// One f64 / i32 array of `n` values allocated with CPLMalloc, as GDALDestroyWarpOptions frees.
fn cpl_array<T: Copy>(values: &[T]) -> *mut T {
    // SAFETY: CPLMalloc aborts on failure rather than returning null; the block is sized for
    // `values.len()` elements of T and fully written before it is returned.
    unsafe {
        let p = gdal_sys::CPLMalloc(std::mem::size_of_val(values)) as *mut T;
        ptr::copy_nonoverlapping(values.as_ptr(), p, values.len());
        p
    }
}

/// rasterio `_calculate_default_transform`: an exact GenImgProj transformer from `src` (the proxy
/// VRT) to `DST_SRS`, with `GCPS_OK=TRUE`, `MAX_GCP_ORDER=0`, then `GDALSuggestedWarpOutput2`
/// with no options. Returns the GDAL geotransform, pixels and lines.
pub fn suggested_warp_output(src: &Dataset, dst_wkt: &str) -> Result<([f64; 6], i32, i32), Error> {
    let mut options = CslStringList::new();
    options
        .set_name_value("GCPS_OK", "TRUE")
        .map_err(gdal_err)?;
    options
        .set_name_value("MAX_GCP_ORDER", "0")
        .map_err(gdal_err)?;
    options
        .set_name_value("DST_SRS", dst_wkt)
        .map_err(gdal_err)?;
    let transformer = GenImgTransformer::new(src, None, &options)?;
    let mut gt = [0.0f64; 6];
    let mut extent = [0.0f64; 4];
    let (mut pixels, mut lines) = (0i32, 0i32);
    // SAFETY: live dataset and transformer; every out-pointer refers to a local of the size GDAL
    // writes (6 doubles, 4 doubles, two ints).
    let err = unsafe {
        gdal_sys::GDALSuggestedWarpOutput2(
            src.c_dataset(),
            Some(gdal_sys::GDALGenImgProjTransform),
            transformer.0,
            gt.as_mut_ptr(),
            &mut pixels,
            &mut lines,
            extent.as_mut_ptr(),
            0,
        )
    };
    if err != CPLErr::CE_None {
        return Err(last_error("GDALSuggestedWarpOutput2 failed"));
    }
    Ok((gt, pixels, lines))
}

/// rasterio `_reproject` for one band of an opened dataset into a destination dataset: exact
/// GenImgProj transformer (`GCPS_OK=TRUE`) wrapped in the approximate transformer at 0.125 px,
/// warp options `UNIFIED_SRC_NODATA=YES`, `NUM_THREADS=1`, `INIT_DEST=NO_DATA`, working type
/// unknown, default memory limit, then `ChunkAndWarpImage` over the whole destination.
pub fn reproject_band(
    src: &Dataset,
    src_band: i32,
    dst: &MemDataset<'_>,
    resampling: GDALResampleAlg::Type,
    src_nodata: Option<f64>,
    dst_nodata: Option<f64>,
) -> Result<(), Error> {
    let mut options = CslStringList::new();
    options
        .set_name_value("GCPS_OK", "TRUE")
        .map_err(gdal_err)?;
    let exact = GenImgTransformer::new(src, Some(&dst.ds), &options)?;
    let approx = ApproxTransformer::new(exact, 0.125)?;

    // SAFETY: GDALCreateWarpOptions returns a fresh, zero-initialised struct (never null; it
    // aborts on allocation failure) that `WarpOptions` now owns.
    let wo = WarpOptions(unsafe { gdal_sys::GDALCreateWarpOptions() });
    let names = [
        ("UNIFIED_SRC_NODATA", "YES"),
        ("NUM_THREADS", "1"),
        ("INIT_DEST", "NO_DATA"),
    ];
    let cnames: Vec<(CString, CString)> = names
        .iter()
        .map(|(k, v)| Ok((cstring(k)?, cstring(v)?)))
        .collect::<Result<_, Error>>()?;
    // SAFETY: `wo.0` is a live struct owned by `wo`. Each CSLSetNameValue call takes ownership of
    // the old list and returns the new one, which is stored back. The arrays assigned below are
    // CPLMalloc'ed, so GDALDestroyWarpOptions frees them correctly; the dataset handles outlive
    // the warp, and the transformer stays owned by `approx`, which outlives `op`.
    unsafe {
        let w = &mut *wo.0;
        for (k, v) in &cnames {
            w.papszWarpOptions =
                gdal_sys::CSLSetNameValue(w.papszWarpOptions, k.as_ptr(), v.as_ptr());
        }
        w.eWorkingDataType = gdal_sys::GDALDataType::GDT_Unknown;
        w.eResampleAlg = resampling;
        if let Some(nd) = src_nodata {
            w.padfSrcNoDataReal = cpl_array(&[nd]);
            w.padfSrcNoDataImag = cpl_array(&[0.0f64]);
        }
        if let Some(nd) = dst_nodata {
            w.padfDstNoDataReal = cpl_array(&[nd]);
            w.padfDstNoDataImag = cpl_array(&[0.0f64]);
        }
        w.nBandCount = 1;
        w.panSrcBands = cpl_array(&[src_band]);
        w.panDstBands = cpl_array(&[1i32]);
        w.pfnTransformer = Some(gdal_sys::GDALApproxTransform);
        w.pTransformerArg = approx.0;
        w.hSrcDS = src.c_dataset();
        w.hDstDS = dst.ds.c_dataset();
    }
    // SAFETY: the options struct is complete and valid; GDAL clones it into the operation.
    let handle = unsafe { gdal_sys::GDALCreateWarpOperation(wo.0) };
    if handle.is_null() {
        return Err(last_error("GDALWarpOperation::Initialize failed"));
    }
    let op = WarpOperation(handle);
    let (w, h) = (dst.width as i32, dst.height as i32);
    // SAFETY: live warp operation whose source, destination and transformer are all still alive
    // (borrowed or owned by guards declared before `op`, so dropped after it).
    let err = unsafe { gdal_sys::GDALChunkAndWarpImage(op.0, 0, 0, w, h) };
    drop(op);
    drop(wo);
    drop(approx);
    if err != CPLErr::CE_None {
        return Err(Error::new("WarpOperationError", "Chunk and warp failed"));
    }
    Ok(())
}

/// A one-band MEM dataset over a caller's buffer (`MEM:::DATAPOINTER=...`), as rasterio's
/// `MemoryDataset` builds over a numpy array. Writes by GDAL land directly in the buffer, so
/// there is no second copy of the raster. The borrow keeps the buffer alive and unaliased for
/// as long as the dataset exists.
pub struct MemDataset<'a> {
    pub ds: Dataset,
    width: usize,
    height: usize,
    _buf: PhantomData<&'a mut [u8]>,
}

/// Element types a MEM dataset can be opened over.
pub trait MemPixel: Copy {
    const GDAL_NAME: &'static str;
}
impl MemPixel for f64 {
    const GDAL_NAME: &'static str = "Float64";
}
impl MemPixel for f32 {
    const GDAL_NAME: &'static str = "Float32";
}
impl MemPixel for u8 {
    const GDAL_NAME: &'static str = "Byte";
}

impl<'a> MemDataset<'a> {
    pub fn over<T: MemPixel>(
        buf: &'a mut [T],
        width: usize,
        height: usize,
        geo_transform: [f64; 6],
        wkt: Option<&str>,
    ) -> Result<Self, Error> {
        if buf.len() != width * height || width == 0 || height == 0 {
            return Err(Error::new(
                "ValueError",
                "MEM buffer does not match its shape",
            ));
        }
        let name = format!(
            "MEM:::DATAPOINTER={:p},PIXELS={width},LINES={height},BANDS=1,DATATYPE={}",
            buf.as_mut_ptr(),
            T::GDAL_NAME
        );
        // The pointer in the name is only dereferenced by GDAL while `ds` lives, and `ds` cannot
        // outlive `buf` (lifetime 'a); the exclusive borrow means nothing else reads or writes the
        // buffer meanwhile.
        let mut ds = Dataset::open_ex(
            &name,
            gdal::DatasetOptions {
                open_flags: gdal::GdalOpenFlags::GDAL_OF_RASTER
                    | gdal::GdalOpenFlags::GDAL_OF_UPDATE,
                ..Default::default()
            },
        )
        .map_err(gdal_err)?;
        ds.set_geo_transform(&geo_transform).map_err(gdal_err)?;
        if let Some(wkt) = wkt {
            ds.set_projection(wkt).map_err(gdal_err)?;
        }
        Ok(Self {
            ds,
            width,
            height,
            _buf: PhantomData,
        })
    }
}

/// shapely `union_all` of one geometry: GEOSUnaryUnion of a GeometryCollection holding it,
/// through OGR's GEOS bridge. Returned as an owned OGR geometry (via WKB, exact doubles).
pub fn unary_union(collection: &Geometry) -> Result<Geometry, Error> {
    struct Owned(OGRGeometryH);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: non-null geometry returned by OGR_G_UnaryUnion, owned here, freed once.
            unsafe { gdal_sys::OGR_G_DestroyGeometry(self.0) }
        }
    }
    // SAFETY: `collection` is a live OGR geometry; the result is a new geometry we own (or null).
    let h = unsafe { gdal_sys::OGR_G_UnaryUnion(collection.c_geometry()) };
    if h.is_null() {
        return Err(last_error("OGR_G_UnaryUnion failed"));
    }
    let owned = Owned(h);
    // SAFETY: live geometry; the buffer is sized by OGR_G_WkbSizeEx before export writes into it.
    let wkb = unsafe {
        let n = gdal_sys::OGR_G_WkbSizeEx(owned.0);
        let mut wkb = vec![0u8; n];
        let err = gdal_sys::OGR_G_ExportToWkb(
            owned.0,
            gdal_sys::OGRwkbByteOrder::wkbNDR,
            wkb.as_mut_ptr(),
        );
        if err != gdal_sys::OGRErr::OGRERR_NONE {
            return Err(last_error("OGR_G_ExportToWkb failed"));
        }
        wkb
    };
    drop(owned);
    Geometry::from_wkb(&wkb).map_err(gdal_err)
}

/// shapely `.centroid` (GEOSGetCentroid) through OGR_G_Centroid, which delegates to GEOS.
pub fn centroid(geom: &Geometry) -> Result<(f64, f64), Error> {
    // SAFETY: OGR_G_CreateGeometry returns a new empty point owned by `point` (freed on drop);
    // OGR_G_Centroid writes into it from the live input geometry and reads nothing else.
    unsafe {
        struct Point(OGRGeometryH);
        impl Drop for Point {
            fn drop(&mut self) {
                // SAFETY: point created below, owned here, destroyed once.
                unsafe { gdal_sys::OGR_G_DestroyGeometry(self.0) }
            }
        }
        let point = Point(gdal_sys::OGR_G_CreateGeometry(
            gdal_sys::OGRwkbGeometryType::wkbPoint,
        ));
        if point.0.is_null() || gdal_sys::OGR_G_Centroid(geom.c_geometry(), point.0) != 0 {
            return Err(last_error("OGR_G_Centroid failed"));
        }
        Ok((
            gdal_sys::OGR_G_GetX(point.0, 0),
            gdal_sys::OGR_G_GetY(point.0, 0),
        ))
    }
}

/// The whole content of a file GDAL can open (a /vsicurl URL or a local path), through VSI.
pub fn read_vsi_file(path: &str) -> Result<Vec<u8>, Error> {
    struct File(*mut gdal_sys::VSILFILE);
    impl Drop for File {
        fn drop(&mut self) {
            // SAFETY: non-null handle from VSIFOpenL, closed exactly once.
            unsafe { gdal_sys::VSIFCloseL(self.0) };
        }
    }
    let cpath = cstring(path)?;
    let mode = cstring("rb")?;
    // SAFETY: both strings are NUL-terminated and outlive the call; a null result is handled.
    let handle = unsafe { gdal_sys::VSIFOpenL(cpath.as_ptr(), mode.as_ptr()) };
    if handle.is_null() {
        return Err(Error::new(
            "FileNotFoundError",
            format!("could not open {path}"),
        ));
    }
    let file = File(handle);
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        // SAFETY: `chunk` has room for chunk.len() bytes; the handle is live (owned by `file`).
        let n = unsafe {
            gdal_sys::VSIFReadL(chunk.as_mut_ptr() as *mut c_void, 1, chunk.len(), file.0)
        };
        out.extend_from_slice(&chunk[..n]);
        if n < chunk.len() {
            break;
        }
    }
    Ok(out)
}

pub fn gdal_err(e: gdal::errors::GdalError) -> Error {
    Error::new("CPLE_AppDefinedError", e.to_string())
}

/// `union_all([shape(g) for g, v in features.shapes(image.astype(uint8), mask=mask,
/// transform=t) if v == value])` (connectivity 4) as peatland's `mask_to_geometry` runs it: a MEM
/// Byte band over the image, a MEM Byte band over the mask, an OGR Memory layer "polygons" with
/// one OFTInteger field, `GDALPolygonize(band, mask, layer, 0, NULL)`; the value-`value` polygons
/// in layer (FID) order go straight into one GeometryCollection and the layer is freed BEFORE the
/// GEOS unary union. The union is handed back wrapped in a GeometryCollection (no WKB copy):
/// reprojection and GEOS distance read a collection of one geometry exactly as the geometry.
/// None when no polygon has `value`.
pub fn polygonize_union_u8(
    image: &mut [u8],
    mask: &mut [u8],
    width: usize,
    height: usize,
    geo_transform: [f64; 6],
    value: i64,
) -> Result<Option<Geometry>, Error> {
    use gdal::vector::{LayerAccess, LayerOptions};
    use gdal_sys::{OGRFieldType, OGRwkbGeometryType};
    let src = MemDataset::over(image, width, height, geo_transform, None)?;
    let msk = MemDataset::over(mask, width, height, geo_transform, None)?;
    let driver = gdal::DriverManager::get_driver_by_name("Memory").map_err(gdal_err)?;
    let mut out = driver.create_vector_only("").map_err(gdal_err)?;
    let layer = out
        .create_layer(LayerOptions {
            name: "polygons",
            ty: OGRwkbGeometryType::wkbPolygon,
            ..Default::default()
        })
        .map_err(gdal_err)?;
    layer
        .create_defn_fields(&[("image_value", OGRFieldType::OFTInteger)])
        .map_err(gdal_err)?;
    let band = src.ds.rasterband(1).map_err(gdal_err)?;
    let mask_band = msk.ds.rasterband(1).map_err(gdal_err)?;
    // SAFETY: the two bands and the layer are live (their datasets are owned above and outlive
    // the call); no options, no progress callback.
    let err = unsafe {
        gdal_sys::GDALPolygonize(
            band.c_rasterband(),
            mask_band.c_rasterband(),
            layer.c_layer(),
            0,
            ptr::null_mut(),
            None,
            ptr::null_mut(),
        )
    };
    if err != CPLErr::CE_None {
        return Err(last_error("GDALPolygonize failed"));
    }
    #[expect(
        clippy::drop_non_drop,
        reason = "ends the layer borrow of the dataset before reading it back"
    )]
    drop(layer);
    let mut gc = Geometry::empty(OGRwkbGeometryType::wkbGeometryCollection).map_err(gdal_err)?;
    let mut any = false;
    {
        let mut layer = out.layer(0).map_err(gdal_err)?;
        for feature in layer.features() {
            let v = feature.field_as_integer(0).map_err(gdal_err)?.unwrap_or(0) as i64;
            if let (true, Some(g)) = (v == value, feature.geometry()) {
                gc.add_geometry(g.clone()).map_err(gdal_err)?;
                any = true;
            }
        }
    }
    drop((out, src, msk));
    if !any {
        return Ok(None);
    }
    // SAFETY: `gc` is a live OGR geometry; the result is a new geometry we own (or null).
    let h = unsafe { gdal_sys::OGR_G_UnaryUnion(gc.c_geometry()) };
    if h.is_null() {
        return Err(last_error("OGR_G_UnaryUnion failed"));
    }
    drop(gc);
    let wrapper = Geometry::empty(OGRwkbGeometryType::wkbGeometryCollection);
    let wrapper = match wrapper {
        Ok(w) => w,
        Err(e) => {
            // SAFETY: `h` is still ours (not handed over); destroyed once.
            unsafe { gdal_sys::OGR_G_DestroyGeometry(h) };
            return Err(gdal_err(e));
        }
    };
    // SAFETY: both handles are live; on success the collection takes ownership of `h`, on failure
    // it stays ours and is destroyed here.
    let err = unsafe { gdal_sys::OGR_G_AddGeometryDirectly(wrapper.c_geometry(), h) };
    if err != gdal_sys::OGRErr::OGRERR_NONE {
        // SAFETY: see above.
        unsafe { gdal_sys::OGR_G_DestroyGeometry(h) };
        return Err(Error::new(
            "CPLE_AppDefinedError",
            "OGR_G_AddGeometryDirectly failed",
        ));
    }
    Ok(Some(wrapper))
}

/// shapely `a.distance(b)` (GEOSDistance) through OGR_G_Distance, which delegates to GEOS.
pub fn distance(a: &Geometry, b: &Geometry) -> f64 {
    // SAFETY: both geometries are live OGR handles; the call only reads them.
    unsafe { gdal_sys::OGR_G_Distance(a.c_geometry(), b.c_geometry()) }
}

/// One row of PROJ's CRS list (`proj_get_crs_info_list_from_database`), as GDAL copies it.
pub struct CrsInfo {
    pub code: String,
    pub name: String,
    pub projected: bool,
    pub deprecated: bool,
    /// (west, south, east, north) when the area of use is known
    pub bbox: Option<[f64; 4]>,
}

/// `OSRGetCRSInfoListFromDatabase(auth_name)`: every CRS of the authority, in PROJ's order.
pub fn crs_info_list(auth_name: &str) -> Result<Vec<CrsInfo>, Error> {
    struct List(*mut *mut gdal_sys::OSRCRSInfo);
    impl Drop for List {
        fn drop(&mut self) {
            // SAFETY: list returned by OSRGetCRSInfoListFromDatabase, destroyed once.
            unsafe { gdal_sys::OSRDestroyCRSInfoList(self.0) }
        }
    }
    let cauth = cstring(auth_name)?;
    let mut n = 0i32;
    // SAFETY: NUL-terminated name, NULL params (GDAL ignores them), out-count is a local.
    let list = List(unsafe {
        gdal_sys::OSRGetCRSInfoListFromDatabase(cauth.as_ptr(), ptr::null(), &mut n)
    });
    if list.0.is_null() {
        return Err(last_error("OSRGetCRSInfoListFromDatabase failed"));
    }
    let text = |p: *mut std::ffi::c_char| {
        // SAFETY: GDAL fills these with NUL-terminated strings or null; copied while the list lives.
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n.max(0) as usize {
        // SAFETY: the list holds `n` valid entries (then a null terminator).
        let info = unsafe { &**list.0.add(i) };
        out.push(CrsInfo {
            code: text(info.pszCode),
            name: text(info.pszName),
            projected: info.eType == gdal_sys::OSRCRSType::OSR_CRS_TYPE_PROJECTED,
            deprecated: info.bDeprecated != 0,
            bbox: (info.bBboxValid != 0).then_some([
                info.dfWestLongitudeDeg,
                info.dfSouthLatitudeDeg,
                info.dfEastLongitudeDeg,
                info.dfNorthLatitudeDeg,
            ]),
        });
    }
    Ok(out)
}

// ============================ benefit agent (2026-11-27): WarpedVRT ============================
// Added for src/benefit (5.9-5.13 read species, hazard and population rasters through
// rasterio's WarpedVRT onto a reference grid). Nothing above this line was changed.

/// rasterio 1.3.10 `WarpedVRT(src, crs=dst_crs, transform=dst_transform, width=, height=,
/// resampling=)` (`_warp.pyx` WarpedVRTReaderBase, the "transform and size given" branch):
/// `create_warp_options` (`UNIFIED_SRC_NODATA=YES`, then `INIT_DEST=NO_DATA`; working type
/// unknown; src/dst nodata = rasterio's `src.nodata` for every band; all bands), an exact
/// `GDALCreateGenImgProjTransformer3(src wkt, src gt, dst wkt, dst gt)` wrapped in the approximate
/// transformer at 0.125, `GDALCreateWarpedVRT`, `GDALSetProjection`, then every band's nodata set
/// to the destination nodata (or deleted when None). The VRT owns the transformer and holds a
/// reference on the source, which is why the source is kept here and dropped after it.
pub struct WarpedVrt {
    // Field order is drop order: the VRT releases its reference on `_src` before `_src` closes.
    pub vrt: Dataset,
    _src: Dataset,
}

pub fn warped_vrt(
    src: Dataset,
    src_nodata: Option<f64>,
    dst_wkt: &str,
    dst_gt: [f64; 6],
    width: usize,
    height: usize,
    resampling: GDALResampleAlg::Type,
) -> Result<WarpedVrt, Error> {
    let src_wkt = cstring(&src.projection())?;
    let dst_wkt_c = cstring(dst_wkt)?;
    let src_gt = src.geo_transform().map_err(gdal_err)?;
    let count = src.raster_count();
    let count_i = i32::try_from(count).map_err(|_| Error::new("ValueError", "too many bands"))?;
    let (w, h) = (
        i32::try_from(width).map_err(|_| Error::new("ValueError", "width too large"))?,
        i32::try_from(height).map_err(|_| Error::new("ValueError", "height too large"))?,
    );
    // SAFETY: both WKT strings are NUL-terminated and both geotransforms are 6 doubles; GDAL
    // copies what it keeps.
    let exact = unsafe {
        gdal_sys::GDALCreateGenImgProjTransformer3(
            src_wkt.as_ptr(),
            src_gt.as_ptr(),
            dst_wkt_c.as_ptr(),
            dst_gt.as_ptr(),
        )
    };
    if exact.is_null() {
        return Err(last_error("GDALCreateGenImgProjTransformer3 failed"));
    }
    let approx = ApproxTransformer::new(GenImgTransformer(exact), 0.125)?;

    // SAFETY: fresh zero-initialised struct owned by `wo`.
    let wo = WarpOptions(unsafe { gdal_sys::GDALCreateWarpOptions() });
    let unified = (cstring("UNIFIED_SRC_NODATA")?, cstring("YES")?);
    let init_dest = (cstring("INIT_DEST")?, cstring("NO_DATA")?);
    let bands: Vec<i32> = (1..=count_i).collect();
    // SAFETY: `wo.0` is live and owned by `wo`; each CSLSetNameValue returns the new list, which
    // is stored back. Arrays are CPLMalloc'ed (freed by GDALDestroyWarpOptions). The source
    // dataset outlives the VRT (kept in WarpedVrt).
    unsafe {
        let o = &mut *wo.0;
        o.papszWarpOptions =
            gdal_sys::CSLSetNameValue(o.papszWarpOptions, unified.0.as_ptr(), unified.1.as_ptr());
        o.papszWarpOptions = gdal_sys::CSLSetNameValue(
            o.papszWarpOptions,
            init_dest.0.as_ptr(),
            init_dest.1.as_ptr(),
        );
        o.eWorkingDataType = gdal_sys::GDALDataType::GDT_Unknown;
        o.eResampleAlg = resampling;
        if let Some(nd) = src_nodata {
            o.padfSrcNoDataReal = cpl_array(&vec![nd; count]);
            o.padfSrcNoDataImag = cpl_array(&vec![0.0f64; count]);
            o.padfDstNoDataReal = cpl_array(&vec![nd; count]);
            o.padfDstNoDataImag = cpl_array(&vec![0.0f64; count]);
        }
        o.nBandCount = count_i;
        o.panSrcBands = cpl_array(&bands);
        o.panDstBands = cpl_array(&bands);
        o.hSrcDS = src.c_dataset();
        o.pfnTransformer = Some(gdal_sys::GDALApproxTransform);
        o.pTransformerArg = approx.0;
    }
    let mut gt = dst_gt;
    // SAFETY: live source handle, complete warp options; GDAL clones the options and takes
    // ownership of the transformer only on success.
    let handle =
        unsafe { gdal_sys::GDALCreateWarpedVRT(src.c_dataset(), w, h, gt.as_mut_ptr(), wo.0) };
    if handle.is_null() {
        return Err(last_error("GDALCreateWarpedVRT failed")); // `approx` still owns and frees the transformer
    }
    std::mem::forget(approx); // now owned by the VRT (VRTWarpedDataset destroys it on close)
    // SAFETY: `handle` is a new dataset nobody else owns; wrapping it hands ownership to `Dataset`.
    let vrt = unsafe { Dataset::from_c_dataset(handle) };
    // SAFETY: live VRT handle and a NUL-terminated WKT.
    unsafe { gdal_sys::GDALSetProjection(vrt.c_dataset(), dst_wkt_c.as_ptr()) };
    for b in 1..=count {
        let band = vrt.rasterband(b).map_err(gdal_err)?;
        // SAFETY: live band handle of the VRT.
        unsafe {
            match src_nodata {
                Some(nd) => gdal_sys::GDALSetRasterNoDataValue(band.c_rasterband(), nd),
                None => gdal_sys::GDALDeleteRasterNoDataValue(band.c_rasterband()),
            };
        }
    }
    drop(wo);
    Ok(WarpedVrt { vrt, _src: src })
}
