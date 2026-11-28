//! Component 6.3 Social Statistics (`people/social_statistics.py`).
//!
//! National statistics tables (`se_v3`, foreign tables of the APP database) for the administrative
//! area 1.2 names. The config tables it runs (`COUNTRY_ISO3`, `SOCIAL_INDICATORS`,
//! `SOCIAL_AREA_NAME_FIELDS`, `SOCIAL_NO_AREA_NAME`, `SOCIAL_LEVEL_SOURCES`,
//! `SOCIAL_MAPPING_TOPICS`) are `social_statistics.json`, dumped from config.py in dict order by
//! `proof/run_python_vector.py --social-config`.
//!
//! UNPROVEN against the database: the local copy's se_v3 foreign tables carry no credentials, so
//! `load_social_rows` never ran. The body is unit-tested on hand-made rows shaped as the SQL returns
//! them (see `tests` below). The PH region lookup (`load_statistical_area`) is on the GIS database.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::common::Aoi;
use crate::db::{MappingKeys, SocialRow, load_social_rows, load_statistical_area};
use crate::py::fsum_py;
use crate::{Error, Result};

fn config() -> &'static Value {
    static CONFIG: OnceLock<Value> = OnceLock::new();
    CONFIG.get_or_init(|| {
        serde_json::from_str(include_str!("social_statistics.json")).unwrap_or(Value::Null)
    })
}

/// `_LEVEL_KEYS`
fn level_key(level: i64) -> Option<&'static str> {
    match level {
        1 => Some("dominant_province"),
        2 => Some("dominant_district"),
        3 => Some("dominant_subdistrict"),
        _ => None,
    }
}

/// `_KNOWN_ABSENT`
fn known_absent(iso3: &str) -> &'static [&'static str] {
    match iso3 {
        "brn" => &["household_number"],
        "idn" => &["household_number", "students_enrolled_number"],
        "vnm" => &["household_number", "permanent_reserved_forest_area"],
        "sgp" => &["permanent_reserved_forest_area"],
        "phl" => &["population_education_levels"],
        _ => &[],
    }
}

/// Where 6.3's two lookups go. `Db` is the real pair of pools; tests supply rows.
pub trait SocialSource {
    /// `load_statistical_area(aoi, table, column)`
    fn statistical_area(
        &self,
        table: &str,
        column: &str,
    ) -> impl Future<Output = Result<Option<String>>>;
    /// `load_social_rows(iso3, table, level, area_name, where)`
    fn social_rows(
        &self,
        iso3: &str,
        table: &str,
        level: i64,
        area_name: Option<&str>,
        filters: &[(String, Option<String>)],
    ) -> impl Future<Output = Result<(Option<i64>, Vec<SocialRow>)>>;
}

/// The GIS pool (PH region layers) and the app-database pool (se_v3), for one AOI.
pub struct Db<'a> {
    pub gis: &'a PgPool,
    pub social: &'a PgPool,
    pub aoi: &'a Aoi,
}

impl SocialSource for Db<'_> {
    async fn statistical_area(&self, table: &str, column: &str) -> Result<Option<String>> {
        load_statistical_area(self.gis, self.aoi, table, column).await
    }
    async fn social_rows(
        &self,
        iso3: &str,
        table: &str,
        level: i64,
        area_name: Option<&str>,
        filters: &[(String, Option<String>)],
    ) -> Result<(Option<i64>, Vec<SocialRow>)> {
        load_social_rows(self.social, iso3, table, level, area_name, filters).await
    }
}

