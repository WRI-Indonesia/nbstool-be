//! Component 2.5 Key Species Presence (`nature/key_species_presence.py`).
//!
//! GBIF points of `sea.key_species_v3` inside the AOI (`db::load_key_species_intersecting`),
//! grouped by species as the pandas `groupby(...).agg(...)` does: modal class / family / basis
//! (ties to the smallest text, `Series.mode` sorts), summed numeric individualCount, record count,
//! latest eventDate (`pd.to_datetime(format="mixed", utc=True)`), sorted by count desc, species asc.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Result;
use crate::common::Aoi;
use crate::db::{Feature, load_key_species_intersecting};

/// KEY_SPECIES_CLASS_FIELDS, in dict order.
const CLASS_FIELDS: [(&str, &str); 4] = [
    ("Aves", "bird_number_of_species"),
    ("Mammalia", "mammal_number_of_species"),
    ("Reptilia", "reptile_number_of_species"),
    ("Amphibia", "amphibian_number_of_species"),
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A UTC timestamp as (year, month, day, hour, minute, second).
type Stamp = (i64, u32, u32, u32, u32, u32);

fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `pd.to_datetime(x, errors="coerce", format="mixed", utc=True)` on the eventDate shapes the
/// table holds (2026-11: `YYYY`, `YYYY-MM`, `YYYY-MM-DD[THH:MM[:SS]][Z]`), None for NaT.
// shortcut: interval values ("A/B") are NaT here. pandas hands them to dateutil, which turns
// 7 of the table's 8 distinct intervals into NaT but "2022-07-25/2022-08-10" into
// 2022-07-26 06:22 UTC; port dateutil's tokenizer if such a record must reach a card. UTC offsets
// ("+07:00") are not in the table and are NaT here too.
pub fn parse_event_date(s: &str) -> Option<Stamp> {
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t.strip_suffix('Z').unwrap_or(t))),
        None => (s, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    let (y, m, d) = match parts.as_slice() {
        [y] => (digits(y, 4)?, 1, 1),
        [y, m] => (digits(y, 4)?, digits(m, 2)?, 1),
        [y, m, d] => (digits(y, 4)?, digits(m, 2)?, digits(d, 2)?),
        _ => return None,
    };
    if time.is_some() && parts.len() != 3 {
        return None;
    }
    let (hh, mm, ss) = match time {
        None => (0, 0, 0),
        Some(t) => match t.split(':').collect::<Vec<_>>().as_slice() {
            [h, mi] => (digits(h, 2)?, digits(mi, 2)?, 0),
            [h, mi, se] => (digits(h, 2)?, digits(mi, 2)?, digits(se, 2)?),
            _ => return None,
        },
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
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
    if !(1..=12).contains(&m) || d < 1 || d > days[m as usize - 1] || hh > 23 || mm > 59 || ss > 59
    {
        return None;
    }
    let stamp = (y as i64, m, d, hh, mm, ss);
    // pandas' nanosecond bounds, 1677-09-21 00:12:43.145224193 .. 2262-04-11 23:47:16.854775807
    (stamp > (1677, 9, 21, 0, 12, 43) && stamp <= (2262, 4, 11, 23, 47, 16)).then_some(stamp)
}

/// `most_frequent(series)`: `series.dropna().mode().iat[0]`, None when every entry is null.
fn most_frequent<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for v in values.flatten() {
        *counts.entry(v).or_default() += 1;
    }
    let top = *counts.values().max()?;
    counts
        .into_iter()
        .find(|(_, c)| *c == top)
        .map(|(v, _)| v.to_string())
}

/// `analyze_key_species(aoi)`
pub async fn analyze_key_species(gis: &PgPool, aoi: &Aoi) -> Result<(Value, Value)> {
    let points = load_key_species_intersecting(gis, aoi).await?;
    Ok(key_species_from(&points))
}

fn view(rows: &[Value]) -> Value {
    json!({"key_species": rows.iter().map(|r| json!({
        "species_id": r["species"],
        "number_of_occurences": r["record_count"],
        "latest_record_year": r["latest_year"],
        "family_id": r["taxon_class"],
    })).collect::<Vec<_>>()})
}

