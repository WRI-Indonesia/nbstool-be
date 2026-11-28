//! InlineImage on python-docx 1.1.2: `StoryPart.new_pic_inline` (image part by SHA-1, relationship,
//! `scaled_dimensions`, `next_id`) and `CT_Inline.new_pic_inline` pretty-printed by lxml.

use crate::Figure;
use crate::opc::{self, Content, Package, Part, Rel};
use crate::xml::{Element, Node, R};
use sha1::{Digest, Sha1};

#[derive(Debug)]
pub struct Images {
    /// every ImagePart: (partname, sha1)
    parts: Vec<(String, [u8; 20])>,
    added: Vec<(String, Vec<u8>)>,
    doc_rels: Vec<Rel>,
    shape_id: u64,
    inserted: usize,
}

impl Images {
    pub fn new(pkg: &Package, doc_name: &str, doc_root: &Element) -> Self {
        let parts = pkg
            .parts
            .iter()
            .filter(|p| p.is_image)
            .filter_map(|p| match &p.content {
                Content::Bytes(b) => Some((p.name.clone(), Sha1::digest(b).into())),
                Content::Xml(_) => None,
            })
            .collect();
        let doc_rels = pkg
            .part(doc_name)
            .map(|p| p.rels.clone())
            .unwrap_or_default();
        Images {
            parts,
            added: Vec::new(),
            doc_rels,
            shape_id: next_id(doc_root),
            inserted: 0,
        }
    }

    pub fn inserted(&self) -> usize {
        self.inserted
    }

    /// `InlineImage._insert_image` minus the run wrapper: the `<wp:inline>` XML.
    pub fn insert(&mut self, fig: &Figure) -> R<String> {
        let png = Png::parse(&fig.png)?;
        let sha: [u8; 20] = Sha1::digest(&fig.png).into();
        let partname = match self.parts.iter().find(|(_, s)| *s == sha) {
            Some((name, _)) => name.clone(),
            None => {
                let name = self.next_image_partname("png");
                self.parts.push((name.clone(), sha));
                self.added.push((name.clone(), fig.png.clone()));
                name
            }
        };
        let rid = match self
            .doc_rels
            .iter()
            .find(|r| r.reltype == opc::RT_IMAGE && !r.external && r.target == partname)
        {
            Some(r) => r.id.clone(),
            None => {
                let id = opc::next_rid(&self.doc_rels);
                self.doc_rels.push(Rel {
                    id: id.clone(),
                    reltype: opc::RT_IMAGE.into(),
                    target: partname,
                    external: false,
                });
                id
            }
        };
        let (cx, cy) = png.scaled(fig.width_emu)?;
        self.inserted += 1;
        let id = self.shape_id;
        let mut name = String::new();
        crate::xml::escape_attr(&fig.filename, &mut name);
        Ok(format!(
            "<wp:inline xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\n  <wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\n  <wp:docPr id=\"{id}\" name=\"Picture {id}\"/>\n  <wp:cNvGraphicFramePr>\n    <a:graphicFrameLocks noChangeAspect=\"1\"/>\n  </wp:cNvGraphicFramePr>\n  <a:graphic>\n    <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\n      <pic:pic>\n        <pic:nvPicPr>\n          <pic:cNvPr id=\"0\" name=\"{name}\"/>\n          <pic:cNvPicPr/>\n        </pic:nvPicPr>\n        <pic:blipFill>\n          <a:blip r:embed=\"{rid}\"/>\n          <a:stretch>\n            <a:fillRect/>\n          </a:stretch>\n        </pic:blipFill>\n        <pic:spPr>\n          <a:xfrm>\n            <a:off x=\"0\" y=\"0\"/>\n            <a:ext cx=\"{cx}\" cy=\"{cy}\"/>\n          </a:xfrm>\n          <a:prstGeom prst=\"rect\"/>\n        </pic:spPr>\n      </pic:pic>\n    </a:graphicData>\n  </a:graphic>\n</wp:inline>\n"
        ))
    }

    /// `ImageParts._next_image_partname`.
    fn next_image_partname(&self, ext: &str) -> String {
        let used: Vec<u64> = self
            .parts
            .iter()
            .filter_map(|(n, _)| partname_idx(n))
            .collect();
        let n = (1..=self.parts.len() as u64)
            .find(|n| !used.contains(n))
            .unwrap_or(self.parts.len() as u64 + 1);
        format!("/word/media/image{n}.{ext}")
    }

