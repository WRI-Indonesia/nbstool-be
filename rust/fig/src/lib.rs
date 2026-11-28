//! nbstool-fig: the v3 map figures of `application/utils/geos/v3/maps.py` without matplotlib.
//!
//! - `render_disturbance_map`: Figure 1 of the feasibility and monitoring documents.
//! - `render_boundary_map`: Figure 2 and the project-card PNG (`/projects/boundary-map`).
//!
//! The class raster is read through `nbstool-geo` (same GDAL, same rasterio.mask rules), so the
//! painted pixels are exact; the drawing reproduces matplotlib's geometry (figure, aspect, tight
//! bbox, legend packing) and contextily's tile choice, and is visually close, not pixel-equal.

mod basemap;
mod ffi;
pub mod geopdf;
mod glyphs;
mod legend;

use std::path::{Path, PathBuf};

use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use nbstool_geo::affine::Affine;
use nbstool_geo::common::{self, Aoi, Ring};
use nbstool_geo::config::REFERENCE_CRS_WKT;
use nbstool_geo::settings::Settings;
use nbstool_geo::{Error, Result};
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub use basemap::{BASEMAPS, Provider};

pub const FIGURE_SIZE_IN: (f64, f64) = (11.3, 4.0);
pub const FIGURE_DPI: f64 = 200.0;
const AOI_HEIGHT_SHARE: f64 = 0.75;
const AOI_OUTLINE: &str = "#2fa35a";
const NO_BASEMAP_COLOUR: &str = "#eef0ea";

/// config.FOREST_CHANGE_RASTER / FOREST_CHANGE_CLASSES
pub const FOREST_CHANGE_RASTER: &str = "threat/forest_change_v3.tif";
pub const FOREST_CHANGE_CLASSES: [(f64, &str, &str); 4] = [
    (1.0, "Remaining", "#3f6b3b"),
    (2.0, "Disturbed", "#e5a63f"),
    (3.0, "Forest loss", "#cf4a3b"),
    (4.0, "Forest gain", "#6ed88a"),
];
/// maps.BOUNDARY_CLASSES: 1.1's classes (Dryland spelled out) then Other (code 0 here).
pub const BOUNDARY_CLASSES: [(u8, &str, &str); 4] = [
    (1, "Dryland forest", "#2c6639"),
    (2, "Mangrove", "#f1fc3e"),
    (3, "Peatland", "#abc963"),
    (0, "Other", "#dadada"),
];
const PATHWAY_RASTER: &str = nbstool_geo::config::PATHWAY_RASTER;
const PATHWAY_ECOSYSTEM_BAND: usize = nbstool_geo::config::PATHWAY_ECOSYSTEM_BAND;

/// A figure as drawn, with the facts the proof compares against matplotlib's.
pub struct Figure {
    pub pixmap: Pixmap,
    pub facts: Facts,
}

#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub xlim: (f64, f64),
    pub ylim: (f64, f64),
    /// axes box in output px, (x0, y0, x1, y1), origin top-left
    pub axes_box: [f64; 4],
    pub legend_frame: [f64; 4],
    pub legend_title: [f64; 4],
    /// (label, text box, handle box, colour)
    pub legend_entries: Vec<(String, [f64; 4], [f64; 4], String)>,
    pub zoom: Option<i32>,
    pub provider_url: Option<String>,
    pub tile_urls: Vec<String>,
    /// the class image handed to the drawing: (height, width), transform, RGBA bytes
    pub raster_shape: (usize, usize),
    pub raster_transform: [f64; 6],
    pub raster_rgba: Vec<u8>,
    pub aoi_bounds: [f64; 4],
}