/// The body after the load.
pub fn key_species_from(points: &[Feature]) -> (Value, Value) {
    let text = |f: &Feature, k: &str| f.get(k).as_str().map(str::to_string);
    let points: Vec<&Feature> = points
        .iter()
        .filter(|f| !f.get("species").is_null())
        .collect();
    if points.is_empty() {
        let results = json!({
            "narrative": "No key species occurrence records fall inside this project area.",
            "tables": {"species": []},
            "values": {"species_count": 0, "record_count": 0},
            "flags": [],
        });
        return (results, view(&[]));
    }

    let mut groups: BTreeMap<String, Vec<&Feature>> = BTreeMap::new();
    for f in &points {
        groups
            .entry(crate::db::py_str(f.get("species")))
            .or_default()
            .push(f);
    }

    struct Rec {
        species: String,
        taxon_class: String,
        family: String,
        total: f64,
        count: usize,
        latest: Option<Stamp>,
        basis: Option<String>,
    }
    let mut recs: Vec<Rec> = groups
        .into_iter()
        .map(|(species, rows)| {
            let class = most_frequent(rows.iter().map(|f| f.get("taxon_class").as_str()));
            let family = most_frequent(rows.iter().map(|f| f.get("family").as_str()));
            let basis = most_frequent(rows.iter().map(|f| f.get("basis_of_record").as_str()));
            // pd.to_numeric(errors="coerce"), NaN skipped by the sum (0.0 when none)
            let total = rows
                .iter()
                .filter_map(|f| {
                    text(f, "individual_count")
                        .and_then(|s| s.parse::<f64>().ok())
                        .filter(|v| !v.is_nan())
                })
                .fold(0.0, |a, b| a + b);
            let latest = rows
                .iter()
                .filter_map(|f| text(f, "event_date").and_then(|s| parse_event_date(&s)))
                .max();
            Rec {
                species,
                taxon_class: class.unwrap_or_default(),
                family: family.unwrap_or_default(),
                total,
                count: rows.len(),
                latest,
                basis: basis.filter(|b| !b.is_empty()),
            }
        })
        .collect();
    recs.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.species.cmp(&b.species))
    });

    let rows: Vec<Value> = recs
        .iter()
        .map(|r| {
            json!({
                "species": r.species,
                "taxon_class": r.taxon_class,
                "family": r.family,
                "total_occurrence": r.total,
                "record_count": r.count,
                "latest_encounter": r.latest.map(|(y, m, d, ..)| format!("{d:02} {} {y}", MONTHS[m as usize - 1])),
                "basis_of_record": r.basis,
                "latest_year": r.latest.map(|s| s.0),
            })
        })
        .collect();

    let mut counts = serde_json::Map::new();
    for (_, field) in CLASS_FIELDS {
        counts.insert(field.into(), json!(0));
    }
    for r in &recs {
        if let Some((_, field)) = CLASS_FIELDS.iter().find(|(c, _)| *c == r.taxon_class) {
            let n = counts[*field].as_i64().unwrap_or(0) + 1;
            counts.insert((*field).into(), json!(n));
        }
    }
    let mut unmapped: Vec<&str> = recs
        .iter()
        .map(|r| r.taxon_class.as_str())
        .filter(|c| !CLASS_FIELDS.iter().any(|(k, _)| k == c))
        .collect();
    unmapped.sort();
    unmapped.dedup();
    let mut flags: Vec<String> = Vec::new();
    if !unmapped.is_empty() {
        flags.push(format!(
            "2.5: {} recorded in the AOI but the card has no heading for that class, so those species may not render.",
            unmapped.join(", ")
        ));
    }

    let named = recs
        .iter()
        .take(5)
        .map(|r| r.species.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let more = if recs.len() > 5 {
        format!(", and {} more", recs.len() - 5)
    } else {
        String::new()
    };
    let results = json!({
        "narrative": format!("{} key species have been recorded in this project area, including {named}{more}.", recs.len()),
        "tables": {"species": rows},
        "values": {
            "species_count": recs.len(),
            "record_count": recs.iter().map(|r| r.count).sum::<usize>(),
            "class_counts": counts,
        },
        "flags": flags,
    });
    let v = view(&rows);
    (results, v)
}

#[cfg(test)]
mod tests {
    use super::parse_event_date;

    #[test]
    fn event_dates_as_pandas() {
        assert_eq!(parse_event_date("2019"), Some((2019, 1, 1, 0, 0, 0)));
        assert_eq!(parse_event_date("2010-12"), Some((2010, 12, 1, 0, 0, 0)));
        assert_eq!(
            parse_event_date("2015-03-04T23:06Z"),
            Some((2015, 3, 4, 23, 6, 0))
        );
        assert_eq!(
            parse_event_date("2015-03-04T05:06:07"),
            Some((2015, 3, 4, 5, 6, 7))
        );
        assert_eq!(parse_event_date("2015-02-30"), None);
        assert_eq!(parse_event_date("1500-01-01"), None);
        assert_eq!(parse_event_date("2006-07-01/2006-07-31"), None);
        assert_eq!(parse_event_date("2012-01/2012-02"), None);
    }
}
