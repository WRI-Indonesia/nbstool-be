//! docxtpl 0.18.0 `patch_xml` (template.py:81-297) and `resolve_listing` (:354-405), regex by
//! regex in the original order. Python `re` is a backtracking engine with look-around, so the
//! patterns that need it go through fancy-regex; the rest through `regex` (same leftmost-first
//! semantics, linear time). `{`/`}` are escaped where Python takes them literally.

use fancy_regex::{Captures as FCaptures, Regex as FRegex, RegexBuilder};
use regex::{Captures, Regex};
use std::sync::OnceLock;

type R<T> = Result<T, String>;

fn fancy(pat: &str) -> FRegex {
    // shortcut: compiled patterns are constants; a bad one is a programming error caught by tests
    RegexBuilder::new(pat)
        .backtrack_limit(100_000_000)
        .build()
        .unwrap_or_else(|e| panic!("{pat}: {e}"))
}

fn plain(pat: &str) -> Regex {
    Regex::new(pat).unwrap_or_else(|e| panic!("{pat}: {e}"))
}

fn fsub(re: &FRegex, text: &str, mut f: impl FnMut(&FCaptures) -> String) -> R<String> {
    re.try_replacen(text, 0, |c: &FCaptures| f(c))
        .map(|c| c.into_owned())
        .map_err(|e| e.to_string())
}

fn group<'a>(c: &'a FCaptures, i: usize) -> &'a str {
    c.get(i).map_or("", |m| m.as_str())
}

struct Patterns {
    braces: FRegex,
    jinja_tags: FRegex,
    run_breaks: Regex,
    colspan: FRegex,
    cellbg: FRegex,
    empty_run: FRegex,
    grid_span: Regex,
    shd: Regex,
    tc_pr: Regex,
    preserve: FRegex,
    r_tag: Regex,
    merge_prev: FRegex,
    merge_next: FRegex,
    strip: Vec<FRegex>,
    strip_comment: Vec<FRegex>,
    vm_tc: FRegex,
    vm_inner: Regex,
    hm_tc: FRegex,
    hm_with: Regex,
    hm_tag: Regex,
    hm_inner: Regex,
    clean: FRegex,
    /// literal parts every match of a costly pattern contains: without them the substitution is
    /// the identity, so it is skipped (fancy-regex tries every start position, Python's sre jumps)
    gate_colspan: Regex,
    gate_cellbg: Regex,
    gate_vm: Regex,
    gate_hm: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        braces: fancy(r"(?s)(?<=\{)(<[^>]*>)+(?=[\{%#])|(?<=[%\}#])(<[^>]*>)+(?=\})"),
        jinja_tags: fancy(r"(?s)\{%(?:(?!%\}).)*|\{#(?:(?!#\}).)*|\{\{(?:(?!\}\}).)*"),
        run_breaks: plain(r"(?s)</w:t>.*?(<w:t>|<w:t [^>]*>)"),
        colspan: fancy(r"(?s)(<w:tc[ >](?:(?!<w:tc[ >]).)*)\{%\s*colspan\s+([^%]*)\s*%\}(.*?</w:tc>)"),
        cellbg: fancy(r"(?s)(<w:tc[ >](?:(?!<w:tc[ >]).)*)\{%\s*cellbg\s+([^%]*)\s*%\}(.*?</w:tc>)"),
        empty_run: fancy(r"(?s)<w:r[ >](?:(?!<w:r[ >]).)*<w:t></w:t>.*?</w:r>"),
        grid_span: plain(r"<w:gridSpan[^/]*/>"),
        shd: plain(r"<w:shd[^/]*/>"),
        tc_pr: plain(r"(<w:tcPr[^>]*>)"),
        preserve: fancy(r"(?s)<w:t>((?:(?!<w:t>).)*)(\{\{.*?\}\}|\{%.*?%\})"),
        r_tag: plain(r"(?s)(\{\{r\s.*?\}\}|\{%r\s.*?%\})"),
        merge_prev: fancy(r"(?s)</w:t>(?:(?!</w:t>).)*?\{%-"),
        merge_next: fancy(r"(?s)-%\}(?:(?!<w:t[ >]|\{%|\{\{).)*?<w:t[^>]*?>"),
        strip: ["tr", "tc", "p", "r"]
            .iter()
            .map(|y| fancy(&format!(r"(?s)<w:{y}[ >](?:(?!<w:{y}[ >]).)*(\{{%|\{{\{{){y} ([^\}}%]*(?:%\}}|\}}\}})).*?</w:{y}>")))
            .collect(),
        strip_comment: ["tr", "tc", "p"]
            .iter()
            .map(|y| fancy(&format!(r"(?s)<w:{y}[ >](?:(?!<w:{y}[ >]).)*(\{{#){y} ([^\}}#]*(?:#\}})).*?</w:{y}>")))
            .collect(),
        vm_tc: fancy(r"(?s)<w:tc[ >](?:(?!<w:tc[ >]).)*?\{%\s*vm\s*%\}.*?</w:tc[ >]"),
        vm_inner: plain(r"(?s)(</w:tcPr[ >].*?<w:t(?:.*?)>)(.*?)(?:\{%\s*vm\s*%\})(.*?)(</w:t>)"),
        hm_tc: fancy(r"(?s)<w:tc[ >](?:(?!<w:tc[ >]).)*?\{%\s*hm\s*%\}.*?</w:tc[ >]"),
        hm_with: plain(r#"(?s)(w:gridSpan w:val=")(\d+)(")"#),
        hm_tag: plain(r"(?s)\{%\s*hm\s*%\}"),
        hm_inner: plain(r"(?s)(</w:tcPr[ >].*?<w:t(?:.*?)>)(.*?)(?:\{%\s*hm\s*%\})(.*?)(</w:t>)"),
        // no DOTALL in the original
        clean: fancy(r"(?<=\{[\{%])(.*?)(?=[\}%]\})"),
        gate_colspan: plain(r"\{%\s*colspan\s"),
        gate_cellbg: plain(r"\{%\s*cellbg\s"),
        gate_vm: plain(r"\{%\s*vm\s*%\}"),
        gate_hm: plain(r"\{%\s*hm\s*%\}"),
    })
}

