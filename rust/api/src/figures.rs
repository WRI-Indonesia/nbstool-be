//! `utils/document_generator/v3`: `boundary_map_path`, `generate_boundary_map` (nbstool-fig), and
//! werkzeug's `send_file` for the routes that serve a file from disk.

use std::path::Path;

use axum::response::Response;

use crate::core::{self, ctx};

pub const BOUNDARY_MAP_FOLDER: &str = "generated-file/boundary-map/";
/// contextily's tile cache, kept apart from the original's joblib cache
pub const TILE_CACHE: &str = "generated-file/tile-cache/rust";

/// `boundary_map_path(session_id)`
pub fn boundary_map_path(session_id: &str) -> String {
    format!("{BOUNDARY_MAP_FOLDER}{session_id}.png")
}

pub(crate) fn init_gdal() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(nbstool_geo::init_gdal);
}

/// The AOI and settings a figure is drawn from (`prepare_aoi_from_session`).
pub async fn aoi_and_settings(
    session_id: &str,
) -> Result<(nbstool_geo::common::Aoi, nbstool_geo::settings::Settings), String> {
    init_gdal();
    let app = &ctx().await.app;
    let aoi = nbstool_geo::common::prepare_aoi_from_session(app, session_id)
        .await
        .map_err(|e| e.message)?;
    let settings = nbstool_geo::settings::Settings::load(app)
        .await
        .map_err(|e| e.message)?;
    Ok((aoi, settings))
}

/// `generate_boundary_map(session_id)`: draw the card picture and upload it
/// (`CloudStorage().upload(path)`). `Ok(None)` when the AOI has no raster coverage.
pub async fn generate_boundary_map(session_id: &str) -> Result<Option<String>, String> {
    let (aoi, settings) = aoi_and_settings(session_id).await?;
    std::fs::create_dir_all(BOUNDARY_MAP_FOLDER).map_err(|e| e.to_string())?;
    let path = boundary_map_path(session_id);
    let target = path.clone();
    let drawn = tokio::task::spawn_blocking(move || {
        nbstool_fig::render_boundary_map(&settings, &aoi, Path::new(&target), Path::new(TILE_CACHE))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.message)?;
    if drawn.is_none() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    crate::gcs::get_bucket().await;
    crate::users_write::gcs_upload(&path, &bytes, "png").await;
    Ok(Some(path))
}

/// werkzeug's `quote_header_value`: a token stays bare, anything else is quoted.
fn header_value(v: &str) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    if !v.is_empty() && v.chars().all(token) {
        v.to_string()
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// `mimetypes.guess_type(name)` for the files these routes serve.
fn guess_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "pdf" => "application/pdf",
        // Python's mimetypes reads the Windows registry, where .csv is Excel's; Linux (staging) text/csv
        #[cfg(windows)]
        "csv" => "application/vnd.ms-excel",
        #[cfg(not(windows))]
        "csv" => "text/csv; charset=utf-8",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "kml" => "application/vnd.google-earth.kml+xml",
        _ => "application/octet-stream",
    }
}

/// Flask's `send_file(path, mimetype=, as_attachment=, download_name=)` for a file on disk:
/// Content-Disposition, Last-Modified, `Cache-Control: no-cache`, the `{mtime}-{size}-{adler32}`
/// ETag.
/// shortcut: conditional (304) and Range (206, `Accept-Ranges`) answers are not made; a full 200.
pub fn send_file(
    path: &str,
    mimetype: Option<&'static str>,
    as_attachment: bool,
    download_name: Option<&str>,
) -> std::io::Result<Response> {
    let abs = std::path::absolute(path)?;
    let meta = std::fs::metadata(&abs)?;
    let data = std::fs::read(&abs)?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as f64 + 1e-9 * d.subsec_nanos() as f64)
        .unwrap_or(0.0);
    let name = download_name
        .map(str::to_string)
        .or_else(|| abs.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let mimetype = mimetype.unwrap_or_else(|| guess_type(&name));
    let mut response = core::respond(200, mimetype, data);
    let disposition = if name.is_ascii() {
        format!(
            "{}; filename={}",
            if as_attachment {
                "attachment"
            } else {
                "inline"
            },
            header_value(&name)
        )
    } else {
        // shortcut: NFKD + ASCII fallback approximated by dropping non-ASCII; no route sends one
        let simple: String = name.chars().filter(char::is_ascii).collect();
        let quoted: String = name
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        format!(
            "{}; filename={}; filename*=UTF-8''{quoted}",
            if as_attachment {
                "attachment"
            } else {
                "inline"
            },
            header_value(&simple)
        )
    };
    let abs_text = abs.to_string_lossy().into_owned();
    let check = adler2::adler32_slice(abs_text.as_bytes());
    let modified = chrono::DateTime::from_timestamp(mtime as i64, 0).unwrap_or_default();
    response = core::with_header(response, "content-disposition", &disposition);
    response = core::with_header(
        response,
        "last-modified",
        &modified.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
    );
    response = core::with_header(response, "cache-control", "no-cache");
    response = core::with_header(
        response,
        "etag",
        &format!("\"{}-{}-{check}\"", core::py_float_repr(mtime), meta.len()),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_values_quote_like_werkzeug() {
        assert_eq!(header_value("a.png"), "a.png");
        assert_eq!(
            header_value("Feasibility Study.docx"),
            "\"Feasibility Study.docx\""
        );
        assert_eq!(
            guess_type("x.DOCX"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
    }
}
