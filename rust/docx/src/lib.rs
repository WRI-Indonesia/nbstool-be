//! `DocxTemplate(template).render(context, autoescape=True); save()` of docxtpl 0.18.0 on
//! python-docx 1.1.2 / lxml 5.3.0 / Jinja2 3.1.4, in Rust. See README.md for every docxtpl
//! behaviour reproduced and the known differences.

mod image;
mod opc;
mod patch;
mod py;
mod tables;
pub mod xml;

use minijinja::value::{Object, Value};
use minijinja::{AutoEscape, Environment, Error, ErrorKind};
use opc::{Content, Package, Rel};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use xml::{Element, Node, R};

pub use patch::patch_xml;

/// An `InlineImage(tpl, path, width=...)` value: the picture bytes (PNG), the file name python-docx
/// records (`os.path.basename(path)`) and the width in EMU (`Mm(165)` = 5,940,000).
#[derive(Clone, Debug)]
pub struct Figure {
    pub filename: String,
    pub png: Vec<u8>,
    pub width_emu: i64,
}

/// `docx.shared.Mm(mm)`.
pub fn mm(mm: f64) -> i64 {
    (mm * 36000.0) as i64
}

pub struct Options<'a> {
    /// context variables holding an InlineImage
    pub figures: &'a [(&'a str, Figure)],
    /// `dcterms:modified` when python-docx has to create docProps/core.xml (UTC,
    /// `%Y-%m-%dT%H:%M:%SZ`); the v3 templates have none, so this is the generation time.
    pub modified_utc: &'a str,
}

/// Render `template` (a .docx) with `context` (a JSON object). A `tags` object in the context
/// also defines the v3 `t(tag_text)` lookup of `v3/context.py`.
pub fn render(template: &[u8], context: &serde_json::Value, opts: &Options) -> R<Vec<u8>> {
    let mut pkg = opc::load(template)?;
    let doc_name = pkg.main_document()?;
    let doc_root = match &pkg
        .part(&doc_name)
        .ok_or("main document part missing")?
        .content
    {
        Content::Xml(el) => el,
        Content::Bytes(_) => return Err("main document is not XML".into()),
    };
    let images = Arc::new(Mutex::new(image::Images::new(&pkg, &doc_name, doc_root)));
    let env = environment(context)?;
    let ctx = context_value(context, opts.figures, &images)?;

    // Body
    let (body_index, body_xml) = body_string(doc_root)?;
    let rendered = render_part(&env, &patch::patch_xml(&body_xml)?, &ctx)?;
    let mut body = xml::parse(&rendered, true)?;
    tables::fix_tables(&mut body)?;
    tables::fix_docpr_ids(&mut body);
    xml::strip_redundant_ns(&mut body, &doc_root.ns.clone());
    match &mut pkg
        .part_mut(&doc_name)
        .ok_or("main document part missing")?
        .content
    {
        Content::Xml(root) => root.children[body_index] = Node::Elem(body),
        Content::Bytes(_) => return Err("main document is not XML".into()),
    }

    // Headers, then footers: same patch + render, re-parsed as python-docx XmlParts
    let doc_rels = pkg
        .part(&doc_name)
        .map(|p| p.rels.clone())
        .unwrap_or_default();
    for reltype in [opc::RT_HEADER, opc::RT_FOOTER] {
        for rel in doc_rels
            .iter()
            .filter(|r| r.reltype == reltype && !r.external)
        {
            let Some(part) = pkg.part_mut(&rel.target) else {
                continue;
            };
            let src = match &part.content {
                Content::Xml(el) => {
                    let mut s = String::new();
                    xml::write_element(el, &mut s);
                    s
                }
                Content::Bytes(b) if b.is_empty() => continue,
                Content::Bytes(b) => {
                    let el = xml::parse(std::str::from_utf8(b).map_err(|e| e.to_string())?, false)?;
                    let mut s = String::new();
                    xml::write_element(&el, &mut s);
                    s
                }
            };
            let before = image_count(&images);
            let out = render_part(&env, &patch::patch_xml(&src)?, &ctx)?;
            if image_count(&images) != before {
                // python-docx would relate the picture to the header part; no template does this
                return Err("an InlineImage in a header or footer is not supported".into());
            }
            part.content = Content::Xml(xml::parse(&out, false)?);
        }
    }

    render_properties(&mut pkg, &env, &ctx, opts.modified_utc)?;

    drop(ctx); // the figure values hold the image state
    let images = Arc::try_unwrap(images)
        .map_err(|_| "image state still shared".to_string())?
        .into_inner()
        .map_err(|_| "image state poisoned".to_string())?;
    images.apply(&mut pkg, &doc_name)?;
    opc::save(&pkg, template)
}

/// The patched body XML docxtpl hands to Jinja (for the proof: equals the Python string).
pub fn patched_body(template: &[u8]) -> R<String> {
    let pkg = opc::load(template)?;
    let doc_name = pkg.main_document()?;
    let Content::Xml(root) = &pkg
        .part(&doc_name)
        .ok_or("main document part missing")?
        .content
    else {
        return Err("main document is not XML".into());
    };
    patch::patch_xml(&body_string(root)?.1)
}

