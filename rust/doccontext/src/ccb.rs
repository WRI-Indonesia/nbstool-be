//! CCB context: `render_ccb_document`'s `data` dict with `_tpl_data_v3` and `_tpl_data_form` of
//! `application/apis/document_apis/ccb/routes.py`, and the CSV `generate_document_form`
//! (`utils/document_generator/__init__.py`) writes next to the docx.
//!
//! The reads stay with the caller: `DocumentData` (sections unpickled to JSON, insertion order
//! kept), `UserSessions` + `User`, `DataAnalyzer`, `Polygons` (`ST_AsGeoJSON(geom)` text and
//! `project_area_size`). `datetime.now().year` is passed in.

use serde_json::{Map, Value, json};

use crate::py::py_sum_numbers;
use crate::v3::{
    Analyzer, fmt_f, get, get_or, hashable, is_num, or, py_add, py_eq, py_gt, py_iter, py_space,
    py_str, py_type,
};

type R<T> = Result<T, String>;

const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The `User` row of the session's owner: `name`, `email`, `extended_data` (JSON values).
pub struct User {
    pub name: Value,
    pub email: Value,
    pub extended_data: Value,
}

/// Everything `render_ccb_document(session_id, document_type)` reads once it found the
/// `DocumentData` row.
pub struct Input<'a> {
    pub session_id: &'a str,
    /// None: no active `UserSessions` row or no such `User`
    pub user: Option<&'a User>,
    /// `section_1` .. `section_5` as unpickled (None = Null)
    pub sections: [&'a Value; 5],
    pub analyzer: Option<&'a Analyzer>,
    /// `Polygons.get_geometry(session_id).first()`: None = no row, Some(None) = NULL geom
    pub geojson: Option<Option<&'a str>>,
    /// `Polygons.query.filter_by(session_id).first().project_area_size` (None: no row or NULL)
    pub polygon_area: Option<f64>,
    /// `datetime.now().year`
    pub current_year: i32,
}

/// The `data` dict `render_ccb_document` hands to `generate_document_form`: the CCB template's
/// whole context, in Python's key order. Err is the text of the exception the Python raises.
pub fn render_data(input: &Input) -> R<Value> {
    let a = input.analyzer;
    let benefit = a.map_or(json!({}), |a| or(&a.benefit_json, json!({})));
    let duration = get(
        &or(&get(&benefit, "assumptions")?, json!({})),
        "duration_years",
    )?;
    let user = match input.user {
        Some(u) => json!({
            "fullname": u.name,
            "email": u.email,
            "phone": or(&get(&or(&u.extended_data, json!({})), "phone")?, json!("")),
        }),
        None => json!({"fullname": "", "email": "", "phone": ""}),
    };
    let mut data = Map::new();
    data.insert("project_id".into(), json!(input.session_id));
    data.insert("user".into(), user);
    data.insert("param".into(), json!({ "project_duration": duration }));
    for (i, s) in input.sections.iter().enumerate() {
        data.insert(format!("section_{}", i + 1), (*s).clone());
    }
    let mut tpl = tpl_data_v3(input)?;
    for (k, v) in tpl_data_form(input.sections[0], input.sections[4])? {
        tpl.insert(k, v);
    }
    data.insert("tpl_data".into(), Value::Object(tpl));
    Ok(Value::Object(data))
}

