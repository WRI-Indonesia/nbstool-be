//! The OPC package as python-docx 1.1.2 loads and saves it: parts reachable through
//! relationships (depth first), XML parts of the registered types re-serialised by lxml,
//! everything else byte for byte; `[Content_Types].xml` and every `.rels` regenerated.

use crate::xml::{self, Element, R};
use std::io::{Cursor, Read, Write};

pub const RT_IMAGE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
pub const RT_HEADER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
pub const RT_FOOTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
pub const RT_OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
pub const RT_CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
pub const CT_CORE_PROPERTIES: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";

/// docx/__init__.py `PartFactory.part_type_for`: the content types loaded as XmlPart.
const XML_PART_TYPES: [&str; 7] = [
    CT_CORE_PROPERTIES,
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
];

/// docx/opc/spec.py `default_content_types`.
const DEFAULT_CONTENT_TYPES: [(&str, &str); 18] = [
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.presentationml.printerSettings",
    ),
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings",
    ),
    (
        "bin",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.printerSettings",
    ),
    ("bmp", "image/bmp"),
    ("emf", "image/x-emf"),
    ("fntdata", "application/x-fontdata"),
    ("gif", "image/gif"),
    ("jpe", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("rels", CT_RELS),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("wdp", "image/vnd.ms-photo"),
    ("wmf", "image/x-wmf"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("xml", "application/xml"),
];

#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    pub reltype: String,
    /// absolute partname for internal targets, the raw reference for external ones
    pub target: String,
    pub external: bool,
}

pub enum Content {
    Xml(Element),
    Bytes(Vec<u8>),
}

pub struct Part {
    pub name: String,
    pub content_type: String,
    pub content: Content,
    pub rels: Vec<Rel>,
    /// loaded through an image relationship (python-docx ImagePart)
    pub is_image: bool,
    /// unchanged bytes of the template entry: copied still compressed on save
    pub from_template: bool,
}

pub struct Package {
    pub rels: Vec<Rel>,
    pub parts: Vec<Part>,
}

impl Package {
    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    pub fn part_mut(&mut self, name: &str) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.name == name)
    }

    pub fn main_document(&self) -> R<String> {
        self.rels
            .iter()
            .find(|r| r.reltype == RT_OFFICE_DOCUMENT && !r.external)
            .map(|r| r.target.clone())
            .ok_or_else(|| "package has no main document".to_string())
    }
}

/// `Rels._next_rId`: the first free `rIdN` from 1.
pub fn next_rid(rels: &[Rel]) -> String {
    (1..=rels.len() + 1)
        .map(|n| format!("rId{n}"))
        .find(|id| !rels.iter().any(|r| r.id == *id))
        .unwrap_or_default()
}

fn zip_err(e: impl std::fmt::Display) -> String {
    format!("docx zip: {e}")
}

/// `posixpath.abspath(posixpath.join(base, ref))`.
fn resolve(base: &str, reference: &str) -> String {
    let joined = if reference.starts_with('/') {
        reference.to_string()
    } else {
        format!("{base}/{reference}")
    };
    let mut out: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

fn base_uri(partname: &str) -> &str {
    match partname.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &partname[..i],
    }
}

fn rels_name(partname: &str) -> String {
    if partname == "/" {
        return "/_rels/.rels".into();
    }
    let (dir, file) = partname.rsplit_once('/').unwrap_or(("", partname));
    format!("{dir}/_rels/{file}.rels")
}

/// `PackURI.relative_ref(baseURI)`.
fn relative_ref(target: &str, base: &str) -> String {
    if base == "/" {
        return target[1..].to_string();
    }
    let t: Vec<&str> = target[1..].split('/').collect();
    let b: Vec<&str> = base[1..].split('/').collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<&str> = vec![".."; b.len() - common];
    parts.extend(&t[common..]);
    parts.join("/")
}

fn ext(partname: &str) -> &str {
    let file = partname.rsplit('/').next().unwrap_or(partname);
    file.rsplit_once('.').map_or("", |(_, e)| e)
}