fn image_count(images: &Arc<Mutex<image::Images>>) -> usize {
    images.lock().map(|i| i.inserted()).unwrap_or(0)
}

/// `xml_to_string(self.docx._element.body)`: the body serialised alone, carrying the root's
/// namespace declarations.
fn body_string(root: &Element) -> R<(usize, String)> {
    let index = root
        .children
        .iter()
        .position(|n| matches!(n, Node::Elem(e) if e.local() == "body"))
        .ok_or("document has no body")?;
    let Node::Elem(body) = &root.children[index] else {
        return Err("document has no body".into());
    };
    let mut out = String::new();
    xml::write_subtree_with_scope(body, &root.ns, &mut out);
    Ok((index, out))
}

/// `render_xml_part`.
fn render_part(env: &Environment, src: &str, ctx: &Value) -> R<String> {
    static P_OPEN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static P_BACK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let open =
        P_OPEN.get_or_init(|| regex::Regex::new(r"<w:p([ >])").unwrap_or_else(|e| panic!("{e}")));
    let back =
        P_BACK.get_or_init(|| regex::Regex::new(r"\n<w:p([ >])").unwrap_or_else(|e| panic!("{e}")));
    let src = open.replace_all(src, "\n<w:p${1}");
    let dst = env
        .render_str(&src, ctx)
        .map_err(|e| format!("template error: {e:#}"))?;
    let dst = back.replace_all(&dst, "<w:p${1}");
    let dst = dst
        .replace("{_{", "{{")
        .replace("}_}", "}}")
        .replace("{_%", "{%")
        .replace("%_}", "%}");
    Ok(patch::resolve_listing(&dst))
}

/// MarkupSafe `escape`.
fn escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// `Environment(autoescape=True)`: every value printed through `str()` and MarkupSafe escaping.
fn environment(context: &serde_json::Value) -> R<Environment<'static>> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.set_formatter(|out, state, value| {
        if let Some(fig) = value.downcast_object_ref::<FigureValue>() {
            let xml = fig
                .insert()
                .map_err(|e| Error::new(ErrorKind::InvalidOperation, e))?;
            return out.write_str(&xml).map_err(Error::from);
        }
        let s = py::py_str(value);
        if value.is_safe() || matches!(state.auto_escape(), AutoEscape::None) {
            return out.write_str(&s).map_err(Error::from);
        }
        let mut esc = String::with_capacity(s.len());
        escape_into(&s, &mut esc);
        out.write_str(&esc).map_err(Error::from)
    });
    if let Some(tags) = context.get("tags").and_then(|t| t.as_object()) {
        // v3/context.py `t`: str(value) of the filled tag, else the literal bracket text
        let lookup: std::collections::HashMap<String, Option<String>> = tags
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    (!v.is_null()).then(|| py::py_str(&Value::from_serialize(v))),
                )
            })
            .collect();
        env.add_function("t", move |tag: String| -> String {
            match lookup.get(&py::norm(&tag)) {
                Some(Some(v)) => v.clone(),
                _ => format!("[{}]", py::html_unescape(&tag)),
            }
        });
    }
    // Jinja2's filters where minijinja's differ: `float` falls back to 0.0 instead of failing,
    // `sum` is Python 3.12's builtin sum (compensated float adds)
    env.add_filter(
        "float",
        |v: Value, default: Option<Value>| -> Result<Value, Error> {
            if v.is_undefined() {
                return Err(Error::new(
                    ErrorKind::UndefinedError,
                    "float() of an undefined value",
                ));
            }
            Ok(py::py_float(&v)
                .map(Value::from)
                .unwrap_or_else(|| default.unwrap_or(Value::from(0.0))))
        },
    );
    env.add_filter("sum", |values: Value| -> Result<Value, Error> {
        let items: Vec<Value> = values.try_iter()?.collect();
        py::py_sum(&items).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "unsupported operand type(s) for +",
            )
        })
    });
    Ok(env)
}

fn context_value(
    context: &serde_json::Value,
    figures: &[(&str, Figure)],
    images: &Arc<Mutex<image::Images>>,
) -> R<Value> {
    let obj = context
        .as_object()
        .ok_or("the context must be a JSON object")?;
    let mut map: BTreeMap<String, Value> = obj
        .iter()
        .filter(|(k, _)| *k != "t")
        .map(|(k, v)| (k.clone(), Value::from_serialize(v)))
        .collect();
    for (name, fig) in figures {
        map.insert(
            name.to_string(),
            Value::from_object(FigureValue {
                figure: fig.clone(),
                images: images.clone(),
            }),
        );
    }
    Ok(Value::from(map))
}

/// InlineImage: inserting it (`__html__`) adds the picture part and relationship.
#[derive(Debug)]
struct FigureValue {
    figure: Figure,
    images: Arc<Mutex<image::Images>>,
}