/// `_tpl_data_v3(session_id, analyzer)`
pub fn tpl_data_v3(input: &Input) -> R<Map<String, Value>> {
    let a = input.analyzer;
    let site = a.map_or(json!({}), |a| or(&a.site_information_json, json!({})));
    let climate = a.map_or(json!({}), |a| or(&a.climate_json, json!({})));
    let mut data = Map::new();
    data.insert("current_year".into(), json!(input.current_year));

    let district = capwords(&or(&get(&site, "district")?, json!("")))?;
    let province = capwords(&or(&get(&site, "province")?, json!("")))?;
    let country = capwords(&or(&get(&site, "country")?, json!("")))?;
    let location: Vec<&str> = [&district, &province, &country]
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(String::as_str)
        .collect();
    data.insert("district".into(), json!(district));
    data.insert("province".into(), json!(province));
    data.insert("project_location".into(), json!(location.join(", ")));

    let (minx, miny, maxx, maxy) = bounds(input.geojson)?;
    let (cx, cy) = box_centroid(minx, miny, maxx, maxy);
    data.insert(
        "geom_center".into(),
        json!({"longitude": cx, "latitude": cy}),
    );
    data.insert("geom_center_dms".into(), coords_to_string(cx, cy));
    data.insert("geom_min_dms".into(), coords_to_string(minx, miny));
    data.insert("geom_max_dms".into(), coords_to_string(maxx, maxy));

    let pathway = a.map_or(json!({}), |a| {
        or(&a.intervention_eligibility_json, json!({}))
    });
    let mut area = if pathway.is_object() {
        get(&pathway, "project_area_ha")?
    } else {
        Value::Null
    };
    if area.is_null() {
        area = input.polygon_area.map_or(Value::Null, |f| json!(f));
    }
    data.insert(
        "area_size".into(),
        json!(is_num(&area).map_or(String::new(), |f| fmt_f(f, 2, true))),
    );

    let mut has_peatland = false;
    for eco in py_iter(&or(&get(&site, "ecosystems")?, json!([])))? {
        if !eco.is_object() {
            continue;
        }
        let name = get(&eco, "name")?;
        if (py_eq(&name, &json!("Peatland")) || py_eq(&name, &json!("Mangrove")))
            && py_gt(&or(&get(&eco, "area")?, json!(0)), &json!(0))?
        {
            has_peatland = true;
            break;
        }
    }
    data.insert("has_peatland".into(), json!(has_peatland));

    let slope = get(&site, "average_slope_percentage")?;
    let elevation_class = or(
        &get(
            &or(&get(&site, "predominant_elevation_dict")?, json!({})),
            "fallback",
        )?,
        json!(""),
    );
    let elevation_pct = is_num(&slope).map_or(String::new(), |f| fmt_f(f, 2, false));
    data.insert(
        "top_elevation".into(),
        json!([{"elevation_class": elevation_class, "elevation_pct": elevation_pct}]),
    );

    let nature = a.map_or(json!({}), |a| or(&a.nature_json, json!({})));
    let num = |v: &Value, prec: usize, comma: bool, missing: &str| {
        json!(is_num(v).map_or(missing.to_string(), |f| fmt_f(f, prec, comma)))
    };
    data.insert(
        "min_elevation".into(),
        num(
            &get(&site, "min_elevation")?,
            0,
            true,
            "[number of minimal elevations]",
        ),
    );
    data.insert(
        "max_elevation".into(),
        num(
            &get(&site, "max_elevation")?,
            0,
            true,
            "[number of maximum elevations]",
        ),
    );
    data.insert(
        "protected_pct".into(),
        num(
            &get(&site, "overlapping_protected_area_percentage")?,
            2,
            false,
            "[percentage of protected area coverage]",
        ),
    );
    data.insert(
        "flii_score".into(),
        num(&get(&nature, "flii_score")?, 2, false, "[FLII Scores]"),
    );
    data.insert(
        "flii_low_pct".into(),
        num(&get(&nature, "low_integrity_percentage")?, 2, false, "[x]"),
    );
    let medium = get(&nature, "medium_integrity_percentage")?;
    let high = get(&nature, "high_integrity_percentage")?;
    let medium_high = if !medium.is_null() || !high.is_null() {
        py_add(&or(&medium, json!(0)), &or(&high, json!(0)))?
    } else {
        Value::Null
    };
    data.insert(
        "flii_medium_high_pct".into(),
        num(&medium_high, 2, false, "[x]"),
    );

    let mut classes: Vec<Value> = Vec::new();
    for c in py_iter(&or(&get(&site, "land_cover_class")?, json!([])))? {
        if c.is_object() && !py_eq(&get(&c, "id")?, &json!("0")) {
            let name = py_str(&get_or(&c, "name", json!(""))?).to_lowercase();
            if name.contains("forest") || name.contains("mangrove") {
                classes.push(get(&c, "name")?);
            }
        }
    }
    classes.truncate(3);
    data.insert("forest_types".into(), forest_types(&classes)?);

    for (key, rows_key, value_key) in [
        (
            "precipitation",
            "historical_precipitations",
            "precipitation",
        ),
        ("temperature", "historical_temperatures", "temperature"),
    ] {
        let block = match monthly_series(&get(&climate, rows_key)?, value_key)? {
            Some(series) => {
                let mut graph = series_stats(&series)?;
                if key == "precipitation" {
                    graph.insert("dry_season".into(), dry_season(&series)?);
                }
                json!({"graph_data": series, "graph": graph})
            }
            None => json!({"graph_data": vec![""; 12], "graph": null}),
        };
        data.insert(key.into(), block);
    }
    Ok(data)
}

