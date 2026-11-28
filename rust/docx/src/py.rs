//! The Python semantics the render needs: `str()` of a context value and context.py's `_norm`.
//! `py_float_repr` is a copy of `rust/api/src/core.rs py_float` (see README: shared crate).

use minijinja::value::{Value, ValueKind};

/// Python's `repr(float)`: shortest digits, exponent form below 1e-4 and from 1e16.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let mut out = String::new();
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits = tie_to_even(f, mantissa.chars().filter(|c| *c != '.').collect(), exp);
    out.push_str(sign);
    if !(-4..16).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        ));
    } else if exp < 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-exp - 1) as usize));
        out.push_str(&digits);
    } else {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            out.push_str(&"0".repeat(int_len - digits.len()));
            out.push_str(".0");
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    }
    out
}

fn tie_to_even(f: f64, digits: String, exp: i32) -> String {
    let n = digits.len();
    let last = digits.as_bytes()[n - 1] - b'0';
    if last.is_multiple_of(2) {
        return digits;
    }
    let near = format!("{:.*e}", n, f.abs());
    let near: String = near
        .split('e')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| *c != '.')
        .collect();
    if !near.ends_with('5') {
        return digits;
    }
    let exact = format!("{:.800e}", f.abs());
    let (exact, exact_exp) = exact.split_once('e').unwrap_or((&exact, ""));
    let exact: String = exact.chars().filter(|c| *c != '.').collect();
    let exact = exact.trim_end_matches('0');
    if exact.len() != n + 1 || !exact.ends_with('5') || exact_exp != exp.to_string() {
        return digits;
    }
    let lower = &exact[..n];
    let even = if (lower.as_bytes()[n - 1] - b'0').is_multiple_of(2) {
        lower.to_string()
    } else {
        match lower.parse::<u128>() {
            Ok(v) if (v + 1).to_string().len() == n => (v + 1).to_string(),
            _ => return digits,
        }
    };
    let parsed: Option<f64> = format!("{}.{}e{exp}", &even[..1], &even[1..]).parse().ok();
    if parsed == Some(f.abs()) {
        even
    } else {
        digits
    }
}

/// Python `repr(str)`.
fn repr_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// `str(value)` as Jinja2 prints a context value (Undefined prints as "").
pub fn py_str(v: &Value) -> String {
    match v.kind() {
        ValueKind::String => v.as_str().unwrap_or_default().to_string(),
        _ => py_repr(v),
    }
}

fn py_repr(v: &Value) -> String {
    match v.kind() {
        ValueKind::Undefined => String::new(),
        ValueKind::None => "None".into(),
        ValueKind::Bool => if v.is_true() { "True" } else { "False" }.into(),
        ValueKind::Number if v.is_integer() => v.to_string(),
        ValueKind::Number => f64::try_from(v.clone()).map_or_else(|_| v.to_string(), py_float_repr),
        ValueKind::String => repr_str(v.as_str().unwrap_or_default()),
        ValueKind::Seq | ValueKind::Iterable => match v.try_iter() {
            Ok(it) => format!(
                "[{}]",
                it.map(|x| py_repr(&x)).collect::<Vec<_>>().join(", ")
            ),
            Err(_) => v.to_string(),
        },
        // shortcut: dict repr follows minijinja's key order (sorted), not Python insertion
        // order; no template prints a whole dict
        ValueKind::Map => match v.try_iter() {
            Ok(it) => format!(
                "{{{}}}",
                it.map(|k| format!(
                    "{}: {}",
                    py_repr(&k),
                    py_repr(&v.get_item(&k).unwrap_or_default())
                ))
                .collect::<Vec<_>>()
                .join(", ")
            ),
            Err(_) => v.to_string(),
        },
        _ => v.to_string(),
    }
}

