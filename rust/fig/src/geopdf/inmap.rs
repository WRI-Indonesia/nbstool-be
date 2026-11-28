//! The GeoPDF "inmap attribution" layer: a transparent PNG the size of the basemap with a
//! matplotlib-scalebar ScaleBar (lower left), a geo_northarrow arrow (upper right) and a
//! Basemap(projection="lcc").shadedrelief() locator inset (lower right) with the zoom-10 extent
//! as a translucent red box.

use std::f64::consts::PI;
use std::path::Path;

use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use tiny_skia::{FillRule, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::Frame;
use crate::glyphs::Face;
use ab_glyph::Font as _;
use nbstool_geo::{Error, Result};

const DPI: f64 = 100.0;
/// rcParams font.size / legend.fontsize "medium": 10 pt
const FONT_PX: f64 = 10.0 * DPI / 72.0;

fn pt(v: f64) -> f64 {
    v * DPI / 72.0
}

fn gerr(e: gdal::errors::GdalError) -> Error {
    Error::new("CRSError", e.to_string())
}

fn transform(from: &str, to: &str, xs: &mut [f64], ys: &mut [f64]) -> Result<()> {
    let mut a = SpatialRef::from_definition(from).map_err(gerr)?;
    a.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut b = SpatialRef::from_definition(to).map_err(gerr)?;
    b.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    CoordTransform::new(&a, &b)
        .map_err(gerr)?
        .transform_coords(xs, ys, &mut [])
        .map_err(gerr)
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    p.anti_alias = true;
    p
}

fn polygon(
    pixmap: &mut Pixmap,
    pts: &[(f64, f64)],
    fill: Option<[u8; 4]>,
    stroke: Option<([u8; 4], f64)>,
) {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x as f32, *y as f32)
        } else {
            pb.line_to(*x as f32, *y as f32)
        }
    }
    pb.close();
    let Some(path) = pb.finish() else { return };
    if let Some(c) = fill {
        pixmap.fill_path(
            &path,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some((c, w)) = stroke {
        let s = Stroke {
            width: w as f32,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint(c), &s, Transform::identity(), None);
    }
}

/// matplotlib TextArea box of one line: (width, up, down) around the baseline
fn text_box(face: &Face, s: &str, em: f64) -> (f64, f64, f64) {
    let metrics = |s: &str| {
        let k = em / face.upem();
        let (mut y0, mut y1, mut w) = (0.0f64, 0.0f64, 0.0);
        for c in s.chars() {
            let id = face.font.glyph_id(c);
            if let Some(o) = face.font.outline(id) {
                let (a, b) = (o.bounds.min.y as f64 * k, o.bounds.max.y as f64 * k);
                y0 = y0.min(a.min(b));
                y1 = y1.max(a.max(b));
            }
            w += face.advance_em(c) * em;
        }
        (w, y1.round() + (-y0).round(), (-y0).round())
    };
    let (w, h, d) = metrics(s);
    let (_, lph, lpd) = metrics("lp");
    let (h, yd) = (h.max(lph), d.max(lpd));
    (w, (lph - lpd).max(h - yd), yd)
}

fn draw_text(pixmap: &mut Pixmap, face: &Face, s: &str, x: f64, baseline: f64, em: f64) {
    let mut pen = x;
    for c in s.chars() {
        face.draw_char(pixmap, c, pen, baseline, em, 1.0, 0.0, [0, 0, 0, 255]);
        pen += face.advance_em(c) * em;
    }
}

/// matplotlib_scalebar: `_calculate_best_length` for SI metres
fn best_length(length_data: f64, dx: f64) -> (f64, i64, &'static str) {
    const PREFERRED: [f64; 15] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 20.0, 25.0, 50.0, 75.0, 100.0, 125.0, 150.0, 200.0, 500.0, 750.0,
    ];
    const UNITS: [(&str, f64); 18] = [
        ("ym", 1e-24),
        ("zm", 1e-21),
        ("am", 1e-18),
        ("fm", 1e-15),
        ("pm", 1e-12),
        ("nm", 1e-9),
        ("um", 1e-6),
        ("mm", 1e-3),
        ("cm", 1e-2),
        ("dm", 1e-1),
        ("m", 1.0),
        ("km", 1e3),
        ("Mm", 1e6),
        ("Gm", 1e9),
        ("Tm", 1e12),
        ("Pm", 1e15),
        ("Em", 1e18),
        ("Zm", 1e21),
    ];
    let value = length_data * dx;
    // calculate_preferred: bisect_right over the sorted factors
    let index = UNITS.iter().filter(|(_, f)| *f <= value).count();
    let (newvalue, units) = if index > 0 {
        (value / UNITS[index - 1].1, UNITS[index - 1].0)
    } else {
        (value, "m")
    };
    let factor = value / newvalue;
    let mut i = PREFERRED.iter().filter(|p| **p < newvalue).count(); // bisect_left
    i = i.saturating_sub(1);
    let i = i.min(PREFERRED.len() - 1);
    (PREFERRED[i] * factor / dx, PREFERRED[i] as i64, units)
}

