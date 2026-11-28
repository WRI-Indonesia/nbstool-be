//! Python number semantics shared by the builders. `py_float_repr` is a copy of
//! `rust/docx/src/py.rs` (itself a copy of `rust/api/src/core.rs py_float`); `py_sum_numbers` of
//! `rust/api/src/core.rs` (Python 3.12 compensated `sum`).

use serde_json::{Value, json};

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

/// `sum(values)` over ints/floats/bools exactly as Python 3.12 does (Neumaier-compensated once a
/// float appears); None when a value is not a number or an int sum overflows i64.
pub fn py_sum_numbers<'a>(values: impl IntoIterator<Item = &'a Value>) -> Option<Value> {
    let (mut int, mut float, mut c): (i64, Option<f64>, f64) = (0, None, 0.0);
    for v in values {
        let i = match v {
            Value::Bool(b) => Some(*b as i64),
            Value::Number(n) if !n.is_f64() => Some(n.as_i64()?),
            Value::Number(_) => None,
            _ => return None,
        };
        match (i, float) {
            (Some(i), None) => int = int.checked_add(i)?,
            (Some(i), Some(f)) => float = Some(f + i as f64),
            (None, None) => float = Some(int as f64 + v.as_f64()?),
            (None, Some(f)) => {
                let x = v.as_f64()?;
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
        Some(f) if c != 0.0 && c.is_finite() => json!(f + c),
        Some(f) => json!(f),
        None => json!(int),
    })
}
