//! `application/utils/geopdf.generate_geopdf` (GET /documents/geopdf): the same GDAL PDF driver
//! composition (assets/base.xml filled the same way), so page size, layer tree and
//! georeferencing come from GDAL exactly as in the original. The raster inputs it composes are
//! redrawn in Rust: the OSM basemap (contextily), the scale bar / north arrow / locator inset
//! (matplotlib), the legend page (reportlab + poppler); the WMS GeoTIFFs, the logo VRT, the
//! polygon Shapefile and the label CSV are made as the original makes them.

mod inmap;
mod legend;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gdal::spatial_ref::SpatialRef;
use gdal::vector::{Geometry, LayerAccess, LayerOptions, OGRwkbGeometryType};
use gdal::{Dataset, DriverManager};
use serde_json::Value;

use crate::Frame;
use crate::basemap::{self, BASEMAPS};
use nbstool_geo::{Error, Result};

static BASE_XML: &str = include_str!("../../assets/base.xml");
static LOGO_PNG: &[u8] = include_bytes!("../../assets/scene-black.logo.png");

const CM: f64 = 0.0264583333;

/// Where the generator works and what it reads.
pub struct GeoPdfEnv<'a> {
    /// the original's `generated-geopdf` folder; a unique subfolder is made and removed
    pub work_root: &'a Path,
    /// contextily tile cache (see basemap.rs)
    pub tile_cache: &'a Path,
    /// basemap-data's shadedrelief.jpg (Basemap.shadedrelief)
    pub shadedrelief: &'a Path,
    /// proof only: answer GETs from / save them to this folder (sha256(url)[:24].bin)
    pub http_cache: Option<&'a Path>,
    /// "YYYY-MM-DD HH:MM:SS" for the legend footer; None = now (UTC)
    pub now_utc: Option<String>,
}

/// Python's repr of a float (`str.format` of a float)
fn py(v: f64) -> String {
    format!("{v:?}")
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil_from_days (H. Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn get(client: &reqwest::blocking::Client, url: &str, cache: Option<&Path>) -> Result<Vec<u8>> {
    use sha2::Digest;
    let file = cache.map(|c| {
        c.join(format!(
            "{}.bin",
            &format!("{:x}", sha2::Sha256::digest(url.as_bytes()))[..24]
        ))
    });
    if let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok()) {
        return Ok(bytes);
    }
    // requests.get: no status check, the body is used whatever it is
    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.bytes())
        .map_err(|e| Error::new("ConnectionError", e.to_string()))?
        .to_vec();
    if let Some(f) = file {
        let _ = std::fs::write(f, &bytes);
    }
    Ok(bytes)
}

