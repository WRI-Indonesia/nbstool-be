//! Python and numpy semantics that reach the output: `sum()` over floats, `round()`, the format
//! specs the narratives use, and numpy's pairwise float summation.

/// Python 3.12 `sum(floats)` (start 0): Neumaier-compensated, compensation added at the end.
pub fn fsum_py(values: impl IntoIterator<Item = f64>) -> f64 {
    let (mut f, mut c) = (0.0f64, 0.0f64);
    for x in values {
        let t = f + x;
        c += if f.abs() >= x.abs() {
            (f - t) + x
        } else {
            (x - t) + f
        };
        f = t;
    }
    if c != 0.0 && c.is_finite() { f + c } else { f }
}

/// `ndarray.sum()` of a contiguous float64 array (numpy 1.26): the reduction runs through the
/// buffered iterator, so the array is cut into blocks of 8192 (the default bufsize), each block
/// is summed pairwise, and the block sums are added in order. Measured against numpy on 1-D and
/// 2-D arrays of 3e3..4e5 elements (proof/README.md).
pub fn np_sum(a: &[f64]) -> f64 {
    let mut total = 0.0;
    for block in a.chunks(8192) {
        total += pairwise_sum(block);
    }
    total
}

/// numpy `pairwise_sum_DOUBLE` (loops_utils.h.src): below 8 elements a plain loop, up to 128 an
/// 8-way unrolled loop, above that split in halves at a multiple of 8.
fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        // numpy starts from -0.0 here so that an all -0.0 input sums to -0.0
        let mut res = -0.0f64;
        for &x in a {
            res += x;
        }
        res
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - (n % 8) {
            for k in 0..8 {
                r[k] += a[i + k];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// Python `round(x)` to an int: ties to even.
pub fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `f"{value:,.{decimals}f}"`. Rust's fixed-precision formatting is exact and ties to even,
/// as Python's is.
pub fn fmt_grouped(value: f64, decimals: usize) -> String {
    let s = format!("{value:.decimals$}");
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", s.as_str()),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// common.fmt_ha
pub fn fmt_ha(value: f64) -> String {
    format!("{} ha", fmt_grouped(value, 0))
}

/// common.fmt_pct
pub fn fmt_pct(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}%")
}

/// common.safe_pct
pub fn safe_pct(part: f64, whole: f64) -> f64 {
    if whole <= 0.0 {
        0.0
    } else {
        part / whole * 100.0
    }
}

/// common.oxford_join
pub fn oxford_join(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// common.sentences: non-empty fragments, stripped, joined by one space.
pub fn sentences(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python `round(x, ndigits)` for a float: the exact binary value rounded half-even to
/// `ndigits` decimals (Rust's fixed-precision formatting is exact and ties-to-even), read back.
pub fn round_ndigits(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

// ---- moved from benefit/util.rs (shared by benefit 5.x and pathway 4.2) ----

/// `np.argsort(v)` (kind="quicksort") as numpy 1.26 runs it where the AVX-512 x86-simd-sort
/// dispatch is not compiled in (MSVC builds) or not available (CPU without AVX512_SKX):
/// `aquicksort_` (introsort, median of three, insertion sort at 16, heapsort past the depth
/// limit). shortcut: a Linux numpy on an AVX512_SKX CPU sorts ties in a different order
/// (x86-simd-sort `avx512_argsort`); port that kernel if staging runs on such a CPU.
pub fn np_argsort(v: &[f64]) -> Vec<usize> {
    let less = |a: f64, b: f64| a < b || (b.is_nan() && !a.is_nan());
    let mut tosort: Vec<usize> = (0..v.len()).collect();
    let num = v.len();
    if num < 2 {
        return tosort;
    }
    const SMALL_QUICKSORT: isize = 15;
    let mut stack: Vec<(isize, isize)> = Vec::new();
    let mut depth_stack: Vec<i32> = Vec::new();
    let (mut pl, mut pr): (isize, isize) = (0, num as isize - 1);
    let mut cdepth: i32 = (usize::BITS - 1 - num.leading_zeros()) as i32 * 2;
    let a = &mut tosort;
    loop {
        let mut do_insertion = true;
        if cdepth < 0 {
            aheapsort(v, &mut a[pl as usize..=pr as usize], &less);
            do_insertion = false;
        } else {
            while pr - pl > SMALL_QUICKSORT {
                let pm = pl + ((pr - pl) >> 1);
                if less(v[a[pm as usize]], v[a[pl as usize]]) {
                    a.swap(pm as usize, pl as usize);
                }
                if less(v[a[pr as usize]], v[a[pm as usize]]) {
                    a.swap(pr as usize, pm as usize);
                }
                if less(v[a[pm as usize]], v[a[pl as usize]]) {
                    a.swap(pm as usize, pl as usize);
                }
                let vp = v[a[pm as usize]];
                let mut pi = pl;
                let mut pj = pr - 1;
                a.swap(pm as usize, pj as usize);
                loop {
                    loop {
                        pi += 1;
                        if !less(v[a[pi as usize]], vp) {
                            break;
                        }
                    }
                    loop {
                        pj -= 1;
                        if !less(vp, v[a[pj as usize]]) {
                            break;
                        }
                    }
                    if pi >= pj {
                        break;
                    }
                    a.swap(pi as usize, pj as usize);
                }
                let pk = pr - 1;
                a.swap(pi as usize, pk as usize);
                if pi - pl < pr - pi {
                    stack.push((pi + 1, pr));
                    pr = pi - 1;
                } else {
                    stack.push((pl, pi - 1));
                    pl = pi + 1;
                }
                cdepth -= 1;
                depth_stack.push(cdepth);
            }
        }
        if do_insertion {
            let mut pi = pl + 1;
            while pi <= pr {
                let vi = a[pi as usize];
                let vpv = v[vi];
                let mut pj = pi;
                let mut pk = pi - 1;
                while pj > pl && less(vpv, v[a[pk as usize]]) {
                    a[pj as usize] = a[pk as usize];
                    pj -= 1;
                    pk -= 1;
                }
                a[pj as usize] = vi;
                pi += 1;
            }
        }
        match (stack.pop(), depth_stack.pop()) {
            (Some((l, r)), Some(d)) => {
                pl = l;
                pr = r;
                cdepth = d;
            }
            _ => break,
        }
    }
    tosort
}

/// numpy `aheapsort_` over one slice of indices (one-based heap, as in heapsort.cpp).
fn aheapsort(v: &[f64], tosort: &mut [usize], less: &impl Fn(f64, f64) -> bool) {
    let mut n = tosort.len();
    // a[k] for k in 1..=n is tosort[k - 1]
    let mut l = n >> 1;
    while l > 0 {
        let tmp = tosort[l - 1];
        let (mut i, mut j) = (l, l << 1);
        while j <= n {
            if j < n && less(v[tosort[j - 1]], v[tosort[j]]) {
                j += 1;
            }
            if less(v[tmp], v[tosort[j - 1]]) {
                tosort[i - 1] = tosort[j - 1];
                i = j;
                j += j;
            } else {
                break;
            }
        }
        tosort[i - 1] = tmp;
        l -= 1;
    }
    while n > 1 {
        let tmp = tosort[n - 1];
        tosort[n - 1] = tosort[0];
        n -= 1;
        let (mut i, mut j) = (1usize, 2usize);
        while j <= n {
            if j < n && less(v[tosort[j - 1]], v[tosort[j]]) {
                j += 1;
            }
            if less(v[tmp], v[tosort[j - 1]]) {
                tosort[i - 1] = tosort[j - 1];
                i = j;
                j += j;
            } else {
                break;
            }
        }
        tosort[i - 1] = tmp;
    }
}

/// pandas 2.2 `nargsort(values, kind="quicksort", ascending=False, na_position="last")`, the
/// indexer `DataFrame.sort_values(col, ascending=False)` applies: NaNs out, the rest reversed,
/// numpy quicksort argsort, the result reversed back, NaNs appended.
pub fn nargsort_desc(values: &[f64]) -> Vec<usize> {
    let non_nan_idx: Vec<usize> = (0..values.len())
        .filter(|i| !values[*i].is_nan())
        .rev()
        .collect();
    let non_nans: Vec<f64> = non_nan_idx.iter().map(|i| values[*i]).collect();
    let order = np_argsort(&non_nans);
    let mut indexer: Vec<usize> = order.iter().map(|k| non_nan_idx[*k]).collect();
    indexer.reverse();
    indexer.extend((0..values.len()).filter(|i| values[*i].is_nan()));
    indexer
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_python() {
        assert_eq!(fmt_grouped(0.5, 0), "0");
        assert_eq!(fmt_grouped(1.5, 0), "2");
        assert_eq!(fmt_grouped(2.5, 0), "2");
        assert_eq!(fmt_grouped(1234567.4, 0), "1,234,567");
        assert_eq!(fmt_grouped(-1234.5, 1), "-1,234.5");
        assert_eq!(fmt_grouped(999.96, 1), "1,000.0");
        assert_eq!(fmt_pct(0.05, 1), "0.1%"); // 0.05 is just above the tie in binary
        assert_eq!(fmt_pct(0.25, 1), "0.2%");
    }

    #[test]
    fn sums_like_python_and_numpy() {
        assert_eq!(fsum_py([0.1; 10]), 1.0);
        // numpy 1.26: (np.arange(20000, dtype=float) * 0.1).sum() == 19999000.0
        let v: Vec<f64> = (0..20000).map(|i| (i as f64) * 0.1).collect();
        assert_eq!(np_sum(&v), 19999000.0);
    }
}