fn parse_rels(bytes: &[u8], source: &str) -> R<Vec<Rel>> {
    let root = xml::parse(std::str::from_utf8(bytes).map_err(zip_err)?, false)?;
    let base = base_uri(source);
    Ok(root
        .elements()
        .filter(|e| e.local() == "Relationship")
        .map(|e| {
            let external = e.attr("TargetMode") == Some("External");
            let raw = e.attr("Target").unwrap_or_default();
            Rel {
                id: e.attr("Id").unwrap_or_default().to_string(),
                reltype: e.attr("Type").unwrap_or_default().to_string(),
                target: if external {
                    raw.to_string()
                } else {
                    resolve(base, raw)
                },
                external,
            }
        })
        .collect())
}

pub fn load(docx: &[u8]) -> R<Package> {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx)).map_err(zip_err)?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name.trim_start_matches('/')).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    let ct = read("[Content_Types].xml").ok_or("no [Content_Types].xml")?;
    let ct = xml::parse(std::str::from_utf8(&ct).map_err(zip_err)?, false)?;
    let defaults: Vec<(String, String)> = ct
        .elements()
        .filter(|e| e.local() == "Default")
        .map(|e| {
            (
                e.attr("Extension").unwrap_or_default().to_lowercase(),
                e.attr("ContentType").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let overrides: Vec<(String, String)> = ct
        .elements()
        .filter(|e| e.local() == "Override")
        .map(|e| {
            (
                e.attr("PartName").unwrap_or_default().to_lowercase(),
                e.attr("ContentType").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let content_type = |name: &str| -> R<String> {
        let lower = name.to_lowercase();
        overrides
            .iter()
            .find(|(p, _)| *p == lower)
            .or_else(|| defaults.iter().find(|(e, _)| *e == ext(&lower)))
            .map(|(_, c)| c.clone())
            .ok_or_else(|| format!("no content type for {name}"))
    };

    let pkg_rels = parse_rels(&read("_rels/.rels").ok_or("no _rels/.rels")?, "/")?;
    // PackageReader._walk_phys_parts: depth first over the relationships, each part once
    let mut parts: Vec<Part> = Vec::new();
    let mut stack: Vec<std::vec::IntoIter<Rel>> = vec![pkg_rels.clone().into_iter()];
    while let Some(top) = stack.last_mut() {
        let Some(rel) = top.next() else {
            stack.pop();
            continue;
        };
        if rel.external || parts.iter().any(|p| p.name == rel.target) {
            continue;
        }
        let name = rel.target.clone();
        let blob = read(&name).ok_or_else(|| format!("missing part {name}"))?;
        let rels = match read(&rels_name(&name)) {
            Some(b) => parse_rels(&b, &name)?,
            None => Vec::new(),
        };
        let content_type = content_type(&name)?;
        // part_class_selector: an image relationship always loads as a binary ImagePart
        let content = if XML_PART_TYPES.contains(&content_type.as_str()) && rel.reltype != RT_IMAGE
        {
            Content::Xml(xml::parse(
                std::str::from_utf8(&blob).map_err(zip_err)?,
                false,
            )?)
        } else {
            Content::Bytes(blob)
        };
        stack.push(rels.clone().into_iter());
        let from_template = matches!(content, Content::Bytes(_));
        parts.push(Part {
            name,
            content_type,
            content,
            rels,
            is_image: rel.reltype == RT_IMAGE,
            from_template,
        });
    }
    Ok(Package {
        rels: pkg_rels,
        parts,
    })
}

fn rels_xml(rels: &[Rel], source: &str) -> Vec<u8> {
    let mut out = String::from(
        "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    let base = base_uri(source);
    for r in rels {
        let target = if r.external {
            r.target.clone()
        } else {
            relative_ref(&r.target, base)
        };
        out.push_str("<Relationship Id=\"");
        xml::escape_attr(&r.id, &mut out);
        out.push_str("\" Type=\"");
        xml::escape_attr(&r.reltype, &mut out);
        out.push_str("\" Target=\"");
        xml::escape_attr(&target, &mut out);
        out.push('"');
        if r.external {
            out.push_str(" TargetMode=\"External\"");
        }
        out.push_str("/>");
    }
    out.push_str("</Relationships>");
    out.into_bytes()
}

/// `OpcPackage.iter_parts`: depth first from the package relationships.
fn part_order(pkg: &Package) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    let mut stack: Vec<std::slice::Iter<Rel>> = vec![pkg.rels.iter()];
    while let Some(top) = stack.last_mut() {
        let Some(rel) = top.next() else {
            stack.pop();
            continue;
        };
        if rel.external {
            continue;
        }
        if let Some(i) = pkg.parts.iter().position(|p| p.name == rel.target)
            && !order.contains(&i)
        {
            order.push(i);
            stack.push(pkg.parts[i].rels.iter());
        }
    }
    order
}

/// `PackageWriter.write`. `template` is the package `pkg` was loaded from: its unchanged binary
/// entries (fonts, pictures) are copied without recompressing (the zip bytes differ from Python's
/// anyway; the entry contents do not).
pub fn save(pkg: &Package, template: &[u8]) -> R<Vec<u8>> {
    let mut src = zip::ZipArchive::new(Cursor::new(template)).map_err(zip_err)?;
    let order = part_order(pkg);
    // _ContentTypesItem: defaults sorted by extension, overrides sorted by partname
    let mut defaults: Vec<(String, String)> = vec![
        ("rels".into(), CT_RELS.into()),
        ("xml".into(), "application/xml".into()),
    ];
    let mut overrides: Vec<(String, String)> = Vec::new();
    for &i in &order {
        let p = &pkg.parts[i];
        let e = ext(&p.name);
        if DEFAULT_CONTENT_TYPES
            .iter()
            .any(|(x, c)| *x == e.to_lowercase() && *c == p.content_type)
        {
            // CaseInsensitiveDict: lower-cased keys, last content type wins
            let e = e.to_lowercase();
            match defaults.iter_mut().find(|(x, _)| *x == e) {
                Some(slot) => slot.1 = p.content_type.clone(),
                None => defaults.push((e, p.content_type.clone())),
            }
        } else {
            overrides.push((p.name.clone(), p.content_type.clone()));
        }
    }
    defaults.sort();
    overrides.sort();
    let mut ct = String::from(
        "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    );
    for (e, c) in &defaults {
        ct.push_str("<Default Extension=\"");
        xml::escape_attr(e, &mut ct);
        ct.push_str("\" ContentType=\"");
        xml::escape_attr(c, &mut ct);
        ct.push_str("\"/>");
    }
    for (p, c) in &overrides {
        ct.push_str("<Override PartName=\"");
        xml::escape_attr(p, &mut ct);
        ct.push_str("\" ContentType=\"");
        xml::escape_attr(c, &mut ct);
        ct.push_str("\"/>");
    }
    ct.push_str("</Types>");

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let put = |zip: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &[u8]| -> R<()> {
        zip.start_file(name.trim_start_matches('/'), opts)
            .map_err(zip_err)?;
        zip.write_all(data).map_err(zip_err)
    };
    put(&mut zip, "[Content_Types].xml", ct.as_bytes())?;
    put(&mut zip, "_rels/.rels", &rels_xml(&pkg.rels, "/"))?;
    for &i in &order {
        let p = &pkg.parts[i];
        let raw = if p.from_template {
            src.index_for_name(p.name.trim_start_matches('/'))
        } else {
            None
        };
        match (&p.content, raw) {
            (Content::Bytes(_), Some(i)) => {
                let entry = src.by_index_raw(i).map_err(zip_err)?;
                zip.raw_copy_file(entry).map_err(zip_err)?;
            }
            (Content::Xml(el), _) => put(&mut zip, &p.name, &xml::to_part_bytes(el))?,
            (Content::Bytes(b), None) => put(&mut zip, &p.name, b)?,
        }
        if !p.rels.is_empty() {
            put(&mut zip, &rels_name(&p.name), &rels_xml(&p.rels, &p.name))?;
        }
    }
    Ok(zip.finish().map_err(zip_err)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        assert_eq!(resolve("/word", "media/a.png"), "/word/media/a.png");
        assert_eq!(resolve("/word", "../customXml/x.xml"), "/customXml/x.xml");
        assert_eq!(relative_ref("/word/media/a.png", "/word"), "media/a.png");
        assert_eq!(relative_ref("/media/a.png", "/word"), "../media/a.png");
        assert_eq!(relative_ref("/word/document.xml", "/"), "word/document.xml");
        assert_eq!(
            rels_name("/word/document.xml"),
            "/word/_rels/document.xml.rels"
        );
    }
}
