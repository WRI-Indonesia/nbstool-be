//! `ax.legend(handles=Patch..., title, loc="upper left", fancybox, framealpha=1, edgecolor="none",
//! borderpad=1, labelspacing=0.9, handlelength=1.2, handleheight=1.2, fontsize=11,
//! title_fontsize=12)`, bold title, left-aligned: matplotlib 3.8.4's Legend/offsetbox packing with
//! DejaVu Sans (matplotlib's default font, embedded). Text metrics are FreeType's unhinted ink
//! box (matplotlib hints them, so boxes differ by under a pixel).

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont, point};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::{Facts, hex_rgb, pt_to_px};

static REGULAR: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
static BOLD: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");

struct Text<'a> {
    font: FontRef<'a>,
    /// px per em
    em: f64,
}

/// (width, height, descent) of the ink box, as `RendererAgg.get_text_width_height_descent`
struct Metrics {
    w: f64,
    h: f64,
    d: f64,
}

impl<'a> Text<'a> {
    fn new(bytes: &'a [u8], pt: f64) -> Option<Self> {
        Some(Text {
            font: FontRef::try_from_slice(bytes).ok()?,
            em: pt_to_px(pt),
        })
    }

    fn per_unit(&self) -> f64 {
        self.em / self.font.units_per_em().unwrap_or(2048.0) as f64
    }

    /// pen x of each glyph (FT2Font::set_text: advance + legacy kerning)
    fn layout(&self, s: &str) -> Vec<(GlyphId, f64)> {
        let k = self.per_unit();
        let mut pen = 0.0;
        let mut prev: Option<GlyphId> = None;
        let mut out = Vec::new();
        for ch in s.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(p) = prev {
                pen += self.font.kern_unscaled(p, id) as f64 * k;
            }
            out.push((id, pen));
            pen += self.font.h_advance_unscaled(id) as f64 * k;
            prev = Some(id);
        }
        out
    }

    /// FT2Font::set_text + get_width_height/get_descent: width = the pen advance (kerned),
    /// height and descent from the ink box with each edge rounded to whole pixels (matplotlib's
    /// autohinter snaps them; it sometimes rounds up, so heights can be 1 px short)
    fn metrics(&self, s: &str) -> Metrics {
        let k = self.per_unit();
        let (mut y0, mut y1) = (0.0f64, 0.0f64);
        let mut end = 0.0;
        for (id, pen) in self.layout(s) {
            if let Some(o) = self.font.outline(id) {
                let (a, b) = (o.bounds.min.y as f64 * k, o.bounds.max.y as f64 * k);
                y0 = y0.min(a.min(b));
                y1 = y1.max(a.max(b));
            }
            end = pen + self.font.h_advance_unscaled(id) as f64 * k;
        }
        let (up, down) = (y1.round(), (-y0).round());
        Metrics {
            w: end,
            h: up + down,
            d: down,
        }
    }

    /// TextArea.get_bbox for one line: (width, up = h_d, down = descent) around the baseline
    fn text_area(&self, s: &str) -> (f64, f64, f64) {
        let m = self.metrics(s);
        let lp = self.metrics("lp");
        let (h, yd) = (m.h.max(lp.h), m.d.max(lp.d));
        let h_d = (lp.h - lp.d).max(h - yd);
        (m.w, h_d, yd)
    }

    /// RendererAgg.draw_text: glyphs with the pen at (x, baseline), black
    fn draw(&self, pixmap: &mut Pixmap, s: &str, x: f64, baseline: f64) {
        let scale = PxScale::from(
            (self.em * self.font.height_unscaled() as f64
                / self.font.units_per_em().unwrap_or(2048.0) as f64) as f32,
        );
        let sf = self.font.as_scaled(scale);
        let (w, h) = (pixmap.width() as i64, pixmap.height() as i64);
        let data = pixmap.data_mut();
        for (id, pen) in self.layout(s) {
            let glyph =
                id.with_scale_and_position(sf.scale(), point((x + pen) as f32, baseline as f32));
            let Some(og) = self.font.outline_glyph(glyph) else {
                continue;
            };
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                if px < 0 || py < 0 || px >= w || py >= h || cov <= 0.0 {
                    return;
                }
                let i = ((py * w + px) * 4) as usize;
                let a = cov.min(1.0);
                for c in &mut data[i..i + 3] {
                    *c = (*c as f32 * (1.0 - a)).round() as u8;
                }
            });
        }
    }
}