/// `', '.join(classes[:-1]) + ' and ' + classes[-1]` / `classes[0]` / the bracket text.
fn forest_types(classes: &[Value]) -> R<Value> {
    let s = |v: &Value| match v {
        Value::String(s) => Ok(s.clone()),
        other => Err(format!(
            "can only concatenate str (not \"{}\") to str",
            py_type(other)
        )),
    };
    Ok(match classes {
        [] => json!("[forest type 1], [forest type 2], and [forest type 3]"),
        [one] => one.clone(),
        [head @ .., last] => {
            let head: Vec<String> = head
                .iter()
                .enumerate()
                .map(|(i, v)| match v {
                    Value::String(x) => Ok(x.clone()),
                    other => Err(format!(
                        "sequence item {i}: expected str instance, {} found",
                        py_type(other)
                    )),
                })
                .collect::<R<_>>()?;
            json!(format!("{} and {}", head.join(", "), s(last)?))
        }
    })
}

/// `string.capwords(s)` of `site.get(k) or ''`.
fn capwords(v: &Value) -> R<String> {
    let Value::String(s) = v else {
        return Err(format!("'{}' object has no attribute 'split'", py_type(v)));
    };
    Ok(s.split(py_space)
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                // shortcut: str.capitalize title-cases the first character; upper-casing differs
                // only for the four Latin digraph letters (U+01C4..U+01CC)
                Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" "))
}

/// `GeoLogic.construct_polygon(session_id)`'s `total_bounds`: a Polygon keeps its exterior ring
/// (`Polygon(coordinates[0])`), a MultiPolygon all of it (`shape(res)`).
fn bounds(geojson: Option<Option<&str>>) -> R<(f64, f64, f64, f64)> {
    let text = match geojson {
        None => return Err("'NoneType' object is not subscriptable".into()),
        Some(None) => {
            return Err("the JSON object must be str, bytes or bytearray, not NoneType".into());
        }
        Some(Some(t)) => t,
    };
    let res: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let kind = res["type"].as_str().unwrap_or_default().to_lowercase();
    let coords = &res["coordinates"];
    let rings: Vec<&Value> = match kind.as_str() {
        "polygon" => coords.get(0).into_iter().collect(),
        "multipolygon" => coords
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|p| p.as_array().into_iter().flatten())
            .collect(),
        // shortcut: other geometry types fail in geopandas with their own message
        other => return Err(format!("unsupported geometry type {other}")),
    };
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for point in rings
        .iter()
        .flat_map(|r| r.as_array().into_iter().flatten())
    {
        let (Some(x), Some(y)) = (
            point.get(0).and_then(Value::as_f64),
            point.get(1).and_then(Value::as_f64),
        ) else {
            return Err("bad coordinate".into());
        };
        b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
    }
    if !b.0.is_finite() {
        // shortcut: an empty geometry gives NaN bounds in geopandas; not reachable from an upload
        return Err("empty geometry".into());
    }
    Ok(b)
}

