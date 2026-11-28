//! `application/utils/geopdf/legend.py`: the legend page reportlab draws and poppler rasterises
//! at 200 dpi (pdf2image default). Layout uses the Helvetica AFM widths reportlab measures with;
//! glyphs are DejaVu Sans scaled the way poppler substitutes a non-embedded Helvetica (the
//! production image has only DejaVu fonts).
//!
//! The original writes its measuring PDF and the final PDF into one BytesIO; poppler recovers the
//! second (the final) document, so only the final page is drawn here.

use std::collections::HashMap;

use serde_json::Value;
use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::glyphs::Face;
use nbstool_geo::{Error, Result};

static HELVETICA_AFM: &str = include_str!("../../fonts/phvr8a.afm");
static HELVETICA_BOLD_AFM: &str = include_str!("../../fonts/phvb8a.afm");

/// WinAnsiEncoding names for 0xA0..=0xFF (Latin-1 range), reportlab's encoding for the standard
/// fonts.
const LATIN1: [&str; 96] = [
    "space",
    "exclamdown",
    "cent",
    "sterling",
    "currency",
    "yen",
    "brokenbar",
    "section",
    "dieresis",
    "copyright",
    "ordfeminine",
    "guillemotleft",
    "logicalnot",
    "hyphen",
    "registered",
    "macron",
    "degree",
    "plusminus",
    "twosuperior",
    "threesuperior",
    "acute",
    "mu",
    "paragraph",
    "periodcentered",
    "cedilla",
    "onesuperior",
    "ordmasculine",
    "guillemotright",
    "onequarter",
    "onehalf",
    "threequarters",
    "questiondown",
    "Agrave",
    "Aacute",
    "Acircumflex",
    "Atilde",
    "Adieresis",
    "Aring",
    "AE",
    "Ccedilla",
    "Egrave",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Igrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Eth",
    "Ntilde",
    "Ograve",
    "Oacute",
    "Ocircumflex",
    "Otilde",
    "Odieresis",
    "multiply",
    "Oslash",
    "Ugrave",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Yacute",
    "Thorn",
    "germandbls",
    "agrave",
    "aacute",
    "acircumflex",
    "atilde",
    "adieresis",
    "aring",
    "ae",
    "ccedilla",
    "egrave",
    "eacute",
    "ecircumflex",
    "edieresis",
    "igrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "eth",
    "ntilde",
    "ograve",
    "oacute",
    "ocircumflex",
    "otilde",
    "odieresis",
    "divide",
    "oslash",
    "ugrave",
    "uacute",
    "ucircumflex",
    "udieresis",
    "yacute",
    "thorn",
    "ydieresis",
];

struct Afm {
    by_code: HashMap<u32, f64>,
    by_name: HashMap<String, f64>,
}

impl Afm {
    fn parse(text: &str) -> Afm {
        let (mut by_code, mut by_name) = (HashMap::new(), HashMap::new());
        for line in text.lines().filter(|l| l.starts_with("C ")) {
            let (mut code, mut wx, mut name) = (-1i64, 0.0, "");
            for part in line.split(';').map(str::trim) {
                let mut it = part.split_whitespace();
                match (it.next(), it.next()) {
                    (Some("C"), Some(v)) => code = v.parse().unwrap_or(-1),
                    (Some("WX"), Some(v)) => wx = v.parse().unwrap_or(0.0),
                    (Some("N"), Some(v)) => name = v,
                    _ => {}
                }
            }
            if code >= 0 {
                by_code.insert(code as u32, wx);
            }
            by_name.insert(name.to_string(), wx);
        }
        Afm { by_code, by_name }
    }

    /// width in 1/1000 em of `c` under WinAnsiEncoding
    fn width(&self, c: char) -> f64 {
        let u = c as u32;
        let name = match u {
            39 => Some("quotesingle"),
            96 => Some("grave"),
            0xA0..=0xFF => Some(LATIN1[(u - 0xA0) as usize]),
            _ => None,
        };
        match name {
            Some(n) => self.by_name.get(n).copied(),
            None => self.by_code.get(&u).copied(),
        }
        .unwrap_or(0.0)
    }

