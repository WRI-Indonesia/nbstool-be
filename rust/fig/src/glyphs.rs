//! Text as filled glyph outlines (DejaVu Sans, embedded) with an arbitrary horizontal scale and
//! slant: what poppler does when it substitutes a non-embedded Helvetica, and plain DejaVu text
//! for the matplotlib pieces of the GeoPDF.

use ab_glyph::{Font, FontRef, OutlineCurve};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

pub(crate) static REGULAR: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
pub(crate) static BOLD: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");

pub(crate) struct Face {
    pub font: FontRef<'static>,
}

impl Face {
    pub fn regular() -> Option<Face> {
        Some(Face {
            font: FontRef::try_from_slice(REGULAR).ok()?,
        })
    }

    pub fn bold() -> Option<Face> {
        Some(Face {
            font: FontRef::try_from_slice(BOLD).ok()?,
        })
    }

    pub fn upem(&self) -> f64 {
        self.font.units_per_em().unwrap_or(2048.0) as f64
    }

    /// advance of `c` in em units
    pub fn advance_em(&self, c: char) -> f64 {
        self.font.h_advance_unscaled(self.font.glyph_id(c)) as f64 / self.upem()
    }

    /// Fill `c` with its origin at (x, baseline) px, `em` px per em, horizontal scale `sx`,
    /// slant `skew` (dx per dy upwards), colour `rgba`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_char(
        &self,
        pixmap: &mut Pixmap,
        c: char,
        x: f64,
        baseline: f64,
        em: f64,
        sx: f64,
        skew: f64,
        rgba: [u8; 4],
    ) {
        let Some(outline) = self.font.outline(self.font.glyph_id(c)) else {
            return;
        };
        let k = em / self.upem();
        // font units are y-up; pixels y-down
        let map = |p: ab_glyph::Point| {
            let (u, v) = (p.x as f64 * k, p.y as f64 * k);
            ((x + u * sx + v * skew) as f32, (baseline - v) as f32)
        };
        let mut pb = PathBuilder::new();
        let mut last: Option<(f32, f32)> = None;
        for curve in &outline.curves {
            let start = match curve {
                OutlineCurve::Line(a, _)
                | OutlineCurve::Quad(a, _, _)
                | OutlineCurve::Cubic(a, _, _, _) => map(*a),
            };
            if last != Some(start) {
                if last.is_some() {
                    pb.close();
                }
                pb.move_to(start.0, start.1);
            }
            let end = match curve {
                OutlineCurve::Line(_, b) => {
                    let b = map(*b);
                    pb.line_to(b.0, b.1);
                    b
                }
                OutlineCurve::Quad(_, c1, b) => {
                    let (c1, b) = (map(*c1), map(*b));
                    pb.quad_to(c1.0, c1.1, b.0, b.1);
                    b
                }
                OutlineCurve::Cubic(_, c1, c2, b) => {
                    let (c1, c2, b) = (map(*c1), map(*c2), map(*b));
                    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, b.0, b.1);
                    b
                }
            };
            last = Some(end);
        }
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
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
}
