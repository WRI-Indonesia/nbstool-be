//! nbstool-geo: the v3 raster analysis of `application/utils/geos/v3`, ported to Rust on the same
//! GDAL (3.8.4) through the `gdal` crate. See README.md for the rasterio -> GDAL option mapping.
//!
//! Layout mirrors the Python package: `common` (AOI contract, raster access, tabulation, shared
//! forest masks), `settings` (layer_path), `config` (layer names, legends, keys), and one module
//! per component under `site_characterisation`. All `unsafe` lives in `ffi`.

pub mod affine;
pub mod benefit;
pub mod common;
pub mod config;
pub mod db;
mod ffi;
mod generated;
pub mod pathway;
pub mod pipeline;
pub mod proof;
pub mod py;
pub mod run_analysis;
pub mod settings;
pub mod site_characterisation;
pub mod threat;

/// A Python exception as the original would raise it: the exception type name (what
/// `pipeline.safe` puts on the wire) and its message text.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub kind: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// GDAL configuration the original runs with, set only where the process environment has not
/// already set it. The Dockerfile's /vsicurl options (no directory listing per open) and
/// rasterio's own default (`GTIFF_IMPLICIT_JPEG_OVR=NO`, `rasterio/env.py`). None of them changes
/// a pixel value; they change how many HTTP requests an open costs.
pub fn init_gdal() {
    for (key, value) in [
        ("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR"),
        ("GTIFF_IMPLICIT_JPEG_OVR", "NO"),
    ] {
        if std::env::var_os(key).is_none() {
            let _ = gdal::config::set_config_option(key, value);
        }
    }
}