/// `box(minx, miny, maxx, maxy).centroid`: GEOS `Centroid` over the ring shapely's `box` builds
/// (counter-clockwise from (maxx, miny)), triangle by triangle from the first point. Not the
/// midpoint: the last bits differ.
fn box_centroid(minx: f64, miny: f64, maxx: f64, maxy: f64) -> (f64, f64) {
    let pts = [
        (maxx, miny),
        (maxx, maxy),
        (minx, maxy),
        (minx, miny),
        (maxx, miny),
    ];
    let base = pts[0];
    let (mut cx, mut cy, mut area) = (0.0, 0.0, 0.0);
    for w in pts.windows(2) {
        let (p1, p2) = (w[0], w[1]);
        let (tx, ty) = (base.0 + p1.0 + p2.0, base.1 + p1.1 + p2.1);
        let a2 = (p1.0 - base.0) * (p2.1 - base.1) - (p2.0 - base.0) * (p1.1 - base.1);
        // the ring is CCW, so GEOS weighs with sign -1
        cx += -a2 * tx;
        cy += -a2 * ty;
        area += -a2;
    }
    if area == 0.0 {
        // shortcut: a degenerate box falls back to GEOS's line/point centroid; midpoint here
        return ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
    }
    (cx / 3.0 / area, cy / 3.0 / area)
}

/// `GeoLogic.coords_to_string(longitude, latitude)`
pub fn coords_to_string(longitude: f64, latitude: f64) -> Value {
    json!({"longitude": dms(longitude, 'E', 'W'), "latitude": dms(latitude, 'N', 'S')})
}

fn dms(value: f64, positive: char, negative: char) -> String {
    let seconds = py_round(value.abs() * 3600.0, 1);
    let (degrees, rest) = py_divmod(seconds, 3600.0);
    let (minutes, seconds) = py_divmod(rest, 60.0);
    format!(
        "{:02}°{:02}'{:0>4}\"{}",
        degrees as i64,
        minutes as i64,
        fmt_f(seconds, 1, false),
        if value >= 0.0 { positive } else { negative }
    )
}

/// `round(f, n)`: correctly rounded, ties to even on the exact binary value (as `{:.n}`).
fn py_round(f: f64, n: usize) -> f64 {
    if !f.is_finite() {
        return f;
    }
    format!("{f:.n$}").parse().unwrap_or(f)
}

/// Python's float `divmod(x, y)` (`float_divmod`).
fn py_divmod(x: f64, y: f64) -> (f64, f64) {
    let mut m = x % y;
    let mut div = (x - m) / y;
    if m != 0.0 {
        if (y < 0.0) != (m < 0.0) {
            m += y;
            div -= 1.0;
        }
    } else {
        m = 0.0f64.copysign(y);
    }
    let floordiv = if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        0.0f64.copysign(x / y)
    };
    (floordiv, m)
}

/// `_monthly_series(rows, value_key)`
fn monthly_series(rows: &Value, value_key: &str) -> R<Option<Vec<Value>>> {
    // {month: value}: a later row overwrites the value, the first equal key stays
    let mut by_month: Vec<(Value, Value)> = Vec::new();
    for r in py_iter(&or(rows, json!([])))? {
        if !r.is_object() {
            continue;
        }
        let month = get(&r, "month")?;
        let value = get(&r, value_key)?;
        hashable(&month)?;
        match by_month.iter_mut().find(|(m, _)| py_eq(m, &month)) {
            Some(slot) => slot.1 = value,
            None => by_month.push((month, value)),
        }
    }
    let mut series = Vec::with_capacity(12);
    for m in 1..=12 {
        let v = by_month
            .iter()
            .find(|(k, _)| py_eq(k, &json!(m)))
            .map_or(Value::Null, |(_, v)| v.clone());
        series.push(v);
    }
    if series.iter().any(|v| is_num(v).is_none()) {
        return Ok(None);
    }
    Ok(Some(
        series
            .into_iter()
            .map(|v| match v {
                Value::Bool(b) => json!(b as i64),
                Value::Number(n) if !n.is_f64() => Value::Number(n),
                v => json!(py_round(v.as_f64().unwrap_or(f64::NAN), 1)),
            })
            .collect(),
    ))
}

/// The first element equal to the series' max (`better(x, best)`) / min.
fn extreme(series: &[Value], better: fn(f64, f64) -> bool) -> Value {
    let mut best = &series[0];
    for v in &series[1..] {
        if better(
            v.as_f64().unwrap_or(f64::NAN),
            best.as_f64().unwrap_or(f64::NAN),
        ) {
            best = v;
        }
    }
    best.clone()
}