/// Python `float(v)`, None where it raises TypeError / ValueError.
pub fn py_float(v: &Value) -> Option<f64> {
    match v.kind() {
        ValueKind::Bool => Some(if v.is_true() { 1.0 } else { 0.0 }),
        ValueKind::Number => f64::try_from(v.clone()).ok(),
        ValueKind::String => {
            let s = v.as_str()?.trim_matches(py_space);
            // underscores only between digits, as in Python's literal grammar
            let b = s.as_bytes();
            for (i, c) in b.iter().enumerate() {
                if *c == b'_'
                    && !(i > 0
                        && b[i - 1].is_ascii_digit()
                        && b.get(i + 1).is_some_and(|n| n.is_ascii_digit()))
                {
                    return None;
                }
            }
            let s = s.replace('_', "");
            let lower = s.to_ascii_lowercase();
            let body = lower.trim_start_matches(['+', '-']);
            if !matches!(body, "inf" | "infinity" | "nan")
                && body.chars().any(|c| c.is_ascii_alphabetic() && c != 'e')
            {
                return None;
            }
            s.parse().ok()
        }
        _ => None,
    }
}

/// Python 3.12 `sum(values)` (start 0) over ints, bools and floats: exact ints until the first
/// float, then Neumaier-compensated float adds (ints added plainly), compensation applied at the
/// end (same as rust/api/src/core.rs `py_sum_numbers`). None where Python raises.
pub fn py_sum(values: &[Value]) -> Option<Value> {
    let (mut int, mut float, mut c): (i64, Option<f64>, f64) = (0, None, 0.0);
    for v in values {
        let i = match v.kind() {
            ValueKind::Bool => Some(v.is_true() as i64),
            ValueKind::Number if v.is_integer() => Some(v.as_i64()?),
            ValueKind::Number => None,
            _ => return None,
        };
        match (i, float) {
            (Some(i), None) => int = int.checked_add(i)?,
            (Some(i), Some(f)) => float = Some(f + i as f64),
            (None, None) => float = Some(int as f64 + f64::try_from(v.clone()).ok()?),
            (None, Some(f)) => {
                let x = f64::try_from(v.clone()).ok()?;
                let t = f + x;
                c += if f.abs() >= x.abs() {
                    (f - t) + x
                } else {
                    (x - t) + f
                };
                float = Some(t);
            }
        }
    }
    Some(match float {
        Some(f) if c != 0.0 && c.is_finite() => Value::from(f + c),
        Some(f) => Value::from(f),
        None => Value::from(int),
    })
}

/// Python `str.isspace` / `\s` for str patterns.
pub fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// context.py `_norm`: html.unescape, whitespace runs to one space, stripped.
pub fn norm(tag: &str) -> String {
    html_unescape(tag)
        .split(py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `html.unescape` for what a converted template can hold: the XML entities and character
/// references (named HTML5 entities other than these are left as they are).
// shortcut: no HTML5 named-entity table; upgrade if a tag text ever carries e.g. `&nbsp;`
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let (ch, used) = entity(tail);
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[used..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(tail: &str) -> (Option<char>, usize) {
    for (name, c) in [
        ("amp;", '&'),
        ("lt;", '<'),
        ("gt;", '>'),
        ("quot;", '"'),
        ("apos;", '\''),
    ] {
        if tail.starts_with(name) {
            return (Some(c), name.len());
        }
    }
    if let Some(num) = tail.strip_prefix('#') {
        let (radix, digits, skip) = match num.strip_prefix(['x', 'X']) {
            Some(h) => (16, h, 2),
            None => (10, num, 1),
        };
        let len = digits
            .find(|c: char| !c.is_digit(radix))
            .unwrap_or(digits.len());
        if len > 0 {
            let code = u32::from_str_radix(&digits[..len], radix).ok();
            let semi = digits[len..].starts_with(';') as usize;
            // html.unescape: invalid code points become U+FFFD
            let c = code
                .and_then(char::from_u32)
                .filter(|c| *c != '\0')
                .unwrap_or('\u{fffd}');
            return (Some(c), skip + len + semi);
        }
    }
    (None, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_and_values() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1.5e-7), "1.5e-07");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_str(&Value::from(3)), "3");
        assert_eq!(py_str(&Value::from(2.5)), "2.5");
        assert_eq!(py_str(&Value::from(())), "None");
        assert_eq!(py_str(&Value::from(true)), "True");
        assert_eq!(norm("  IP&amp;LC \n x "), "IP&LC x");
    }
}