fn fill_rect(pixmap: &mut Pixmap, r: [f64; 4], colour: &str) {
    let [cr, cg, cb] = hex_rgb(colour);
    let mut paint = Paint::default();
    paint.set_color_rgba8(cr, cg, cb, 255);
    paint.anti_alias = true;
    if let Some(rect) = Rect::from_ltrb(r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// FancyBboxPatch "round,pad=0,rounding_size=0.2" with mutation_scale = fontsize in px
fn fill_round_rect(pixmap: &mut Pixmap, r: [f64; 4], radius: f64) {
    let [x0, y0, x1, y1] = r.map(|v| v as f32);
    let d = (radius as f32).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + d, y0);
    pb.line_to(x1 - d, y0);
    pb.quad_to(x1, y0, x1, y0 + d);
    pb.line_to(x1, y1 - d);
    pb.quad_to(x1, y1, x1 - d, y1);
    pb.line_to(x0 + d, y1);
    pb.quad_to(x0, y1, x0, y1 - d);
    pb.line_to(x0, y0 + d);
    pb.quad_to(x0, y0, x0 + d, y0);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

pub(crate) fn draw_legend(
    pixmap: &mut Pixmap,
    classes: &[(&str, &str)],
    title: &str,
    facts: &mut Facts,
) {
    let (Some(label_font), Some(title_font)) = (Text::new(REGULAR, 11.0), Text::new(BOLD, 12.0))
    else {
        return;
    };
    let fs = pt_to_px(11.0);
    let (pad, sep, borderaxespad) = (1.0 * fs, 0.9 * fs, 0.5 * fs);
    let (handle_w, handle_up, handle_down) = (1.2 * fs, 0.85 * fs, 0.175 * fs);
    let textpad = 0.8 * fs;

    let (tw, t_up, t_down) = title_font.text_area(title);
    let items: Vec<_> = classes
        .iter()
        .map(|(l, c)| (*l, *c, label_font.text_area(l)))
        .collect();
    let column_w = items
        .iter()
        .map(|(_, _, (w, ..))| handle_w + textpad + w)
        .fold(0.0, f64::max);
    let column_h: f64 = items
        .iter()
        .map(|(_, _, (_, up, down))| up.max(handle_up) + down.max(handle_down))
        .sum::<f64>()
        + sep * (items.len().saturating_sub(1)) as f64;
    let (fx, fy) = (borderaxespad, borderaxespad);
    let frame = [
        fx,
        fy,
        fx + 2.0 * pad + tw.max(column_w),
        fy + 2.0 * pad + t_up + t_down + sep + column_h,
    ];
    fill_round_rect(pixmap, frame, 0.2 * fs);
    facts.legend_frame = frame;

    let (x, top) = (fx + pad, fy + pad);
    facts.legend_title = [x, top, x + tw, top + t_up + t_down];
    title_font.draw(pixmap, title, x, top + t_up);

    let mut y = top + t_up + t_down + sep;
    for (label, colour, (w, up, down)) in items {
        let baseline = y + up.max(handle_up);
        let handle = [
            x,
            baseline - handle_up,
            x + handle_w,
            baseline + handle_down,
        ];
        fill_rect(pixmap, handle, colour);
        let tx = x + handle_w + textpad;
        label_font.draw(pixmap, label, tx, baseline);
        facts.legend_entries.push((
            label.to_string(),
            [tx, baseline - up, tx + w, baseline + down],
            handle,
            colour.to_string(),
        ));
        y = baseline + down.max(handle_down) + sep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_match_matplotlib() {
        // RendererAgg.get_text_width_height_descent at 200 dpi (matplotlib 3.8.4)
        let t = Text::new(REGULAR, 11.0).unwrap();
        for (s, w, h, d) in [
            ("lp", 27.875, 29.0, 6.0),
            ("Remaining", 162.0, 29.0, 6.0),
            ("Other", 86.75, 23.0, 0.0),
        ] {
            let m = t.metrics(s);
            assert!(
                (m.w - w).abs() < 0.5 && m.h == h && m.d == d,
                "{s}: {} {} {}",
                m.w,
                m.h,
                m.d
            );
        }
        let b = Text::new(BOLD, 12.0).unwrap();
        let m = b.metrics("Disturbance class");
        assert!(
            (m.w - 332.625).abs() < 1.0 && (m.h - 26.0).abs() <= 1.0,
            "{} {}",
            m.w,
            m.h
        );
    }
}