/// OSM wiki zoom maths of `calculate_boundaries` -> (lng_left, lat_top, lng_right, lat_bottom)
fn calculate_boundaries(
    lat: f64,
    lng: f64,
    zoom: i32,
    width: f64,
    height: f64,
) -> (f64, f64, f64, f64) {
    let c = 40075.0;
    let y = PI * lat / 180.0;
    let s = c * y.cos() / 2f64.powi(zoom + 8);
    let s_deg = s * y.cos() / 100.0;
    (
        lng - width / 2.0 * s_deg,
        lat + height / 2.0 * s_deg,
        lng + width / 2.0 * s_deg,
        lat - height / 2.0 * s_deg,
    )
}

/// Build the PNG. `centroid` = (lon, lat) of the polygon (OGR Centroid); `fig_cm_in` the figure
/// size the original passes (`total_base_* * cm`).
pub(crate) fn render(
    frame: &Frame,
    centroid: (f64, f64),
    fig_cm_in: (f64, f64),
    shadedrelief: &Path,
) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(frame.width.max(1), frame.height.max(1))
        .ok_or_else(|| Error::new("ValueError", "empty figure"))?;
    let missing = || Error::new("RuntimeError", "font");
    let face = Face::regular().ok_or_else(missing)?;
    let (xs, ys) = (frame.xlim.1 - frame.xlim.0, frame.ylim.1 - frame.ylim.0);
    let px_per_x = frame.ax_w / xs;
    let px_per_y = frame.ax_h / ys;

    // ---- ScaleBar(dx = distance of the two corners in EPSG:32619, "lower left", border_pad=1,
    // pad=0.5, label="Map Scale", scale_loc="right")
    let (mut cx, mut cy) = (
        vec![frame.xlim.0, frame.xlim.1],
        vec![frame.ylim.0, frame.ylim.1],
    );
    transform("EPSG:4326", "EPSG:32619", &mut cx, &mut cy)?;
    let dx = ((cx[1] - cx[0]).powi(2) + (cy[1] - cy[0]).powi(2)).sqrt();
    let (len_data, value, units) = best_length(xs * 0.2, dx);
    let (bar_w, bar_h) = (len_data * px_per_x, ys * 0.01 * px_per_y);
    let scale_text = format!("{value} {units}");
    let sep = pt(5.0);
    let (tw, tup, tdown) = text_box(&face, &scale_text, FONT_PX);
    let (lw, lup, ldown) = text_box(&face, "Map Scale", FONT_PX);
    let row_h = bar_h.max(tup + tdown);
    let row_w = bar_w + sep + tw;
    let (cw, ch) = (row_w.max(lw), lup + ldown + sep + row_h);
    let (pad, border) = (0.5 * FONT_PX, 1.0 * FONT_PX);
    let (box_w, box_h) = (cw + 2.0 * pad, ch + 2.0 * pad);
    let (bx0, by1) = (border, frame.ax_h - border);
    let by0 = by1 - box_h;
    if let Some(r) = Rect::from_xywh(bx0 as f32, by0 as f32, box_w as f32, box_h as f32) {
        pixmap.fill_rect(r, &paint([255, 255, 255, 255]), Transform::identity(), None);
    }
    let (cx0, cy0) = (bx0 + pad, by0 + pad);
    draw_text(
        &mut pixmap,
        &face,
        "Map Scale",
        cx0 + (cw - lw) / 2.0,
        cy0 + lup,
        FONT_PX,
    );
    let (rx0, ry0) = (cx0 + (cw - row_w) / 2.0, cy0 + lup + ldown + sep);
    if let Some(r) = Rect::from_xywh(
        rx0 as f32,
        (ry0 + (row_h - bar_h) / 2.0) as f32,
        bar_w as f32,
        bar_h as f32,
    ) {
        pixmap.fill_rect(r, &paint([0, 0, 0, 255]), Transform::identity(), None);
    }
    draw_text(
        &mut pixmap,
        &face,
        &scale_text,
        rx0 + bar_w + sep,
        ry0 + (row_h - (tup + tdown)) / 2.0 + tup,
        FONT_PX,
    );

    // ---- add_north_arrow(ax, scale=.25, xlim_pos=.98, ylim_pos=.94, color="#000", text_scaler=4,
    // text_yT=-2.5)
    let scale = ((xs.abs() + ys.abs()) / 2.0 / 45.0) * 0.25;
    let (xt, yt) = (frame.xlim.0 + xs * 0.98, frame.ylim.0 + ys * 0.94);
    let to_px = |(x, y): (f64, f64)| frame.to_px(xt + x * scale, yt + y * scale);
    let a: Vec<_> = [(0.0, 5.0), (0.0, 1.0), (2.0, 0.0)]
        .into_iter()
        .map(to_px)
        .collect();
    let b: Vec<_> = [(0.0, 5.0), (0.0, 1.0), (-2.0, 0.0)]
        .into_iter()
        .map(to_px)
        .collect();
    const N: &[f64] = &[
        3.248, 5.059, 3.248, 5.019, 3.260, 5.019, 3.316, 5.007, 3.339, 4.950, 3.339, 4.479, 3.315,
        4.425, 3.260, 4.414, 3.248, 4.414, 3.248, 4.374, 3.423, 4.374, 3.767, 4.904, 3.767, 4.479,
        3.743, 4.425, 3.688, 4.414, 3.676, 4.414, 3.676, 4.374, 3.917, 4.374, 3.917, 4.414, 3.904,
        4.414, 3.849, 4.426, 3.826, 4.483, 3.826, 5.059, 3.757, 5.059, 3.397, 4.506, 3.397, 4.950,
        3.421, 5.007, 3.476, 5.019, 3.489, 5.019, 3.489, 5.059,
    ];
    let n: Vec<(f64, f64)> = N.chunks(2).map(|p| (p[0], -p[1])).collect();
    let (nx0, nx1) = n
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.0), b.max(p.0))
        });
    let (ny0, ny1) = n
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.1), b.max(p.1))
        });
    let (cxn, cyn) = ((nx1 - nx0).abs() / 2.0 + nx0, (ny1 - ny0).abs() / 2.0 + ny0);
    let n_px: Vec<_> = n
        .iter()
        .map(|(x, y)| to_px(((x - cxn) * 4.0, (y - cyn) * 4.0 - 2.5)))
        .collect();
    let black = [0, 0, 0, 255];
    polygon(&mut pixmap, &a, None, Some((black, pt(2.0))));
    polygon(&mut pixmap, &b, Some(black), Some((black, pt(2.0))));
    polygon(&mut pixmap, &n_px, Some(black), None);

    // ---- inset_axes(width="20%", height="20%", loc="lower right"), borderpad 0.5 font size
    let (lon0, lat0) = centroid;
    let (w_px, h_px) = (fig_cm_in.0 * 100.0, fig_cm_in.1 * 100.0);
    let (ll, ut, lr, lb) = calculate_boundaries(lat0, lon0, 7, w_px, h_px);
    let lcc = format!(
        "+proj=lcc +lat_1={lat0} +lat_2={lat0} +lat_0={lat0} +lon_0={lon0} +R=6370997 +units=m +no_defs"
    );
    // Basemap(llcrnrlon=ll, urcrnrlon=lr, llcrnrlat=lb, urcrnrlat=ut)
    let (mut kx, mut ky) = (vec![ll, lr], vec![lb, ut]);
    transform("EPSG:4326", &lcc, &mut kx, &mut ky)?;
    let (mx0, my0, mx1, my1) = (kx[0], ky[0], kx[1], ky[1]);
    let bp = 0.5 * FONT_PX;
    let (bw, bh) = (0.2 * frame.ax_w, 0.2 * frame.ax_h);
    let (ix1, iy1) = (frame.ax_w - bp, frame.ax_h - bp);
    // Basemap: aspect "equal", anchor C
    let map_aspect = (my1 - my0) / (mx1 - mx0);
    let (iw, ih) = if bw * map_aspect <= bh {
        (bw, bw * map_aspect)
    } else {
        (bh / map_aspect, bh)
    };
    let (ix0, iy0) = (ix1 - bw + (bw - iw) / 2.0, iy1 - bh + (bh - ih) / 2.0);

    let relief = image::open(shadedrelief)
        .map_err(|e| Error::new("FileNotFoundError", e.to_string()))?
        .to_rgb8();
    let (rw, rh) = (relief.width() as usize, relief.height() as usize);
    let (c0, c1) = (
        ix0.floor().max(0.0) as usize,
        (ix0 + iw).ceil().min(frame.width as f64) as usize,
    );
    let (r0, r1) = (
        iy0.floor().max(0.0) as usize,
        (iy0 + ih).ceil().min(frame.height as f64) as usize,
    );
    let (mut gx, mut gy) = (Vec::new(), Vec::new());
    for r in r0..r1 {
        for c in c0..c1 {
            gx.push(mx0 + (c as f64 + 0.5 - ix0) / iw * (mx1 - mx0));
            gy.push(my1 - (r as f64 + 0.5 - iy0) / ih * (my1 - my0));
        }
    }
    transform(&lcc, "EPSG:4326", &mut gx, &mut gy)?;
    let pw = pixmap.width() as usize;
    let data = pixmap.data_mut();
    let mut k = 0;
    for r in r0..r1 {
        for c in c0..c1 {
            let (lon, lat) = (gx[k], gy[k]);
            k += 1;
            let (fx, fy) = (c as f64 + 0.5, r as f64 + 0.5);
            if fx < ix0 || fx > ix0 + iw || fy < iy0 || fy > iy0 + ih || !lon.is_finite() {
                continue;
            }
            // warpimage: pixel centres at -180 + (i + .5) * 360 / w, rows north to south
            let col = ((lon + 180.0) / 360.0 * rw as f64 - 0.5).clamp(0.0, (rw - 1) as f64);
            let row = ((90.0 - lat) / 180.0 * rh as f64 - 0.5).clamp(0.0, (rh - 1) as f64);
            let (a0, b0) = (col.floor() as usize, row.floor() as usize);
            let (a1, b1) = ((a0 + 1).min(rw - 1), (b0 + 1).min(rh - 1));
            let (tx, ty) = (col - a0 as f64, row - b0 as f64);
            let o = (r * pw + c) * 4;
            for ch in 0..3 {
                let v = |x: usize, y: usize| relief.get_pixel(x as u32, y as u32).0[ch] as f64;
                let val = v(a0, b0) * (1.0 - tx) * (1.0 - ty)
                    + v(a1, b0) * tx * (1.0 - ty)
                    + v(a0, b1) * (1.0 - tx) * ty
                    + v(a1, b1) * tx * ty;
                data[o + ch] = val.round() as u8;
            }
            data[o + 3] = 255;
        }
    }
    drop(relief);
    // zoom-10 extent, Polygon(facecolor="red", alpha=0.4), no edge, clipped to the inset
    let (zl, zt, zr, zb) = calculate_boundaries(lat0, lon0, 10, w_px, h_px);
    let (mut qx, mut qy) = (vec![zl, zl, zr, zr], vec![zb, zt, zt, zb]);
    transform("EPSG:4326", &lcc, &mut qx, &mut qy)?;
    let q: Vec<(f64, f64)> = qx
        .iter()
        .zip(&qy)
        .map(|(x, y)| {
            (
                ix0 + (x - mx0) / (mx1 - mx0) * iw,
                iy0 + (my1 - y) / (my1 - my0) * ih,
            )
        })
        .collect();
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in q.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x as f32, *y as f32)
        } else {
            pb.line_to(*x as f32, *y as f32)
        }
    }
    pb.close();
    let clip_rect = Rect::from_xywh(ix0 as f32, iy0 as f32, iw as f32, ih as f32);
    if let (Some(path), Some(clip_rect)) = (pb.finish(), clip_rect) {
        let mut mask = tiny_skia::Mask::new(pixmap.width(), pixmap.height());
        if let Some(m) = mask.as_mut() {
            m.fill_path(
                &PathBuilder::from_rect(clip_rect),
                FillRule::Winding,
                false,
                Transform::identity(),
            );
        }
        pixmap.fill_path(
            &path,
            &paint([255, 0, 0, 102]),
            FillRule::Winding,
            Transform::identity(),
            mask.as_ref(),
        );
        // the inset's spines: 0.8 pt black
        let s = Stroke {
            width: pt(0.8) as f32,
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &PathBuilder::from_rect(clip_rect),
            &paint(black),
            &s,
            Transform::identity(),
            None,
        );
    }
    pixmap
        .encode_png()
        .map_err(|e| Error::new("OSError", e.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn scalebar_length() {
        // 0.2 of the span is 5.4 km -> 5 km (bisect_left then one step down)
        let (len, v, u) = super::best_length(0.2, 5400.0 / 0.2);
        assert_eq!((v, u), (5, "km"));
        assert!((len - 5000.0 / 27000.0).abs() < 1e-12);
    }
}