    pub fn apply(self, pkg: &mut Package, doc_name: &str) -> R<()> {
        for (name, png) in self.added {
            pkg.parts.push(Part {
                name,
                content_type: "image/png".into(),
                content: Content::Bytes(png),
                rels: Vec::new(),
                is_image: true,
                from_template: false,
            });
        }
        pkg.part_mut(doc_name)
            .ok_or("main document part missing")?
            .rels = self.doc_rels;
        Ok(())
    }
}

/// `PackURI.idx`: `([a-zA-Z]+)([1-9][0-9]*)?` matched at the start of the extension-less filename.
fn partname_idx(name: &str) -> Option<u64> {
    let file = name.rsplit('/').next()?;
    let stem = match file.rfind('.') {
        Some(0) | None => file,
        Some(i) => &file[..i],
    };
    let letters = stem
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(stem.len());
    if letters == 0 {
        return None;
    }
    let rest = &stem[letters..];
    if !rest.starts_with(|c: char| ('1'..='9').contains(&c)) {
        return None;
    }
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..digits].parse().ok()
}

/// `StoryPart.next_id`: one more than the largest all-digit `@id` (no namespace) in the document.
fn next_id(root: &Element) -> u64 {
    fn walk(e: &Element, max: &mut Option<u64>) {
        for (k, v) in &e.attrs {
            if k == "id"
                && !v.is_empty()
                && v.chars().all(|c| c.is_ascii_digit())
                && let Ok(n) = v.parse::<u64>()
            {
                *max = Some(max.map_or(n, |m| m.max(n)));
            }
        }
        for c in &e.children {
            if let Node::Elem(c) = c {
                walk(c, max);
            }
        }
    }
    let mut max = None;
    walk(root, &mut max);
    max.map_or(1, |m| m + 1)
}

/// docx/image/png.py: IHDR size and pHYs dpi (72 when absent or not per metre).
struct Png {
    px_w: u32,
    px_h: u32,
    dpi_x: u32,
    dpi_y: u32,
}

impl Png {
    fn parse(b: &[u8]) -> R<Png> {
        if !b.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("figure is not a PNG image".into());
        }
        let be = |i: usize| -> Option<u32> {
            Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
        };
        let (mut size, mut phys) = (None, None);
        let mut i = 8;
        while let (Some(len), Some(kind)) = (be(i), b.get(i + 4..i + 8)) {
            let data = i + 8;
            match kind {
                b"IHDR" if size.is_none() => size = Some((be(data), be(data + 4))),
                b"pHYs" if phys.is_none() => {
                    phys = Some((be(data), be(data + 4), b.get(data + 8).copied()))
                }
                b"IEND" => break,
                _ => {}
            }
            i = data + len as usize + 4;
        }
        let (Some((Some(px_w), Some(px_h))), dpi) = (size, phys) else {
            return Err("PNG has no IHDR".into());
        };
        let dpi = |ppu: Option<u32>| match (dpi, ppu) {
            (Some((_, _, Some(1))), Some(p)) if p != 0 => {
                (p as f64 * 0.0254).round_ties_even() as u32
            }
            _ => 72,
        };
        let (x, y) = (dpi(phys.and_then(|p| p.0)), dpi(phys.and_then(|p| p.1)));
        Ok(Png {
            px_w,
            px_h,
            dpi_x: x,
            dpi_y: y,
        })
    }

    /// `Image.scaled_dimensions(width, None)`.
    fn scaled(&self, width: i64) -> R<(i64, i64)> {
        if self.dpi_x == 0 || self.dpi_y == 0 {
            return Err("float division by zero".into());
        }
        let native_w = (self.px_w as f64 / self.dpi_x as f64 * 914400.0) as i64;
        let native_h = (self.px_h as f64 / self.dpi_y as f64 * 914400.0) as i64;
        if native_w == 0 {
            return Err("float division by zero".into());
        }
        let factor = width as f64 / native_w as f64;
        Ok((width, (native_h as f64 * factor).round_ties_even() as i64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idx() {
        assert_eq!(partname_idx("/word/media/image12.png"), Some(12));
        assert_eq!(partname_idx("/word/media/image.png"), None);
        assert_eq!(partname_idx("/word/media/image0.png"), None);
    }
}