/// Python `repr()` of a str (or None): single quotes unless the text holds a single quote and no
/// double quote, backslash and control characters escaped.
fn py_repr(s: Option<&str>) -> String {
    let Some(s) = s else { return "None".into() };
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
            // shortcut: only C0/DEL/C1 controls are escaped; Python also escapes other
            // non-printable code points (unassigned, separators), absent from admin names.
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn cat(c: &Option<String>) -> Value {
    c.as_ref().map_or(Value::Null, |s| Value::String(s.clone()))
}

/// `_percentage_of(rows, exclude)`
fn percentage_of(rows: &[SocialRow], exclude: &[String]) -> Value {
    let kept: Vec<&SocialRow> = rows
        .iter()
        .filter(|(c, _, _)| !is_excluded(c, exclude))
        .collect();
    if kept.is_empty() {
        return json!([]);
    }
    if kept.iter().all(|(_, u, _)| u.as_deref() == Some("percent")) {
        return Value::Array(
            kept.iter()
                .map(|(c, _, v)| json!({"id": cat(c), "percentage": v}))
                .collect(),
        );
    }
    let total = fsum_py(kept.iter().map(|r| r.2));
    Value::Array(
        kept.iter()
            .map(|(c, _, v)| json!({"id": cat(c), "percentage": if total != 0.0 { json!(v / total * 100.0) } else { Value::Null }}))
            .collect(),
    )
}

fn is_excluded(c: &Option<String>, exclude: &[String]) -> bool {
    c.as_ref().is_some_and(|c| exclude.iter().any(|e| e == c))
}

/// `_read(rows, kind, exclude)`
fn read(rows: &[SocialRow], kind: &str, exclude: &[String]) -> Result<Value> {
    let kept: Vec<&SocialRow> = rows
        .iter()
        .filter(|(c, _, _)| !is_excluded(c, exclude))
        .collect();
    Ok(match kind {
        "value" => {
            if kept.is_empty() {
                Value::Null
            } else {
                json!(fsum_py(kept.iter().map(|r| r.2)))
            }
        }
        "shares" => percentage_of(rows, exclude),
        "category_values" => Value::Array(
            kept.iter()
                .map(|(c, _, v)| json!({"id": cat(c), "value": v}))
                .collect(),
        ),
        "diseases" => Value::Array(
            kept.iter()
                .map(|(c, _, _)| json!({"id": cat(c), "contagious_id": null}))
                .collect(),
        ),
        "top_id" | "top_value" => {
            // max(kept, key=value): the first of equal maxima
            let mut best: Option<&SocialRow> = None;
            for r in &kept {
                if best.is_none_or(|b| r.2 > b.2) {
                    best = Some(r);
                }
            }
            match best {
                None => Value::Null,
                Some((c, _, v)) => {
                    if kind == "top_id" {
                        cat(c)
                    } else {
                        json!(v)
                    }
                }
            }
        }
        "coverage_pct" => {
            let total = fsum_py(rows.iter().map(|r| r.2));
            if total != 0.0 {
                json!(fsum_py(kept.iter().map(|r| r.2)) / total * 100.0)
            } else {
                Value::Null
            }
        }
        other => {
            return Err(Error::new(
                "ValueError",
                format!("unknown read kind '{other}'"),
            ));
        }
    })
}

/// `_section_level(fields)`: the most used level, ties to the coarser (smaller) one.
fn section_level(fields: &Map<String, Value>) -> i64 {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for f in fields.values() {
        *counts.entry(f["level"].as_i64().unwrap_or(0)).or_default() += 1;
    }
    counts
        .iter()
        .min_by_key(|(level, n)| (std::cmp::Reverse(**n), **level))
        .map_or(0, |(l, _)| *l)
}

fn truthy_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `analyze_social_statistics(aoi, admin_values)` on the real databases.
pub async fn analyze_social_statistics(
    gis: &PgPool,
    social: &PgPool,
    keys: &MappingKeys,
    aoi: &Aoi,
    admin_values: Option<&Value>,
) -> Result<(Value, Value)> {
    social_statistics_from(&Db { gis, social, aoi }, keys, admin_values).await
}

/// The component body over any `SocialSource`.
pub async fn social_statistics_from(
    src: &impl SocialSource,
    keys: &MappingKeys,
    admin_values: Option<&Value>,
) -> Result<(Value, Value)> {
    let empty = json!({});
    let admin = admin_values.filter(|v| v.is_object()).unwrap_or(&empty);
    let cfg = config();
    let country = truthy_str(&admin["dominant_country"]);
    let mut flags: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut retryable = false;

    let Some(country) = country else {
        let results = json!({
            "narrative": "",
            "tables": {},
            "values": {"country": null, "iso3": null},
            "missing": ["6.3: no administrative area was resolved for this AOI, so no national statistics could be looked up."],
        });
        return Ok((results, json!({})));
    };

    let iso3 = cfg["COUNTRY_ISO3"][&country].as_str().map(str::to_string);
    let indicators = iso3
        .as_deref()
        .and_then(|i| cfg["SOCIAL_INDICATORS"][i].as_object());
    let (Some(iso3), Some(indicators)) = (iso3.clone(), indicators) else {
        let results = json!({
            "narrative": "",
            "tables": {},
            "values": {"country": country, "iso3": iso3},
            "missing": [format!("6.3: no social statistics are loaded for {country}.")],
        });
        return Ok((results, json!({})));
    };

    // truthiness of admin_values.get('transboundary')
    if admin["transboundary"].as_bool() == Some(true) {
        flags.push(format!(
            "6.3: the AOI crosses a national border; every figure below is for {country}, the country holding the \
             largest share of it."
        ));
    }
    let provinces: Vec<String> = admin["provinces"]
        .as_array()
        .map(|a| a.iter().map(crate::db::py_str).collect())
        .unwrap_or_default();
    if provinces.len() > 1 {
        flags.push(format!(
            "6.3: the AOI spans {} provinces ({}); province-level figures are for {}, the largest share.",
            provinces.len(),
            provinces.join(", "),
            crate::db::py_str(&admin["dominant_province"]),
        ));
    }

    let mut candidates: HashMap<i64, Vec<String>> = HashMap::new();
    let mut area_sources: HashMap<i64, String> = HashMap::new();
    for level in 0..=3 {
        let names = level_key(level)
            .and_then(|k| truthy_str(&admin[k]))
            .into_iter()
            .collect();
        candidates.insert(level, names);
        area_sources.insert(level, "1.2".into());
    }

    if let Some(sources) = cfg["SOCIAL_LEVEL_SOURCES"][&iso3].as_object() {
        for (level, source) in sources {
            let level: i64 = level
                .parse()
                .map_err(|_| Error::new("KeyError", level.clone()))?;
            if let Some(key) = source.as_str() {
                candidates.insert(level, truthy_str(&admin[key]).into_iter().collect());
                area_sources.insert(
                    level,
                    format!("1.2's {}", key.strip_prefix("dominant_").unwrap_or(key)),
                );
                continue;
            }
            let pairs: Vec<(String, String)> = source
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut resolved: Vec<String> = Vec::new();
            for (table, column) in &pairs {
                match src.statistical_area(table, column).await {
                    Err(e) => {
                        flags.push(format!(
                            "6.3: {table} could not be read ({}), so a level-{level} figure for {country} may be null. \
                             Falling back to 1.2's name would not help: it names a different administrative tier and \
                             matches no row.",
                            e.kind
                        ));
                        retryable = true;
                    }
                    Ok(Some(name)) if !name.is_empty() && !resolved.contains(&name) => {
                        resolved.push(name)
                    }
                    Ok(_) => {}
                }
            }
            candidates.insert(level, resolved);
            area_sources.insert(
                level,
                pairs
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" then "),
            );
        }
    }

    let area_names: HashMap<i64, Option<String>> = candidates
        .iter()
        .map(|(l, n)| (*l, n.first().cloned()))
        .collect();
    let area_name_of = |level: i64| area_names.get(&level).cloned().flatten();

    if let Some(c1) = candidates.get(&1).filter(|c| c.len() > 1) {
        notes.push(format!(
            "6.3: this area sits in the part of {country} that was re-districted, so the statistics tables disagree \
             about which region it is in: {} in the newer tables, {} in the older ones. Each figure below is labelled \
             with the region its own table uses.",
            py_repr(Some(&c1[0])),
            py_repr(Some(&c1[1])),
        ));
    }
    if cfg["SOCIAL_LEVEL_SOURCES"].get(&iso3).is_some() {
        notes.push(format!(
            "6.3: the statistics for {country} are published one administrative tier above the boundary layer, so the \
             areas below are not the ones the ADMINISTRATIVE BOUNDARIES card names. Level 1 is {} and level 2 is {}, \
             against 1.2's province {} and district {}. Both are correct at their own tier.",
            py_repr(area_name_of(1).as_deref()),
            py_repr(area_name_of(2).as_deref()),
            py_repr(admin["dominant_province"].as_str()),
            py_repr(admin["dominant_district"].as_str()),
        ));
    }

    let no_area_name = cfg["SOCIAL_NO_AREA_NAME"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == iso3.as_str()));
    let mut sections = Map::new();
    let mut values = Map::new();
    let mut missing: BTreeMap<String, String> = BTreeMap::new();

    for (section, fields) in indicators {
        let Some(fields) = fields.as_object() else {
            continue;
        };
        let mut payload = Map::new();
        let mut sources = Map::new();
        let mut used: HashMap<i64, String> = HashMap::new();

        for (field, spec) in fields {
            let level = spec["level"].as_i64().unwrap_or(0);
            let table = spec["table"].as_str().unwrap_or_default();
            let names: Vec<Option<String>> = if level != 0 {
                candidates
                    .get(&level)
                    .map(|n| n.iter().cloned().map(Some).collect())
                    .unwrap_or_default()
            } else {
                vec![None]
            };
            if level != 0 && names.is_empty() {
                payload.insert(field.clone(), Value::Null);
                let key = level_key(level).unwrap_or_default();
                missing.insert(
                    field.clone(),
                    format!(
                        "no {} from {}",
                        key.strip_prefix("dominant_").unwrap_or(key),
                        area_sources[&level]
                    ),
                );
                continue;
            }
            let filters: Vec<(String, Option<String>)> = spec["where"]
                .as_object()
                .map(|w| {
                    w.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                        .collect()
                })
                .unwrap_or_default();
            let exclude: Vec<String> = spec["exclude"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let mut year = None;
            let mut rows: Vec<SocialRow> = Vec::new();
            let mut area_name: Option<String> = None;
            let mut undefined = false;
            for name in &names {
                area_name = name.clone();
                match src
                    .social_rows(&iso3, table, level, name.as_deref(), &filters)
                    .await
                {
                    Ok((y, r)) => {
                        year = y;
                        rows = r;
                        if !rows.is_empty() {
                            break;
                        }
                    }
                    Err(e) if e.kind == "ProgrammingError" => {
                        undefined = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if undefined {
                payload.insert(field.clone(), Value::Null);
                missing.insert(
                    field.clone(),
                    format!("{iso3}_{table} is not in the database."),
                );
                continue;
            }

            let value = read(&rows, spec["read"].as_str().unwrap_or_default(), &exclude)?;
            let is_missing = value.is_null() || value.as_array().is_some_and(Vec::is_empty);
            payload.insert(field.clone(), value);
            sources.insert(
                field.clone(),
                json!({"table": format!("{iso3}_{table}"), "area": area_name.clone().unwrap_or_else(|| country.clone()), "year": year}),
            );
            if level != 0
                && !rows.is_empty()
                && let Some(a) = &area_name
            {
                used.entry(level).or_insert_with(|| a.clone());
            }
            if is_missing {
                let tried: Vec<String> = names.iter().map(|n| py_repr(n.as_deref())).collect();
                missing.insert(
                    field.clone(),
                    format!("{iso3}_{table} has no row for {}", tried.join(" or ")),
                );
            }
        }

        if !no_area_name {
            let name_fields: Vec<(String, i64)> =
                match cfg["SOCIAL_AREA_NAME_FIELDS"][&iso3][section].as_object() {
                    Some(m) => m
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0)))
                        .collect(),
                    None => vec![("administrative_area_name".into(), section_level(fields))],
                };
            for (name_field, level) in name_fields {
                let v = if level != 0 {
                    used.get(&level)
                        .cloned()
                        .or_else(|| area_name_of(level))
                        .map_or(Value::Null, Value::String)
                } else {
                    Value::String(country.clone())
                };
                payload.insert(name_field, v);
            }
        }

        let mut years: Vec<i64> = sources
            .values()
            .filter_map(|s| s["year"].as_i64())
            .collect();
        years.sort();
        years.dedup();
        if years.len() > 1 {
            notes.push(format!(
                "6.3: the {section} figures are not all from the same year ({}); each field's year is in `values` \
                 because the newest published year differs between these tables.",
                years.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
        sections.insert(section.clone(), Value::Object(payload));
        values.insert(section.clone(), Value::Object(sources));
    }

    let known = known_absent(&iso3);
    if !known.is_empty() {
        absent.push(format!(
            "6.3: {country} publishes no {}; the field is null because the table does not exist, not because the value is zero.",
            known.join(", ")
        ));
    }
    for (field, reason) in &missing {
        if !known.contains(&field.as_str()) {
            absent.push(format!("6.3: {field} is null -- {reason}."));
        }
    }

    values.insert("country".into(), json!(country));
    values.insert("iso3".into(), json!(iso3));
    let results = json!({
        "narrative": "",
        "tables": {},
        "values": values,
        "flags": flags,
        "missing": absent,
        "notes": notes,
        "retryable": retryable,
    });

    if let Some(topics) = cfg["SOCIAL_MAPPING_TOPICS"].as_object() {
        for fields in sections.values_mut() {
            let Some(fields) = fields.as_object_mut() else {
                continue;
            };
            for (field, topic) in topics {
                let (t, st) = (topic[0].as_str(), topic[1].as_str());
                let scope = (
                    "site_characteristic",
                    Some("people"),
                    t,
                    st,
                    Some(iso3.as_str()),
                );
                let key_of = |label: &Value| -> Value {
                    match label {
                        Value::Null => Value::Null,
                        l => keys
                            .key(scope, Some(&crate::db::py_str(l)), false)
                            .map_or(Value::Null, Value::String),
                    }
                };
                match fields.get_mut(field) {
                    Some(Value::Array(list)) => {
                        for row in list.iter_mut() {
                            if let Some(obj) = row.as_object_mut().filter(|o| o.contains_key("id"))
                            {
                                let k = key_of(&obj["id"]);
                                obj.insert("key".into(), k);
                            }
                        }
                    }
                    Some(Value::String(s)) => {
                        let k = key_of(&Value::String(s.clone()));
                        fields.insert(format!("{field}_key"), k);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok((results, Value::Object(sections)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows keyed by (table, area): what `load_social_rows` would return, written by hand from the
    /// SQL's shape (one row per category, already summed, ordered by category).
    #[expect(clippy::type_complexity, reason = "test rows shaped as the SQL rows")]
    struct Fixture {
        regions: Vec<(&'static str, Result<Option<String>>)>,
        rows: Vec<(
            &'static str,
            Option<&'static str>,
            Result<(Option<i64>, Vec<SocialRow>)>,
        )>,
    }

    impl SocialSource for Fixture {
        async fn statistical_area(&self, table: &str, _column: &str) -> Result<Option<String>> {
            self.regions
                .iter()
                .find(|(t, _)| *t == table)
                .map_or(Ok(None), |(_, r)| r.clone())
        }
        async fn social_rows(
            &self,
            _iso3: &str,
            table: &str,
            level: i64,
            area_name: Option<&str>,
            _filters: &[(String, Option<String>)],
        ) -> Result<(Option<i64>, Vec<SocialRow>)> {
            if level != 0 && area_name.is_none() {
                return Ok((None, Vec::new()));
            }
            self.rows
                .iter()
                .find(|(t, a, _)| *t == table && (*a == area_name || a.is_none()))
                .map_or(Ok((None, Vec::new())), |(_, _, r)| r.clone())
        }
    }

    fn row(c: &str, u: &str, v: f64) -> SocialRow {
        (Some(c.into()), Some(u.into()), v)
    }

    fn run(f: &Fixture, admin: Value) -> (Value, Value) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        rt.block_on(social_statistics_from(
            f,
            &MappingKeys::default(),
            Some(&admin),
        ))
        .expect("ok")
    }

    #[test]
    fn config_matches_python_shape() {
        assert!(config()["SOCIAL_INDICATORS"]["tha"].is_object());
        assert_eq!(
            config()["SOCIAL_LEVEL_SOURCES"]["phl"]["2"],
            "dominant_province"
        );
    }

    #[test]
    fn no_country_and_unknown_country() {
        let f = Fixture {
            regions: vec![],
            rows: vec![],
        };
        let (r, v) = run(&f, json!({}));
        assert_eq!(r["values"], json!({"country": null, "iso3": null}));
        assert_eq!(v, json!({}));
        let (r, _) = run(&f, json!({"dominant_country": "Atlantis"}));
        assert_eq!(
            r["missing"][0],
            "6.3: no social statistics are loaded for Atlantis."
        );
        assert_eq!(r["values"], json!({"country": "Atlantis", "iso3": null}));
    }

    #[test]
    fn reads_and_absences() {
        // Thailand, province only; every table answers for 'Chiang Mai' except a missing one.
        let f = Fixture {
            regions: vec![],
            rows: vec![
                (
                    "unemployment_rate",
                    Some("Chiang Mai"),
                    Ok((Some(2023), vec![row("Total", "percent", 1.25)])),
                ),
                (
                    "employment_by_sector",
                    Some("Chiang Mai"),
                    Ok((
                        Some(2022),
                        vec![
                            row("Agriculture", "persons", 30.0),
                            row("Services", "persons", 10.0),
                        ],
                    )),
                ),
                (
                    "number_of_households",
                    Some("Chiang Mai"),
                    Err(Error::new("ProgrammingError", "x")),
                ),
            ],
        };
        let (r, v) = run(
            &f,
            json!({"dominant_country": "Thailand", "dominant_province": "Chiang Mai", "provinces": ["Chiang Mai"]}),
        );
        assert_eq!(r["values"]["iso3"], "tha");
        // shares from counts: 30/40 and 10/40
        let s = &v["employment"]["employment_sectors"];
        assert_eq!(s[0]["percentage"], json!(75.0));
        assert_eq!(s[1]["id"], "Services");
        assert_eq!(s[0]["key"], Value::Null);
        assert_eq!(v["employment"]["unemployment_rate_percentage"], json!(1.25));
        assert_eq!(v["employment"]["administrative_area_name"], "Chiang Mai");
        assert_eq!(
            r["values"]["employment"]["unemployment_rate_percentage"]["year"],
            2023
        );
        // the two years of one section are noted
        assert!(
            r["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n.as_str().unwrap().contains("(2022, 2023)"))
        );
        // a missing table is reported, not raised
        assert!(
            r["missing"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m.as_str().unwrap().contains("is not in the database"))
        );
        assert_eq!(r["retryable"], false);
    }

    #[test]
    fn philippines_region_vintages() {
        let f = Fixture {
            regions: vec![
                (
                    "phl.regions_18",
                    Ok(Some("Negros Island Region (NIR)".into())),
                ),
                ("phl.regions_17", Ok(Some("Region VI".into()))),
            ],
            rows: vec![],
        };
        let (r, _) = run(
            &f,
            json!({"dominant_country": "Philippines", "dominant_province": "Negros Occidental", "dominant_district": "Bago"}),
        );
        let notes = r["notes"].as_array().unwrap();
        assert!(notes[0].as_str().unwrap().contains(
            "'Negros Island Region (NIR)' in the newer tables, 'Region VI' in the older ones"
        ));
        assert!(notes[1].as_str().unwrap().contains(
            "Level 1 is 'Negros Island Region (NIR)' and level 2 is 'Negros Occidental'"
        ));
        let f = Fixture {
            regions: vec![("phl.regions_18", Err(Error::new("OperationalError", "x")))],
            rows: vec![],
        };
        let (r, _) = run(
            &f,
            json!({"dominant_country": "Philippines", "dominant_province": "Palawan"}),
        );
        assert_eq!(r["retryable"], true);
        assert!(
            r["flags"][0]
                .as_str()
                .unwrap()
                .starts_with("6.3: phl.regions_18 could not be read (OperationalError)")
        );
    }

    #[test]
    fn read_kinds() {
        let rows = vec![
            row("A", "persons", 3.0),
            row("B", "persons", 5.0),
            row("None", "persons", 2.0),
        ];
        let ex = vec!["None".to_string()];
        assert_eq!(read(&rows, "value", &ex).unwrap(), json!(8.0));
        assert_eq!(read(&rows, "top_id", &ex).unwrap(), json!("B"));
        assert_eq!(read(&rows, "top_value", &[]).unwrap(), json!(5.0));
        assert_eq!(read(&rows, "coverage_pct", &ex).unwrap(), json!(80.0));
        assert_eq!(read(&[], "value", &[]).unwrap(), Value::Null);
        assert_eq!(read(&[], "shares", &[]).unwrap(), json!([]));
        assert!(read(&rows, "nope", &[]).is_err());
    }

    #[test]
    fn repr_quotes_as_python() {
        assert_eq!(py_repr(Some("Region VI")), "'Region VI'");
        assert_eq!(py_repr(Some("Cote d'Ivoire")), "\"Cote d'Ivoire\"");
        assert_eq!(py_repr(None), "None");
    }
}