fn cell_patch(p: &Patterns, c: &FCaptures, drop: &Regex, insert: &str) -> R<String> {
    let cell = format!("{}{}", group(c, 1), group(c, 3));
    let cell = fsub(&p.empty_run, &cell, |_| String::new())?;
    let cell = drop.replacen(&cell, 1, "").into_owned();
    let tag = insert.replace("{}", group(c, 2));
    Ok(p.tc_pr
        .replace_all(&cell, |m: &Captures| format!("{}{tag}", &m[1]))
        .into_owned())
}

/// `DocxTemplate.patch_xml`.
pub fn patch_xml(src: &str) -> R<String> {
    let p = patterns();
    let mut x = fsub(&p.braces, src, |_| String::new())?;
    x = fsub(&p.jinja_tags, &x, |c| {
        p.run_breaks.replace_all(&c[0], "").into_owned()
    })?;

    // colspan / cellbg: the callbacks run nested regexes that can fail, collected after
    let mut failure = None;
    for (re, gate, drop, insert) in [
        (
            &p.colspan,
            &p.gate_colspan,
            &p.grid_span,
            r#"<w:gridSpan w:val="{{{}}}"/>"#,
        ),
        (
            &p.cellbg,
            &p.gate_cellbg,
            &p.shd,
            r#"<w:shd w:val="clear" w:color="auto" w:fill="{{{}}}"/>"#,
        ),
    ] {
        if !gate.is_match(&x) {
            continue;
        }
        x = fsub(re, &x, |c| {
            cell_patch(p, c, drop, insert).unwrap_or_else(|e| {
                failure.get_or_insert(e);
                String::new()
            })
        })?;
    }
    if let Some(e) = failure {
        return Err(e);
    }

    x = fsub(&p.preserve, &x, |c| {
        format!(
            r#"<w:t xml:space="preserve">{}{}"#,
            group(c, 1),
            group(c, 2)
        )
    })?;
    x = p
        .r_tag
        .replace_all(&x, |c: &Captures| {
            format!(r#"</w:t></w:r><w:r><w:t xml:space="preserve">{}</w:t></w:r><w:r><w:t xml:space="preserve">"#, &c[1])
        })
        .into_owned();
    if x.contains("{%-") {
        x = fsub(&p.merge_prev, &x, |_| "{%".to_string())?;
    }
    if x.contains("-%}") {
        x = fsub(&p.merge_next, &x, |_| "%}".to_string())?;
    }
    let tags = |y: &str| [format!("{{%{y} "), format!("{{{{{y} ")];
    for (re, y) in p.strip.iter().zip(["tr", "tc", "p", "r"]) {
        if tags(y).iter().any(|t| x.contains(t.as_str())) {
            x = fsub(re, &x, |c| format!("{} {}", group(c, 1), group(c, 2)))?;
        }
    }
    for (re, y) in p.strip_comment.iter().zip(["tr", "tc", "p"]) {
        if x.contains(&format!("{{#{y} ")) {
            x = fsub(re, &x, |c| format!("{} {}", group(c, 1), group(c, 2)))?;
        }
    }
    if p.gate_vm.is_match(&x) {
        x = vm(p, &x)?;
    }
    if p.gate_hm.is_match(&x) {
        x = hm(p, &x)?;
    }
    clean_tags(p, &x)
}

fn vm(p: &Patterns, x: &str) -> R<String> {
    fsub(&p.vm_tc, x, |c| {
        p.vm_inner
            .replace_all(&c[0], |m: &Captures| {
                format!(
                    r#"<w:vMerge w:val="{{% if loop.first %}}restart{{% else %}}continue{{% endif %}}"/>{}{{% if loop.first %}}{}{}{{% endif %}}{}"#,
                    &m[1], &m[2], &m[3], &m[4]
                )
            })
            .into_owned()
    })
}

fn hm(p: &Patterns, x: &str) -> R<String> {
    fsub(&p.hm_tc, x, |c| {
        let cell = &c[0];
        let xml = if cell.contains("w:gridSpan") {
            let with = p.hm_with.replace_all(cell, |m: &Captures| {
                format!("{}{{{{ {} * loop.length }}}}{}", &m[1], &m[2], &m[3])
            });
            p.hm_tag.replace_all(&with, "").into_owned()
        } else {
            p.hm_inner
                .replace_all(cell, |m: &Captures| {
                    format!(
                        r#"<w:gridSpan w:val="{{{{ loop.length }}}}"/>{}{}{}{}"#,
                        &m[1], &m[2], &m[3], &m[4]
                    )
                })
                .into_owned()
        };
        format!("{{% if loop.first %}}{xml}{{% endif %}}")
    })
}

fn clean_tags(p: &Patterns, x: &str) -> R<String> {
    fsub(&p.clean, x, |c| {
        c[0].replace("&#8216;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace(['\u{201c}', '\u{201d}'], "\"")
            .replace(['\u{2018}', '\u{2019}'], "'")
    })
}

struct Listing {
    para: Regex,
    ppr: Regex,
    run: Regex,
    rpr: Regex,
    text: Regex,
}

fn listing() -> &'static Listing {
    static L: OnceLock<Listing> = OnceLock::new();
    L.get_or_init(|| Listing {
        para: plain(r"(?s)<w:p(?: [^>]*)?>.*?</w:p>"),
        ppr: plain(r"<w:pPr>.*?</w:pPr>"),
        run: plain(r"(?s)<w:r(?: [^>]*)?>.*?</w:r>"),
        rpr: plain(r"<w:rPr>.*?</w:rPr>"),
        text: plain(r"(?s)<w:t(?: [^>]*)?>.*?</w:t>"),
    })
}

