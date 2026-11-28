//! docxtpl `fix_tables` (template.py:491-580) and `fix_docpr_ids` (:582-586) on the rendered
//! body. Element names are matched with the `w:` / `wp:` prefixes the templates use.

use crate::xml::{Element, Node, R};

fn find<'a>(e: &'a Element, name: &str) -> Option<&'a Element> {
    e.elements().find(|c| c.name == name)
}

/// `t.iter("w:tr")`: every row below the table, nested tables included, in document order.
fn rows<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
    for c in e.elements() {
        if c.name == "w:tr" {
            out.push(c);
        }
        rows(c, out);
    }
}

fn py_float(s: Option<&str>) -> R<f64> {
    let s = s.ok_or("float() argument must be a string or a real number, not 'NoneType'")?;
    s.trim()
        .parse::<f64>()
        .map_err(|_| format!("could not convert string to float: '{s}'"))
}

/// `str(int(x))`.
fn int_str(x: f64) -> String {
    format!("{}", x.trunc() as i64)
}

pub fn fix_tables(e: &mut Element) -> R<()> {
    if e.name == "w:tbl" {
        fix_table(e)?;
    }
    for c in e.elements_mut() {
        fix_tables(c)?;
    }
    Ok(())
}

fn fix_table(t: &mut Element) -> R<()> {
    let (cell_counts, span_counts) = {
        let mut rs = Vec::new();
        rows(t, &mut rs);
        let mut cells = Vec::new();
        let mut spans = Vec::new();
        for r in rs {
            let tcs: Vec<&Element> = r.elements().filter(|c| c.name == "w:tc").collect();
            cells.push(tcs.len());
            let mut total = 0i64;
            for tc in tcs {
                let span = find(tc, "w:tcPr").and_then(|p| find(p, "w:gridSpan"));
                total += match span {
                    Some(g) => {
                        let v = g
                            .attr("w:val")
                            .ok_or("int() argument must be a string, not 'NoneType'")?;
                        v.trim()
                            .parse::<i64>()
                            .map_err(|_| format!("invalid literal for int() with base 10: '{v}'"))?
                    }
                    None => 1,
                };
            }
            spans.push(total);
        }
        (cells, spans)
    };
    let grid = t
        .elements_mut()
        .find(|c| c.name == "w:tblGrid")
        .ok_or("'NoneType' object has no attribute 'findall'")?;
    let ncols = grid.elements().filter(|c| c.name == "w:gridCol").count();
    let mut to_add = 0usize;
    for &n in &cell_counts {
        if ncols + to_add < n {
            to_add = n - ncols;
        }
    }
    if to_add > 0 {
        let mut width = 0.0;
        for c in grid.elements().filter(|c| c.name == "w:gridCol") {
            if let Some(w) = c.attr("w:w") {
                width += py_float(Some(w))?;
            }
        }
        if width > 0.0 {
            let old_average = width / ncols as f64;
            let new_average = width / (ncols + to_add) as f64;
            for c in grid.elements_mut().filter(|c| c.name == "w:gridCol") {
                let v = py_float(c.attr("w:w"))?;
                c.set_attr("w:w", int_str(v * new_average / old_average));
            }
            for _ in 0..to_add {
                let mut col = Element::new("w:gridCol");
                col.attrs.push(("w:w".into(), int_str(new_average)));
                grid.children.push(Node::Elem(col));
            }
        }
    }
    let columns_len = grid.elements().filter(|c| c.name == "w:gridCol").count() as i64;
    let cells_len_max = span_counts.iter().copied().fold(0, i64::max);
    let to_remove = columns_len - cells_len_max;
    if to_remove > 0 {
        let positions: Vec<usize> = grid
            .children
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n, Node::Elem(e) if e.name == "w:gridCol"))
            .map(|(i, _)| i)
            .collect();
        let cut = &positions[positions.len() - to_remove as usize..];
        let mut removed_width = 0.0;
        for &i in cut {
            if let Node::Elem(c) = &grid.children[i] {
                removed_width += py_float(c.attr("w:w"))?;
            }
        }
        for &i in cut.iter().rev() {
            grid.children.remove(i);
        }
        let left = grid.elements().filter(|c| c.name == "w:gridCol").count();
        let extra_space = if left > 0 {
            (removed_width / left as f64).trunc()
        } else {
            0.0
        };
        for c in grid.elements_mut().filter(|c| c.name == "w:gridCol") {
            let v = py_float(c.attr("w:w"))?;
            c.set_attr("w:w", int_str(v + extra_space));
        }
    }
    Ok(())
}

/// Every `wp:docPr/@id` renumbered 1001, 1002, ... in document order.
pub fn fix_docpr_ids(body: &mut Element) {
    fn walk(e: &mut Element, n: &mut u64) {
        if e.name == "wp:docPr" {
            *n += 1;
            e.set_attr("id", n.to_string());
        }
        for c in e.elements_mut() {
            walk(c, n);
        }
    }
    let mut n = 1000;
    walk(body, &mut n);
}