/// `str.format` of base.xml: `{name}` replaced, `{{` / `}}` unescaped
fn format_xml(vars: &HashMap<&str, String>) -> Result<String> {
    // open(...).read() in text mode: universal newlines
    let base = BASE_XML.replace(['\r'], "");
    let mut out = String::with_capacity(base.len() * 2);
    let mut rest = base.as_str();
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if tail.starts_with('{') {
            let end = tail.find('}').ok_or_else(|| {
                Error::new("ValueError", "Single '{' encountered in format string")
            })?;
            let key = &tail[1..end];
            out.push_str(
                vars.get(key)
                    .ok_or_else(|| Error::new("KeyError", format!("'{key}'")))?,
            );
            rest = &tail[end + 1..];
        } else {
            return Err(Error::new(
                "ValueError",
                "Single '}' encountered in format string",
            ));
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The layer tree / raster XML blocks, the layers found (in tree order) and the name reference.
fn layer_tree(
    base_layer_list: &Value,
    layers: &[String],
    curpath: &str,
) -> (
    String,
    String,
    Vec<String>,
    HashMap<String, legend::NameRef>,
) {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let id = |name: &str| name.to_lowercase().replace(' ', "_");
    let raster = |l: &str| {
        format!(
            "<IfLayerOn layerId=\"{l}\">\n                    <Raster georeferencingId=\"georeferenced\" dataset=\"{curpath}/{l}.tif\">\n                        <Blending function=\"Normal\" opacity=\"1.00\" />\n                    </Raster>\n                </IfLayerOn>\n\n"
        )
    };
    let (mut xml_layer, mut xml_raster) = (String::new(), String::new());
    let mut found = Vec::new();
    let mut names = HashMap::new();
    for group in base_layer_list.as_array().into_iter().flatten() {
        let title = s(&group["title"]);
        let (mut group_created, mut group_layer, mut group_raster) =
            (false, String::new(), String::new());
        for parent in group["items"]["global"].as_array().into_iter().flatten() {
            let pname = s(&parent["name"]);
            let (mut parent_layer, mut parent_raster) = (String::new(), String::new());
            let player = parent["layers"].as_str().filter(|l| !l.is_empty());
            if let Some(pl) = player {
                if !layers.iter().any(|l| l == pl) {
                    continue;
                }
                found.push(pl.to_string());
                names.insert(
                    pl.rsplit(':').next().unwrap_or(pl).to_string(),
                    (pname.clone(), title.clone(), String::new()),
                );
                let l = pl.replace(':', "_");
                group_created = true;
                group_layer +=
                    &format!("<Layer id=\"{l}\" name=\"{pname}\" initiallyVisible=\"true\" />\n");
                group_raster += &raster(&l);
            }
            for child in parent["child"].as_array().into_iter().flatten() {
                let gs = s(&child["gs_name"]);
                if !layers.contains(&gs) {
                    continue;
                }
                found.push(gs.clone());
                names.insert(
                    gs.rsplit(':').next().unwrap_or(&gs).to_string(),
                    (s(&child["layer_name"]), title.clone(), pname.clone()),
                );
                let l = gs.replace(':', "_");
                group_created = true;
                parent_layer += &format!(
                    "<Layer id=\"{l}\" name=\"{}\" initiallyVisible=\"true\" />\n",
                    s(&child["layer_name"])
                );
                parent_raster += &raster(&l);
            }
            if !parent_layer.is_empty() {
                group_layer += &format!(
                    "<Layer id=\"{}\" name=\"{pname}\" initiallyVisible=\"true\">\n                    {parent_layer}\n                </Layer>\n",
                    id(&pname)
                );
                group_raster += &format!(
                    "<IfLayerOn layerId=\"{}\">\n                    {parent_raster}\n                </IfLayerOn>\n\n",
                    id(&pname)
                );
            }
        }
        if group_created {
            xml_layer += &format!(
                "<Layer id=\"{}\" name=\"{title}\" initiallyVisible=\"true\">\n                {group_layer}\n            </Layer>\n",
                id(&title)
            );
            xml_raster += &format!(
                "<IfLayerOn layerId=\"{}\">\n                {group_raster}\n            </IfLayerOn>\n\n",
                id(&title)
            );
        }
    }
    (xml_layer, xml_raster, found, names)
}

/// The labels CSV (title split at 40 characters as the original does)
fn labels_csv(width: i64, height: i64, title: &str) -> String {
    let (x_title, y_title) = (width - 120, height - 25);
    let mut csv = format!(
        "id,WKT,OGR_STYLE\n1,\"POINT({x_title} {y_title})\",\"LABEL(f:\"\"Helvetica\"\",t:\"\"Interactive\"\",s:50pt,bo:1,it:1)\"\n2,\"POINT({} {})\",\"LABEL(f:\"\"Helvetica\"\",t:\"\"GeoPDF\"\",s:50pt,bo:1,it:1)\"",
        x_title + 17,
        y_title - 20
    );
    let x3 = py(width as f64 / 2.0);
    let limit = 40;
    if title.chars().count() <= limit {
        csv += &format!(
            "\n3,\"POINT({x3} {})\",\"LABEL(f:\"\"Helvetica\"\",t:\"\"{title}\"\",s:50pt,bo:1,p:5)\"",
            y_title - 5
        );
    } else {
        let mut lines = Vec::new();
        let mut tmp = String::new();
        for word in title.split(' ') {
            if format!("{tmp} {word}").trim().chars().count() > limit {
                lines.push(tmp.trim().to_string());
                tmp.clear();
            }
            tmp += &format!(" {word}");
        }
        lines.push(tmp);
        for (k, line) in lines.iter().enumerate() {
            let idx = k as i64 + 3;
            csv += &format!(
                "\n{idx},\"POINT({x3} {})\",\"LABEL(f:\"\"Helvetica\"\",t:\"\"{line}\"\",s:50pt,bo:1,p:5)\"",
                y_title - 20 * (idx - 3) + 5
            );
        }
    }
    csv
}

/// `geom.Centroid()` of a (multi)polygon: area-weighted centroid of the rings (GEOS
/// Centroid; summation order differs, so the last digits can).
fn centroid(geom: &Geometry) -> (f64, f64) {
    fn rings(g: &Geometry, out: &mut Vec<(Vec<(f64, f64)>, bool)>) {
        let n = g.geometry_count();
        if n == 0 {
            out.push((
                (0..g.point_count() as i32)
                    .map(|i| g.get_point(i))
                    .map(|p| (p.0, p.1))
                    .collect(),
                false,
            ));
            return;
        }
        let is_polygon = matches!(
            g.geometry_type(),
            OGRwkbGeometryType::wkbPolygon | OGRwkbGeometryType::wkbPolygon25D
        );
        for i in 0..n {
            let len = out.len();
            rings(&g.get_geometry(i), out);
            if is_polygon && i > 0 {
                out[len].1 = true; // hole
            }
        }
    }
    let mut all = Vec::new();
    rings(geom, &mut all);
    let (mut a, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for (ring, hole) in &all {
        let (mut ra, mut rx, mut ry) = (0.0, 0.0, 0.0);
        for w in ring.windows(2) {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            let cross = x0 * y1 - x1 * y0;
            ra += cross;
            rx += (x0 + x1) * cross;
            ry += (y0 + y1) * cross;
        }
        // holes count against the shell whatever their winding
        let sign = if (ra >= 0.0) != *hole { 1.0 } else { -1.0 };
        a += sign * ra.abs();
        sx += sign * rx * ra.signum();
        sy += sign * ry * ra.signum();
    }
    (sx / (3.0 * a), sy / (3.0 * a))
}

fn gerr(e: gdal::errors::GdalError) -> Error {
    Error::new("RuntimeError", e.to_string())
}

/// `generate_geopdf(geom, layers, base_layer_list, geopdf_title)` -> the PDF bytes.
/// `geom_geojson`: the session polygon in EPSG:4326 (ST_AsGeoJSON of polygons.geom).
pub fn generate_geopdf(
    geom_geojson: &str,
    layers: &[String],
    base_layer_list: &Value,
    title: &str,
    env: &GeoPdfEnv,
) -> Result<Vec<u8>> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let curpath: PathBuf = env
        .work_root
        .join(format!("{:x}-{:x}", std::process::id(), stamp));
    std::fs::create_dir_all(&curpath).map_err(|e| Error::new("OSError", e.to_string()))?;
    let result = generate_in(&curpath, geom_geojson, layers, base_layer_list, title, env);
    let _ = std::fs::remove_dir_all(&curpath); // shutil.rmtree(curpath)
    result.map(|(pdf, _)| pdf)
}

/// The work itself; also returns the composition XML (the proof compares it).
pub fn generate_in(
    curpath: &Path,
    geom_geojson: &str,
    layers: &[String],
    base_layer_list: &Value,
    title: &str,
    env: &GeoPdfEnv,
) -> Result<(Vec<u8>, String)> {
    let cur = curpath.to_string_lossy().replace('\\', "/");
    let (xml_layer, xml_raster, found, names) = layer_tree(base_layer_list, layers, &cur);
    let layers = found; // "prevent injection / invalid layer"
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| Error::new("ConnectionError", e.to_string()))?;

    // legend JSON per layer, then the legend page
    let mut documents = Vec::new();
    for layer in &layers {
        let url = format!(
            "https://nbstool.scenecoalition.org/layer/nbs/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetLegendGraphic&FORMAT=application/json&LAYER={layer}"
        );
        let bytes = get(&client, &url, env.http_cache)?;
        documents.push(
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| Error::new("JSONDecodeError", e.to_string()))?,
        );
    }
    let now = env.now_utc.clone().unwrap_or_else(utc_now);
    let (legend_png, add_height) = legend::draw_legend(&documents, &names, &now)?;
    std::fs::write(curpath.join("legend.png"), &legend_png)
        .map_err(|e| Error::new("OSError", e.to_string()))?;

    let base_height = 460i64;
    let (width, height) = (842i64, base_height + add_height);
    let geom = Geometry::from_geojson(geom_geojson).map_err(gerr)?;
    let env4 = geom.envelope();
    let (mut kiri, mut kanan, mut bawah, mut atas) = (env4.MinX, env4.MaxX, env4.MinY, env4.MaxY);
    let mut total_x = kanan - kiri;
    let mut total_y = atas - bawah;
    let (base_x1, base_x2, base_y1, base_y2) = (20i64, width - 20, height - 450, height - 60);
    let total_base_x = (base_x2 - base_x1) as f64;
    let total_base_y = (base_y2 - base_y1) as f64;
    if total_x > total_y {
        atas += 0.03;
        bawah -= 0.03;
    } else {
        kiri -= 0.03;
        kanan += 0.03;
    }
    total_x = kanan - kiri;
    total_y = atas - bawah;
    let mul_x = total_x * 10f64.powi(5);
    let mul_y = total_y * 10f64.powi(5);
    let sud_x = mul_y / (total_base_y / total_base_x);
    let addon_x = (sud_x - mul_x) / 2.0 / 10f64.powi(5);
    kiri -= addon_x;
    kanan += addon_x;
    let zoom_mult = 1.0 / 5.0;
    let add_x = 1.0 * zoom_mult;
    let add_y = total_base_y / total_base_x * zoom_mult;
    kiri -= add_x;
    kanan += add_x;
    atas += add_y;
    bawah -= add_y;

    let logo = (150.0 * 0.75, 55.0 * 0.75);
    let (logo_x1, logo_x2) = (20i64, 20.0 + logo.0);
    let logo_y1 = (height - 30) as f64 - logo.1 / 2.0;
    let logo_y2 = logo_y1 + logo.1;

    let p = |name: &str| format!("{cur}/{name}");
    let mut vars: HashMap<&str, String> = HashMap::new();
    for (k, v) in [
        ("width", width.to_string()),
        ("height", height.to_string()),
        ("geo_x1", base_x1.to_string()),
        ("geo_y1", base_y1.to_string()),
        ("geo_x2", base_x2.to_string()),
        ("geo_y2", base_y2.to_string()),
        ("longitude_kiri", py(kiri)),
        ("longitude_kanan", py(kanan)),
        ("latitude_bawah", py(bawah)),
        ("latitude_atas", py(atas)),
        ("logo_x1", logo_x1.to_string()),
        ("logo_x2", py(logo_x2)),
        ("logo_y1", py(logo_y1)),
        ("logo_y2", py(logo_y2)),
        ("logo_path", p("logo.vrt")),
        ("basemap_path", p("basemap.jpg")),
        ("inmap_path", p("inmap.png")),
        ("polygon_path", p("test.shp")),
        ("legend_path", p("legend.png")),
        ("labels_path", p("labels.csv")),
        ("legend_x1", "0".into()),
        ("legend_y1", "0".into()),
        ("legend_x2", width.to_string()),
        ("legend_y2", (height - base_height).to_string()),
        ("layers_xml_layer", xml_layer),
        ("layers_xml_raster", xml_raster),
    ] {
        vars.insert(k, v);
    }
    let xml = format_xml(&vars)?;

    // basemap.jpg: plt.subplots(figsize=(802 cm, 390 cm)) at 100 dpi, OSM only, tight_layout(pad=0)
    let fig_in = (total_base_x * CM, total_base_y * CM);
    let frame = Frame::fit(
        (kiri, kanan),
        (bawah, atas),
        fig_in,
        100.0,
        (0.0, 1.0, 0.0, 1.0),
    );
    let bm = basemap::basemap(&frame, env.tile_cache, &BASEMAPS[..1])
        .ok_or_else(|| Error::new("HTTPError", "basemap tiles could not be fetched"))?;
    let rgb: Vec<u8> = bm.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 75)
        .encode(
            &rgb,
            frame.width,
            frame.height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| Error::new("OSError", e.to_string()))?;
    std::fs::write(curpath.join("basemap.jpg"), &jpg)
        .map_err(|e| Error::new("OSError", e.to_string()))?;

    // inmap.png
    let (cx, cy) = centroid(&geom);
    let inmap_png = inmap::render(&frame, (cx, cy), fig_in, env.shadedrelief)?;
    std::fs::write(curpath.join("inmap.png"), &inmap_png)
        .map_err(|e| Error::new("OSError", e.to_string()))?;

    // logo.vrt: gdal.Translate(logo_path, <logo png>, format="VRT")
    let logo_png = curpath.join("scene-black.logo.png");
    std::fs::write(&logo_png, LOGO_PNG).map_err(|e| Error::new("OSError", e.to_string()))?;
    let vrt = DriverManager::get_driver_by_name("VRT").map_err(gerr)?;
    Dataset::open(&logo_png)
        .map_err(gerr)?
        .create_copy(&vrt, curpath.join("logo.vrt"), &Default::default())
        .map_err(gerr)?;

    // test.shp: layer "test", EPSG:4326, the polygon
    {
        let shp = DriverManager::get_driver_by_name("ESRI Shapefile").map_err(gerr)?;
        let mut ds = shp
            .create_vector_only(curpath.join("test.shp"))
            .map_err(gerr)?;
        let srs = SpatialRef::from_epsg(4326).map_err(gerr)?;
        let lyr = ds
            .create_layer(LayerOptions {
                name: "test",
                srs: Some(&srs),
                ty: OGRwkbGeometryType::wkbUnknown,
                options: None,
            })
            .map_err(gerr)?;
        let mut f = gdal::vector::Feature::new(lyr.defn()).map_err(gerr)?;
        f.set_geometry(Geometry::from_wkt(&geom.wkt().map_err(gerr)?).map_err(gerr)?)
            .map_err(gerr)?;
        f.create(&lyr).map_err(gerr)?;
    }

    // WMS GetMap GeoTIFF per layer (the body is written whatever it is)
    for layer in &layers {
        let url = format!(
            "https://gis.scenecoalition.org/geoserver/nbs/wms?service=WMS&version=1.1.0&request=GetMap&layers={layer}&bbox={},{},{},{}&width={width}&height={height}&srs=EPSG:4326&styles=&format=image/geotiff",
            py(kiri),
            py(bawah),
            py(kanan),
            py(atas)
        );
        let bytes = get(&client, &url, env.http_cache)?;
        std::fs::write(
            curpath.join(format!("{}.tif", layer.replace(':', "_"))),
            bytes,
        )
        .map_err(|e| Error::new("OSError", e.to_string()))?;
    }

    std::fs::write(curpath.join("labels.csv"), labels_csv(width, height, title))
        .map_err(|e| Error::new("OSError", e.to_string()))?;

    // pdf_drv.Create(out_path, 0, 0, 0, GDT_Unknown, options=["COMPOSITION_FILE=" + xml])
    let out = curpath.join("geo.pdf");
    if std::env::var_os("NBSTOOL_FIG_KEEP_XML").is_some() {
        let _ = std::fs::write(curpath.join("composition.xml"), &xml);
    }
    let _ = {
        let pdf = DriverManager::get_driver_by_name("PDF").map_err(gerr)?;
        let mut opts = gdal::raster::RasterCreationOptions::new();
        opts.add_string(&format!("COMPOSITION_FILE={xml}"))
            .map_err(gerr)?;
        // the composition errors are logged by GDAL and the partial file kept, as in the original
        // composition errors are logged by GDAL; whatever file it wrote is served, as the original
        let created = crate::ffi::create_unknown_type(&pdf, &out, &opts);
        std::fs::read(&out)
            .map_err(|e| Error::new("FileNotFoundError", format!("{e}; {:?}", created.err())))?
    };
    let bytes = std::fs::read(&out).map_err(|e| Error::new("FileNotFoundError", e.to_string()))?;
    Ok((bytes, xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_long_title_splits_like_python() {
        let csv = labels_csv(
            842,
            1013,
            "Test Title GeoPDF Test Title GeoPDF Test Title GeoPDF",
        );
        assert!(csv.contains("3,\"POINT(421.0 993)\",\"LABEL(f:\"\"Helvetica\"\",t:\"\"Test Title GeoPDF Test Title GeoPDF Test\"\""), "{csv}");
        assert!(csv.contains("4,\"POINT(421.0 973)\",\"LABEL(f:\"\"Helvetica\"\",t:\"\" Title GeoPDF\"\",s:50pt,bo:1,p:5)\""), "{csv}");
    }

    #[test]
    fn py_float_repr() {
        assert_eq!(py(421.0), "421.0");
        assert_eq!(py(962.375), "962.375");
    }
}