fn index_of(series: &[Value], v: &Value) -> usize {
    series.iter().position(|x| py_eq(x, v)).unwrap_or(0)
}

/// `_series_stats(series)`
fn series_stats(series: &[Value]) -> R<Map<String, Value>> {
    let max = extreme(series, |x, b| x > b);
    let min = extreme(series, |x, b| x < b);
    let total = py_sum_numbers(series).ok_or("sum")?;
    let total_f = total.as_f64().unwrap_or(f64::NAN);
    let mut stats = Map::new();
    stats.insert(
        "max_month".into(),
        json!(MONTH_ABBR[index_of(series, &max)]),
    );
    stats.insert(
        "min_month".into(),
        json!(MONTH_ABBR[index_of(series, &min)]),
    );
    let mut out = Map::new();
    out.insert("max".into(), max);
    out.insert("min".into(), min);
    out.insert(
        "mean".into(),
        json!(py_round(total_f / series.len() as f64, 1)),
    );
    out.insert("total".into(), json!(fmt_f(total_f, 1, true)));
    out.extend(stats);
    Ok(out)
}

/// `_dry_season(series)`
fn dry_season(series: &[Value]) -> R<Value> {
    let s: Vec<f64> = series
        .iter()
        .map(|v| v.as_f64().unwrap_or(f64::NAN))
        .collect();
    let min = extreme(series, |x, b| x < b);
    let peak = index_of(series, &min);
    let (mut start, mut end, mut total) = (peak, peak, 0usize);
    let order: Vec<usize> = (0..12).map(|n| (n + peak) % 12).collect();
    for &mi in order.iter().rev() {
        if (s[mi] - s[(mi + 11) % 12]).abs() > 30.0 {
            start = mi;
            total += order.iter().position(|&o| o == mi).unwrap_or(0) + 1;
            break;
        }
    }
    for &mi in &order {
        if (s[mi] - s[(mi + 1) % 12]).abs() > 30.0 {
            end = mi;
            total += order.iter().position(|&o| o == mi).unwrap_or(0);
            break;
        }
    }
    Ok(json!({
        "peak": peak, "start": start, "end": end,
        "peak_month": MONTH_ABBR[peak], "start_month": MONTH_ABBR[start], "end_month": MONTH_ABBR[end],
        "category": if total <= 3 { "short" } else { "long" },
    }))
}

/// `_tpl_data_form(document_data)`: `ghg_period_years` and `fauna`.
pub fn tpl_data_form(section_1: &Value, section_5: &Value) -> R<Vec<(String, Value)>> {
    let s1 = if section_1.is_object() {
        section_1.clone()
    } else {
        json!({})
    };
    let s5 = if section_5.is_object() {
        section_5.clone()
    } else {
        json!({})
    };
    let start = strptime_ymd(&or(&get(&s1, "ghg_start")?, json!("")))?;
    let period = match start {
        None => "[xx]".to_string(),
        Some(start) => match strptime_ymd(&or(&get(&s1, "ghg_end")?, json!("")))? {
            None => "[xx]".to_string(),
            Some(end) => fmt_f((end - start) as f64 / 365.25, 0, false),
        },
    };
    let mut fauna = Vec::new();
    for key in [
        "proj_birds",
        "proj_mammals",
        "proj_amphibians",
        "proj_reptiles",
    ] {
        for row in py_iter(&or(&get(&s5, key)?, json!([])))? {
            if row.is_object() {
                fauna.push(row);
            }
        }
    }
    Ok(vec![
        ("ghg_period_years".into(), json!(period)),
        ("fauna".into(), Value::Array(fauna)),
    ])
}