impl Object for FigureValue {}

impl FigureValue {
    fn insert(&self) -> R<String> {
        let pic = self
            .images
            .lock()
            .map_err(|_| "image state poisoned".to_string())?
            .insert(&self.figure)?;
        Ok(format!(
            "</w:t></w:r><w:r><w:drawing>{pic}</w:drawing></w:r><w:r><w:t xml:space=\"preserve\">"
        ))
    }
}

const NS_CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";

/// `render_properties` (+ python-docx creating a default core-properties part when the template
/// has none, + v3 `_fix_core_properties`, which puts the cp elements in the core namespace).
fn render_properties(pkg: &mut Package, env: &Environment, ctx: &Value, modified: &str) -> R<()> {
    let core = pkg
        .rels
        .iter()
        .find(|r| r.reltype == opc::RT_CORE_PROPERTIES && !r.external)
        .map(|r| r.target.clone());
    let core = match core {
        Some(name) => name,
        None => {
            let default = format!(
                "<cp:coreProperties xmlns:cp=\"{NS_CP}\" xmlns:dc=\"{NS_DC}\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>Word Document</dc:title><cp:lastModifiedBy>python-docx</cp:lastModifiedBy><cp:revision>1</cp:revision><dcterms:modified xsi:type=\"dcterms:W3CDTF\">{modified}</dcterms:modified></cp:coreProperties>"
            );
            let name = "/docProps/core.xml".to_string();
            pkg.parts.push(opc::Part {
                name: name.clone(),
                content_type: opc::CT_CORE_PROPERTIES.into(),
                content: Content::Xml(xml::parse(&default, false)?),
                rels: Vec::new(),
                is_image: false,
                from_template: false,
            });
            let id = opc::next_rid(&pkg.rels);
            pkg.rels.push(Rel {
                id,
                reltype: opc::RT_CORE_PROPERTIES.into(),
                target: name.clone(),
                external: false,
            });
            name
        }
    };
    let part = pkg.part_mut(&core).ok_or("core properties part missing")?;
    let Content::Xml(root) = &mut part.content else {
        return Err("core properties are not XML".into());
    };
    let dc = root
        .ns
        .iter()
        .find(|(_, u)| u == NS_DC)
        .map(|(p, _)| p.clone())
        .unwrap_or_else(|| "dc".into());
    // author, comments, identifier, language, subject, title
    for local in [
        "creator",
        "description",
        "identifier",
        "language",
        "subject",
        "title",
    ] {
        let name = format!("{dc}:{local}");
        let initial = root
            .elements()
            .find(|e| e.name == name)
            .map(text_of)
            .unwrap_or_default();
        let value = env
            .render_str(&initial, ctx)
            .map_err(|e| format!("template error: {e:#}"))?;
        if value.chars().count() > 255 {
            return Err("exceeded 255 char limit for property, got:\n\n'%s'".replace("%s", &value));
        }
        let existing = root.elements_mut().find(|e| e.name == name);
        match existing {
            Some(e) => {
                // lxml `.text = value`: replaces the leading text, keeps child elements
                e.children.retain(|n| !matches!(n, Node::Text(_)));
                e.children.insert(0, Node::Text(value));
            }
            None => {
                let mut e = Element::new(&name);
                e.children.push(Node::Text(value));
                root.children.push(Node::Elem(e));
            }
        }
    }
    Ok(())
}

fn text_of(e: &Element) -> String {
    match e.children.first() {
        Some(Node::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_str(src: &str, ctx: serde_json::Value) -> Result<String, String> {
        let env = environment(&ctx)?;
        env.render_str(src, Value::from_serialize(&ctx))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn jinja2_semantics() {
        let ctx = serde_json::json!({"x": null, "xs": [0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1],
                                     "s": ["1_000", " 2 ", "abc", null, 0, ""], "q": "a&b<c>\"d'e"});
        // attribute of None is Undefined, printed as ""
        assert_eq!(render_str("[{{ x.y }}]", ctx.clone()).unwrap(), "[]");
        // Python 3.12 sum is compensated: 1.0, not 0.9999999999999999
        assert_eq!(render_str("{{ xs | sum }}", ctx.clone()).unwrap(), "1.0");
        // Jinja2 float: unparsable -> 0.0
        assert_eq!(
            render_str("{{ s | map('float') | list | join(',') }}", ctx.clone()).unwrap(),
            "1000.0,2.0,0.0,0.0,0.0,0.0"
        );
        assert_eq!(
            render_str("{{ s | select | list | count }}", ctx.clone()).unwrap(),
            "3"
        );
        // MarkupSafe escaping
        assert_eq!(
            render_str("{{ q }}", ctx.clone()).unwrap(),
            "a&amp;b&lt;c&gt;&#34;d&#39;e"
        );
        assert_eq!(
            render_str("{{ 2023 + 1 - 1 }} {{ 1.5 }} {{ none }} {{ true }}", ctx).unwrap(),
            "2023 1.5 None True"
        );
    }
}
