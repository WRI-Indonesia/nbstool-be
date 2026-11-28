//! ALL `unsafe` of nbstool-fig: one GDAL call the `gdal` crate does not expose.

use std::ffi::CString;
use std::path::Path;

use gdal::raster::RasterCreationOptions;
use nbstool_geo::{Error, Result};

/// `driver.Create(path, 0, 0, 0, GDT_Unknown, options)`: the PDF driver only takes its
/// COMPOSITION_FILE branch for exactly this signature (the `gdal` crate's typed `create_*`
/// passes a concrete data type, which makes an empty 0x0 raster instead). The returned handle is
/// closed at once; the composition is written by then.
pub(crate) fn create_unknown_type(
    driver: &gdal::Driver,
    path: &Path,
    options: &RasterCreationOptions,
) -> Result<()> {
    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| Error::new("ValueError", e.to_string()))?;
    // SAFETY: the driver handle comes from a live `gdal::Driver`; `c_path` and the option list
    // outlive the call; GDALCreate copies what it keeps. The returned dataset, if any, is owned
    // here and closed exactly once below.
    let ds = unsafe {
        gdal_sys::GDALCreate(
            driver.c_driver(),
            c_path.as_ptr(),
            0,
            0,
            0,
            gdal_sys::GDALDataType::GDT_Unknown as _,
            options.as_ptr(),
        )
    };
    if ds.is_null() {
        return Err(Error::new("RuntimeError", "GDALCreate returned NULL"));
    }
    // SAFETY: `ds` is a non-null dataset handle from GDALCreate, not shared, closed once.
    unsafe { gdal_sys::GDALClose(ds) };
    Ok(())
}