/// `datetime.strptime(v, '%Y-%m-%d')` as a day number; Ok(None) for the ValueError the caller
/// catches. shortcut: `\d` is ASCII here, Python also takes other Unicode decimal digits.
fn strptime_ymd(v: &Value) -> R<Option<i64>> {
    let Value::String(s) = v else {
        return Err(format!(
            "strptime() argument 1 must be str, not {}",
            py_type(v)
        ));
    };
    let b = s.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    let num = |i: usize, n: usize| s[i..i + n].trim_start().parse::<u32>().unwrap_or(0);
    if !(0..4).all(digit) || !at(4, b'-') {
        return Ok(None);
    }
    // the regex's alternatives in order: the first that lets the whole pattern match wins
    let months: [(bool, usize); 3] = [
        (
            at(5, b'1') && b.get(6).is_some_and(|c| (b'0'..=b'2').contains(c)),
            2,
        ),
        (
            at(5, b'0') && b.get(6).is_some_and(|c| (b'1'..=b'9').contains(c)),
            2,
        ),
        (b.get(5).is_some_and(|c| (b'1'..=b'9').contains(c)), 1),
    ];
    for (ok, mlen) in months {
        if !ok || !at(5 + mlen, b'-') {
            continue;
        }
        let d = 6 + mlen;
        let nz = |i: usize| b.get(i).is_some_and(|c| (b'1'..=b'9').contains(c));
        let days: [(bool, usize); 5] = [
            (at(d, b'3') && (at(d + 1, b'0') || at(d + 1, b'1')), 2),
            ((at(d, b'1') || at(d, b'2')) && digit(d + 1), 2),
            (at(d, b'0') && nz(d + 1), 2),
            (nz(d), 1),
            (at(d, b' ') && nz(d + 1), 2),
        ];
        let Some(&(_, dlen)) = days.iter().find(|(ok, _)| *ok) else {
            continue;
        };
        if d + dlen != b.len() {
            return Ok(None); // "unconverted data remains"
        }
        let (y, m, day) = (num(0, 4), num(5, mlen), num(d, dlen));
        return Ok(days_from_civil(y, m, day));
    }
    Ok(None)
}

/// Proleptic Gregorian day number; None for a date `datetime` rejects (year 0, Feb 30, ...).
fn days_from_civil(y: u32, m: u32, d: u32) -> Option<i64> {
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let dim = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if y == 0 || !(1..=12).contains(&m) || d == 0 || d > dim[m as usize - 1] {
        return None;
    }
    let (y, m, d) = (y as i64 - (m <= 2) as i64, m as i64, d as i64);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    Some(era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy)
}

/// The CSV `generate_document_form` writes (`csv.DictWriter`, fieldnames = the data's keys, one
/// row): `str()` of each value (None as empty), QUOTE_MINIMAL, `\r\n` line ends.
pub fn csv(data: &Value) -> String {
    let field = |s: &str| {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let Value::Object(map) = data else {
        return String::new();
    };
    let header: Vec<String> = map.keys().map(|k| field(k)).collect();
    let row: Vec<String> = map
        .values()
        .map(|v| {
            if v.is_null() {
                String::new()
            } else {
                field(&py_str(v))
            }
        })
        .collect();
    format!("{}\r\n{}\r\n", header.join(","), row.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_helpers() {
        // GeoLogic.coords_to_string(107.947749, -7.3143333)
        assert_eq!(
            coords_to_string(107.947749, -7.3143333),
            json!({"longitude": "107°56'51.9\"E", "latitude": "07°18'51.6\"S"})
        );
        assert_eq!(
            strptime_ymd(&json!("2020-1-5")).ok().flatten(),
            days_from_civil(2020, 1, 5)
        );
        assert_eq!(strptime_ymd(&json!("2020-02-30")).ok().flatten(), None);
        assert_eq!(strptime_ymd(&json!("2020-01-35")).ok().flatten(), None);
        assert_eq!(
            strptime_ymd(&json!("2020-01- 5")).ok().flatten(),
            days_from_civil(2020, 1, 5)
        );
        assert_eq!(
            days_from_civil(2030, 1, 1)
                .zip(days_from_civil(2020, 1, 1))
                .map(|(a, b)| a - b),
            Some(3653)
        );
        assert!(strptime_ymd(&json!(5)).is_err());
        assert_eq!(
            capwords(&json!("  kOTA  bandung\tbarat")).ok(),
            Some("Kota Bandung Barat".into())
        );
    }
}
