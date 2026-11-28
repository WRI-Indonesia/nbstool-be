//! Every `unsafe` call of the crate. The safe `gdal` crate has no unary union; shapely's
//! `union_all` is GEOSUnaryUnion of a GeometryCollection, which OGR_G_UnaryUnion makes through
//! OGR's GEOS bridge (same pattern as rust/geo/src/ffi.rs).

use gdal::vector::Geometry;
use gdal_sys::OGRGeometryH;

/// GEOS unary union of `collection`, as an owned OGR geometry (via WKB, exact doubles).
pub fn unary_union(collection: &Geometry) -> Result<Geometry, String> {
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
        return Err(last_error_msg("OGR_G_UnaryUnion failed"));
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
            return Err(last_error_msg("OGR_G_ExportToWkb failed"));
        }
        wkb
    };
    drop(owned);
    Geometry::from_wkb(&wkb).map_err(|e| e.to_string())
}

fn last_error_msg(context: &str) -> String {
    // SAFETY: CPLGetLastErrorMsg returns a pointer to a thread-local NUL-terminated buffer owned by
    // GDAL (never null); it is copied out at once.
    let msg = unsafe { std::ffi::CStr::from_ptr(gdal_sys::CPLGetLastErrorMsg()) }
        .to_string_lossy()
        .into_owned();
    if msg.is_empty() {
        context.to_string()
    } else {
        msg
    }
}

/// A /vsimem file over a buffer Rust keeps owning (`bTakeOwnership = FALSE`). The gdal crate's
/// `create_mem_file` hands a Rust `Vec` to GDAL, which later frees it with the C runtime's
/// `free`: a mismatched allocator (heap corruption seen on Windows).
/// The caller keeps `data` alive and unmoved-in-heap until the file is unlinked.
pub fn mem_file_over(path: &str, data: &[u8]) -> Result<(), String> {
    let c = std::ffi::CString::new(path).map_err(|_| "embedded NUL in a file name".to_string())?;
    // SAFETY: GDAL only reads the buffer (opened read-only by the drivers) and does not free it;
    // the caller guarantees it outlives the /vsimem entry (unlinked before the buffer drops).
    let h = unsafe {
        gdal_sys::VSIFileFromMemBuffer(c.as_ptr(), data.as_ptr() as *mut u8, data.len() as u64, 0)
    };
    if h.is_null() {
        return Err(last_error_msg("VSIFileFromMemBuffer failed"));
    }
    // SAFETY: non-null handle just returned, closed once.
    unsafe { gdal_sys::VSIFCloseL(h) };
    Ok(())
}