    /// `pdfmetrics.stringWidth(s, font, size)`
    fn string_width(&self, s: &str, size: f64) -> f64 {
        s.chars().map(|c| self.width(c)).sum::<f64>() * size / 1000.0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Regular,
    Bold,
    Oblique,
    BoldOblique,
}

/// `get_legend`: layerName -> items (name, fill, type), first-seen layer order
pub(crate) struct Item {
    name: String,
    fill: String,
    kind: String,
}

pub(crate) fn get_legend(documents: &[Value]) -> Vec<(String, Vec<Item>)> {
    let mut out: Vec<(String, Vec<Item>)> = Vec::new();
    let s = |v: &Value| v.as_str().map(str::to_string);
    for doc in documents {
        for layer in doc["Legend"].as_array().into_iter().flatten() {
            let layer_name = s(&layer["layerName"]).unwrap_or_default();
            let mut push = |item: Item| match out.iter_mut().find(|(k, _)| *k == layer_name) {
                Some((_, v)) => v.push(item),
                None => out.push((layer_name.clone(), vec![item])),
            };
            for rule in layer["rules"].as_array().into_iter().flatten() {
                let rule_name = s(&rule["name"])
                    .filter(|n| !n.is_empty())
                    .or_else(|| s(&rule["title"]));
                for sym in rule["symbolizers"].as_array().into_iter().flatten() {
                    let Some((kind, body)) = sym.as_object().and_then(|o| o.iter().next()) else {
                        continue;
                    };
                    match kind.as_str() {
                        "Polygon" => {
                            let Some(fill) = s(&body["fill"]).filter(|f| !f.is_empty()) else {
                                continue;
                            };
                            push(Item {
                                name: rule_name.clone().unwrap_or_default(),
                                fill,
                                kind: "value".into(),
                            });
                        }
                        "Raster" => {
                            let cm = &body["colormap"];
                            for e in cm["entries"].as_array().into_iter().flatten() {
                                push(Item {
                                    name: s(&e["label"]).unwrap_or_default(),
                                    fill: s(&e["color"]).unwrap_or_default(),
                                    kind: s(&cm["type"]).unwrap_or_default(),
                                });
                            }
                        }
                        "Line" => push(Item {
                            name: rule_name.clone().unwrap_or_default(),
                            fill: s(&body["stroke"]).unwrap_or_default(),
                            kind: "value".into(),
                        }),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

/// (name, group_name, parent_name) of a layer, `layers_name_reference`
pub type NameRef = (String, String, String);

struct Page {
    pixmap: Option<Pixmap>,
    height_pt: f64,
    regular: Afm,
    bold: Afm,
    face: Face,
    face_bold: Face,
}

const PX_PER_PT: f64 = 200.0 / 72.0;
const PAGE_WIDTH: f64 = 842.0;

fn hex(fill: &str) -> Result<[f64; 3]> {
    let bad = || {
        Error::new(
            "ValueError",
            format!("invalid literal for int() with base 16: '{fill}'"),
        )
    };
    let part = |i: usize| {
        fill.get(i..i + 2)
            .ok_or_else(bad)
            .and_then(|h| u8::from_str_radix(h, 16).map_err(|_| bad()))
    };
    // rgb_scale divides by 256
    Ok([
        part(1)? as f64 / 256.0,
        part(3)? as f64 / 256.0,
        part(5)? as f64 / 256.0,
    ])
}

fn rgba(c: [f64; 3]) -> [u8; 4] {
    [
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
        255,
    ]
}

impl Page {
    fn afm(&self, style: Style) -> &Afm {
        if matches!(style, Style::Bold | Style::BoldOblique) {
            &self.bold
        } else {
            &self.regular
        }
    }

    fn width(&self, style: Style, size: f64, s: &str) -> f64 {
        self.afm(style).string_width(s, size)
    }

    fn y(&self, y_pt: f64) -> f64 {
        (self.height_pt - y_pt) * PX_PER_PT
    }

    /// `add_text`: one line at baseline (x, y) pt
    fn text(&mut self, style: Style, size: f64, x: f64, y: f64, s: &str) {
        let baseline = self.y(y);
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        let bold = matches!(style, Style::Bold | Style::BoldOblique);
        let (face, afm) = if bold {
            (&self.face_bold, &self.bold)
        } else {
            (&self.face, &self.regular)
        };
        // poppler: a substituted font is narrowed by width('m') ratio when under 0.9
        let ratio = afm.width('m') / 1000.0 / face.advance_em('m');
        let sx = if ratio < 0.9 { ratio } else { 1.0 };
        let skew = if matches!(style, Style::Oblique | Style::BoldOblique) {
            0.2126
        } else {
            0.0
        }; // 12 degrees
        let em = size * PX_PER_PT;
        let mut pen = x * PX_PER_PT;
        for c in s.chars() {
            face.draw_char(pixmap, c, pen, baseline, em, sx, skew, [0, 0, 0, 255]);
            pen += afm.width(c) / 1000.0 * em;
        }
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, colour: [u8; 4]) {
        let (top, bottom) = (self.y(y + h), self.y(y));
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(colour[0], colour[1], colour[2], colour[3]);
        // the gradient's 500 sub-pixel stripes: anti-aliased seams would wash the ramp out
        paint.anti_alias = false;
        if let Some(r) = Rect::from_ltrb(
            (x * PX_PER_PT) as f32,
            top as f32,
            ((x + w) * PX_PER_PT) as f32,
            bottom as f32,
        ) {
            pixmap.fill_rect(r, &paint, Transform::identity(), None);
        }
    }

    /// `canvas.rect(x, y, w, h)`: stroked (1 pt black), not filled
    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (top, bottom) = (self.y(y + h), self.y(y));
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        let mut pb = PathBuilder::new();
        if let Some(r) = Rect::from_ltrb(
            (x * PX_PER_PT) as f32,
            top as f32,
            ((x + w) * PX_PER_PT) as f32,
            bottom.max(top + 0.01) as f32,
        ) {
            pb.push_rect(r);
        }
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(0, 0, 0, 255);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: PX_PER_PT as f32,
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// `canvas.circle(cx, cy, r, stroke=1, fill=1)`
    fn circle(&mut self, cx: f64, cy: f64, r: f64, fill: [u8; 4]) {
        let cy = self.y(cy);
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        let Some(path) =
            PathBuilder::from_circle((cx * PX_PER_PT) as f32, cy as f32, (r * PX_PER_PT) as f32)
        else {
            return;
        };
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color_rgba8(fill[0], fill[1], fill[2], fill[3]);
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        paint.set_color_rgba8(0, 0, 0, 255);
        let stroke = Stroke {
            width: PX_PER_PT as f32,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// `stroke_legend`: returns (max_x, total_y); draws only when the page has a pixmap
    fn stroke_legend(
        &mut self,
        name: &str,
        name_ref: Option<&NameRef>,
        legend: &[Item],
        init_x: f64,
        init_y: f64,
    ) -> Result<(f64, f64)> {
        let page_height = self.height_pt;
        let init_y = page_height - init_y;
        let mut max_x = init_x;
        let (x, mut y) = (init_x, init_y - 13.0);
        max_x = f64::max(max_x, self.width(Style::Bold, 13.0, name));
        self.text(Style::Bold, 13.0, x, y, name);
        y -= 13.0;
        let mut label = name_ref.map(|r| r.1.clone()).unwrap_or_default();
        if let Some(r) = name_ref.filter(|r| !label.is_empty() && !r.2.is_empty()) {
            label += &format!(" - {}", r.2);
        }
        max_x = f64::max(max_x, self.width(Style::Regular, 9.0, &label));
        self.text(Style::Regular, 9.0, x, y, &label);
        y -= 5.0;

        let x_text = init_x + 16.0;
        let x_val = x_text - 12.5;
        let (font_size, h_label, circle_size) = (12.0, 16.0, 5.0);
        let mut gcolor = Vec::new();
        let mut is_ramp = false;
        let y_first_item = y;
        for item in legend {
            let fill = if item.fill.is_empty() {
                "#ffffff"
            } else {
                item.fill.as_str()
            };
            let colour = hex(fill)?;
            gcolor.push(colour);
            y -= h_label;
            let label = item.name.replace('\u{2070}', "");
            max_x = f64::max(max_x, self.width(Style::Regular, font_size, &label));
            self.text(Style::Regular, font_size, x_text, y, &label);
            if item.kind == "value" || item.kind == "values" {
                self.circle(
                    x_val + circle_size / 2.0,
                    y + font_size / 3.0,
                    circle_size,
                    rgba(colour),
                );
            } else if item.kind == "ramp" {
                is_ramp = true;
            }
        }
        if is_ramp {
            let n = legend.len() as f64;
            let (bx, by, bw, bh) = (
                x_val - 2.0,
                y_first_item - h_label * n - 1.0,
                circle_size + 5.0,
                h_label * n - 5.0,
            );
            gcolor.reverse();
            // draw_vertical_multi_gradient(steps=500)
            let segments = gcolor.len().saturating_sub(1);
            if segments == 0 {
                return Err(Error::new(
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                ));
            }
            let steps = 500usize;
            let per = steps / segments;
            for i in 0..steps {
                let (mut seg, mut t) = (i / per, (i % per) as f64 / per as f64);
                if seg >= segments {
                    seg = segments - 1;
                    t = 1.0;
                }
                let (a, b) = (gcolor[seg], gcolor[seg + 1]);
                let c = [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ];
                let sh = bh / steps as f64;
                self.fill_rect(bx, by + i as f64 * sh, bw, sh, rgba(c));
            }
            self.stroke_rect(bx, by, bw, bh);
        }
        Ok((max_x, page_height - y))
    }
}

/// `draw_legend(list_json, layers_name_reference)` -> (PNG of the final page, page height in pt)
pub(crate) fn draw_legend(
    documents: &[Value],
    names: &HashMap<String, NameRef>,
    now_utc: &str,
) -> Result<(Vec<u8>, i64)> {
    let legend_layers = get_legend(documents);
    let missing = || Error::new("RuntimeError", "font");
    let mut page = Page {
        pixmap: None,
        height_pt: 595.0,
        regular: Afm::parse(HELVETICA_AFM),
        bold: Afm::parse(HELVETICA_BOLD_AFM),
        face: Face::regular().ok_or_else(missing)?,
        face_bold: Face::bold().ok_or_else(missing)?,
    };
    let display = |key: &str| {
        names
            .get(key)
            .map(|r| r.0.clone())
            .unwrap_or_else(|| key.to_string())
    };

    // measuring pass on the 842 x 595 page
    let (mut max_x_all, mut details) = (0.0f64, Vec::new());
    for (key, legend) in &legend_layers {
        let (max_x, total_y) =
            page.stroke_legend(&display(key), names.get(key), legend, 9.0, 0.0)?;
        let max_x = max_x + 10.0;
        max_x_all = max_x_all.max(max_x);
        details.push((key.clone(), max_x, total_y));
    }
    let (padding, minmax_x) = (30.0, 30.0);
    if max_x_all == 0.0 {
        return Err(Error::new("ZeroDivisionError", "float division by zero"));
    }
    let n_columns = ((PAGE_WIDTH - minmax_x * 2.0) / max_x_all).floor() as usize;
    if n_columns == 0 {
        return Err(Error::new("ZeroDivisionError", "division by zero"));
    }
    let column_width = PAGE_WIDTH / n_columns as f64;
    let mut stack = vec![0.0f64; n_columns];
    details.sort_by(|a, b| a.2.total_cmp(&b.2)); // sorted(): stable
    let mut positions = Vec::new();
    for (i, (key, _, total_y)) in details.iter().enumerate() {
        let col = i % n_columns;
        positions.push((
            key.clone(),
            col as f64 * column_width + minmax_x,
            stack[col],
        ));
        stack[col] += total_y + padding;
    }
    let total_height_used = stack.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let add_height = 55.0;
    let page_height = total_height_used + padding + add_height + 10.0;

    // final page, rasterised like pdftoppm -r 200 (ceil of the page size)
    let (w, h) = (
        (PAGE_WIDTH * PX_PER_PT).ceil() as u32,
        (page_height * PX_PER_PT).ceil() as u32,
    );
    let mut pixmap =
        Pixmap::new(w, h).ok_or_else(|| Error::new("ValueError", "legend page size"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    page.pixmap = Some(pixmap);
    page.height_pt = page_height;

    let mut y_bot = padding + 10.0 - add_height;
    let mut y = page_height - 10.0 - 16.0;
    let x = minmax_x;
    page.text(Style::BoldOblique, 16.0, x, y, "Legend");
    y_bot -= 16.0;
    y -= 16.0;
    page.stroke_rect(x, y, PAGE_WIDTH - minmax_x * 2.0, 0.1);
    for (key, px, py) in &positions {
        let legend = &legend_layers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[]);
        page.stroke_legend(
            &display(key),
            names.get(key),
            legend,
            *px,
            py + padding - 5.0 - y_bot,
        )?;
    }
    let mut y = page_height - (total_height_used + add_height - 10.0);
    page.stroke_rect(x, y, PAGE_WIDTH - minmax_x * 2.0, 0.1);
    y -= 10.0;
    page.text(
        Style::Oblique,
        6.0,
        x,
        y,
        &format!("{now_utc} UTC - https://nbstool.scenecoalition.org/interactive-map"),
    );

    let png = page
        .pixmap
        .take()
        .map(|p| p.encode_png())
        .transpose()
        .map_err(|e| Error::new("OSError", e.to_string()))?
        .unwrap_or_default();
    Ok((png, page_height as i64))
}
