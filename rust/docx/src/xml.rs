//! A small XML tree with libxml2's parse rules and lxml's serialisation (lxml 5.3.0, libxml2
//! 2.12), enough for WordprocessingML parts. Names are kept as written (`w:p`); namespace
//! declarations are kept per element, in source order, like libxml2's `nsDef` list.

use std::fmt::Write as _;

pub type R<T> = Result<T, String>;

#[derive(Clone, Debug)]
pub enum Node {
    Elem(Element),
    Text(String),
    Comment(String),
    Pi(String),
}

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    /// `(prefix, uri)`, prefix "" for the default namespace.
    pub ns: Vec<(String, String)>,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Element {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// lxml `el.set(name, value)`: replaces in place, else appends.
    pub fn set_attr(&mut self, name: &str, value: String) {
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = value,
            None => self.attrs.push((name.to_string(), value)),
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children
            .iter()
            .filter_map(|n| if let Node::Elem(e) = n { Some(e) } else { None })
    }

    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children
            .iter_mut()
            .filter_map(|n| if let Node::Elem(e) = n { Some(e) } else { None })
    }

    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    pub fn prefix(&self) -> &str {
        self.name.split_once(':').map_or("", |(p, _)| p)
    }
}

/// libxml2's parser. `keep_blanks = false` is python-docx's `remove_blank_text=True`.
pub fn parse(src: &str, keep_blanks: bool) -> R<Element> {
    // End-of-line handling (XML 1.0 2.11) before anything else.
    let src: std::borrow::Cow<str> = if src.contains('\r') {
        src.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        src.into()
    };
    let mut p = Parser {
        s: src.as_bytes(),
        i: 0,
        keep_blanks,
        space: vec![false],
    };
    p.skip_prolog()?;
    let root = p.element()?;
    p.skip_misc();
    if p.i != p.s.len() {
        return Err(p.err("content after the root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    keep_blanks: bool,
    /// xml:space="preserve" in scope, per open element.
    space: Vec<bool>,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("XML parse error at byte {}: {what}", self.i)
    }

    fn starts(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    fn text_until(&mut self, end: &str) -> R<&str> {
        let rest = &self.s[self.i..];
        let n = find(rest, end.as_bytes()).ok_or_else(|| self.err(&format!("missing {end}")))?;
        let out = std::str::from_utf8(&rest[..n]).map_err(|_| self.err("invalid UTF-8"))?;
        self.i += n + end.len();
        Ok(out)
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn skip_misc(&mut self) {
        loop {
            self.skip_ws();
            if self.starts("<!--") {
                if self.text_until("-->").is_err() {
                    return;
                }
            } else if self.starts("<?") {
                if self.text_until("?>").is_err() {
                    return;
                }
            } else {
                return;
            }
        }
    }

    fn skip_prolog(&mut self) -> R<()> {
        if self.starts("\u{feff}") {
            self.i += 3;
        }
        self.skip_misc();
        if self.starts("<!DOCTYPE") {
            return Err(self.err("DOCTYPE is not supported"));
        }
        Ok(())
    }

    fn name(&mut self) -> R<String> {
        let start = self.i;
        while self.i < self.s.len()
            && !matches!(
                self.s[self.i],
                b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>' | b'='
            )
        {
            self.i += 1;
        }
        if start == self.i {
            return Err(self.err("expected a name"));
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn element(&mut self) -> R<Element> {
        if !self.starts("<") {
            return Err(self.err("expected an element"));
        }
        self.i += 1;
        let mut el = Element::new(&self.name()?);
        let mut space = *self.space.last().unwrap_or(&false);
        loop {
            self.skip_ws();
            if self.starts("/>") {
                self.i += 2;
                return Ok(el);
            }
            if self.starts(">") {
                self.i += 1;
                break;
            }
            let key = self.name()?;
            self.skip_ws();
            if !self.starts("=") {
                return Err(self.err("expected ="));
            }
            self.i += 1;
            self.skip_ws();
            let q = *self
                .s
                .get(self.i)
                .ok_or_else(|| self.err("eof in attribute"))?;
            if q != b'"' && q != b'\'' {
                return Err(self.err("expected a quote"));
            }
            self.i += 1;
            let raw = self.text_until(if q == b'"' { "\"" } else { "'" })?;
            if raw.contains('<') {
                return Err(self.err("< in attribute value"));
            }
            // attribute-value normalisation: literal whitespace becomes a space, refs do not
            let value = unescape(&raw.replace(['\n', '\t'], " ")).map_err(|e| self.err(&e))?;
            if key == "xmlns" {
                el.ns.push((String::new(), value));
            } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                el.ns.push((prefix.to_string(), value));
            } else {
                if key == "xml:space" {
                    space = value == "preserve";
                }
                el.attrs.push((key, value));
            }
        }
        self.space.push(space);
        loop {
            if self.i >= self.s.len() {
                return Err(self.err("unclosed element"));
            }
            if self.starts("</") {
                self.i += 2;
                let name = self.name()?;
                if name != el.name {
                    return Err(self.err(&format!("mismatched end tag {name} for {}", el.name)));
                }
                self.skip_ws();
                if !self.starts(">") {
                    return Err(self.err("expected >"));
                }
                self.i += 1;
                self.space.pop();
                return Ok(el);
            }
            if self.starts("<!--") {
                self.i += 4;
                let c = self.text_until("-->")?.to_string();
                el.children.push(Node::Comment(c));
            } else if self.starts("<![CDATA[") {
                // lxml's default parser has strip_cdata=True: CDATA is plain text
                self.i += 9;
                let t = self.text_until("]]>")?.to_string();
                push_text(&mut el, &t);
            } else if self.starts("<?") {
                self.i += 2;
                let pi = self.text_until("?>")?.to_string();
                el.children.push(Node::Pi(pi));
            } else if self.starts("<") {
                let child = self.element()?;
                el.children.push(Node::Elem(child));
            } else {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'<' {
                    self.i += 1;
                }
                let raw = std::str::from_utf8(&self.s[start..self.i])
                    .map_err(|_| self.err("invalid UTF-8"))?;
                if !self.keep_blanks && self.is_blank(raw, &el) {
                    continue;
                }
                let text = unescape(raw).map_err(|e| self.err(&e))?;
                push_text(&mut el, &text);
            }
        }
    }

    /// libxml2 `areBlanks` with keepBlanks = 0 and no DTD.
    fn is_blank(&self, raw: &str, el: &Element) -> bool {
        if !raw
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            return false;
        }
        if *self.space.last().unwrap_or(&false) || self.i >= self.s.len() {
            return false;
        }
        if el.children.is_empty() && self.starts("</") {
            return false;
        }
        let is_text = |n: Option<&Node>| matches!(n, Some(Node::Text(_)));
        !(is_text(el.children.last()) || is_text(el.children.first()))
    }
}

fn push_text(el: &mut Element, t: &str) {
    if t.is_empty() {
        return;
    }
    if let Some(Node::Text(prev)) = el.children.last_mut() {
        prev.push_str(t);
    } else {
        el.children.push(Node::Text(t.to_string()));
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Predefined entities and character references.
pub fn unescape(s: &str) -> Result<String, String> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').ok_or("unterminated entity reference")?;
        let ent = &tail[..end];
        let ch = match ent {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(h) = ent.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = ent.strip_prefix('#') {
                    d.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32)
                    .ok_or_else(|| format!("unknown entity &{ent};"))?
            }
        };
        out.push(ch);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// lxml `etree.tostring(el, encoding="UTF-8", standalone=True)`, i.e. python-docx's
/// `serialize_part_xml`.
pub fn to_part_bytes(root: &Element) -> Vec<u8> {
    let mut out = String::from("<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n");
    write_element(root, &mut out);
    out.into_bytes()
}

pub fn write_element(el: &Element, out: &mut String) {
    write_with_ns(el, &el.ns, out);
}

fn write_with_ns(el: &Element, ns: &[(String, String)], out: &mut String) {
    out.push('<');
    out.push_str(&el.name);
    for (p, uri) in ns {
        out.push_str(if p.is_empty() { " xmlns" } else { " xmlns:" });
        out.push_str(p);
        out.push_str("=\"");
        escape_attr(uri, out);
        out.push('"');
    }
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape_attr(v, out);
        out.push('"');
    }
    if el.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in &el.children {
        match c {
            Node::Elem(e) => write_element(e, out),
            Node::Text(t) => escape_text(t, out),
            Node::Comment(c) => {
                let _ = write!(out, "<!--{c}-->");
            }
            Node::Pi(p) => {
                let _ = write!(out, "<?{p}?>");
            }
        }
    }
    out.push_str("</");
    out.push_str(&el.name);
    out.push('>');
}

/// lxml `tostring(el, encoding="unicode")` of a NON-root element: `xmlCopyNode(el, 2)` keeps the
/// element's own declarations and adds its own namespace's, then `_copyParentNamespaces` adds
/// every other in-scope declaration of the ancestors (`ancestors_ns`, nearest element first).
pub fn write_subtree_with_scope(el: &Element, ancestors_ns: &[(String, String)], out: &mut String) {
    let mut decls = el.ns.clone();
    let own = el.prefix();
    if !decls.iter().any(|(p, _)| p == own)
        && let Some(d) = ancestors_ns.iter().find(|(p, _)| p == own)
    {
        decls.push(d.clone());
    }
    for d in ancestors_ns {
        if !decls.iter().any(|(p, _)| *p == d.0) {
            decls.push(d.clone());
        }
    }
    write_with_ns(el, &decls, out);
}

/// lxml's `moveNodeToDocument` (`_stripRedundantNamespaceDeclarations` + `_fixCNs`) over the
/// whole moved subtree, `root_ns` being the new parent's declarations: a declaration whose URI
/// is already declared by an ancestor (`xmlSearchNsByHref`: nearest element first, in
/// declaration order, the prefix not redeclared in between) is dropped, and the names that used
/// it take that ancestor's prefix, which can differ (`xmlns:wp14="...wordml"` under a root
/// `xmlns:w14="...wordml"` turns `wp14:textId` into `w14:textId`).
// shortcut: a URI found only as a default namespace (`xmlns="..."`) is treated like any other;
// libxml2 skips it for attributes. No template declares one inside the body.
pub fn strip_redundant_ns(el: &mut Element, root_ns: &[(String, String)]) {
    let mut levels = vec![root_ns.to_vec()];
    let mut bind: Vec<(String, String)> = root_ns
        .iter()
        .map(|(p, _)| (p.clone(), p.clone()))
        .collect();
    move_ns(el, &mut levels, &mut bind);
}

fn move_ns(
    el: &mut Element,
    levels: &mut Vec<Vec<(String, String)>>,
    bind: &mut Vec<(String, String)>,
) {
    let bind_mark = bind.len();
    let mut kept = Vec::new();
    for (prefix, uri) in std::mem::take(&mut el.ns) {
        match search_by_href(levels, &uri) {
            Some(found) => bind.push((prefix, found)),
            None => {
                bind.push((prefix.clone(), prefix.clone()));
                kept.push((prefix, uri));
            }
        }
    }
    el.ns = kept.clone();
    let rename = |name: &mut String, bind: &[(String, String)], default_ns: bool| {
        let (prefix, local) = match name.split_once(':') {
            Some((p, l)) => (p.to_string(), l.to_string()),
            None if default_ns => (String::new(), name.clone()),
            None => return,
        };
        if let Some((_, to)) = bind.iter().rev().find(|(from, _)| *from == prefix)
            && *to != prefix
        {
            *name = if to.is_empty() {
                local
            } else {
                format!("{to}:{local}")
            };
        }
    };
    rename(&mut el.name, bind, true);
    for (k, _) in &mut el.attrs {
        rename(k, bind, false);
    }
    levels.push(kept);
    for c in el.elements_mut() {
        move_ns(c, levels, bind);
    }
    levels.pop();
    bind.truncate(bind_mark);
}

fn search_by_href(levels: &[Vec<(String, String)>], uri: &str) -> Option<String> {
    for (depth, level) in levels.iter().enumerate().rev() {
        for (p, u) in level {
            let shadowed = levels[depth + 1..]
                .iter()
                .any(|l| l.iter().any(|(q, _)| q == p));
            if u == uri && !shadowed {
                return Some(p.clone());
            }
        }
    }
    None
}

pub fn escape_text(t: &str, out: &mut String) {
    for ch in t.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

pub fn escape_attr(t: &str, out: &mut String) {
    for ch in t.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_rules() {
        let src = "<?xml version=\"1.0\"?>\r\n<a xmlns:w=\"u\"  b='x&amp;\"y'>\n <w:t>1 &#60; 2</w:t>\n<e></e><f>  </f></a>";
        let el = parse(src, false).unwrap();
        let mut out = String::new();
        write_element(&el, &mut out);
        assert_eq!(
            out,
            "<a xmlns:w=\"u\" b=\"x&amp;&quot;y\"><w:t>1 &lt; 2</w:t><e/><f>  </f></a>"
        );
        let kept = parse("<a>\n <b/></a>", true).unwrap();
        assert_eq!(kept.children.len(), 2);
        let attr = parse("<a b=\"1\n2&#10;3\"/>", true).unwrap();
        assert_eq!(attr.attr("b"), Some("1 2\n3"));
    }
}