/// `DocxTemplate.resolve_listing`: \t, \a, \n, \f inside `<w:t>` become Word markup.
pub fn resolve_listing(xml: &str) -> String {
    let l = listing();
    l.para
        .replace_all(xml, |pm: &Captures| {
            let para = &pm[0];
            let ppr = l.ppr.find(para).map_or("", |m| m.as_str());
            l.run
                .replace_all(para, |rm: &Captures| {
                    let run = &rm[0];
                    let rpr = l.rpr.find(run).map_or("", |m| m.as_str());
                    l.text
                        .replace_all(run, |tm: &Captures| {
                            tm[0]
                                .replace('\t', &format!(r#"</w:t></w:r><w:r>{rpr}<w:tab/></w:r><w:r>{rpr}<w:t xml:space="preserve">"#))
                                .replace('\u{7}', &format!(r#"</w:t></w:r></w:p><w:p>{ppr}<w:r>{rpr}<w:t xml:space="preserve">"#))
                                .replace('\n', r#"</w:t><w:br/><w:t xml:space="preserve">"#)
                                .replace(
                                    '\u{c}',
                                    &format!(
                                        r#"</w:t></w:r></w:p><w:p><w:r><w:br w:type="page"/></w:r></w:p><w:p>{ppr}<w:r>{rpr}<w:t xml:space="preserve">"#
                                    ),
                                )
                        })
                        .into_owned()
                })
                .into_owned()
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        let _ = patterns();
        let _ = listing();
    }

    #[test]
    fn tr_tag_and_quotes() {
        let src = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>{%tr for r in rows %}</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>{{ t(“a”) }}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let out = patch_xml(src).unwrap();
        assert_eq!(
            out,
            r#"<w:tbl>{% for r in rows %}<w:tr><w:tc><w:p><w:r><w:t xml:space="preserve">{{ t("a") }}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#
        );
    }
}