pub fn hex_rgb(hex: &str) -> [u8; 3] {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// `aoi.geometry.to_crs(4326)`: the polygons in lon/lat (pyproj always_xy order).
fn aoi_4326(aoi: &Aoi) -> Result<Vec<Vec<Ring>>> {
    let gerr = |e: gdal::errors::GdalError| Error::new("CRSError", e.to_string());
    let mut src = SpatialRef::from_wkt(REFERENCE_CRS_WKT).map_err(gerr)?;
    src.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut dst = SpatialRef::from_epsg(4326).map_err(gerr)?;
    dst.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let ct = CoordTransform::new(&src, &dst).map_err(gerr)?;
    let mut polygons = aoi.polygons.clone();
    for ring in polygons.iter_mut().flatten() {
        let mut xs: Vec<f64> = ring.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f64> = ring.iter().map(|p| p[1]).collect();
        ct.transform_coords(&mut xs, &mut ys, &mut [])
            .map_err(gerr)?;
        for (p, (x, y)) in ring.iter_mut().zip(xs.into_iter().zip(ys)) {
            *p = [x, y];
        }
    }
    Ok(polygons)
}

/// maps._clip: `mask(src, shapes, crop=True, filled=False, indexes=band)` -> (data, numpy mask,
/// outside-the-shapes mask, transform, width, height); None when the shapes miss the raster.
#[allow(clippy::type_complexity)]
fn clip(
    settings: &Settings,
    layer: &str,
    polygons: &[Vec<Ring>],
    is_multi: bool,
    band: usize,
) -> Result<Option<(Vec<f64>, Vec<bool>, Vec<bool>, Affine, usize, usize)>> {
    let src = common::open_layer(settings, layer)?;
    let (outside, t, w) = match common::raster_geometry_mask_crop(&src, polygons, is_multi, false) {
        Err(e) if e.kind == "ValueError" => return Ok(None),
        r => r?,
    };
    let (data, read_mask) = common::read_window_masked(&src, band, &w)?;
    let mask = read_mask
        .iter()
        .zip(&outside)
        .map(|(a, b)| *a || *b)
        .collect();
    Ok(Some((
        data,
        mask,
        outside,
        t,
        w.width as usize,
        w.height as usize,
    )))
}

/// Figure 1. None when the AOI has no raster coverage or every pixel is 0 / masked.
pub fn disturbance_figure(
    settings: &Settings,
    aoi: &Aoi,
    tile_cache: &Path,
) -> Result<Option<Figure>> {
    let polygons = aoi_4326(aoi)?;
    let Some((data, mask, _, t, w, h)) =
        clip(settings, FOREST_CHANGE_RASTER, &polygons, aoi.is_multi, 1)?
    else {
        return Ok(None);
    };
    // codes = data.filled(0); `if not codes.any()`
    let codes: Vec<f64> = data
        .iter()
        .zip(&mask)
        .map(|(d, m)| if *m { 0.0 } else { *d })
        .collect();
    if !codes.iter().any(|c| *c != 0.0) {
        return Ok(None);
    }
    let mut rgba = vec![0u8; w * h * 4];
    for (i, c) in codes.iter().enumerate() {
        if let Some((_, _, colour)) = FOREST_CHANGE_CLASSES.iter().find(|(code, _, _)| code == c) {
            let [r, g, b] = hex_rgb(colour);
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    let legend: Vec<(&str, &str)> = FOREST_CHANGE_CLASSES
        .iter()
        .map(|(_, l, c)| (*l, *c))
        .collect();
    draw(
        &polygons,
        rgba,
        (h, w),
        t,
        &legend,
        "Disturbance class",
        tile_cache,
    )
    .map(Some)
}

/// Figure 2 / the project card. None when the AOI lies outside the pathway raster.
pub fn boundary_figure(
    settings: &Settings,
    aoi: &Aoi,
    tile_cache: &Path,
) -> Result<Option<Figure>> {
    let polygons = aoi_4326(aoi)?;
    let Some((data, mask, outside, t, w, h)) = clip(
        settings,
        PATHWAY_RASTER,
        &polygons,
        aoi.is_multi,
        PATHWAY_ECOSYSTEM_BAND,
    )?
    else {
        return Ok(None);
    };
    let mut rgba = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let filled = if mask[i] { 0.0 } else { data[i] };
        let code = nbstool_geo::config::PATHWAY_ECO_TO_AXIS3
            .iter()
            .find(|(src, _)| *src as f64 == filled)
            .map_or(0, |(_, axis3)| *axis3 as u8);
        // pixels inside the polygon with no class are Other; outside the polygon transparent
        if outside[i] {
            continue;
        }
        if let Some((_, _, colour)) = BOUNDARY_CLASSES.iter().find(|(c, _, _)| *c == code) {
            let [r, g, b] = hex_rgb(colour);
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    let legend: Vec<(&str, &str)> = BOUNDARY_CLASSES.iter().map(|(_, l, c)| (*l, *c)).collect();
    draw(
        &polygons,
        rgba,
        (h, w),
        t,
        &legend,
        "Ecosystem type",
        tile_cache,
    )
    .map(Some)
}

/// `render_disturbance_map(aoi, output_path)`: the PNG path, or None.
pub fn render_disturbance_map(
    settings: &Settings,
    aoi: &Aoi,
    output_path: &Path,
    tile_cache: &Path,
) -> Result<Option<PathBuf>> {
    save(disturbance_figure(settings, aoi, tile_cache)?, output_path)
}

/// `render_boundary_map(aoi, output_path)`: the PNG path, or None.
pub fn render_boundary_map(
    settings: &Settings,
    aoi: &Aoi,
    output_path: &Path,
    tile_cache: &Path,
) -> Result<Option<PathBuf>> {
    save(boundary_figure(settings, aoi, tile_cache)?, output_path)
}

fn save(fig: Option<Figure>, path: &Path) -> Result<Option<PathBuf>> {
    let Some(fig) = fig else { return Ok(None) };
    fig.pixmap
        .save_png(path)
        .map_err(|e| Error::new("OSError", e.to_string()))?;
    Ok(Some(path.to_path_buf()))
}

/// matplotlib's frame: `plt.subplots(figsize)` default subplot box, the limits maps._draw sets,
/// the axes box after `apply_aspect` (imshow leaves aspect "equal" = 1), and the tight bbox
/// (= the axes box: everything else is clipped to it) at FIGURE_DPI.
pub(crate) struct Frame {
    pub xlim: (f64, f64),
    pub ylim: (f64, f64),
    /// axes size in output px (float) and the PNG size (matplotlib truncates)
    pub ax_w: f64,
    pub ax_h: f64,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    fn new(bounds: [f64; 4]) -> Frame {
        let [minx, miny, maxx, maxy] = bounds;
        let mid_lat = (miny + maxy) / 2.0;
        let lon_per_lat = 1.0 / mid_lat.to_radians().cos();
        let frame_aspect = FIGURE_SIZE_IN.0 / FIGURE_SIZE_IN.1;
        let mut height = (maxy - miny) / AOI_HEIGHT_SHARE;
        let mut width = height * frame_aspect * lon_per_lat;
        if (maxx - minx) > width * AOI_HEIGHT_SHARE {
            width = (maxx - minx) / AOI_HEIGHT_SHARE;
            height = width / (frame_aspect * lon_per_lat);
        }
        let (cx, cy) = ((minx + maxx) / 2.0, mid_lat);
        let xlim = (cx - width / 2.0, cx + width / 2.0);
        let ylim = (cy - height / 2.0, cy + height / 2.0);
        // plt.subplots default SubplotParams(left .125, right .9, bottom .11, top .88)
        Frame::fit(
            xlim,
            ylim,
            FIGURE_SIZE_IN,
            FIGURE_DPI,
            (0.125, 0.9, 0.11, 0.88),
        )
    }

    /// An aspect-1 axes with these limits in a 1x1 GridSpec of `subplot` (left, right, bottom,
    /// top), the figure saved with `bbox_inches="tight", pad_inches=0` at `dpi`.
    pub(crate) fn fit(
        xlim: (f64, f64),
        ylim: (f64, f64),
        fig_in: (f64, f64),
        dpi: f64,
        subplot: (f64, f64, f64, f64),
    ) -> Frame {
        // GridSpec 1x1, same float ops
        let (left, right, bottom, top) = subplot;
        let (x0, x1) = (left, left + (right - left));
        let (y0, y1) = (top - (top - bottom), top);
        let (pw, ph) = (x1 - x0, y1 - y0);
        // Axes.apply_aspect(adjustable="box", anchor "C"), Bbox.shrunk_to_aspect
        let box_aspect = 1.0 * ((ylim.1 - ylim.0) / (xlim.1 - xlim.0));
        let fig_aspect = fig_in.1 / fig_in.0;
        let (mut w, mut h) = (pw, pw * box_aspect / fig_aspect);
        if h > ph {
            w = ph * fig_aspect / box_aspect;
            h = ph;
        }
        let ax0 = x0 + 0.5 * (pw - w);
        let ay0 = y0 + 0.5 * (ph - h);
        let (fw, fh) = (fig_in.0 * dpi, fig_in.1 * dpi);
        let ax_w = (ax0 + w) * fw - ax0 * fw;
        let ax_h = (ay0 + h) * fh - ay0 * fh;
        // bbox_inches="tight": inches, then the new canvas at dpi, int() truncation
        let width = ((ax_w / dpi) * dpi) as u32;
        let height = ((ax_h / dpi) * dpi) as u32;
        Frame {
            xlim,
            ylim,
            ax_w,
            ax_h,
            width,
            height,
        }
    }

    /// data (lon, lat) -> output px
    pub(crate) fn to_px(&self, lon: f64, lat: f64) -> (f64, f64) {
        (
            (lon - self.xlim.0) / (self.xlim.1 - self.xlim.0) * self.ax_w,
            (self.ylim.1 - lat) / (self.ylim.1 - self.ylim.0) * self.ax_h,
        )
    }

    /// output pixel centre -> data (lon, lat)
    pub(crate) fn to_data(&self, px: f64, py: f64) -> (f64, f64) {
        (
            self.xlim.0 + px / self.ax_w * (self.xlim.1 - self.xlim.0),
            self.ylim.1 - py / self.ax_h * (self.ylim.1 - self.ylim.0),
        )
    }
}

pub(crate) fn pt_to_px(pt: f64) -> f64 {
    pt * FIGURE_DPI / 72.0
}

/// maps._draw
fn draw(
    polygons: &[Vec<Ring>],
    rgba: Vec<u8>,
    shape: (usize, usize),
    t: Affine,
    classes: &[(&str, &str)],
    legend_title: &str,
    tile_cache: &Path,
) -> Result<Figure> {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in polygons.iter().flatten().flatten() {
        bounds = [
            bounds[0].min(p[0]),
            bounds[1].min(p[1]),
            bounds[2].max(p[0]),
            bounds[3].max(p[1]),
        ];
    }
    let frame = Frame::new(bounds);
    let mut pixmap = Pixmap::new(frame.width.max(1), frame.height.max(1))
        .ok_or_else(|| Error::new("ValueError", "empty figure"))?;
    let mut facts = Facts {
        xlim: frame.xlim,
        ylim: frame.ylim,
        axes_box: [0.0, 0.0, frame.ax_w, frame.ax_h],
        raster_shape: shape,
        raster_transform: [t.a, t.b, t.c, t.d, t.e, t.f],
        aoi_bounds: bounds,
        ..Facts::default()
    };

    // zorder 1: basemap (contextily), else a plain face with the axes frame drawn
    match basemap::basemap(&frame, tile_cache, &BASEMAPS) {
        Some(b) => {
            facts.zoom = Some(b.zoom);
            facts.provider_url = Some(b.provider_url);
            facts.tile_urls = b.urls;
            pixmap.data_mut().copy_from_slice(&b.rgba);
        }
        None => {
            let [r, g, b] = hex_rgb(NO_BASEMAP_COLOUR);
            pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        }
    }

    // zorder 2: imshow(rgba, extent, interpolation="nearest"): each output pixel centre samples
    // the cell under it
    let (rows, cols) = shape;
    let (left, top) = t.apply(0.0, 0.0);
    let (right, bottom) = t.apply(cols as f64, rows as f64);
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();
    for py in 0..h {
        for px in 0..w {
            let (lon, lat) = frame.to_data(px as f64 + 0.5, py as f64 + 0.5);
            let c = ((lon - left) / (right - left) * cols as f64).floor();
            let r = ((top - lat) / (top - bottom) * rows as f64).floor();
            if c < 0.0 || r < 0.0 || c >= cols as f64 || r >= rows as f64 {
                continue;
            }
            let i = (r as usize * cols + c as usize) * 4;
            if rgba[i + 3] == 255 {
                data[(py * w + px) * 4..(py * w + px) * 4 + 4].copy_from_slice(&rgba[i..i + 4]);
            }
        }
    }
    facts.raster_rgba = rgba;

    // zorder 3: aoi_4326.boundary.plot(color=AOI_OUTLINE, linewidth=2): LineCollection, butt
    // caps, round joins
    let mut pb = PathBuilder::new();
    for ring in polygons.iter().flatten() {
        for (k, p) in ring.iter().enumerate() {
            let (x, y) = frame.to_px(p[0], p[1]);
            if k == 0 {
                pb.move_to(x as f32, y as f32)
            } else {
                pb.line_to(x as f32, y as f32)
            }
        }
    }
    if let Some(path) = pb.finish() {
        let [r, g, b] = hex_rgb(AOI_OUTLINE);
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 255);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: pt_to_px(2.0) as f32,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    legend::draw_legend(&mut pixmap, classes, legend_title, &mut facts);
    Ok(Figure { pixmap, facts })
}
