//! v3 feasibility / monitoring context: `build_context` of
//! `utils/document_generator/v3/context.py`, `feasibility_prefill`/`merge_form` of `prefill.py`
//! and the merge half of `document_apis/utils.py load_draft`. Moved from
//! `rust/api/src/documents_csv.rs` (which ported `tags` only) and extended to the full context.

use serde_json::{Map, Value, json};

use crate::py::{py_float_repr, py_sum_numbers};

// ---- document_apis/utils.py ----

/// The `DataAnalyzer` jsonb columns `build_context` reads.
#[derive(Clone, Debug, Default)]
pub struct Analyzer {
    pub site_information_json: Value,
    pub nature_json: Value,
    pub climate_json: Value,
    pub people_json: Value,
    pub threat_json: Value,
    pub intervention_eligibility_json: Value,
    pub benefit_json: Value,
}

/// `_stored_dict`: a jsonb value that is not an object reads as empty.
pub fn stored_dict(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// `load_draft(session_id, cert_type, base_types)` after its reads: `drafts` are the
/// `(form, user_input)` columns of the `DocumentData` rows found, in `(*base_types, cert_type)`
/// order (missing rows left out). Returns `(form, user_input)`: stored answers over the prefill,
/// later drafts winning (`merge_form` / `{**user_input, **stored}`).
pub fn load_draft(
    analyzer: Option<&Analyzer>,
    drafts: &[(Value, Value)],
) -> Result<(Value, Value), String> {
    let mut form = feasibility_prefill(analyzer)?;
    let mut user_input = Map::new();
    for (f, u) in drafts {
        form.extend(stored_dict(Some(f.clone())));
        user_input.extend(stored_dict(Some(u.clone())));
    }
    Ok((Value::Object(form), Value::Object(user_input)))
}

/// `feasibility_prefill(analyzer)` of `v3/prefill.py`.
pub fn feasibility_prefill(analyzer: Option<&Analyzer>) -> Result<Map<String, Value>, String> {
    let Some(analyzer) = analyzer else {
        return Ok(Map::new());
    };
    let people = or(&analyzer.people_json, json!({}));
    let demography = get_or(&people, "social_demography", json!({}))?;
    let economy = get_or(&people, "economy", json!({}))?;
    let mut prefill = Map::new();
    for (key, value) in [
        ("seHouseholds", get(&demography, "household_number")?),
        ("seIncome", get(&economy, "avg_household_income")?),
    ] {
        if !empty_like(&value) {
            prefill.insert(key.into(), value);
        }
    }
    Ok(prefill)
}

/// The text `_centroid_text` builds from the AOI centroid (EPSG:4326 lat `y`, lon `x`).
pub fn centroid_words(y: f64, x: f64) -> String {
    let lat = format!(
        "{}° {}",
        fmt_f(y.abs(), 4, false),
        if y < 0.0 { 'S' } else { 'N' }
    );
    let lon = format!(
        "{}° {}",
        fmt_f(x.abs(), 4, false),
        if x < 0.0 { 'W' } else { 'E' }
    );
    format!("the project boundary file (centroid {lat}, {lon})")
}

// ---- utils/document_generator/v3/context.py ----

/// A Python dict of tag -> value (None = `Value::Null`), keeping first-insertion positions.
#[derive(Default)]
struct Tags(Vec<(String, Value)>);

impl Tags {
    fn set(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    fn get(&self, key: &str) -> Value {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map_or(Value::Null, |(_, v)| v.clone())
    }

    fn update(&mut self, other: Tags) {
        for (k, v) in other.0 {
            self.set(&k, v);
        }
    }
}

type R<T> = Result<T, String>;

/// `_norm(tag)`: html.unescape, whitespace runs to one space, stripped.
fn norm(tag: &str) -> String {
    collapse(&html_unescape(tag))
}

/// `_ha`: thousands separators, 2 dp, for an int or float (bool included).
fn ha(v: &Value) -> Value {
    is_num(v).map_or(Value::Null, |f| json!(fmt_f(f, 2, true)))
}

/// `_pct`: 2 dp.
fn pct(v: &Value) -> Value {
    is_num(v).map_or(Value::Null, |f| json!(fmt_f(f, 2, false)))
}

/// `_label`: the `fallback` of a label dict.
fn label(v: &Value) -> Value {
    match v {
        Value::Object(map) => map.get("fallback").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// `_join(items, sep)`: str() of the truthy items, or None.
fn join(items: &[Value], sep: &str) -> Value {
    let items: Vec<String> = items.iter().filter(|i| truthy(i)).map(py_str).collect();
    if items.is_empty() {
        Value::Null
    } else {
        json!(items.join(sep))
    }
}

/// `max(items, key=lambda e: e.get(key) or 0, default=None)`.
fn max_by_key(items: &[Value], key: &str) -> R<Option<Value>> {
    let mut best: Option<(Value, &Value)> = None;
    for item in items {
        let k = or(&get(item, key)?, json!(0));
        match &best {
            Some((bk, _)) if !py_gt(&k, bk)? => {}
            _ => best = Some((k, item)),
        }
    }
    Ok(best.map(|(_, v)| v.clone()))
}

/// `sum(... for ...)` from int 0.
fn py_sum(items: impl IntoIterator<Item = Value>) -> R<Value> {
    let items: Vec<Value> = items.into_iter().collect();
    if let Some(sum) = py_sum_numbers(&items) {
        return Ok(sum); // Python 3.12's compensated float sum
    }
    let mut acc = json!(0);
    for item in items {
        acc = py_add(&acc, &item)?;
    }
    Ok(acc)
}

fn site_characterisation_tags(si: &Value, pathway: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    let s = |k: &str| get(si, k);
    tags.set("Site Characterisation: Level 0", s("country")?);
    tags.set("Site Characterisation: Level 1", s("province")?);
    tags.set("Site Characterisation: Level 2", s("district")?);
    tags.set("Site Characterisation: Level 3", s("subdistrict")?);
    tags.set("Site Characterisation: Level 4", s("village")?);
    tags.set("Site Characterisation: Country", s("country")?);
    let location = join(
        &[
            s("village")?,
            s("subdistrict")?,
            s("district")?,
            s("province")?,
        ],
        ", ",
    );
    tags.set(
        "Site Characterisation: Villages, Sub-District, District, Province",
        location,
    );
    tags.set(
        "Site Characterisation: Administrative Location",
        join(&[s("district")?, s("province")?, s("country")?], ", "),
    );

    let project_area = get(pathway, "project_area_ha")?;
    tags.set("Site Characterisation: number of", ha(&project_area));
    tags.set("Site Characterisation: total AOI (ha)", ha(&project_area));
    tags.set(
        "Site Characterisation: Total Project Area in Hectares",
        ha(&project_area),
    );

    let ecosystems = py_iter(&get_or(si, "ecosystems", json!([]))?)?;
    let (mut peat, mut mangrove) = (json!({}), json!({}));
    for e in ecosystems.iter().filter(|e| e.is_object()) {
        let name = get(e, "name")?;
        hashable(&name)?;
        match name.as_str() {
            Some("Peatland") => peat = e.clone(),
            Some("Mangrove") => mangrove = e.clone(),
            _ => {}
        }
    }
    tags.set(
        "Site Characterisation: percentage of peatland number area",
        pct(&get(&peat, "percentage")?),
    );
    tags.set(
        "Site Characterisation: percentage of mangrove number area",
        pct(&get(&mangrove, "percentage")?),
    );
    let mut candidates = Vec::new();
    for e in ecosystems.iter().filter(|e| e.is_object()) {
        if !py_eq(&get(e, "name")?, &json!("Other/Unclassified")) {
            candidates.push(e.clone());
        }
    }
    let dominant = max_by_key(&candidates, "area")?;
    let dominant_name = match &dominant {
        Some(d) if truthy(d) => get(d, "name")?,
        _ => Value::Null,
    };
    tags.set(
        "Site Characterisation: dominant ecosystem type(s)",
        dominant_name.clone(),
    );
    tags.set(
        "Site Characterisation: dominant ecosystem type",
        dominant_name,
    );

    tags.set(
        "Site Characterisation: elevation class",
        label(&s("predominant_elevation_dict")?),
    );
    tags.set(
        "Site Characterisation: percentage of slope number average",
        pct(&s("average_slope_percentage")?),
    );
    let slopes = py_iter(&or(&s("slopes")?, json!([])))?;
    let slope = match max_by_key(&slopes, "area")? {
        Some(d) if truthy(&d) => label(&get(&d, "dict")?),
        _ => Value::Null,
    };
    tags.set("Site Characterisation: slope categories", slope);

    let classes: Vec<Value> = py_iter(&get_or(si, "land_cover_class", json!([]))?)?
        .into_iter()
        .filter(Value::is_object)
        .collect();
    let mut named = Vec::new();
    for c in &classes {
        if !py_eq(&get(c, "id")?, &json!("0")) {
            named.push(c.clone());
        }
    }
    let top: Vec<Value> = named
        .iter()
        .take(3)
        .map(|c| get(c, "name"))
        .collect::<R<_>>()?;
    tags.set(
        "Site Characterisation: top 3 land cover class",
        join(&top, ", "),
    );
    let first = match named.first() {
        Some(c) => get(c, "name")?,
        None => Value::Null,
    };
    tags.set("Site Characterisation: Land cover class", first);
    for (position, ordinal) in ["1st", "2nd", "3rd"].into_iter().enumerate() {
        let row = named.get(position).cloned().unwrap_or_else(|| json!({}));
        tags.set(
            &format!("Site Characterisation: {ordinal} land cover class"),
            get(&row, "name")?,
        );
        tags.set(
            &format!("Site Characterisation: {ordinal} land cover class area (ha)"),
            ha(&get(&row, "area")?),
        );
        tags.set(
            &format!("Site Characterisation: {ordinal} land cover class area (% of AOI)"),
            pct(&get(&row, "percentage")?),
        );
    }
    let forest = |field: &str| -> R<Value> {
        let mut parts = Vec::new();
        for c in &classes {
            if py_str(&get_or(c, "name", json!(""))?)
                .to_lowercase()
                .contains("forest")
            {
                parts.push(or(&get(c, field)?, json!(0)));
            }
        }
        py_sum(parts)
    };
    let forest_pct = forest("percentage")?;
    tags.set(
        "Site Characterisation: percentage of forest coverage number area",
        if classes.is_empty() {
            Value::Null
        } else {
            pct(&forest_pct)
        },
    );
    let forest_ha = forest("area")?;
    tags.set(
        "Site Characterisation: Forest Area Directly Involved (ha)",
        if classes.is_empty() {
            Value::Null
        } else {
            ha(&forest_ha)
        },
    );

    tags.set(
        "Site Characterisation: historical deforestation rate",
        pct(&s("historical_deforestation_percentage")?),
    );
    tags.set(
        "Site Characterisation: rate_pct",
        pct(&s("historical_deforestation_percentage")?),
    );
    let start = s("historical_deforestation_year_start")?;
    let end = s("historical_deforestation_year_end")?;
    tags.set(
        "Site Characterisation: time period",
        if truthy(&start) && truthy(&end) {
            json!(format!("{}–{}", py_str(&start), py_str(&end)))
        } else {
            Value::Null
        },
    );
    let mut risks = Vec::new();
    for (name, key) in [
        ("Flood", "flood_risk_dict"),
        ("Tropical typhoon", "typhoon_risk_dict"),
        ("Landslide", "landslide_risk_dict"),
        ("Drought", "drought_risk_dict"),
        ("Fire", "fire_risk_dict"),
    ] {
        let l = label(&s(key)?);
        if truthy(&l) {
            risks.push(json!(format!("{name} ({})", py_str(&l))));
        }
    }
    let risks = join(&risks, ", ");
    tags.set(
        "Site Characterisation: Flood/Tropical typhoon/Landslide/Drought/Fire risk",
        risks.clone(),
    );
    tags.set(
        "Site Characterisation: Flood / Tropical typhoon / Landslide / Drought / Fire risk",
        risks,
    );

    tags.set(
        "Site Characterisation: Project Boundary Map",
        json!("Figure 2"),
    );
    Ok(tags)
}

fn burned_area_tags(climate: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    tags.set(
        "Site Characterisation: Historical Burned Area",
        ha(&get(climate, "total_burned_area")?),
    );
    Ok(tags)
}

fn threat_tags(threat: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    let overview = get_or(threat, "all ecosystems", json!({}))?;
    let dryland = get_or(threat, "dryland forest", json!({}))?;
    let mangrove = get_or(threat, "mangrove", json!({}))?;
    let peatland = get_or(threat, "peatland", json!({}))?;

    let ecosystems = py_iter(&or(&get(&overview, "ecosystems")?, json!([])))?;
    let mut candidates = Vec::new();
    for e in ecosystems.iter().filter(|e| e.is_object()) {
        if !py_eq(&get(e, "label")?, &json!("Other")) {
            candidates.push(e.clone());
        }
    }
    let eco_label = match max_by_key(&candidates, "area_ha")? {
        Some(d) if truthy(&d) => get(&d, "label")?,
        _ => Value::Null,
    };
    tags.set("Threat: Forest / Mangrove / Peatland", eco_label.clone());
    tags.set("Threat: Forest / Mangrove  / Peatland", eco_label.clone());
    tags.set(
        "Threat: ecosystem (forest / mangrove / peatland)",
        eco_label.clone(),
    );
    tags.set(
        "Threat: total ecosystem area (ha)",
        ha(&get(&overview, "total_ecosystem_area_ha")?),
    );

    let key = or(&eco_label, json!(""));
    hashable(&key)?;
    let (chosen, is_mangrove) = match key.as_str() {
        Some("Mangrove") => (&mangrove, true),
        Some("Peatland") => (&peatland, false),
        _ => (&dryland, false),
    };
    // `or {}` makes a new dict, so `tab is mangrove` fails for an empty mangrove tab
    let tab_is_mangrove = is_mangrove && truthy(chosen);
    let tab = or(chosen, json!({}));
    tags.set(
        "Threat: remaining intact cover (ha)",
        ha(&get(&tab, "remaining_forest_ha")?),
    );
    tags.set(
        "Threat: disturbed / degraded area (ha)",
        ha(&get(&tab, "disturbed_area_ha")?),
    );
    tags.set(
        "Threat: disturbed /degraded area (ha)",
        ha(&get(&tab, "disturbed_area_ha")?),
    );
    tags.set(
        "Threat: percentage of disturbed area",
        pct(&get(&tab, "disturbed_percentage")?),
    );
    let mut loss = get(&dryland, "forest_loss_ha")?;
    if loss.is_null() {
        loss = get(&peatland, "converted_loss_ha")?;
    }
    tags.set(
        "Threat: forest disturbed and converted/loss (ha)",
        ha(&loss),
    );
    tags.set("Site Characterisation: forest loss area", ha(&loss));

    let drivers = or(&get(&tab, "drivers")?, json!({}));
    let non_natural = or(&get(&drivers, "non_natural")?, json!([]));
    let pressure = if tab_is_mangrove {
        get(&mangrove, "main_pressure")?
    } else {
        join(&py_iter(&non_natural)?, ", ")
    };
    tags.set(
        "Threat: Main pressure (Logging / Road access / Agriculture Expansion / Settlement / \
         Mining / Coastal conversion / Drainage / risk)",
        pressure,
    );
    let n = py_len(&non_natural)?;
    tags.set(
        "Threat: driver 1",
        if n > 0 {
            py_index(&non_natural, 0)?
        } else {
            Value::Null
        },
    );
    tags.set(
        "Threat: driver 2",
        if n > 1 {
            py_index(&non_natural, 1)?
        } else {
            Value::Null
        },
    );
    tags.set(
        "Threat: natural drivers (flooding / forest fire / drought / typhoon / landslide / \
         extreme climate)",
        join(&py_iter(&or(&get(&drivers, "natural")?, json!([])))?, ", "),
    );

    tags.set(
        "Threat: canal proximity (High / Moderate / Low)",
        get(&peatland, "canal_proximity")?,
    );
    tags.set(
        "Threat: drainage pressure (High / Moderate / Low)",
        get(&peatland, "drainage_pressure")?,
    );
    tags.set(
        "Threat: fire risk (High / Moderate / Low)",
        get(&peatland, "fire_risk")?,
    );
    Ok(tags)
}

fn pathway_tags(pathway: &Value, threat: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    let dryland = get_or(threat, "dryland forest", json!({}))?;
    tags.set(
        "NbS Pathway: hectare of remaining forest",
        ha(&get(&dryland, "remaining_forest_ha")?),
    );
    tags.set(
        "NbS Pathway: percentage of remaining forest",
        pct(&get(&dryland, "remaining_forest_percentage")?),
    );

    // totals: only str names can be looked up below; the float accumulator only decides errors
    let mut totals: Vec<(String, Value)> = Vec::new();
    let mut activities: Vec<Value> = Vec::new();
    for eco in py_iter(&get_or(pathway, "ecosystems", json!([]))?)? {
        for intervention in py_iter(&get_or(&eco, "interventions", json!([]))?)? {
            let name = get(&intervention, "intervention")?;
            hashable(&name)?;
            let current = match name.as_str() {
                Some(n) => totals.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()),
                None => None,
            }
            .unwrap_or_else(|| json!(0.0));
            let sum = py_add(&current, &or(&get(&intervention, "area_ha")?, json!(0.0)))?;
            if let Some(n) = name.as_str() {
                match totals.iter_mut().find(|(k, _)| k == n) {
                    Some(slot) => slot.1 = sum,
                    None => totals.push((n.to_string(), sum)),
                }
            }
            for activity in py_iter(&get_or(&intervention, "activities", json!([]))?)? {
                let label = get(&activity, "activity")?;
                if truthy(&label) && !activities.iter().any(|a| py_eq(a, &label)) {
                    activities.push(label);
                }
            }
        }
    }
    let project_area = or(&get(pathway, "project_area_ha")?, json!(0));
    for name in ["Protect", "Manage", "Restore"] {
        let lower = name.to_lowercase();
        let total = totals
            .iter()
            .find(|(k, _)| k == name)
            .map_or(Value::Null, |(_, v)| v.clone());
        tags.set(
            &format!("NbS Pathway: hectare area eligible to {lower}"),
            ha(&total),
        );
        if truthy(&project_area) {
            let share = py_div(&or(&total, json!(0)), &project_area)? * 100.0;
            tags.set(
                &format!("NbS Pathway: percent area eligible to {lower}"),
                json!(fmt_f(share, 2, false)),
            );
        }
    }
    tags.set(
        "NbS Pathway: List of NbS Activities",
        join(&activities, ", "),
    );
    Ok(tags)
}

fn nature_tags(nature: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    let n = |k: &str| get(nature, k);
    tags.set("Nature: FLII score", pct(&n("flii_score")?));
    tags.set("Nature: Mean FLII/10", pct(&n("flii_score")?));
    tags.set(
        "Nature: average of Forest Landscape Integrity Index score",
        pct(&n("flii_score")?),
    );
    tags.set("Nature: % High FLII", pct(&n("high_integrity_percentage")?));
    tags.set(
        "Nature: % Medium FLII",
        pct(&n("medium_integrity_percentage")?),
    );
    tags.set("Nature: % Low FLII", pct(&n("low_integrity_percentage")?));
    let integrity = n("dominant_integrity_class")?;
    tags.set("Nature: Integrity Category", integrity.clone());
    tags.set(
        "Nature: Integrity Category (High/Medium/Low/No forest cover)",
        integrity.clone(),
    );
    tags.set(
        "Nature: forest integrity class (high / medium / low / no forest)",
        integrity,
    );

    tags.set(
        "Nature: total wildlife species",
        n("total_wildlife_species")?,
    );
    tags.set(
        "Nature: amphibian species count",
        n("amphibian_number_of_species")?,
    );
    tags.set(
        "Nature: [amphibian species count",
        n("amphibian_number_of_species")?,
    );
    tags.set("Nature: bird species count", n("bird_number_of_species")?);
    tags.set(
        "Nature: mammal species count",
        n("mammal_number_of_species")?,
    );
    tags.set(
        "Nature: reptile species count",
        n("reptile_number_of_species")?,
    );
    tags.set(
        "Nature: endangered tree species",
        n("endangered_tree_number_of_species")?,
    );
    tags.set(
        "Nature: number of endangered tree species",
        n("endangered_tree_number_of_species")?,
    );

    let mut key_species = Vec::new();
    for s in py_iter(&get_or(nature, "key_species", json!([]))?)?
        .iter()
        .filter(|s| s.is_object())
    {
        key_species.push(get(s, "species_id")?);
    }
    tags.set("Nature: keystone species", join(&key_species, ", "));
    let names_tag =
        "Nature: keystone species names (comma-separated, with \"and\" before the final species)";
    if key_species.len() > 1 {
        let head = join(&key_species[..key_species.len() - 1], ", ");
        let Some(head) = head.as_str() else {
            return Err("unsupported operand type(s) for +: 'NoneType' and 'str'".into());
        };
        let last = &key_species[key_species.len() - 1];
        let Some(last) = last.as_str() else {
            return Err(format!(
                "can only concatenate str (not \"{}\") to str",
                py_type(last)
            ));
        };
        tags.set(names_tag, json!(format!("{head} and {last}")));
    } else {
        tags.set(names_tag, join(&key_species, ", "));
    }

    let summary = or(&n("iucn_summary")?, json!({}));
    let endangered = py_sum(
        ["CR", "EN", "VU"]
            .iter()
            .map(|code| get_or(&summary, code, json!(0)))
            .collect::<R<Vec<_>>>()?,
    )?;
    tags.set(
        "Nature: number of endangered species",
        if truthy(&summary) {
            endangered
        } else {
            Value::Null
        },
    );

    let mut kba_names = Vec::new();
    for k in py_iter(&get_or(
        nature,
        "overlapping_key_biodiversity_areas",
        json!([]),
    )?)?
    .iter()
    .filter(|k| k.is_object())
    {
        kba_names.push(get(k, "overlapping_key_biodiversity_name")?);
    }
    let kba = match join(&kba_names, ", ") {
        Value::Null if nature.get("overlapping_key_biodiversity_areas").is_some() => {
            json!("none (0 KBAs)")
        }
        joined => joined,
    };
    tags.set("Nature: KBA name(s) / count", kba);
    tags.set(
        "Nature: KBA overlap area",
        ha(&n("overlapping_key_biodiversity_area_total_size")?),
    );
    tags.set(
        "Nature: KBA overlap %",
        pct(&n("overlapping_key_biodiversity_area_percentage")?),
    );
    tags.set(
        "Nature: in percent of total AOI area",
        pct(&n("overlapping_key_biodiversity_area_percentage")?),
    );

    let mut endangered_names = Vec::new();
    for s in py_iter(&get_or(nature, "species_list", json!([]))?)?
        .iter()
        .filter(|s| s.is_object())
    {
        if matches!(
            get(s, "redlist_category")?.as_str(),
            Some("CR" | "EN" | "VU")
        ) {
            endangered_names.push(get(s, "scientific_name")?);
        }
    }
    tags.set(
        "Nature: name of endangered species",
        join(&endangered_names, ", "),
    );

    tags.set(
        "Nature: keystone species names",
        tags.get("Nature: keystone species"),
    );
    tags.set("amphibian species count", n("amphibian_number_of_species")?);

    let sig_rows: Vec<Value> = py_iter(&get_or(nature, "conservation_significances", json!([]))?)?
        .into_iter()
        .filter(Value::is_object)
        .collect();
    if !sig_rows.is_empty() {
        let budget = |row: &Value| -> R<f64> {
            Ok(py_float(&get(row, "budget_id")?).unwrap_or(f64::INFINITY))
        };
        let mut best: Vec<(String, Value)> = Vec::new();
        for row in &sig_rows {
            let scenario = py_str(&or(&get(row, "scenario_id")?, json!("")));
            if scenario.is_empty() {
                continue;
            }
            match best.iter_mut().find(|(k, _)| *k == scenario) {
                None => best.push((scenario, row.clone())),
                Some(slot) => {
                    if budget(row)? < budget(&slot.1)? {
                        slot.1 = row.clone();
                    }
                }
            }
        }
        let mut ordered = Vec::new();
        for (_, row) in best {
            let key = py_neg(&or(&get(&row, "percentage")?, json!(0)))?;
            ordered.push((key, row));
        }
        // list.sort is stable, and so is sort_by; keys are numbers
        ordered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let Some((_, first)) = ordered.first() else {
            return Err("list index out of range".into());
        };
        tags.set(
            "Nature: biodiversity / water / carbon",
            get(first, "scenario_id")?,
        );
        let mut parts = Vec::new();
        for (_, r) in &ordered {
            parts.push(json!(format!(
                "{} {}%",
                py_str(&get(r, "scenario_id")?),
                py_str(&pct(&get(r, "percentage")?))
            )));
        }
        tags.set(
            "Nature: Conservation Significance (biodiversity/water/carbon)",
            join(&parts, ", "),
        );
    }
    if let Some(rank) = is_num(&n("conservation_priority_rank")?) {
        tags.set(
            "Nature: conservation significance score",
            json!(format!("global top {}%", fmt_f(rank, 0, true))),
        );
    }
    Ok(tags)
}

/// F03 form key -> template tag(s), `_FORM_SCALARS` in its order.
const FORM_SCALARS: &[(&str, &[&str])] = &[
    ("seYearSource", &["People: Year of Data Source"]),
    (
        "seHouseholds",
        &["People: Household population and number of households in The Project Area"],
    ),
    ("seDisabilities", &["People: Disabilities"]),
    (
        "seSchools",
        &["People: Number of Schools within project area"],
    ),
    (
        "seHealthFac",
        &["People: Health Facility within project area"],
    ),
    ("seDiseases", &["People: Endemic Infectious Diseases"]),
    (
        "seSocialForestry",
        &["People: Social Forestry Beneficiaries"],
    ),
    ("seIncome", &["People: Average Household income"]),
    ("seExpenditure", &["People: Average Household expenditure"]),
    ("sePoverty", &["People: Number of Household in poverty"]),
    (
        "seUndernourished",
        &["People: Number of undernourished Household"],
    ),
    (
        "seElectricity",
        &["People: Number of Households with Access to electricity"],
    ),
    (
        "seWash",
        &["People: Number of Households with Access to WASH"],
    ),
    (
        "seLegal",
        &[
            "People: Forestry Legal Framework",
            "People: forestry legal framework",
        ],
    ),
    (
        "seBenefit",
        &[
            "People: Benefit Sharing Mechanism",
            "People: benefit sharing mechanism",
        ],
    ),
    ("seTenure", &["People: Indigenous Land Tenure"]),
    (
        "seMrv",
        &[
            "People: MRV Institutional Capacity",
            "People: MRV institutional capacity level",
        ],
    ),
    ("seSafeguard", &["People: Safeguard Information System"]),
    ("seGrievance", &["People: Grievance Redress Mechanism"]),
    ("seAdaptive", &["People: Adaptive Management Response Time"]),
];

/// `_LIVELIHOOD_LABELS`, falling back to the slug with dashes as spaces.
fn livelihood_label(slug: &str) -> String {
    match slug {
        "formal-sectors" => "formal sectors",
        "informal-sectors" => "informal sectors",
        "entrepreneur" => "entrepreneur",
        "fishery-aquaculture" => "fishery/aquaculture",
        "nbs-agroforestry" => "NbS agroforestry",
        "nbs-reforestation" => "NbS reforestation",
        "nbs-mangrove-restoration" => "NbS mangrove restoration",
        "nbs-ntfp" => "NbS NTFP",
        "nbs-forest-patrol" => "NbS forest patrol and protection",
        "nbs-seedling-nursery" => "NbS seedling nursery",
        "agriculture-agroforestry" => "agriculture/agroforestry",
        "forestry-nbs" => "forestry/NbS",
        "other" => "other",
        other => return other.replace('-', " "),
    }
    .to_string()
}

const IPLC_TAGS: [&str; 3] = [
    "People: IP&LC or Ethnicity Identification",
    "People: identified IP&LC or ethnicity name",
    "People: identified IP&LC community",
];
const AGE_BANDS: [&str; 3] = ["0–14", "15–44", "45+"];

/// `_num`: a form number (int, float or numeric string) as float; bool and the rest None.
fn num(v: &Value) -> Option<f64> {
    if v.is_boolean() {
        None
    } else {
        py_float(v).ok()
    }
}

/// `_num(v) or 0`
fn num_or0(v: &Value) -> f64 {
    num(v).filter(|f| *f != 0.0).unwrap_or(0.0)
}

/// `_counts_by_label(counts)` over `counts.items()`.
fn counts_by_label(counts: Option<Vec<(String, Value)>>) -> Value {
    let Some(counts) = counts else {
        return Value::Null;
    };
    let mut rows: Vec<(f64, String)> = counts.into_iter().map(|(k, v)| (num_or0(&v), k)).collect();
    rows.sort_by(|a, b| {
        (-a.0)
            .partial_cmp(&(-b.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let parts: Vec<Value> = rows
        .into_iter()
        .filter(|(n, _)| *n > 0.0)
        .map(|(n, k)| {
            json!(format!(
                "{} ({} households)",
                livelihood_label(&k),
                fmt_f(n, 0, true)
            ))
        })
        .collect();
    join(&parts, ", ")
}

/// `_age_band(label)`: by the label's leading number. shortcut: ASCII digits only, where `\d`
/// also takes other Unicode decimal digits.
fn age_band(label: &Value) -> Option<&'static str> {
    let text = py_str(label);
    let digits: String = text
        .trim_start_matches(py_space)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    // int() of a long digit run cannot overflow in Python; anything that long is >= 45
    let low = digits.parse::<u64>().unwrap_or(u64::MAX);
    Some(if low < 15 {
        AGE_BANDS[0]
    } else if low < 45 {
        AGE_BANDS[1]
    } else {
        AGE_BANDS[2]
    })
}

/// `{key: total}` in insertion order.
type Totals = Vec<(String, f64)>;

fn add_total(totals: &mut Totals, key: &str, n: f64) {
    match totals.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 += n,
        None => totals.push((key.to_string(), n)),
    }
}

/// `sum(d.values())`. shortcut: the values are kept as floats, so Python's int/float mix is not
/// told apart; all-int totals sum exactly either way.
fn total_sum(totals: &Totals) -> f64 {
    let values: Vec<Value> = totals.iter().map(|(_, v)| json!(v)).collect();
    py_sum_numbers(&values)
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN)
}

/// `_age_structure(by_band)`
fn age_structure(by_band: &Totals) -> Value {
    let total = total_sum(by_band);
    if total == 0.0 {
        return Value::Null;
    }
    let parts: Vec<Value> = AGE_BANDS
        .iter()
        .map(|band| {
            let v = by_band
                .iter()
                .find(|(k, _)| k == band)
                .map_or(0.0, |(_, v)| *v);
            json!(format!(
                "{band}: {} ({}%)",
                fmt_f(v, 0, true),
                fmt_f(v / total * 100.0, 1, false)
            ))
        })
        .collect();
    join(&parts, ", ")
}

/// `_matrix_totals(matrix)`: (gender, age, education) totals.
fn matrix_totals(matrix: &Value) -> (Totals, Totals, Totals) {
    let (mut gender, mut age, mut education) = (Totals::new(), Totals::new(), Totals::new());
    for (age_label, by_edu) in items(matrix) {
        for (edu, by_gender) in items(by_edu) {
            for (g, n) in items(by_gender) {
                let n = num_or0(n);
                let band = age_band(&json!(age_label));
                if !g.is_empty() {
                    add_total(&mut gender, g, n);
                }
                if let Some(band) = band {
                    add_total(&mut age, band, n);
                }
                if !edu.is_empty() {
                    add_total(&mut education, edu, n);
                }
            }
        }
    }
    (gender, age, education)
}

/// `_sum_numeric_leaves(value)`
fn sum_numeric_leaves(v: &Value) -> f64 {
    if let Some(f) = is_num(v) {
        return f;
    }
    match v {
        Value::Object(_) => items(v)
            .iter()
            .fold(0.0, |acc, (_, x)| acc + sum_numeric_leaves(x)),
        Value::Array(list) => list.iter().fold(0.0, |acc, x| acc + sum_numeric_leaves(x)),
        _ => {
            let text = py_str(v);
            let dots = text.matches('.').count();
            let mut cleaned = text.replace(',', "");
            // str.replace(".", "", count - 1): the first count-1 dots go
            for _ in 1..dots {
                cleaned = cleaned.replacen('.', "", 1);
            }
            py_float(&json!(cleaned)).unwrap_or(0.0)
        }
    }
}

fn form_tags(form: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    for (key, tag_names) in FORM_SCALARS {
        let mut value = get(form, key)?;
        if empty_like(&value) {
            continue;
        }
        if value.is_array() || value.is_object() {
            value = match &value {
                Value::Array(list) if list.iter().all(Value::is_string) => join(list, ", "),
                _ => Value::Null,
            };
        }
        if value.is_null() {
            continue;
        }
        for tag in *tag_names {
            tags.set(tag, value.clone());
        }
    }

    let matrix = get(form, "seMatrix")?;
    if truthy(&matrix) {
        let total = sum_numeric_leaves(&matrix);
        if total != 0.0 {
            tags.set(
                "People: Total Population in The Project Area",
                json!(fmt_f(total, 0, true)),
            );
        }
    }

    let villages = get(form, "seVillages")?;
    match &villages {
        Value::Array(list) if !list.is_empty() => {
            let mut count = 0;
            for v in list {
                let name = if v.is_object() {
                    get(v, "name")?
                } else {
                    json!(py_str(v))
                };
                if truthy(&name) {
                    count += 1;
                }
            }
            tags.set("People: Number of Villages", json!(count));
        }
        Value::Null | Value::Array(_) => {}
        Value::String(s) if s.is_empty() => {}
        _ => tags.set("People: Number of Villages", villages.clone()),
    }

    let (gender, age, _) = matrix_totals(&matrix);
    let population = total_sum(&gender);
    if population != 0.0 {
        let parts: Vec<String> = [("male", "Male"), ("female", "Female"), ("other", "Other")]
            .iter()
            .map(|(g, title)| {
                let v = gender.iter().find(|(k, _)| k == g).map_or(0.0, |(_, v)| *v);
                format!("{}% {title}", fmt_f(v / population * 100.0, 1, false))
            })
            .collect();
        tags.set(
            "People: Gender disaggregation within Project Area (% Male / % Female / % Other)",
            json!(parts.join(" / ")),
        );
    }
    tags.set("People: Age Group Within Project Area", age_structure(&age));

    if let Value::Array(iplc) = get(form, "seIplc")? {
        let mut names = Vec::new();
        for g in &iplc {
            names.push(if g.is_object() {
                get(g, "name")?
            } else {
                g.clone()
            });
        }
        let names = join(&names, ", ");
        for tag in IPLC_TAGS {
            tags.set(tag, names.clone());
        }
    }

    let employment = get(form, "seEmployment")?;
    if employment.is_object() {
        let employed = num(&get(&employment, "employed")?);
        tags.set(
            "People: Employment Status",
            employed.map_or(Value::Null, |e| json!(fmt_f(e, 0, true))),
        );
        let working_age = items(&employment)
            .iter()
            .fold(0.0, |acc, (_, v)| acc + num_or0(v));
        if let Some(e) = employed.filter(|_| working_age != 0.0) {
            tags.set(
                "People: employment%",
                json!(format!("{}%", fmt_f(e / working_age * 100.0, 2, false))),
            );
        }
    }

    let livelihood = get(form, "seLivelihood")?;
    if livelihood.is_object() {
        let mut rows: Vec<(String, Value)> = items(&livelihood)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        rows.sort_by(|a, b| {
            (-num_or0(&a.1))
                .partial_cmp(&(-num_or0(&b.1)))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(3);
        tags.set(
            "People: Type of livelihood (Agriculture, Forestry, Fishery)",
            counts_by_label(Some(rows)),
        );
    }
    let hh = get(form, "seHhLivelihood")?;
    let hh = hh.is_object().then(|| {
        items(&hh)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    });
    tags.set("People: Household based on livelihood", counts_by_label(hh));

    let households = num(&get(form, "seHouseholds")?);
    for (key, tag) in [
        ("seElectricity", "People: electricity%"),
        ("seWash", "People: WASH%"),
    ] {
        let count = num(&get(form, key)?);
        if let (Some(h), Some(c)) = (households.filter(|h| *h != 0.0), count)
            && c <= h
        {
            tags.set(tag, json!(fmt_f(c / h * 100.0, 2, false)));
        }
    }
    tags.0.retain(|(_, v)| !v.is_null());
    Ok(tags)
}

/// `format(v, spec)` for the `.1f` the people tags apply to raw values.
fn format_f(v: &Value, prec: usize) -> R<String> {
    match is_num(v) {
        Some(f) => Ok(fmt_f(f, prec, false)),
        None => match v {
            Value::String(_) => Err("Unknown format code 'f' for object of type 'str'".into()),
            other => Err(format!(
                "unsupported format string passed to {}.__format__",
                py_type(other)
            )),
        },
    }
}

fn people_tags(people: &Value) -> R<Tags> {
    let mut tags = Tags::default();
    let demo = get_or(people, "social_demography", json!({}))?;
    let total = get(&demo, "total_population")?;
    tags.set(
        "People: Total Population",
        match is_num(&total) {
            Some(f) if truthy(&total) => json!(fmt_f(f, 0, true)),
            _ => Value::Null,
        },
    );
    let households = get(&demo, "household_number")?;
    tags.set(
        "People: Household population and number of households",
        households.clone(),
    );
    tags.set(
        "People: Physical Vulnerability",
        get(&demo, "physical_vulnerability_level_id")?,
    );
    tags.set(
        "People: Environmental Vulnerability",
        get(&demo, "environmental_vulnerability_level_id")?,
    );
    tags.set(
        "People: Economic Vulnerability",
        get(&demo, "economic_vulnerability_level_id")?,
    );
    tags.set(
        "People: Social Vulnerability",
        get(&demo, "social_vulnerability_level_id")?,
    );
    let male = get(&demo, "male_population_percentage")?;
    let female = get(&demo, "female_population_percentage")?;
    if !male.is_null() && !female.is_null() {
        let text = format!(
            "{}% Male / {}% Female",
            format_f(&male, 1)?,
            format_f(&female, 1)?
        );
        tags.set(
            "People: Gender disaggregation (% Male / % Female)",
            json!(text),
        );
    }
    if truthy(&get(&demo, "total_population")?) && get(&demo, "household_number")?.is_null() {
        tags.set(
            "People: Household population and number of households",
            json!("no published household figure"),
        );
    }

    let rows = py_add(
        &or(&get(&demo, "age_distributions_male")?, json!([])),
        &or(&get(&demo, "age_distributions_female")?, json!([])),
    )?;
    let mut by_band = Totals::new();
    for row in py_iter(&rows)? {
        let band = if row.is_object() {
            age_band(&get(&row, "age_group_id")?)
        } else {
            None
        };
        if let Some(band) = band {
            add_total(&mut by_band, band, num_or0(&get(&row, "population")?));
        }
    }
    tags.set("People: Age Group", age_structure(&by_band));
    Ok(tags)
}

/// `_carbon(net_key, gross_key)`
fn carbon(benefit: &Value, net_key: &str, gross_key: &str) -> R<Value> {
    let net = get(&or(&get(benefit, net_key)?, json!({})), "net_tco2e")?;
    if !net.is_null() {
        return Ok(net);
    }
    let gross = or(&get(benefit, gross_key)?, json!({}));
    Ok(if truthy(&get(&gross, "applicable")?) {
        get(&gross, "total_tco2e")?
    } else {
        Value::Null
    })
}

/// `_benefit_pillar_counts(benefit)`: `benefit_<pillar>_count` -> distinct 5.1 benefits.
fn benefit_pillar_counts(benefit: &Value, out: &mut Map<String, Value>) -> R<()> {
    let gb = or(&get(benefit, "general_benefit")?, json!({}));
    let rows = or(&get(&gb, "benefits")?, json!([]));
    for pillar in ["nature", "people", "climate"] {
        let mut distinct: Vec<Value> = Vec::new();
        for row in py_iter(&rows)? {
            let benefit = get(&row, "benefit")?;
            if py_eq(&get(&row, "pillar")?, &json!(pillar)) {
                hashable(&benefit)?;
                if !distinct.iter().any(|d| py_eq(d, &benefit)) {
                    distinct.push(benefit);
                }
            }
        }
        let count = if truthy(&get(&gb, "applicable")?) {
            json!(distinct.len().to_string())
        } else {
            json!("[Potential Benefit: x]")
        };
        out.insert(format!("benefit_{pillar}_count"), count);
    }
    Ok(())
}

/// `_education_totals(form)`
fn education_totals(form: &Value, out: &mut Map<String, Value>) -> R<()> {
    let (_, _, education) = matrix_totals(&get(form, "seMatrix")?);
    for level in ["primary", "secondary", "higher"] {
        let value = education.iter().find(|(k, _)| k == level).map_or_else(
            || "[People: Education Level]".to_string(),
            |(_, v)| fmt_f(*v, 0, true),
        );
        out.insert(format!("education_{level}"), json!(value));
    }
    Ok(())
}

/// `build_context(analyzer, form, user_input, extra_tags)`: the whole docxtpl context minus the
/// `t` function, in Python's key order (`tags` is t's lookup; `nbstool_docx::render` rebuilds `t`
/// from it). `form` and `user_input` are the dicts `load_draft` returns; `extra_tags` is in
/// insertion order. Err is the message of the exception the Python raises.
pub fn build_context(
    analyzer: Option<&Analyzer>,
    form: &Value,
    user_input: &Value,
    extra_tags: &[(String, Value)],
) -> R<Value> {
    let col = |f: fn(&Analyzer) -> &Value| analyzer.map_or(json!({}), |a| or(f(a), json!({})));
    let si = col(|a| &a.site_information_json);
    let nature = col(|a| &a.nature_json);
    let climate = col(|a| &a.climate_json);
    let people = col(|a| &a.people_json);
    let threat = col(|a| &a.threat_json);
    let pathway = col(|a| &a.intervention_eligibility_json);
    let benefit = col(|a| &a.benefit_json);
    let form = or(form, json!({}));

    let benefit_avoided = carbon(&benefit, "net_emission_reduction", "avoided_emissions")?;
    let benefit_sequestered = carbon(&benefit, "net_carbon_removal", "arr_sequestration")?;
    let net_errs_total = get(&or(&get(&benefit, "net_errs")?, json!({})), "net_tco2e")?;

    let mut tags = Tags::default();
    if !net_errs_total.is_null() {
        tags.set(
            "Potential Benefit: total carbon stock stored",
            ha(&net_errs_total),
        );
    } else if !benefit_avoided.is_null() || !benefit_sequestered.is_null() {
        let total = py_add(
            &or(&benefit_avoided, json!(0.0)),
            &or(&benefit_sequestered, json!(0.0)),
        )?;
        tags.set("Potential Benefit: total carbon stock stored", ha(&total));
    }
    tags.update(site_characterisation_tags(&si, &pathway)?);
    tags.update(burned_area_tags(&climate)?);
    tags.update(threat_tags(&threat)?);
    tags.update(pathway_tags(&pathway, &threat)?);
    tags.update(nature_tags(&nature)?);
    tags.update(people_tags(&people)?);
    tags.update(form_tags(&form)?);

    for (key, value) in extra_tags {
        if !(value.is_null() || value == "") {
            tags.set(key, value.clone());
        }
    }
    for (key, value) in items(user_input) {
        if value.is_null() || value == "" {
            continue;
        }
        let key = norm(key);
        let key = if key.contains(": ") {
            key
        } else {
            format!("User input: {key}")
        };
        tags.set(&key, value.clone());
    }

    // named land cover classes; the first three are tags, the rest a loop list
    let mut land_cover_rest = Vec::new();
    let mut named = 0usize;
    for c in py_iter(&get_or(&si, "land_cover_class", json!([]))?)? {
        if c.is_object() && !py_eq(&get(&c, "id")?, &json!("0")) {
            named += 1;
            if named > 3 {
                land_cover_rest.push(json!({
                    "name": get(&c, "name")?,
                    "area": ha(&get(&c, "area")?),
                    "pct": pct(&get(&c, "percentage")?),
                }));
            }
        }
    }

    // plan_ecos: {id-or-name lowercased: eco}, last wins
    let mut plan_ecos: Vec<(String, Value)> = Vec::new();
    let mp_plan = or(&get(&form, "mpPlan")?, json!({}));
    for eco in py_iter(&or(&get(&mp_plan, "ecos")?, json!([])))? {
        if eco.is_object() {
            let id = get(&eco, "id")?;
            let id = if truthy(&id) { id } else { get(&eco, "name")? };
            let key = py_str(&or(&id, json!(""))).to_lowercase();
            match plan_ecos.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = eco,
                None => plan_ecos.push((key, eco)),
            }
        }
    }
    let assumptions = or(&get(&benefit, "assumptions")?, json!({}));
    let mut selections = get(&assumptions, "selections")?;
    if !truthy(&selections) {
        selections = or(&get(&benefit, "selections")?, json!({}));
    }

    let mut cards: Vec<(String, Value)> = Vec::new();
    for e in py_iter(&get_or(&pathway, "ecosystems", json!([]))?)? {
        if e.is_object() {
            let label = get(&e, "label")?;
            hashable(&label)?;
            if let Some(l) = label.as_str() {
                match cards.iter_mut().find(|(k, _)| k == l) {
                    Some(slot) => slot.1 = e,
                    None => cards.push((l.to_string(), e)),
                }
            }
        }
    }
    let mut all_chosen: Vec<Value> = Vec::new();
    let mut ecos: Vec<Value> = Vec::new();
    for (label, plan_key) in [
        ("Forest", "forest"),
        ("Mangrove", "mangrove"),
        ("Peatland", "peatland"),
    ] {
        let card = or(
            &cards
                .iter()
                .find(|(k, _)| k == label)
                .map_or(Value::Null, |(_, v)| v.clone()),
            json!({}),
        );
        let mut interventions: Vec<(String, Value)> = Vec::new();
        for i in py_iter(&get_or(&card, "interventions", json!([]))?)? {
            let name = get(&i, "intervention")?;
            hashable(&name)?;
            if let Some(n) = name.as_str() {
                match interventions.iter_mut().find(|(k, _)| k == n) {
                    Some(slot) => slot.1 = i,
                    None => interventions.push((n.to_string(), i)),
                }
            }
        }
        let plan = or(
            &plan_ecos
                .iter()
                .find(|(k, _)| k == plan_key)
                .map_or(Value::Null, |(_, v)| v.clone()),
            json!({}),
        );
        let plan_activities: Vec<Value> = py_iter(&get_or(&plan, "activities", json!([]))?)?
            .into_iter()
            .filter(Value::is_object)
            .collect();

        let mut entry = Map::new();
        entry.insert("label".into(), json!(label));
        let mut names: Vec<Value> = Vec::new();
        for name in ["Protect", "Manage", "Restore"] {
            let lower = name.to_lowercase();
            let intervention = or(
                &interventions
                    .iter()
                    .find(|(k, _)| k == name)
                    .map_or(Value::Null, |(_, v)| v.clone()),
                json!({}),
            );
            let area = ha(&get(&intervention, "area_ha")?);
            let area = if truthy(&area) {
                area
            } else {
                json!(format!("[NbS Pathway: hectare area eligible to {lower}]"))
            };
            entry.insert(format!("{lower}_ha"), area);
            let mut chosen = Vec::new();
            for a in &plan_activities {
                if py_str(&get_or(a, "pw", json!(""))?).to_uppercase() == name.to_uppercase() {
                    let n = get(a, "name")?;
                    if truthy(&n) {
                        chosen.push(n);
                    }
                }
            }
            let mut available_entries = Vec::new();
            for a in py_iter(&get_or(&intervention, "activities", json!([]))?)? {
                if truthy(&get(&a, "activity")?) {
                    available_entries.push(a);
                }
            }
            if chosen.is_empty() {
                let selection = or(&get(&selections, plan_key)?, json!({}));
                if truthy(&get(&selection, &lower)?) {
                    let ids: Vec<String> =
                        py_iter(&or(&get(&selection, "activities")?, json!([])))?
                            .iter()
                            .map(py_str)
                            .collect();
                    for a in &available_entries {
                        if ids.is_empty() || ids.contains(&py_str(&get(a, "activity_id")?)) {
                            chosen.push(get(a, "activity")?);
                        }
                    }
                }
            }
            let acts = if !chosen.is_empty() {
                chosen
            } else if !plan_ecos.is_empty() || truthy(&selections) {
                Vec::new()
            } else {
                available_entries
                    .iter()
                    .map(|a| get(a, "activity"))
                    .collect::<R<_>>()?
            };
            entry.insert(
                format!("activities_{lower}"),
                or(&join(&acts, ", "), json!(CHOSEN_BRACKET)),
            );
            for a in acts {
                if !names.iter().any(|n| py_eq(n, &a)) {
                    names.push(a);
                }
            }
        }
        entry.insert(
            "activities_all".into(),
            or(&join(&names, ", "), json!(CHOSEN_BRACKET)),
        );
        for n in names {
            if !all_chosen.iter().any(|c| py_eq(c, &n)) {
                all_chosen.push(n);
            }
        }
        let mut indicator_rows = Vec::new();
        for activity in &plan_activities {
            for group in py_iter(&get_or(activity, "groups", json!([]))?)? {
                if !group.is_object() {
                    continue;
                }
                for item in py_iter(&get_or(&group, "items", json!([]))?)? {
                    if item.is_object() {
                        indicator_rows.push(json!({
                            "category": or(&get(&group, "cat")?, json!("")),
                            "benefit": or(&get(&group, "benefit")?, json!("")),
                            "indicator": or(&get(&item, "label")?, json!("")),
                            "unit": or(&get(&item, "unit")?, json!("")),
                            "freq": or(&get(&item, "freq")?, json!("")),
                            "source": or(&get(&item, "source")?, json!("")),
                        }));
                    }
                }
            }
        }
        entry.insert("indicator_rows".into(), Value::Array(indicator_rows));
        ecos.push(Value::Object(entry));
    }

    let mut species_rows = Vec::new();
    for s in py_iter(&get_or(&nature, "species_list", json!([]))?)? {
        if s.is_object() {
            species_rows.push(json!({
                "taxon_class": get(&s, "taxon_class")?,
                "scientific_name": get(&s, "scientific_name")?,
                "redlist_category": get(&s, "redlist_category")?,
            }));
        }
    }

    if !all_chosen.is_empty() {
        tags.set(
            "NbS Pathway: Chosen NbS Activities",
            join(&all_chosen, ", "),
        );
        tags.set(
            "NbS Pathway: x NbS Activities",
            json!(format!("{} NbS activities", all_chosen.len())),
        );
    }
    let mut chosen_pathways = Vec::new();
    for pw in ["Protect", "Manage", "Restore"] {
        for key in ["forest", "mangrove", "peatland"] {
            if truthy(&get(
                &or(&get(&selections, key)?, json!({})),
                &pw.to_lowercase(),
            )?) {
                chosen_pathways.push(json!(pw));
                break;
            }
        }
    }
    if chosen_pathways.is_empty() {
        let present = or(
            &get(
                &or(&get(&benefit, "general_benefit")?, json!({})),
                "pathways_present",
            )?,
            json!([]),
        );
        for pw in ["Protect", "Manage", "Restore"] {
            if py_contains(&present, &json!(pw))? {
                chosen_pathways.push(json!(pw));
            }
        }
    }
    tags.set(
        "NbS Pathway: Protect / Manage / Restore",
        join(&chosen_pathways, " + "),
    );
    let mut run_duration = get(&assumptions, "duration_years")?;
    if !truthy(&run_duration) {
        run_duration = get(&or(&get(&pathway, "duration_years")?, json!({})), "default")?;
    }
    if truthy(&run_duration) {
        let text = json!(format!("{} years", py_str(&run_duration)));
        tags.set("NbS Pathway: x duration", text.clone());
        tags.set(
            "NbS Pathway: Duration of protection (10–40 years, default 30)",
            text,
        );
    }

    // lookup = {_norm(k): v for k, v in tags.items() if v is not None}
    let mut lookup = Tags::default();
    for (k, v) in tags.0 {
        if !v.is_null() {
            lookup.set(&norm(&k), v);
        }
    }

    let carbon_text = |v: &Value| {
        if v.is_null() {
            json!("[Potential Benefit: X tonnes]")
        } else {
            ha(v)
        }
    };
    let mut out = Map::new();
    out.insert("tags".into(), Value::Object(lookup.0.into_iter().collect()));
    out.insert("land_cover_rest".into(), Value::Array(land_cover_rest));
    out.insert("species_rows".into(), Value::Array(species_rows));
    out.insert(
        "keystone_present".into(),
        json!(truthy(&get(&nature, "key_species")?)),
    );
    for (i, eco) in ecos.into_iter().enumerate() {
        out.insert(format!("eco{i}"), eco);
    }
    out.insert(
        "benefit_avoided_tco2e".into(),
        carbon_text(&benefit_avoided),
    );
    out.insert(
        "benefit_sequestered_tco2e".into(),
        carbon_text(&benefit_sequestered),
    );
    benefit_pillar_counts(&benefit, &mut out)?;
    education_totals(&form, &mut out)?;
    Ok(Value::Object(out))
}

const CHOSEN_BRACKET: &str = "[NbS Pathway: Chosen NbS Activities]";

// ---- Python semantics over JSON values ----

pub(crate) fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `v in (None, "", [], {})`
pub(crate) fn empty_like(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// `v or default`
pub(crate) fn or(v: &Value, default: Value) -> Value {
    if truthy(v) { v.clone() } else { default }
}

/// `v.get(key)`: only a dict has `.get`.
pub(crate) fn get(v: &Value, key: &str) -> R<Value> {
    get_or(v, key, Value::Null)
}

/// `v.get(key, default)`
pub(crate) fn get_or(v: &Value, key: &str, default: Value) -> R<Value> {
    match v {
        Value::Object(map) => Ok(map.get(key).cloned().unwrap_or(default)),
        other => Err(format!(
            "'{}' object has no attribute 'get'",
            py_type(other)
        )),
    }
}

/// `isinstance(v, (int, float))` (bool is an int), as a float.
pub(crate) fn is_num(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(*b as i64 as f64),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// An int-like value (bool or JSON integer) as i64.
pub(crate) fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) if !n.is_f64() => n.as_i64(),
        _ => None,
    }
}

pub(crate) fn hashable(v: &Value) -> R<()> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(format!("unhashable type: '{}'", py_type(v))),
        _ => Ok(()),
    }
}

/// `a + b`
pub(crate) fn py_add(a: &Value, b: &Value) -> R<Value> {
    if let (Some(x), Some(y)) = (as_int(a), as_int(b))
        && let Some(s) = x.checked_add(y)
    {
        return Ok(json!(s));
    }
    if let (Some(x), Some(y)) = (is_num(a), is_num(b)) {
        return Ok(json!(x + y));
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Ok(json!(format!("{x}{y}"))),
        (Value::Array(x), Value::Array(y)) => {
            Ok(Value::Array(x.iter().chain(y).cloned().collect()))
        }
        (Value::String(_), other) => Err(format!(
            "can only concatenate str (not \"{}\") to str",
            py_type(other)
        )),
        (Value::Array(_), other) => Err(format!(
            "can only concatenate list (not \"{}\") to list",
            py_type(other)
        )),
        _ => Err(format!(
            "unsupported operand type(s) for +: '{}' and '{}'",
            py_type(a),
            py_type(b)
        )),
    }
}

/// `a / b`, a number.
#[expect(
    clippy::redundant_guards,
    reason = "Python's `b == 0` test, -0.0 included, kept readable as written"
)]
fn py_div(a: &Value, b: &Value) -> R<f64> {
    match (is_num(a), is_num(b)) {
        (Some(_), Some(y)) if y == 0.0 => Err(if as_int(a).is_some() && as_int(b).is_some() {
            "division by zero".into()
        } else {
            "float division by zero".into()
        }),
        (Some(x), Some(y)) => Ok(x / y),
        _ => Err(format!(
            "unsupported operand type(s) for /: '{}' and '{}'",
            py_type(a),
            py_type(b)
        )),
    }
}

/// `-v`, as a sort key.
fn py_neg(v: &Value) -> R<f64> {
    is_num(v)
        .map(|f| -f)
        .ok_or_else(|| format!("bad operand type for unary -: '{}'", py_type(v)))
}

/// `a == b`
pub(crate) fn py_eq(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (is_num(a), is_num(b)) {
        return x == y;
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => false,
    }
}

/// `a > b`
pub(crate) fn py_gt(a: &Value, b: &Value) -> R<bool> {
    if let (Some(x), Some(y)) = (is_num(a), is_num(b)) {
        return Ok(x > y);
    }
    match (a, b) {
        // code point order is UTF-8 byte order
        (Value::String(x), Value::String(y)) => Ok(x > y),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !py_eq(p, q) {
                    return py_gt(p, q);
                }
            }
            Ok(x.len() > y.len())
        }
        _ => Err(format!(
            "'>' not supported between instances of '{}' and '{}'",
            py_type(a),
            py_type(b)
        )),
    }
}

/// `item in container`
fn py_contains(container: &Value, item: &Value) -> R<bool> {
    match container {
        Value::Array(list) => Ok(list.iter().any(|x| py_eq(x, item))),
        Value::String(s) => match item {
            Value::String(i) => Ok(s.contains(i.as_str())),
            other => Err(format!(
                "'in <string>' requires string as left operand, not {}",
                py_type(other)
            )),
        },
        Value::Object(map) => {
            hashable(item)?;
            Ok(item.as_str().is_some_and(|k| map.contains_key(k)))
        }
        other => Err(format!(
            "argument of type '{}' is not iterable",
            py_type(other)
        )),
    }
}

/// `len(v)`
fn py_len(v: &Value) -> R<usize> {
    match v {
        Value::Array(list) => Ok(list.len()),
        Value::String(s) => Ok(s.chars().count()),
        Value::Object(map) => Ok(map.len()),
        other => Err(format!("object of type '{}' has no len()", py_type(other))),
    }
}

/// `v[i]`
fn py_index(v: &Value, i: usize) -> R<Value> {
    match v {
        Value::Array(list) => list
            .get(i)
            .cloned()
            .ok_or_else(|| "list index out of range".into()),
        Value::String(s) => s
            .chars()
            .nth(i)
            .map(|c| json!(c.to_string()))
            .ok_or_else(|| "string index out of range".into()),
        // JSON keys are strings: an int key is never there (KeyError)
        Value::Object(_) => Err(i.to_string()),
        other => Err(format!("'{}' object is not subscriptable", py_type(other))),
    }
}

/// `float(v)`; Err stands for the TypeError / ValueError the callers catch.
fn py_float(v: &Value) -> Result<f64, ()> {
    match v {
        Value::Bool(_) | Value::Number(_) => is_num(v).ok_or(()),
        Value::String(s) => py_number_text(s)
            .and_then(|t| t.parse::<f64>().ok())
            .ok_or(()),
        _ => Err(()),
    }
}

/// The text float() accepts: stripped, underscores allowed only between digits.
fn py_number_text(s: &str) -> Option<String> {
    let s = s.trim_matches(py_space);
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'_'
            && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// `for x in v`: a list yields items, a dict its keys, a str its characters.
pub(crate) fn py_iter(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(jsonb_keys(map).into_iter().map(|k| json!(k)).collect()),
        Value::String(s) => Ok(s.chars().map(|c| json!(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", py_type(other))),
    }
}

/// `v.items()` of a dict in jsonb order; nothing for anything else.
fn items(v: &Value) -> Vec<(&String, &Value)> {
    match v {
        Value::Object(map) => jsonb_keys(map).into_iter().map(|k| (k, &map[k])).collect(),
        _ => Vec::new(),
    }
}

/// A dict's keys in Python's order. psycopg2 hands back jsonb objects in jsonb's own key order
/// (shorter keys first, then bytes), which is the order a jsonb value's text parses into under
/// serde_json's `preserve_order`; a merged dict (`load_draft`) keeps its merge order.
fn jsonb_keys(map: &Map<String, Value>) -> Vec<&String> {
    map.keys().collect()
}

/// `str(v)`
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(v)` for a value json.loads produced.
pub(crate) fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) if !n.is_f64() => n.to_string(),
        Value::Number(n) => py_float_repr(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            jsonb_keys(map)
                .into_iter()
                .map(|k| format!("{}: {}", repr_str(k), py_repr(&map[k])))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `repr(str)`. shortcut: `str.isprintable` is approximated (controls, the Zs/Zl/Zp
/// separators, the common Cf format characters, private use); an unassigned code point is kept
/// where Python escapes it. Only reachable through a list or dict written to a CSV.
pub(crate) fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !py_printable(c) => out.push_str(&match c as u32 {
                n @ 0..=0xff => format!("\\x{n:02x}"),
                n @ 0x100..=0xffff => format!("\\u{n:04x}"),
                n => format!("\\U{n:08x}"),
            }),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str.isprintable()` for one character (see `repr_str`).
fn py_printable(c: char) -> bool {
    let n = c as u32;
    !(n < 0x20
        || (0x7f..=0xa0).contains(&n)
        || n == 0xad
        || n == 0x1680
        || (0x2000..=0x200f).contains(&n)
        || (0x2028..=0x202f).contains(&n)
        || (0x205f..=0x206f).contains(&n)
        || n == 0x3000
        || n == 0xfeff
        || (0xfff9..=0xfffb).contains(&n)
        || (0xe000..=0xf8ff).contains(&n)
        || n >= 0xf0000
        || [
            0x600, 0x601, 0x602, 0x603, 0x604, 0x605, 0x61c, 0x6dd, 0x70f, 0x180e,
        ]
        .contains(&n))
}

/// `format(f, ",.Nf")` / `format(f, ".Nf")`: Python rounds the exact binary value half-even,
/// as Rust's `{:.N}` does.
pub(crate) fn fmt_f(f: f64, prec: usize, comma: bool) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let s = format!("{f:.prec$}");
    if !comma {
        return s;
    }
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s.as_str()),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(frac) => format!("{sign}{grouped}.{frac}"),
        None => format!("{sign}{grouped}"),
    }
}

/// str.isspace: Rust's White_Space plus the four separators U+001C..U+001F.
pub(crate) fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `" ".join(s.split())`
fn collapse(s: &str) -> String {
    s.split(py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python's `html.unescape`.
fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let j = i + 1;
        if j < n && chars[j] == '#' {
            // `#[0-9]+;?` then `#[xX][0-9a-fA-F]+;?`
            let (radix, start) = if j + 1 < n && (chars[j + 1] == 'x' || chars[j + 1] == 'X') {
                (16, j + 2)
            } else {
                (10, j + 1)
            };
            let mut k = start;
            while k < n && chars[k].is_digit(radix) && chars[k].is_ascii() {
                k += 1;
            }
            // `&#x` without hex digits matches neither branch (x is no decimal digit)
            if k > start {
                let digits: String = chars[start..k].iter().collect();
                let code = u64::from_str_radix(&digits, radix).unwrap_or(u64::MAX);
                out.push_str(&numeric_charref(code));
                i = if k < n && chars[k] == ';' { k + 1 } else { k };
                continue;
            }
            out.push('&');
            i += 1;
            continue;
        }
        // `[^\t\n\f <&#;]{1,32};?`
        let mut k = j;
        while k < n
            && k - j < 32
            && !matches!(chars[k], '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';')
        {
            k += 1;
        }
        if k == j {
            out.push('&');
            i += 1;
            continue;
        }
        if k < n && chars[k] == ';' {
            k += 1;
        }
        let name: String = chars[j..k].iter().collect();
        out.push_str(&named_charref(&name));
        i = k;
    }
    out
}

fn numeric_charref(code: u64) -> String {
    if let Some(c) = INVALID_CHARREFS.iter().find(|(n, _)| *n as u64 == code) {
        return c.1.to_string();
    }
    if (0xD800..=0xDFFF).contains(&code) || code > 0x10FFFF {
        return "\u{fffd}".into();
    }
    let invalid = (0x1..=0x8).contains(&code)
        || code == 0xb
        || (0xe..=0x1f).contains(&code)
        || (0x7f..=0x9f).contains(&code)
        || (0xfdd0..=0xfdef).contains(&code)
        || (code & 0xfffe) == 0xfffe;
    if invalid {
        return String::new();
    }
    char::from_u32(code as u32).map_or_else(|| "\u{fffd}".into(), |c| c.to_string())
}

fn html5(name: &str) -> Option<&'static str> {
    HTML5
        .binary_search_by(|(k, _)| k.as_bytes().cmp(name.as_bytes()))
        .ok()
        .map(|i| HTML5[i].1)
}

fn named_charref(name: &str) -> String {
    if let Some(v) = html5(name) {
        return v.to_string();
    }
    let chars: Vec<char> = name.chars().collect();
    for x in (2..chars.len()).rev() {
        let head: String = chars[..x].iter().collect();
        if let Some(v) = html5(&head) {
            let tail: String = chars[x..].iter().collect();
            return format!("{v}{tail}");
        }
    }
    format!("&{name}")
}

/// `html._invalid_charrefs`
const INVALID_CHARREFS: &[(u32, char)] = &[
    (0x00, '\u{fffd}'),
    (0x0d, '\r'),
    (0x80, '\u{20ac}'),
    (0x81, '\u{81}'),
    (0x82, '\u{201a}'),
    (0x83, '\u{192}'),
    (0x84, '\u{201e}'),
    (0x85, '\u{2026}'),
    (0x86, '\u{2020}'),
    (0x87, '\u{2021}'),
    (0x88, '\u{2c6}'),
    (0x89, '\u{2030}'),
    (0x8a, '\u{160}'),
    (0x8b, '\u{2039}'),
    (0x8c, '\u{152}'),
    (0x8d, '\u{8d}'),
    (0x8e, '\u{17d}'),
    (0x8f, '\u{8f}'),
    (0x90, '\u{90}'),
    (0x91, '\u{2018}'),
    (0x92, '\u{2019}'),
    (0x93, '\u{201c}'),
    (0x94, '\u{201d}'),
    (0x95, '\u{2022}'),
    (0x96, '\u{2013}'),
    (0x97, '\u{2014}'),
    (0x98, '\u{2dc}'),
    (0x99, '\u{2122}'),
    (0x9a, '\u{161}'),
    (0x9b, '\u{203a}'),
    (0x9c, '\u{153}'),
    (0x9d, '\u{9d}'),
    (0x9e, '\u{17e}'),
    (0x9f, '\u{178}'),
];

/// `html.entities.html5`, sorted by name.
const HTML5: &[(&str, &str)] = &[
    ("AElig", "\u{c6}"),
    ("AElig;", "\u{c6}"),
    ("AMP", "&"),
    ("AMP;", "&"),
    ("Aacute", "\u{c1}"),
    ("Aacute;", "\u{c1}"),
    ("Abreve;", "\u{102}"),
    ("Acirc", "\u{c2}"),
    ("Acirc;", "\u{c2}"),
    ("Acy;", "\u{410}"),
    ("Afr;", "\u{1d504}"),
    ("Agrave", "\u{c0}"),
    ("Agrave;", "\u{c0}"),
    ("Alpha;", "\u{391}"),
    ("Amacr;", "\u{100}"),
    ("And;", "\u{2a53}"),
    ("Aogon;", "\u{104}"),
    ("Aopf;", "\u{1d538}"),
    ("ApplyFunction;", "\u{2061}"),
    ("Aring", "\u{c5}"),
    ("Aring;", "\u{c5}"),
    ("Ascr;", "\u{1d49c}"),
    ("Assign;", "\u{2254}"),
    ("Atilde", "\u{c3}"),
    ("Atilde;", "\u{c3}"),
    ("Auml", "\u{c4}"),
    ("Auml;", "\u{c4}"),
    ("Backslash;", "\u{2216}"),
    ("Barv;", "\u{2ae7}"),
    ("Barwed;", "\u{2306}"),
    ("Bcy;", "\u{411}"),
    ("Because;", "\u{2235}"),
    ("Bernoullis;", "\u{212c}"),
    ("Beta;", "\u{392}"),
    ("Bfr;", "\u{1d505}"),
    ("Bopf;", "\u{1d539}"),
    ("Breve;", "\u{2d8}"),
    ("Bscr;", "\u{212c}"),
    ("Bumpeq;", "\u{224e}"),
    ("CHcy;", "\u{427}"),
    ("COPY", "\u{a9}"),
    ("COPY;", "\u{a9}"),
    ("Cacute;", "\u{106}"),
    ("Cap;", "\u{22d2}"),
    ("CapitalDifferentialD;", "\u{2145}"),
    ("Cayleys;", "\u{212d}"),
    ("Ccaron;", "\u{10c}"),
    ("Ccedil", "\u{c7}"),
    ("Ccedil;", "\u{c7}"),
    ("Ccirc;", "\u{108}"),
    ("Cconint;", "\u{2230}"),
    ("Cdot;", "\u{10a}"),
    ("Cedilla;", "\u{b8}"),
    ("CenterDot;", "\u{b7}"),
    ("Cfr;", "\u{212d}"),
    ("Chi;", "\u{3a7}"),
    ("CircleDot;", "\u{2299}"),
    ("CircleMinus;", "\u{2296}"),
    ("CirclePlus;", "\u{2295}"),
    ("CircleTimes;", "\u{2297}"),
    ("ClockwiseContourIntegral;", "\u{2232}"),
    ("CloseCurlyDoubleQuote;", "\u{201d}"),
    ("CloseCurlyQuote;", "\u{2019}"),
    ("Colon;", "\u{2237}"),
    ("Colone;", "\u{2a74}"),
    ("Congruent;", "\u{2261}"),
    ("Conint;", "\u{222f}"),
    ("ContourIntegral;", "\u{222e}"),
    ("Copf;", "\u{2102}"),
    ("Coproduct;", "\u{2210}"),
    ("CounterClockwiseContourIntegral;", "\u{2233}"),
    ("Cross;", "\u{2a2f}"),
    ("Cscr;", "\u{1d49e}"),
    ("Cup;", "\u{22d3}"),
    ("CupCap;", "\u{224d}"),
    ("DD;", "\u{2145}"),
    ("DDotrahd;", "\u{2911}"),
    ("DJcy;", "\u{402}"),
    ("DScy;", "\u{405}"),
    ("DZcy;", "\u{40f}"),
    ("Dagger;", "\u{2021}"),
    ("Darr;", "\u{21a1}"),
    ("Dashv;", "\u{2ae4}"),
    ("Dcaron;", "\u{10e}"),
    ("Dcy;", "\u{414}"),
    ("Del;", "\u{2207}"),
    ("Delta;", "\u{394}"),
    ("Dfr;", "\u{1d507}"),
    ("DiacriticalAcute;", "\u{b4}"),
    ("DiacriticalDot;", "\u{2d9}"),
    ("DiacriticalDoubleAcute;", "\u{2dd}"),
    ("DiacriticalGrave;", "`"),
    ("DiacriticalTilde;", "\u{2dc}"),
    ("Diamond;", "\u{22c4}"),
    ("DifferentialD;", "\u{2146}"),
    ("Dopf;", "\u{1d53b}"),
    ("Dot;", "\u{a8}"),
    ("DotDot;", "\u{20dc}"),
    ("DotEqual;", "\u{2250}"),
    ("DoubleContourIntegral;", "\u{222f}"),
    ("DoubleDot;", "\u{a8}"),
    ("DoubleDownArrow;", "\u{21d3}"),
    ("DoubleLeftArrow;", "\u{21d0}"),
    ("DoubleLeftRightArrow;", "\u{21d4}"),
    ("DoubleLeftTee;", "\u{2ae4}"),
    ("DoubleLongLeftArrow;", "\u{27f8}"),
    ("DoubleLongLeftRightArrow;", "\u{27fa}"),
    ("DoubleLongRightArrow;", "\u{27f9}"),
    ("DoubleRightArrow;", "\u{21d2}"),
    ("DoubleRightTee;", "\u{22a8}"),
    ("DoubleUpArrow;", "\u{21d1}"),
    ("DoubleUpDownArrow;", "\u{21d5}"),
    ("DoubleVerticalBar;", "\u{2225}"),
    ("DownArrow;", "\u{2193}"),
    ("DownArrowBar;", "\u{2913}"),
    ("DownArrowUpArrow;", "\u{21f5}"),
    ("DownBreve;", "\u{311}"),
    ("DownLeftRightVector;", "\u{2950}"),
    ("DownLeftTeeVector;", "\u{295e}"),
    ("DownLeftVector;", "\u{21bd}"),
    ("DownLeftVectorBar;", "\u{2956}"),
    ("DownRightTeeVector;", "\u{295f}"),
    ("DownRightVector;", "\u{21c1}"),
    ("DownRightVectorBar;", "\u{2957}"),
    ("DownTee;", "\u{22a4}"),
    ("DownTeeArrow;", "\u{21a7}"),
    ("Downarrow;", "\u{21d3}"),
    ("Dscr;", "\u{1d49f}"),
    ("Dstrok;", "\u{110}"),
    ("ENG;", "\u{14a}"),
    ("ETH", "\u{d0}"),
    ("ETH;", "\u{d0}"),
    ("Eacute", "\u{c9}"),
    ("Eacute;", "\u{c9}"),
    ("Ecaron;", "\u{11a}"),
    ("Ecirc", "\u{ca}"),
    ("Ecirc;", "\u{ca}"),
    ("Ecy;", "\u{42d}"),
    ("Edot;", "\u{116}"),
    ("Efr;", "\u{1d508}"),
    ("Egrave", "\u{c8}"),
    ("Egrave;", "\u{c8}"),
    ("Element;", "\u{2208}"),
    ("Emacr;", "\u{112}"),
    ("EmptySmallSquare;", "\u{25fb}"),
    ("EmptyVerySmallSquare;", "\u{25ab}"),
    ("Eogon;", "\u{118}"),
    ("Eopf;", "\u{1d53c}"),
    ("Epsilon;", "\u{395}"),
    ("Equal;", "\u{2a75}"),
    ("EqualTilde;", "\u{2242}"),
    ("Equilibrium;", "\u{21cc}"),
    ("Escr;", "\u{2130}"),
    ("Esim;", "\u{2a73}"),
    ("Eta;", "\u{397}"),
    ("Euml", "\u{cb}"),
    ("Euml;", "\u{cb}"),
    ("Exists;", "\u{2203}"),
    ("ExponentialE;", "\u{2147}"),
    ("Fcy;", "\u{424}"),
    ("Ffr;", "\u{1d509}"),
    ("FilledSmallSquare;", "\u{25fc}"),
    ("FilledVerySmallSquare;", "\u{25aa}"),
    ("Fopf;", "\u{1d53d}"),
    ("ForAll;", "\u{2200}"),
    ("Fouriertrf;", "\u{2131}"),
    ("Fscr;", "\u{2131}"),
    ("GJcy;", "\u{403}"),
    ("GT", ">"),
    ("GT;", ">"),
    ("Gamma;", "\u{393}"),
    ("Gammad;", "\u{3dc}"),
    ("Gbreve;", "\u{11e}"),
    ("Gcedil;", "\u{122}"),
    ("Gcirc;", "\u{11c}"),
    ("Gcy;", "\u{413}"),
    ("Gdot;", "\u{120}"),
    ("Gfr;", "\u{1d50a}"),
    ("Gg;", "\u{22d9}"),
    ("Gopf;", "\u{1d53e}"),
    ("GreaterEqual;", "\u{2265}"),
    ("GreaterEqualLess;", "\u{22db}"),
    ("GreaterFullEqual;", "\u{2267}"),
    ("GreaterGreater;", "\u{2aa2}"),
    ("GreaterLess;", "\u{2277}"),
    ("GreaterSlantEqual;", "\u{2a7e}"),
    ("GreaterTilde;", "\u{2273}"),
    ("Gscr;", "\u{1d4a2}"),
    ("Gt;", "\u{226b}"),
    ("HARDcy;", "\u{42a}"),
    ("Hacek;", "\u{2c7}"),
    ("Hat;", "^"),
    ("Hcirc;", "\u{124}"),
    ("Hfr;", "\u{210c}"),
    ("HilbertSpace;", "\u{210b}"),
    ("Hopf;", "\u{210d}"),
    ("HorizontalLine;", "\u{2500}"),
    ("Hscr;", "\u{210b}"),
    ("Hstrok;", "\u{126}"),
    ("HumpDownHump;", "\u{224e}"),
    ("HumpEqual;", "\u{224f}"),
    ("IEcy;", "\u{415}"),
    ("IJlig;", "\u{132}"),
    ("IOcy;", "\u{401}"),
    ("Iacute", "\u{cd}"),
    ("Iacute;", "\u{cd}"),
    ("Icirc", "\u{ce}"),
    ("Icirc;", "\u{ce}"),
    ("Icy;", "\u{418}"),
    ("Idot;", "\u{130}"),
    ("Ifr;", "\u{2111}"),
    ("Igrave", "\u{cc}"),
    ("Igrave;", "\u{cc}"),
    ("Im;", "\u{2111}"),
    ("Imacr;", "\u{12a}"),
    ("ImaginaryI;", "\u{2148}"),
    ("Implies;", "\u{21d2}"),
    ("Int;", "\u{222c}"),
    ("Integral;", "\u{222b}"),
    ("Intersection;", "\u{22c2}"),
    ("InvisibleComma;", "\u{2063}"),
    ("InvisibleTimes;", "\u{2062}"),
    ("Iogon;", "\u{12e}"),
    ("Iopf;", "\u{1d540}"),
    ("Iota;", "\u{399}"),
    ("Iscr;", "\u{2110}"),
    ("Itilde;", "\u{128}"),
    ("Iukcy;", "\u{406}"),
    ("Iuml", "\u{cf}"),
    ("Iuml;", "\u{cf}"),
    ("Jcirc;", "\u{134}"),
    ("Jcy;", "\u{419}"),
    ("Jfr;", "\u{1d50d}"),
    ("Jopf;", "\u{1d541}"),
    ("Jscr;", "\u{1d4a5}"),
    ("Jsercy;", "\u{408}"),
    ("Jukcy;", "\u{404}"),
    ("KHcy;", "\u{425}"),
    ("KJcy;", "\u{40c}"),
    ("Kappa;", "\u{39a}"),
    ("Kcedil;", "\u{136}"),
    ("Kcy;", "\u{41a}"),
    ("Kfr;", "\u{1d50e}"),
    ("Kopf;", "\u{1d542}"),
    ("Kscr;", "\u{1d4a6}"),
    ("LJcy;", "\u{409}"),
    ("LT", "<"),
    ("LT;", "<"),
    ("Lacute;", "\u{139}"),
    ("Lambda;", "\u{39b}"),
    ("Lang;", "\u{27ea}"),
    ("Laplacetrf;", "\u{2112}"),
    ("Larr;", "\u{219e}"),
    ("Lcaron;", "\u{13d}"),
    ("Lcedil;", "\u{13b}"),
    ("Lcy;", "\u{41b}"),
    ("LeftAngleBracket;", "\u{27e8}"),
    ("LeftArrow;", "\u{2190}"),
    ("LeftArrowBar;", "\u{21e4}"),
    ("LeftArrowRightArrow;", "\u{21c6}"),
    ("LeftCeiling;", "\u{2308}"),
    ("LeftDoubleBracket;", "\u{27e6}"),
    ("LeftDownTeeVector;", "\u{2961}"),
    ("LeftDownVector;", "\u{21c3}"),
    ("LeftDownVectorBar;", "\u{2959}"),
    ("LeftFloor;", "\u{230a}"),
    ("LeftRightArrow;", "\u{2194}"),
    ("LeftRightVector;", "\u{294e}"),
    ("LeftTee;", "\u{22a3}"),
    ("LeftTeeArrow;", "\u{21a4}"),
    ("LeftTeeVector;", "\u{295a}"),
    ("LeftTriangle;", "\u{22b2}"),
    ("LeftTriangleBar;", "\u{29cf}"),
    ("LeftTriangleEqual;", "\u{22b4}"),
    ("LeftUpDownVector;", "\u{2951}"),
    ("LeftUpTeeVector;", "\u{2960}"),
    ("LeftUpVector;", "\u{21bf}"),
    ("LeftUpVectorBar;", "\u{2958}"),
    ("LeftVector;", "\u{21bc}"),
    ("LeftVectorBar;", "\u{2952}"),
    ("Leftarrow;", "\u{21d0}"),
    ("Leftrightarrow;", "\u{21d4}"),
    ("LessEqualGreater;", "\u{22da}"),
    ("LessFullEqual;", "\u{2266}"),
    ("LessGreater;", "\u{2276}"),
    ("LessLess;", "\u{2aa1}"),
    ("LessSlantEqual;", "\u{2a7d}"),
    ("LessTilde;", "\u{2272}"),
    ("Lfr;", "\u{1d50f}"),
    ("Ll;", "\u{22d8}"),
    ("Lleftarrow;", "\u{21da}"),
    ("Lmidot;", "\u{13f}"),
    ("LongLeftArrow;", "\u{27f5}"),
    ("LongLeftRightArrow;", "\u{27f7}"),
    ("LongRightArrow;", "\u{27f6}"),
    ("Longleftarrow;", "\u{27f8}"),
    ("Longleftrightarrow;", "\u{27fa}"),
    ("Longrightarrow;", "\u{27f9}"),
    ("Lopf;", "\u{1d543}"),
    ("LowerLeftArrow;", "\u{2199}"),
    ("LowerRightArrow;", "\u{2198}"),
    ("Lscr;", "\u{2112}"),
    ("Lsh;", "\u{21b0}"),
    ("Lstrok;", "\u{141}"),
    ("Lt;", "\u{226a}"),
    ("Map;", "\u{2905}"),
    ("Mcy;", "\u{41c}"),
    ("MediumSpace;", "\u{205f}"),
    ("Mellintrf;", "\u{2133}"),
    ("Mfr;", "\u{1d510}"),
    ("MinusPlus;", "\u{2213}"),
    ("Mopf;", "\u{1d544}"),
    ("Mscr;", "\u{2133}"),
    ("Mu;", "\u{39c}"),
    ("NJcy;", "\u{40a}"),
    ("Nacute;", "\u{143}"),
    ("Ncaron;", "\u{147}"),
    ("Ncedil;", "\u{145}"),
    ("Ncy;", "\u{41d}"),
    ("NegativeMediumSpace;", "\u{200b}"),
    ("NegativeThickSpace;", "\u{200b}"),
    ("NegativeThinSpace;", "\u{200b}"),
    ("NegativeVeryThinSpace;", "\u{200b}"),
    ("NestedGreaterGreater;", "\u{226b}"),
    ("NestedLessLess;", "\u{226a}"),
    ("NewLine;", "\u{a}"),
    ("Nfr;", "\u{1d511}"),
    ("NoBreak;", "\u{2060}"),
    ("NonBreakingSpace;", "\u{a0}"),
    ("Nopf;", "\u{2115}"),
    ("Not;", "\u{2aec}"),
    ("NotCongruent;", "\u{2262}"),
    ("NotCupCap;", "\u{226d}"),
    ("NotDoubleVerticalBar;", "\u{2226}"),
    ("NotElement;", "\u{2209}"),
    ("NotEqual;", "\u{2260}"),
    ("NotEqualTilde;", "\u{2242}\u{338}"),
    ("NotExists;", "\u{2204}"),
    ("NotGreater;", "\u{226f}"),
    ("NotGreaterEqual;", "\u{2271}"),
    ("NotGreaterFullEqual;", "\u{2267}\u{338}"),
    ("NotGreaterGreater;", "\u{226b}\u{338}"),
    ("NotGreaterLess;", "\u{2279}"),
    ("NotGreaterSlantEqual;", "\u{2a7e}\u{338}"),
    ("NotGreaterTilde;", "\u{2275}"),
    ("NotHumpDownHump;", "\u{224e}\u{338}"),
    ("NotHumpEqual;", "\u{224f}\u{338}"),
    ("NotLeftTriangle;", "\u{22ea}"),
    ("NotLeftTriangleBar;", "\u{29cf}\u{338}"),
    ("NotLeftTriangleEqual;", "\u{22ec}"),
    ("NotLess;", "\u{226e}"),
    ("NotLessEqual;", "\u{2270}"),
    ("NotLessGreater;", "\u{2278}"),
    ("NotLessLess;", "\u{226a}\u{338}"),
    ("NotLessSlantEqual;", "\u{2a7d}\u{338}"),
    ("NotLessTilde;", "\u{2274}"),
    ("NotNestedGreaterGreater;", "\u{2aa2}\u{338}"),
    ("NotNestedLessLess;", "\u{2aa1}\u{338}"),
    ("NotPrecedes;", "\u{2280}"),
    ("NotPrecedesEqual;", "\u{2aaf}\u{338}"),
    ("NotPrecedesSlantEqual;", "\u{22e0}"),
    ("NotReverseElement;", "\u{220c}"),
    ("NotRightTriangle;", "\u{22eb}"),
    ("NotRightTriangleBar;", "\u{29d0}\u{338}"),
    ("NotRightTriangleEqual;", "\u{22ed}"),
    ("NotSquareSubset;", "\u{228f}\u{338}"),
    ("NotSquareSubsetEqual;", "\u{22e2}"),
    ("NotSquareSuperset;", "\u{2290}\u{338}"),
    ("NotSquareSupersetEqual;", "\u{22e3}"),
    ("NotSubset;", "\u{2282}\u{20d2}"),
    ("NotSubsetEqual;", "\u{2288}"),
    ("NotSucceeds;", "\u{2281}"),
    ("NotSucceedsEqual;", "\u{2ab0}\u{338}"),
    ("NotSucceedsSlantEqual;", "\u{22e1}"),
    ("NotSucceedsTilde;", "\u{227f}\u{338}"),
    ("NotSuperset;", "\u{2283}\u{20d2}"),
    ("NotSupersetEqual;", "\u{2289}"),
    ("NotTilde;", "\u{2241}"),
    ("NotTildeEqual;", "\u{2244}"),
    ("NotTildeFullEqual;", "\u{2247}"),
    ("NotTildeTilde;", "\u{2249}"),
    ("NotVerticalBar;", "\u{2224}"),
    ("Nscr;", "\u{1d4a9}"),
    ("Ntilde", "\u{d1}"),
    ("Ntilde;", "\u{d1}"),
    ("Nu;", "\u{39d}"),
    ("OElig;", "\u{152}"),
    ("Oacute", "\u{d3}"),
    ("Oacute;", "\u{d3}"),
    ("Ocirc", "\u{d4}"),
    ("Ocirc;", "\u{d4}"),
    ("Ocy;", "\u{41e}"),
    ("Odblac;", "\u{150}"),
    ("Ofr;", "\u{1d512}"),
    ("Ograve", "\u{d2}"),
    ("Ograve;", "\u{d2}"),
    ("Omacr;", "\u{14c}"),
    ("Omega;", "\u{3a9}"),
    ("Omicron;", "\u{39f}"),
    ("Oopf;", "\u{1d546}"),
    ("OpenCurlyDoubleQuote;", "\u{201c}"),
    ("OpenCurlyQuote;", "\u{2018}"),
    ("Or;", "\u{2a54}"),
    ("Oscr;", "\u{1d4aa}"),
    ("Oslash", "\u{d8}"),
    ("Oslash;", "\u{d8}"),
    ("Otilde", "\u{d5}"),
    ("Otilde;", "\u{d5}"),
    ("Otimes;", "\u{2a37}"),
    ("Ouml", "\u{d6}"),
    ("Ouml;", "\u{d6}"),
    ("OverBar;", "\u{203e}"),
    ("OverBrace;", "\u{23de}"),
    ("OverBracket;", "\u{23b4}"),
    ("OverParenthesis;", "\u{23dc}"),
    ("PartialD;", "\u{2202}"),
    ("Pcy;", "\u{41f}"),
    ("Pfr;", "\u{1d513}"),
    ("Phi;", "\u{3a6}"),
    ("Pi;", "\u{3a0}"),
    ("PlusMinus;", "\u{b1}"),
    ("Poincareplane;", "\u{210c}"),
    ("Popf;", "\u{2119}"),
    ("Pr;", "\u{2abb}"),
    ("Precedes;", "\u{227a}"),
    ("PrecedesEqual;", "\u{2aaf}"),
    ("PrecedesSlantEqual;", "\u{227c}"),
    ("PrecedesTilde;", "\u{227e}"),
    ("Prime;", "\u{2033}"),
    ("Product;", "\u{220f}"),
    ("Proportion;", "\u{2237}"),
    ("Proportional;", "\u{221d}"),
    ("Pscr;", "\u{1d4ab}"),
    ("Psi;", "\u{3a8}"),
    ("QUOT", "\u{22}"),
    ("QUOT;", "\u{22}"),
    ("Qfr;", "\u{1d514}"),
    ("Qopf;", "\u{211a}"),
    ("Qscr;", "\u{1d4ac}"),
    ("RBarr;", "\u{2910}"),
    ("REG", "\u{ae}"),
    ("REG;", "\u{ae}"),
    ("Racute;", "\u{154}"),
    ("Rang;", "\u{27eb}"),
    ("Rarr;", "\u{21a0}"),
    ("Rarrtl;", "\u{2916}"),
    ("Rcaron;", "\u{158}"),
    ("Rcedil;", "\u{156}"),
    ("Rcy;", "\u{420}"),
    ("Re;", "\u{211c}"),
    ("ReverseElement;", "\u{220b}"),
    ("ReverseEquilibrium;", "\u{21cb}"),
    ("ReverseUpEquilibrium;", "\u{296f}"),
    ("Rfr;", "\u{211c}"),
    ("Rho;", "\u{3a1}"),
    ("RightAngleBracket;", "\u{27e9}"),
    ("RightArrow;", "\u{2192}"),
    ("RightArrowBar;", "\u{21e5}"),
    ("RightArrowLeftArrow;", "\u{21c4}"),
    ("RightCeiling;", "\u{2309}"),
    ("RightDoubleBracket;", "\u{27e7}"),
    ("RightDownTeeVector;", "\u{295d}"),
    ("RightDownVector;", "\u{21c2}"),
    ("RightDownVectorBar;", "\u{2955}"),
    ("RightFloor;", "\u{230b}"),
    ("RightTee;", "\u{22a2}"),
    ("RightTeeArrow;", "\u{21a6}"),
    ("RightTeeVector;", "\u{295b}"),
    ("RightTriangle;", "\u{22b3}"),
    ("RightTriangleBar;", "\u{29d0}"),
    ("RightTriangleEqual;", "\u{22b5}"),
    ("RightUpDownVector;", "\u{294f}"),
    ("RightUpTeeVector;", "\u{295c}"),
    ("RightUpVector;", "\u{21be}"),
    ("RightUpVectorBar;", "\u{2954}"),
    ("RightVector;", "\u{21c0}"),
    ("RightVectorBar;", "\u{2953}"),
    ("Rightarrow;", "\u{21d2}"),
    ("Ropf;", "\u{211d}"),
    ("RoundImplies;", "\u{2970}"),
    ("Rrightarrow;", "\u{21db}"),
    ("Rscr;", "\u{211b}"),
    ("Rsh;", "\u{21b1}"),
    ("RuleDelayed;", "\u{29f4}"),
    ("SHCHcy;", "\u{429}"),
    ("SHcy;", "\u{428}"),
    ("SOFTcy;", "\u{42c}"),
    ("Sacute;", "\u{15a}"),
    ("Sc;", "\u{2abc}"),
    ("Scaron;", "\u{160}"),
    ("Scedil;", "\u{15e}"),
    ("Scirc;", "\u{15c}"),
    ("Scy;", "\u{421}"),
    ("Sfr;", "\u{1d516}"),
    ("ShortDownArrow;", "\u{2193}"),
    ("ShortLeftArrow;", "\u{2190}"),
    ("ShortRightArrow;", "\u{2192}"),
    ("ShortUpArrow;", "\u{2191}"),
    ("Sigma;", "\u{3a3}"),
    ("SmallCircle;", "\u{2218}"),
    ("Sopf;", "\u{1d54a}"),
    ("Sqrt;", "\u{221a}"),
    ("Square;", "\u{25a1}"),
    ("SquareIntersection;", "\u{2293}"),
    ("SquareSubset;", "\u{228f}"),
    ("SquareSubsetEqual;", "\u{2291}"),
    ("SquareSuperset;", "\u{2290}"),
    ("SquareSupersetEqual;", "\u{2292}"),
    ("SquareUnion;", "\u{2294}"),
    ("Sscr;", "\u{1d4ae}"),
    ("Star;", "\u{22c6}"),
    ("Sub;", "\u{22d0}"),
    ("Subset;", "\u{22d0}"),
    ("SubsetEqual;", "\u{2286}"),
    ("Succeeds;", "\u{227b}"),
    ("SucceedsEqual;", "\u{2ab0}"),
    ("SucceedsSlantEqual;", "\u{227d}"),
    ("SucceedsTilde;", "\u{227f}"),
    ("SuchThat;", "\u{220b}"),
    ("Sum;", "\u{2211}"),
    ("Sup;", "\u{22d1}"),
    ("Superset;", "\u{2283}"),
    ("SupersetEqual;", "\u{2287}"),
    ("Supset;", "\u{22d1}"),
    ("THORN", "\u{de}"),
    ("THORN;", "\u{de}"),
    ("TRADE;", "\u{2122}"),
    ("TSHcy;", "\u{40b}"),
    ("TScy;", "\u{426}"),
    ("Tab;", "\u{9}"),
    ("Tau;", "\u{3a4}"),
    ("Tcaron;", "\u{164}"),
    ("Tcedil;", "\u{162}"),
    ("Tcy;", "\u{422}"),
    ("Tfr;", "\u{1d517}"),
    ("Therefore;", "\u{2234}"),
    ("Theta;", "\u{398}"),
    ("ThickSpace;", "\u{205f}\u{200a}"),
    ("ThinSpace;", "\u{2009}"),
    ("Tilde;", "\u{223c}"),
    ("TildeEqual;", "\u{2243}"),
    ("TildeFullEqual;", "\u{2245}"),
    ("TildeTilde;", "\u{2248}"),
    ("Topf;", "\u{1d54b}"),
    ("TripleDot;", "\u{20db}"),
    ("Tscr;", "\u{1d4af}"),
    ("Tstrok;", "\u{166}"),
    ("Uacute", "\u{da}"),
    ("Uacute;", "\u{da}"),
    ("Uarr;", "\u{219f}"),
    ("Uarrocir;", "\u{2949}"),
    ("Ubrcy;", "\u{40e}"),
    ("Ubreve;", "\u{16c}"),
    ("Ucirc", "\u{db}"),
    ("Ucirc;", "\u{db}"),
    ("Ucy;", "\u{423}"),
    ("Udblac;", "\u{170}"),
    ("Ufr;", "\u{1d518}"),
    ("Ugrave", "\u{d9}"),
    ("Ugrave;", "\u{d9}"),
    ("Umacr;", "\u{16a}"),
    ("UnderBar;", "_"),
    ("UnderBrace;", "\u{23df}"),
    ("UnderBracket;", "\u{23b5}"),
    ("UnderParenthesis;", "\u{23dd}"),
    ("Union;", "\u{22c3}"),
    ("UnionPlus;", "\u{228e}"),
    ("Uogon;", "\u{172}"),
    ("Uopf;", "\u{1d54c}"),
    ("UpArrow;", "\u{2191}"),
    ("UpArrowBar;", "\u{2912}"),
    ("UpArrowDownArrow;", "\u{21c5}"),
    ("UpDownArrow;", "\u{2195}"),
    ("UpEquilibrium;", "\u{296e}"),
    ("UpTee;", "\u{22a5}"),
    ("UpTeeArrow;", "\u{21a5}"),
    ("Uparrow;", "\u{21d1}"),
    ("Updownarrow;", "\u{21d5}"),
    ("UpperLeftArrow;", "\u{2196}"),
    ("UpperRightArrow;", "\u{2197}"),
    ("Upsi;", "\u{3d2}"),
    ("Upsilon;", "\u{3a5}"),
    ("Uring;", "\u{16e}"),
    ("Uscr;", "\u{1d4b0}"),
    ("Utilde;", "\u{168}"),
    ("Uuml", "\u{dc}"),
    ("Uuml;", "\u{dc}"),
    ("VDash;", "\u{22ab}"),
    ("Vbar;", "\u{2aeb}"),
    ("Vcy;", "\u{412}"),
    ("Vdash;", "\u{22a9}"),
    ("Vdashl;", "\u{2ae6}"),
    ("Vee;", "\u{22c1}"),
    ("Verbar;", "\u{2016}"),
    ("Vert;", "\u{2016}"),
    ("VerticalBar;", "\u{2223}"),
    ("VerticalLine;", "|"),
    ("VerticalSeparator;", "\u{2758}"),
    ("VerticalTilde;", "\u{2240}"),
    ("VeryThinSpace;", "\u{200a}"),
    ("Vfr;", "\u{1d519}"),
    ("Vopf;", "\u{1d54d}"),
    ("Vscr;", "\u{1d4b1}"),
    ("Vvdash;", "\u{22aa}"),
    ("Wcirc;", "\u{174}"),
    ("Wedge;", "\u{22c0}"),
    ("Wfr;", "\u{1d51a}"),
    ("Wopf;", "\u{1d54e}"),
    ("Wscr;", "\u{1d4b2}"),
    ("Xfr;", "\u{1d51b}"),
    ("Xi;", "\u{39e}"),
    ("Xopf;", "\u{1d54f}"),
    ("Xscr;", "\u{1d4b3}"),
    ("YAcy;", "\u{42f}"),
    ("YIcy;", "\u{407}"),
    ("YUcy;", "\u{42e}"),
    ("Yacute", "\u{dd}"),
    ("Yacute;", "\u{dd}"),
    ("Ycirc;", "\u{176}"),
    ("Ycy;", "\u{42b}"),
    ("Yfr;", "\u{1d51c}"),
    ("Yopf;", "\u{1d550}"),
    ("Yscr;", "\u{1d4b4}"),
    ("Yuml;", "\u{178}"),
    ("ZHcy;", "\u{416}"),
    ("Zacute;", "\u{179}"),
    ("Zcaron;", "\u{17d}"),
    ("Zcy;", "\u{417}"),
    ("Zdot;", "\u{17b}"),
    ("ZeroWidthSpace;", "\u{200b}"),
    ("Zeta;", "\u{396}"),
    ("Zfr;", "\u{2128}"),
    ("Zopf;", "\u{2124}"),
    ("Zscr;", "\u{1d4b5}"),
    ("aacute", "\u{e1}"),
    ("aacute;", "\u{e1}"),
    ("abreve;", "\u{103}"),
    ("ac;", "\u{223e}"),
    ("acE;", "\u{223e}\u{333}"),
    ("acd;", "\u{223f}"),
    ("acirc", "\u{e2}"),
    ("acirc;", "\u{e2}"),
    ("acute", "\u{b4}"),
    ("acute;", "\u{b4}"),
    ("acy;", "\u{430}"),
    ("aelig", "\u{e6}"),
    ("aelig;", "\u{e6}"),
    ("af;", "\u{2061}"),
    ("afr;", "\u{1d51e}"),
    ("agrave", "\u{e0}"),
    ("agrave;", "\u{e0}"),
    ("alefsym;", "\u{2135}"),
    ("aleph;", "\u{2135}"),
    ("alpha;", "\u{3b1}"),
    ("amacr;", "\u{101}"),
    ("amalg;", "\u{2a3f}"),
    ("amp", "&"),
    ("amp;", "&"),
    ("and;", "\u{2227}"),
    ("andand;", "\u{2a55}"),
    ("andd;", "\u{2a5c}"),
    ("andslope;", "\u{2a58}"),
    ("andv;", "\u{2a5a}"),
    ("ang;", "\u{2220}"),
    ("ange;", "\u{29a4}"),
    ("angle;", "\u{2220}"),
    ("angmsd;", "\u{2221}"),
    ("angmsdaa;", "\u{29a8}"),
    ("angmsdab;", "\u{29a9}"),
    ("angmsdac;", "\u{29aa}"),
    ("angmsdad;", "\u{29ab}"),
    ("angmsdae;", "\u{29ac}"),
    ("angmsdaf;", "\u{29ad}"),
    ("angmsdag;", "\u{29ae}"),
    ("angmsdah;", "\u{29af}"),
    ("angrt;", "\u{221f}"),
    ("angrtvb;", "\u{22be}"),
    ("angrtvbd;", "\u{299d}"),
    ("angsph;", "\u{2222}"),
    ("angst;", "\u{c5}"),
    ("angzarr;", "\u{237c}"),
    ("aogon;", "\u{105}"),
    ("aopf;", "\u{1d552}"),
    ("ap;", "\u{2248}"),
    ("apE;", "\u{2a70}"),
    ("apacir;", "\u{2a6f}"),
    ("ape;", "\u{224a}"),
    ("apid;", "\u{224b}"),
    ("apos;", "'"),
    ("approx;", "\u{2248}"),
    ("approxeq;", "\u{224a}"),
    ("aring", "\u{e5}"),
    ("aring;", "\u{e5}"),
    ("ascr;", "\u{1d4b6}"),
    ("ast;", "*"),
    ("asymp;", "\u{2248}"),
    ("asympeq;", "\u{224d}"),
    ("atilde", "\u{e3}"),
    ("atilde;", "\u{e3}"),
    ("auml", "\u{e4}"),
    ("auml;", "\u{e4}"),
    ("awconint;", "\u{2233}"),
    ("awint;", "\u{2a11}"),
    ("bNot;", "\u{2aed}"),
    ("backcong;", "\u{224c}"),
    ("backepsilon;", "\u{3f6}"),
    ("backprime;", "\u{2035}"),
    ("backsim;", "\u{223d}"),
    ("backsimeq;", "\u{22cd}"),
    ("barvee;", "\u{22bd}"),
    ("barwed;", "\u{2305}"),
    ("barwedge;", "\u{2305}"),
    ("bbrk;", "\u{23b5}"),
    ("bbrktbrk;", "\u{23b6}"),
    ("bcong;", "\u{224c}"),
    ("bcy;", "\u{431}"),
    ("bdquo;", "\u{201e}"),
    ("becaus;", "\u{2235}"),
    ("because;", "\u{2235}"),
    ("bemptyv;", "\u{29b0}"),
    ("bepsi;", "\u{3f6}"),
    ("bernou;", "\u{212c}"),
    ("beta;", "\u{3b2}"),
    ("beth;", "\u{2136}"),
    ("between;", "\u{226c}"),
    ("bfr;", "\u{1d51f}"),
    ("bigcap;", "\u{22c2}"),
    ("bigcirc;", "\u{25ef}"),
    ("bigcup;", "\u{22c3}"),
    ("bigodot;", "\u{2a00}"),
    ("bigoplus;", "\u{2a01}"),
    ("bigotimes;", "\u{2a02}"),
    ("bigsqcup;", "\u{2a06}"),
    ("bigstar;", "\u{2605}"),
    ("bigtriangledown;", "\u{25bd}"),
    ("bigtriangleup;", "\u{25b3}"),
    ("biguplus;", "\u{2a04}"),
    ("bigvee;", "\u{22c1}"),
    ("bigwedge;", "\u{22c0}"),
    ("bkarow;", "\u{290d}"),
    ("blacklozenge;", "\u{29eb}"),
    ("blacksquare;", "\u{25aa}"),
    ("blacktriangle;", "\u{25b4}"),
    ("blacktriangledown;", "\u{25be}"),
    ("blacktriangleleft;", "\u{25c2}"),
    ("blacktriangleright;", "\u{25b8}"),
    ("blank;", "\u{2423}"),
    ("blk12;", "\u{2592}"),
    ("blk14;", "\u{2591}"),
    ("blk34;", "\u{2593}"),
    ("block;", "\u{2588}"),
    ("bne;", "=\u{20e5}"),
    ("bnequiv;", "\u{2261}\u{20e5}"),
    ("bnot;", "\u{2310}"),
    ("bopf;", "\u{1d553}"),
    ("bot;", "\u{22a5}"),
    ("bottom;", "\u{22a5}"),
    ("bowtie;", "\u{22c8}"),
    ("boxDL;", "\u{2557}"),
    ("boxDR;", "\u{2554}"),
    ("boxDl;", "\u{2556}"),
    ("boxDr;", "\u{2553}"),
    ("boxH;", "\u{2550}"),
    ("boxHD;", "\u{2566}"),
    ("boxHU;", "\u{2569}"),
    ("boxHd;", "\u{2564}"),
    ("boxHu;", "\u{2567}"),
    ("boxUL;", "\u{255d}"),
    ("boxUR;", "\u{255a}"),
    ("boxUl;", "\u{255c}"),
    ("boxUr;", "\u{2559}"),
    ("boxV;", "\u{2551}"),
    ("boxVH;", "\u{256c}"),
    ("boxVL;", "\u{2563}"),
    ("boxVR;", "\u{2560}"),
    ("boxVh;", "\u{256b}"),
    ("boxVl;", "\u{2562}"),
    ("boxVr;", "\u{255f}"),
    ("boxbox;", "\u{29c9}"),
    ("boxdL;", "\u{2555}"),
    ("boxdR;", "\u{2552}"),
    ("boxdl;", "\u{2510}"),
    ("boxdr;", "\u{250c}"),
    ("boxh;", "\u{2500}"),
    ("boxhD;", "\u{2565}"),
    ("boxhU;", "\u{2568}"),
    ("boxhd;", "\u{252c}"),
    ("boxhu;", "\u{2534}"),
    ("boxminus;", "\u{229f}"),
    ("boxplus;", "\u{229e}"),
    ("boxtimes;", "\u{22a0}"),
    ("boxuL;", "\u{255b}"),
    ("boxuR;", "\u{2558}"),
    ("boxul;", "\u{2518}"),
    ("boxur;", "\u{2514}"),
    ("boxv;", "\u{2502}"),
    ("boxvH;", "\u{256a}"),
    ("boxvL;", "\u{2561}"),
    ("boxvR;", "\u{255e}"),
    ("boxvh;", "\u{253c}"),
    ("boxvl;", "\u{2524}"),
    ("boxvr;", "\u{251c}"),
    ("bprime;", "\u{2035}"),
    ("breve;", "\u{2d8}"),
    ("brvbar", "\u{a6}"),
    ("brvbar;", "\u{a6}"),
    ("bscr;", "\u{1d4b7}"),
    ("bsemi;", "\u{204f}"),
    ("bsim;", "\u{223d}"),
    ("bsime;", "\u{22cd}"),
    ("bsol;", "\u{5c}"),
    ("bsolb;", "\u{29c5}"),
    ("bsolhsub;", "\u{27c8}"),
    ("bull;", "\u{2022}"),
    ("bullet;", "\u{2022}"),
    ("bump;", "\u{224e}"),
    ("bumpE;", "\u{2aae}"),
    ("bumpe;", "\u{224f}"),
    ("bumpeq;", "\u{224f}"),
    ("cacute;", "\u{107}"),
    ("cap;", "\u{2229}"),
    ("capand;", "\u{2a44}"),
    ("capbrcup;", "\u{2a49}"),
    ("capcap;", "\u{2a4b}"),
    ("capcup;", "\u{2a47}"),
    ("capdot;", "\u{2a40}"),
    ("caps;", "\u{2229}\u{fe00}"),
    ("caret;", "\u{2041}"),
    ("caron;", "\u{2c7}"),
    ("ccaps;", "\u{2a4d}"),
    ("ccaron;", "\u{10d}"),
    ("ccedil", "\u{e7}"),
    ("ccedil;", "\u{e7}"),
    ("ccirc;", "\u{109}"),
    ("ccups;", "\u{2a4c}"),
    ("ccupssm;", "\u{2a50}"),
    ("cdot;", "\u{10b}"),
    ("cedil", "\u{b8}"),
    ("cedil;", "\u{b8}"),
    ("cemptyv;", "\u{29b2}"),
    ("cent", "\u{a2}"),
    ("cent;", "\u{a2}"),
    ("centerdot;", "\u{b7}"),
    ("cfr;", "\u{1d520}"),
    ("chcy;", "\u{447}"),
    ("check;", "\u{2713}"),
    ("checkmark;", "\u{2713}"),
    ("chi;", "\u{3c7}"),
    ("cir;", "\u{25cb}"),
    ("cirE;", "\u{29c3}"),
    ("circ;", "\u{2c6}"),
    ("circeq;", "\u{2257}"),
    ("circlearrowleft;", "\u{21ba}"),
    ("circlearrowright;", "\u{21bb}"),
    ("circledR;", "\u{ae}"),
    ("circledS;", "\u{24c8}"),
    ("circledast;", "\u{229b}"),
    ("circledcirc;", "\u{229a}"),
    ("circleddash;", "\u{229d}"),
    ("cire;", "\u{2257}"),
    ("cirfnint;", "\u{2a10}"),
    ("cirmid;", "\u{2aef}"),
    ("cirscir;", "\u{29c2}"),
    ("clubs;", "\u{2663}"),
    ("clubsuit;", "\u{2663}"),
    ("colon;", ":"),
    ("colone;", "\u{2254}"),
    ("coloneq;", "\u{2254}"),
    ("comma;", ","),
    ("commat;", "@"),
    ("comp;", "\u{2201}"),
    ("compfn;", "\u{2218}"),
    ("complement;", "\u{2201}"),
    ("complexes;", "\u{2102}"),
    ("cong;", "\u{2245}"),
    ("congdot;", "\u{2a6d}"),
    ("conint;", "\u{222e}"),
    ("copf;", "\u{1d554}"),
    ("coprod;", "\u{2210}"),
    ("copy", "\u{a9}"),
    ("copy;", "\u{a9}"),
    ("copysr;", "\u{2117}"),
    ("crarr;", "\u{21b5}"),
    ("cross;", "\u{2717}"),
    ("cscr;", "\u{1d4b8}"),
    ("csub;", "\u{2acf}"),
    ("csube;", "\u{2ad1}"),
    ("csup;", "\u{2ad0}"),
    ("csupe;", "\u{2ad2}"),
    ("ctdot;", "\u{22ef}"),
    ("cudarrl;", "\u{2938}"),
    ("cudarrr;", "\u{2935}"),
    ("cuepr;", "\u{22de}"),
    ("cuesc;", "\u{22df}"),
    ("cularr;", "\u{21b6}"),
    ("cularrp;", "\u{293d}"),
    ("cup;", "\u{222a}"),
    ("cupbrcap;", "\u{2a48}"),
    ("cupcap;", "\u{2a46}"),
    ("cupcup;", "\u{2a4a}"),
    ("cupdot;", "\u{228d}"),
    ("cupor;", "\u{2a45}"),
    ("cups;", "\u{222a}\u{fe00}"),
    ("curarr;", "\u{21b7}"),
    ("curarrm;", "\u{293c}"),
    ("curlyeqprec;", "\u{22de}"),
    ("curlyeqsucc;", "\u{22df}"),
    ("curlyvee;", "\u{22ce}"),
    ("curlywedge;", "\u{22cf}"),
    ("curren", "\u{a4}"),
    ("curren;", "\u{a4}"),
    ("curvearrowleft;", "\u{21b6}"),
    ("curvearrowright;", "\u{21b7}"),
    ("cuvee;", "\u{22ce}"),
    ("cuwed;", "\u{22cf}"),
    ("cwconint;", "\u{2232}"),
    ("cwint;", "\u{2231}"),
    ("cylcty;", "\u{232d}"),
    ("dArr;", "\u{21d3}"),
    ("dHar;", "\u{2965}"),
    ("dagger;", "\u{2020}"),
    ("daleth;", "\u{2138}"),
    ("darr;", "\u{2193}"),
    ("dash;", "\u{2010}"),
    ("dashv;", "\u{22a3}"),
    ("dbkarow;", "\u{290f}"),
    ("dblac;", "\u{2dd}"),
    ("dcaron;", "\u{10f}"),
    ("dcy;", "\u{434}"),
    ("dd;", "\u{2146}"),
    ("ddagger;", "\u{2021}"),
    ("ddarr;", "\u{21ca}"),
    ("ddotseq;", "\u{2a77}"),
    ("deg", "\u{b0}"),
    ("deg;", "\u{b0}"),
    ("delta;", "\u{3b4}"),
    ("demptyv;", "\u{29b1}"),
    ("dfisht;", "\u{297f}"),
    ("dfr;", "\u{1d521}"),
    ("dharl;", "\u{21c3}"),
    ("dharr;", "\u{21c2}"),
    ("diam;", "\u{22c4}"),
    ("diamond;", "\u{22c4}"),
    ("diamondsuit;", "\u{2666}"),
    ("diams;", "\u{2666}"),
    ("die;", "\u{a8}"),
    ("digamma;", "\u{3dd}"),
    ("disin;", "\u{22f2}"),
    ("div;", "\u{f7}"),
    ("divide", "\u{f7}"),
    ("divide;", "\u{f7}"),
    ("divideontimes;", "\u{22c7}"),
    ("divonx;", "\u{22c7}"),
    ("djcy;", "\u{452}"),
    ("dlcorn;", "\u{231e}"),
    ("dlcrop;", "\u{230d}"),
    ("dollar;", "$"),
    ("dopf;", "\u{1d555}"),
    ("dot;", "\u{2d9}"),
    ("doteq;", "\u{2250}"),
    ("doteqdot;", "\u{2251}"),
    ("dotminus;", "\u{2238}"),
    ("dotplus;", "\u{2214}"),
    ("dotsquare;", "\u{22a1}"),
    ("doublebarwedge;", "\u{2306}"),
    ("downarrow;", "\u{2193}"),
    ("downdownarrows;", "\u{21ca}"),
    ("downharpoonleft;", "\u{21c3}"),
    ("downharpoonright;", "\u{21c2}"),
    ("drbkarow;", "\u{2910}"),
    ("drcorn;", "\u{231f}"),
    ("drcrop;", "\u{230c}"),
    ("dscr;", "\u{1d4b9}"),
    ("dscy;", "\u{455}"),
    ("dsol;", "\u{29f6}"),
    ("dstrok;", "\u{111}"),
    ("dtdot;", "\u{22f1}"),
    ("dtri;", "\u{25bf}"),
    ("dtrif;", "\u{25be}"),
    ("duarr;", "\u{21f5}"),
    ("duhar;", "\u{296f}"),
    ("dwangle;", "\u{29a6}"),
    ("dzcy;", "\u{45f}"),
    ("dzigrarr;", "\u{27ff}"),
    ("eDDot;", "\u{2a77}"),
    ("eDot;", "\u{2251}"),
    ("eacute", "\u{e9}"),
    ("eacute;", "\u{e9}"),
    ("easter;", "\u{2a6e}"),
    ("ecaron;", "\u{11b}"),
    ("ecir;", "\u{2256}"),
    ("ecirc", "\u{ea}"),
    ("ecirc;", "\u{ea}"),
    ("ecolon;", "\u{2255}"),
    ("ecy;", "\u{44d}"),
    ("edot;", "\u{117}"),
    ("ee;", "\u{2147}"),
    ("efDot;", "\u{2252}"),
    ("efr;", "\u{1d522}"),
    ("eg;", "\u{2a9a}"),
    ("egrave", "\u{e8}"),
    ("egrave;", "\u{e8}"),
    ("egs;", "\u{2a96}"),
    ("egsdot;", "\u{2a98}"),
    ("el;", "\u{2a99}"),
    ("elinters;", "\u{23e7}"),
    ("ell;", "\u{2113}"),
    ("els;", "\u{2a95}"),
    ("elsdot;", "\u{2a97}"),
    ("emacr;", "\u{113}"),
    ("empty;", "\u{2205}"),
    ("emptyset;", "\u{2205}"),
    ("emptyv;", "\u{2205}"),
    ("emsp13;", "\u{2004}"),
    ("emsp14;", "\u{2005}"),
    ("emsp;", "\u{2003}"),
    ("eng;", "\u{14b}"),
    ("ensp;", "\u{2002}"),
    ("eogon;", "\u{119}"),
    ("eopf;", "\u{1d556}"),
    ("epar;", "\u{22d5}"),
    ("eparsl;", "\u{29e3}"),
    ("eplus;", "\u{2a71}"),
    ("epsi;", "\u{3b5}"),
    ("epsilon;", "\u{3b5}"),
    ("epsiv;", "\u{3f5}"),
    ("eqcirc;", "\u{2256}"),
    ("eqcolon;", "\u{2255}"),
    ("eqsim;", "\u{2242}"),
    ("eqslantgtr;", "\u{2a96}"),
    ("eqslantless;", "\u{2a95}"),
    ("equals;", "="),
    ("equest;", "\u{225f}"),
    ("equiv;", "\u{2261}"),
    ("equivDD;", "\u{2a78}"),
    ("eqvparsl;", "\u{29e5}"),
    ("erDot;", "\u{2253}"),
    ("erarr;", "\u{2971}"),
    ("escr;", "\u{212f}"),
    ("esdot;", "\u{2250}"),
    ("esim;", "\u{2242}"),
    ("eta;", "\u{3b7}"),
    ("eth", "\u{f0}"),
    ("eth;", "\u{f0}"),
    ("euml", "\u{eb}"),
    ("euml;", "\u{eb}"),
    ("euro;", "\u{20ac}"),
    ("excl;", "!"),
    ("exist;", "\u{2203}"),
    ("expectation;", "\u{2130}"),
    ("exponentiale;", "\u{2147}"),
    ("fallingdotseq;", "\u{2252}"),
    ("fcy;", "\u{444}"),
    ("female;", "\u{2640}"),
    ("ffilig;", "\u{fb03}"),
    ("fflig;", "\u{fb00}"),
    ("ffllig;", "\u{fb04}"),
    ("ffr;", "\u{1d523}"),
    ("filig;", "\u{fb01}"),
    ("fjlig;", "fj"),
    ("flat;", "\u{266d}"),
    ("fllig;", "\u{fb02}"),
    ("fltns;", "\u{25b1}"),
    ("fnof;", "\u{192}"),
    ("fopf;", "\u{1d557}"),
    ("forall;", "\u{2200}"),
    ("fork;", "\u{22d4}"),
    ("forkv;", "\u{2ad9}"),
    ("fpartint;", "\u{2a0d}"),
    ("frac12", "\u{bd}"),
    ("frac12;", "\u{bd}"),
    ("frac13;", "\u{2153}"),
    ("frac14", "\u{bc}"),
    ("frac14;", "\u{bc}"),
    ("frac15;", "\u{2155}"),
    ("frac16;", "\u{2159}"),
    ("frac18;", "\u{215b}"),
    ("frac23;", "\u{2154}"),
    ("frac25;", "\u{2156}"),
    ("frac34", "\u{be}"),
    ("frac34;", "\u{be}"),
    ("frac35;", "\u{2157}"),
    ("frac38;", "\u{215c}"),
    ("frac45;", "\u{2158}"),
    ("frac56;", "\u{215a}"),
    ("frac58;", "\u{215d}"),
    ("frac78;", "\u{215e}"),
    ("frasl;", "\u{2044}"),
    ("frown;", "\u{2322}"),
    ("fscr;", "\u{1d4bb}"),
    ("gE;", "\u{2267}"),
    ("gEl;", "\u{2a8c}"),
    ("gacute;", "\u{1f5}"),
    ("gamma;", "\u{3b3}"),
    ("gammad;", "\u{3dd}"),
    ("gap;", "\u{2a86}"),
    ("gbreve;", "\u{11f}"),
    ("gcirc;", "\u{11d}"),
    ("gcy;", "\u{433}"),
    ("gdot;", "\u{121}"),
    ("ge;", "\u{2265}"),
    ("gel;", "\u{22db}"),
    ("geq;", "\u{2265}"),
    ("geqq;", "\u{2267}"),
    ("geqslant;", "\u{2a7e}"),
    ("ges;", "\u{2a7e}"),
    ("gescc;", "\u{2aa9}"),
    ("gesdot;", "\u{2a80}"),
    ("gesdoto;", "\u{2a82}"),
    ("gesdotol;", "\u{2a84}"),
    ("gesl;", "\u{22db}\u{fe00}"),
    ("gesles;", "\u{2a94}"),
    ("gfr;", "\u{1d524}"),
    ("gg;", "\u{226b}"),
    ("ggg;", "\u{22d9}"),
    ("gimel;", "\u{2137}"),
    ("gjcy;", "\u{453}"),
    ("gl;", "\u{2277}"),
    ("glE;", "\u{2a92}"),
    ("gla;", "\u{2aa5}"),
    ("glj;", "\u{2aa4}"),
    ("gnE;", "\u{2269}"),
    ("gnap;", "\u{2a8a}"),
    ("gnapprox;", "\u{2a8a}"),
    ("gne;", "\u{2a88}"),
    ("gneq;", "\u{2a88}"),
    ("gneqq;", "\u{2269}"),
    ("gnsim;", "\u{22e7}"),
    ("gopf;", "\u{1d558}"),
    ("grave;", "`"),
    ("gscr;", "\u{210a}"),
    ("gsim;", "\u{2273}"),
    ("gsime;", "\u{2a8e}"),
    ("gsiml;", "\u{2a90}"),
    ("gt", ">"),
    ("gt;", ">"),
    ("gtcc;", "\u{2aa7}"),
    ("gtcir;", "\u{2a7a}"),
    ("gtdot;", "\u{22d7}"),
    ("gtlPar;", "\u{2995}"),
    ("gtquest;", "\u{2a7c}"),
    ("gtrapprox;", "\u{2a86}"),
    ("gtrarr;", "\u{2978}"),
    ("gtrdot;", "\u{22d7}"),
    ("gtreqless;", "\u{22db}"),
    ("gtreqqless;", "\u{2a8c}"),
    ("gtrless;", "\u{2277}"),
    ("gtrsim;", "\u{2273}"),
    ("gvertneqq;", "\u{2269}\u{fe00}"),
    ("gvnE;", "\u{2269}\u{fe00}"),
    ("hArr;", "\u{21d4}"),
    ("hairsp;", "\u{200a}"),
    ("half;", "\u{bd}"),
    ("hamilt;", "\u{210b}"),
    ("hardcy;", "\u{44a}"),
    ("harr;", "\u{2194}"),
    ("harrcir;", "\u{2948}"),
    ("harrw;", "\u{21ad}"),
    ("hbar;", "\u{210f}"),
    ("hcirc;", "\u{125}"),
    ("hearts;", "\u{2665}"),
    ("heartsuit;", "\u{2665}"),
    ("hellip;", "\u{2026}"),
    ("hercon;", "\u{22b9}"),
    ("hfr;", "\u{1d525}"),
    ("hksearow;", "\u{2925}"),
    ("hkswarow;", "\u{2926}"),
    ("hoarr;", "\u{21ff}"),
    ("homtht;", "\u{223b}"),
    ("hookleftarrow;", "\u{21a9}"),
    ("hookrightarrow;", "\u{21aa}"),
    ("hopf;", "\u{1d559}"),
    ("horbar;", "\u{2015}"),
    ("hscr;", "\u{1d4bd}"),
    ("hslash;", "\u{210f}"),
    ("hstrok;", "\u{127}"),
    ("hybull;", "\u{2043}"),
    ("hyphen;", "\u{2010}"),
    ("iacute", "\u{ed}"),
    ("iacute;", "\u{ed}"),
    ("ic;", "\u{2063}"),
    ("icirc", "\u{ee}"),
    ("icirc;", "\u{ee}"),
    ("icy;", "\u{438}"),
    ("iecy;", "\u{435}"),
    ("iexcl", "\u{a1}"),
    ("iexcl;", "\u{a1}"),
    ("iff;", "\u{21d4}"),
    ("ifr;", "\u{1d526}"),
    ("igrave", "\u{ec}"),
    ("igrave;", "\u{ec}"),
    ("ii;", "\u{2148}"),
    ("iiiint;", "\u{2a0c}"),
    ("iiint;", "\u{222d}"),
    ("iinfin;", "\u{29dc}"),
    ("iiota;", "\u{2129}"),
    ("ijlig;", "\u{133}"),
    ("imacr;", "\u{12b}"),
    ("image;", "\u{2111}"),
    ("imagline;", "\u{2110}"),
    ("imagpart;", "\u{2111}"),
    ("imath;", "\u{131}"),
    ("imof;", "\u{22b7}"),
    ("imped;", "\u{1b5}"),
    ("in;", "\u{2208}"),
    ("incare;", "\u{2105}"),
    ("infin;", "\u{221e}"),
    ("infintie;", "\u{29dd}"),
    ("inodot;", "\u{131}"),
    ("int;", "\u{222b}"),
    ("intcal;", "\u{22ba}"),
    ("integers;", "\u{2124}"),
    ("intercal;", "\u{22ba}"),
    ("intlarhk;", "\u{2a17}"),
    ("intprod;", "\u{2a3c}"),
    ("iocy;", "\u{451}"),
    ("iogon;", "\u{12f}"),
    ("iopf;", "\u{1d55a}"),
    ("iota;", "\u{3b9}"),
    ("iprod;", "\u{2a3c}"),
    ("iquest", "\u{bf}"),
    ("iquest;", "\u{bf}"),
    ("iscr;", "\u{1d4be}"),
    ("isin;", "\u{2208}"),
    ("isinE;", "\u{22f9}"),
    ("isindot;", "\u{22f5}"),
    ("isins;", "\u{22f4}"),
    ("isinsv;", "\u{22f3}"),
    ("isinv;", "\u{2208}"),
    ("it;", "\u{2062}"),
    ("itilde;", "\u{129}"),
    ("iukcy;", "\u{456}"),
    ("iuml", "\u{ef}"),
    ("iuml;", "\u{ef}"),
    ("jcirc;", "\u{135}"),
    ("jcy;", "\u{439}"),
    ("jfr;", "\u{1d527}"),
    ("jmath;", "\u{237}"),
    ("jopf;", "\u{1d55b}"),
    ("jscr;", "\u{1d4bf}"),
    ("jsercy;", "\u{458}"),
    ("jukcy;", "\u{454}"),
    ("kappa;", "\u{3ba}"),
    ("kappav;", "\u{3f0}"),
    ("kcedil;", "\u{137}"),
    ("kcy;", "\u{43a}"),
    ("kfr;", "\u{1d528}"),
    ("kgreen;", "\u{138}"),
    ("khcy;", "\u{445}"),
    ("kjcy;", "\u{45c}"),
    ("kopf;", "\u{1d55c}"),
    ("kscr;", "\u{1d4c0}"),
    ("lAarr;", "\u{21da}"),
    ("lArr;", "\u{21d0}"),
    ("lAtail;", "\u{291b}"),
    ("lBarr;", "\u{290e}"),
    ("lE;", "\u{2266}"),
    ("lEg;", "\u{2a8b}"),
    ("lHar;", "\u{2962}"),
    ("lacute;", "\u{13a}"),
    ("laemptyv;", "\u{29b4}"),
    ("lagran;", "\u{2112}"),
    ("lambda;", "\u{3bb}"),
    ("lang;", "\u{27e8}"),
    ("langd;", "\u{2991}"),
    ("langle;", "\u{27e8}"),
    ("lap;", "\u{2a85}"),
    ("laquo", "\u{ab}"),
    ("laquo;", "\u{ab}"),
    ("larr;", "\u{2190}"),
    ("larrb;", "\u{21e4}"),
    ("larrbfs;", "\u{291f}"),
    ("larrfs;", "\u{291d}"),
    ("larrhk;", "\u{21a9}"),
    ("larrlp;", "\u{21ab}"),
    ("larrpl;", "\u{2939}"),
    ("larrsim;", "\u{2973}"),
    ("larrtl;", "\u{21a2}"),
    ("lat;", "\u{2aab}"),
    ("latail;", "\u{2919}"),
    ("late;", "\u{2aad}"),
    ("lates;", "\u{2aad}\u{fe00}"),
    ("lbarr;", "\u{290c}"),
    ("lbbrk;", "\u{2772}"),
    ("lbrace;", "{"),
    ("lbrack;", "["),
    ("lbrke;", "\u{298b}"),
    ("lbrksld;", "\u{298f}"),
    ("lbrkslu;", "\u{298d}"),
    ("lcaron;", "\u{13e}"),
    ("lcedil;", "\u{13c}"),
    ("lceil;", "\u{2308}"),
    ("lcub;", "{"),
    ("lcy;", "\u{43b}"),
    ("ldca;", "\u{2936}"),
    ("ldquo;", "\u{201c}"),
    ("ldquor;", "\u{201e}"),
    ("ldrdhar;", "\u{2967}"),
    ("ldrushar;", "\u{294b}"),
    ("ldsh;", "\u{21b2}"),
    ("le;", "\u{2264}"),
    ("leftarrow;", "\u{2190}"),
    ("leftarrowtail;", "\u{21a2}"),
    ("leftharpoondown;", "\u{21bd}"),
    ("leftharpoonup;", "\u{21bc}"),
    ("leftleftarrows;", "\u{21c7}"),
    ("leftrightarrow;", "\u{2194}"),
    ("leftrightarrows;", "\u{21c6}"),
    ("leftrightharpoons;", "\u{21cb}"),
    ("leftrightsquigarrow;", "\u{21ad}"),
    ("leftthreetimes;", "\u{22cb}"),
    ("leg;", "\u{22da}"),
    ("leq;", "\u{2264}"),
    ("leqq;", "\u{2266}"),
    ("leqslant;", "\u{2a7d}"),
    ("les;", "\u{2a7d}"),
    ("lescc;", "\u{2aa8}"),
    ("lesdot;", "\u{2a7f}"),
    ("lesdoto;", "\u{2a81}"),
    ("lesdotor;", "\u{2a83}"),
    ("lesg;", "\u{22da}\u{fe00}"),
    ("lesges;", "\u{2a93}"),
    ("lessapprox;", "\u{2a85}"),
    ("lessdot;", "\u{22d6}"),
    ("lesseqgtr;", "\u{22da}"),
    ("lesseqqgtr;", "\u{2a8b}"),
    ("lessgtr;", "\u{2276}"),
    ("lesssim;", "\u{2272}"),
    ("lfisht;", "\u{297c}"),
    ("lfloor;", "\u{230a}"),
    ("lfr;", "\u{1d529}"),
    ("lg;", "\u{2276}"),
    ("lgE;", "\u{2a91}"),
    ("lhard;", "\u{21bd}"),
    ("lharu;", "\u{21bc}"),
    ("lharul;", "\u{296a}"),
    ("lhblk;", "\u{2584}"),
    ("ljcy;", "\u{459}"),
    ("ll;", "\u{226a}"),
    ("llarr;", "\u{21c7}"),
    ("llcorner;", "\u{231e}"),
    ("llhard;", "\u{296b}"),
    ("lltri;", "\u{25fa}"),
    ("lmidot;", "\u{140}"),
    ("lmoust;", "\u{23b0}"),
    ("lmoustache;", "\u{23b0}"),
    ("lnE;", "\u{2268}"),
    ("lnap;", "\u{2a89}"),
    ("lnapprox;", "\u{2a89}"),
    ("lne;", "\u{2a87}"),
    ("lneq;", "\u{2a87}"),
    ("lneqq;", "\u{2268}"),
    ("lnsim;", "\u{22e6}"),
    ("loang;", "\u{27ec}"),
    ("loarr;", "\u{21fd}"),
    ("lobrk;", "\u{27e6}"),
    ("longleftarrow;", "\u{27f5}"),
    ("longleftrightarrow;", "\u{27f7}"),
    ("longmapsto;", "\u{27fc}"),
    ("longrightarrow;", "\u{27f6}"),
    ("looparrowleft;", "\u{21ab}"),
    ("looparrowright;", "\u{21ac}"),
    ("lopar;", "\u{2985}"),
    ("lopf;", "\u{1d55d}"),
    ("loplus;", "\u{2a2d}"),
    ("lotimes;", "\u{2a34}"),
    ("lowast;", "\u{2217}"),
    ("lowbar;", "_"),
    ("loz;", "\u{25ca}"),
    ("lozenge;", "\u{25ca}"),
    ("lozf;", "\u{29eb}"),
    ("lpar;", "("),
    ("lparlt;", "\u{2993}"),
    ("lrarr;", "\u{21c6}"),
    ("lrcorner;", "\u{231f}"),
    ("lrhar;", "\u{21cb}"),
    ("lrhard;", "\u{296d}"),
    ("lrm;", "\u{200e}"),
    ("lrtri;", "\u{22bf}"),
    ("lsaquo;", "\u{2039}"),
    ("lscr;", "\u{1d4c1}"),
    ("lsh;", "\u{21b0}"),
    ("lsim;", "\u{2272}"),
    ("lsime;", "\u{2a8d}"),
    ("lsimg;", "\u{2a8f}"),
    ("lsqb;", "["),
    ("lsquo;", "\u{2018}"),
    ("lsquor;", "\u{201a}"),
    ("lstrok;", "\u{142}"),
    ("lt", "<"),
    ("lt;", "<"),
    ("ltcc;", "\u{2aa6}"),
    ("ltcir;", "\u{2a79}"),
    ("ltdot;", "\u{22d6}"),
    ("lthree;", "\u{22cb}"),
    ("ltimes;", "\u{22c9}"),
    ("ltlarr;", "\u{2976}"),
    ("ltquest;", "\u{2a7b}"),
    ("ltrPar;", "\u{2996}"),
    ("ltri;", "\u{25c3}"),
    ("ltrie;", "\u{22b4}"),
    ("ltrif;", "\u{25c2}"),
    ("lurdshar;", "\u{294a}"),
    ("luruhar;", "\u{2966}"),
    ("lvertneqq;", "\u{2268}\u{fe00}"),
    ("lvnE;", "\u{2268}\u{fe00}"),
    ("mDDot;", "\u{223a}"),
    ("macr", "\u{af}"),
    ("macr;", "\u{af}"),
    ("male;", "\u{2642}"),
    ("malt;", "\u{2720}"),
    ("maltese;", "\u{2720}"),
    ("map;", "\u{21a6}"),
    ("mapsto;", "\u{21a6}"),
    ("mapstodown;", "\u{21a7}"),
    ("mapstoleft;", "\u{21a4}"),
    ("mapstoup;", "\u{21a5}"),
    ("marker;", "\u{25ae}"),
    ("mcomma;", "\u{2a29}"),
    ("mcy;", "\u{43c}"),
    ("mdash;", "\u{2014}"),
    ("measuredangle;", "\u{2221}"),
    ("mfr;", "\u{1d52a}"),
    ("mho;", "\u{2127}"),
    ("micro", "\u{b5}"),
    ("micro;", "\u{b5}"),
    ("mid;", "\u{2223}"),
    ("midast;", "*"),
    ("midcir;", "\u{2af0}"),
    ("middot", "\u{b7}"),
    ("middot;", "\u{b7}"),
    ("minus;", "\u{2212}"),
    ("minusb;", "\u{229f}"),
    ("minusd;", "\u{2238}"),
    ("minusdu;", "\u{2a2a}"),
    ("mlcp;", "\u{2adb}"),
    ("mldr;", "\u{2026}"),
    ("mnplus;", "\u{2213}"),
    ("models;", "\u{22a7}"),
    ("mopf;", "\u{1d55e}"),
    ("mp;", "\u{2213}"),
    ("mscr;", "\u{1d4c2}"),
    ("mstpos;", "\u{223e}"),
    ("mu;", "\u{3bc}"),
    ("multimap;", "\u{22b8}"),
    ("mumap;", "\u{22b8}"),
    ("nGg;", "\u{22d9}\u{338}"),
    ("nGt;", "\u{226b}\u{20d2}"),
    ("nGtv;", "\u{226b}\u{338}"),
    ("nLeftarrow;", "\u{21cd}"),
    ("nLeftrightarrow;", "\u{21ce}"),
    ("nLl;", "\u{22d8}\u{338}"),
    ("nLt;", "\u{226a}\u{20d2}"),
    ("nLtv;", "\u{226a}\u{338}"),
    ("nRightarrow;", "\u{21cf}"),
    ("nVDash;", "\u{22af}"),
    ("nVdash;", "\u{22ae}"),
    ("nabla;", "\u{2207}"),
    ("nacute;", "\u{144}"),
    ("nang;", "\u{2220}\u{20d2}"),
    ("nap;", "\u{2249}"),
    ("napE;", "\u{2a70}\u{338}"),
    ("napid;", "\u{224b}\u{338}"),
    ("napos;", "\u{149}"),
    ("napprox;", "\u{2249}"),
    ("natur;", "\u{266e}"),
    ("natural;", "\u{266e}"),
    ("naturals;", "\u{2115}"),
    ("nbsp", "\u{a0}"),
    ("nbsp;", "\u{a0}"),
    ("nbump;", "\u{224e}\u{338}"),
    ("nbumpe;", "\u{224f}\u{338}"),
    ("ncap;", "\u{2a43}"),
    ("ncaron;", "\u{148}"),
    ("ncedil;", "\u{146}"),
    ("ncong;", "\u{2247}"),
    ("ncongdot;", "\u{2a6d}\u{338}"),
    ("ncup;", "\u{2a42}"),
    ("ncy;", "\u{43d}"),
    ("ndash;", "\u{2013}"),
    ("ne;", "\u{2260}"),
    ("neArr;", "\u{21d7}"),
    ("nearhk;", "\u{2924}"),
    ("nearr;", "\u{2197}"),
    ("nearrow;", "\u{2197}"),
    ("nedot;", "\u{2250}\u{338}"),
    ("nequiv;", "\u{2262}"),
    ("nesear;", "\u{2928}"),
    ("nesim;", "\u{2242}\u{338}"),
    ("nexist;", "\u{2204}"),
    ("nexists;", "\u{2204}"),
    ("nfr;", "\u{1d52b}"),
    ("ngE;", "\u{2267}\u{338}"),
    ("nge;", "\u{2271}"),
    ("ngeq;", "\u{2271}"),
    ("ngeqq;", "\u{2267}\u{338}"),
    ("ngeqslant;", "\u{2a7e}\u{338}"),
    ("nges;", "\u{2a7e}\u{338}"),
    ("ngsim;", "\u{2275}"),
    ("ngt;", "\u{226f}"),
    ("ngtr;", "\u{226f}"),
    ("nhArr;", "\u{21ce}"),
    ("nharr;", "\u{21ae}"),
    ("nhpar;", "\u{2af2}"),
    ("ni;", "\u{220b}"),
    ("nis;", "\u{22fc}"),
    ("nisd;", "\u{22fa}"),
    ("niv;", "\u{220b}"),
    ("njcy;", "\u{45a}"),
    ("nlArr;", "\u{21cd}"),
    ("nlE;", "\u{2266}\u{338}"),
    ("nlarr;", "\u{219a}"),
    ("nldr;", "\u{2025}"),
    ("nle;", "\u{2270}"),
    ("nleftarrow;", "\u{219a}"),
    ("nleftrightarrow;", "\u{21ae}"),
    ("nleq;", "\u{2270}"),
    ("nleqq;", "\u{2266}\u{338}"),
    ("nleqslant;", "\u{2a7d}\u{338}"),
    ("nles;", "\u{2a7d}\u{338}"),
    ("nless;", "\u{226e}"),
    ("nlsim;", "\u{2274}"),
    ("nlt;", "\u{226e}"),
    ("nltri;", "\u{22ea}"),
    ("nltrie;", "\u{22ec}"),
    ("nmid;", "\u{2224}"),
    ("nopf;", "\u{1d55f}"),
    ("not", "\u{ac}"),
    ("not;", "\u{ac}"),
    ("notin;", "\u{2209}"),
    ("notinE;", "\u{22f9}\u{338}"),
    ("notindot;", "\u{22f5}\u{338}"),
    ("notinva;", "\u{2209}"),
    ("notinvb;", "\u{22f7}"),
    ("notinvc;", "\u{22f6}"),
    ("notni;", "\u{220c}"),
    ("notniva;", "\u{220c}"),
    ("notnivb;", "\u{22fe}"),
    ("notnivc;", "\u{22fd}"),
    ("npar;", "\u{2226}"),
    ("nparallel;", "\u{2226}"),
    ("nparsl;", "\u{2afd}\u{20e5}"),
    ("npart;", "\u{2202}\u{338}"),
    ("npolint;", "\u{2a14}"),
    ("npr;", "\u{2280}"),
    ("nprcue;", "\u{22e0}"),
    ("npre;", "\u{2aaf}\u{338}"),
    ("nprec;", "\u{2280}"),
    ("npreceq;", "\u{2aaf}\u{338}"),
    ("nrArr;", "\u{21cf}"),
    ("nrarr;", "\u{219b}"),
    ("nrarrc;", "\u{2933}\u{338}"),
    ("nrarrw;", "\u{219d}\u{338}"),
    ("nrightarrow;", "\u{219b}"),
    ("nrtri;", "\u{22eb}"),
    ("nrtrie;", "\u{22ed}"),
    ("nsc;", "\u{2281}"),
    ("nsccue;", "\u{22e1}"),
    ("nsce;", "\u{2ab0}\u{338}"),
    ("nscr;", "\u{1d4c3}"),
    ("nshortmid;", "\u{2224}"),
    ("nshortparallel;", "\u{2226}"),
    ("nsim;", "\u{2241}"),
    ("nsime;", "\u{2244}"),
    ("nsimeq;", "\u{2244}"),
    ("nsmid;", "\u{2224}"),
    ("nspar;", "\u{2226}"),
    ("nsqsube;", "\u{22e2}"),
    ("nsqsupe;", "\u{22e3}"),
    ("nsub;", "\u{2284}"),
    ("nsubE;", "\u{2ac5}\u{338}"),
    ("nsube;", "\u{2288}"),
    ("nsubset;", "\u{2282}\u{20d2}"),
    ("nsubseteq;", "\u{2288}"),
    ("nsubseteqq;", "\u{2ac5}\u{338}"),
    ("nsucc;", "\u{2281}"),
    ("nsucceq;", "\u{2ab0}\u{338}"),
    ("nsup;", "\u{2285}"),
    ("nsupE;", "\u{2ac6}\u{338}"),
    ("nsupe;", "\u{2289}"),
    ("nsupset;", "\u{2283}\u{20d2}"),
    ("nsupseteq;", "\u{2289}"),
    ("nsupseteqq;", "\u{2ac6}\u{338}"),
    ("ntgl;", "\u{2279}"),
    ("ntilde", "\u{f1}"),
    ("ntilde;", "\u{f1}"),
    ("ntlg;", "\u{2278}"),
    ("ntriangleleft;", "\u{22ea}"),
    ("ntrianglelefteq;", "\u{22ec}"),
    ("ntriangleright;", "\u{22eb}"),
    ("ntrianglerighteq;", "\u{22ed}"),
    ("nu;", "\u{3bd}"),
    ("num;", "#"),
    ("numero;", "\u{2116}"),
    ("numsp;", "\u{2007}"),
    ("nvDash;", "\u{22ad}"),
    ("nvHarr;", "\u{2904}"),
    ("nvap;", "\u{224d}\u{20d2}"),
    ("nvdash;", "\u{22ac}"),
    ("nvge;", "\u{2265}\u{20d2}"),
    ("nvgt;", ">\u{20d2}"),
    ("nvinfin;", "\u{29de}"),
    ("nvlArr;", "\u{2902}"),
    ("nvle;", "\u{2264}\u{20d2}"),
    ("nvlt;", "<\u{20d2}"),
    ("nvltrie;", "\u{22b4}\u{20d2}"),
    ("nvrArr;", "\u{2903}"),
    ("nvrtrie;", "\u{22b5}\u{20d2}"),
    ("nvsim;", "\u{223c}\u{20d2}"),
    ("nwArr;", "\u{21d6}"),
    ("nwarhk;", "\u{2923}"),
    ("nwarr;", "\u{2196}"),
    ("nwarrow;", "\u{2196}"),
    ("nwnear;", "\u{2927}"),
    ("oS;", "\u{24c8}"),
    ("oacute", "\u{f3}"),
    ("oacute;", "\u{f3}"),
    ("oast;", "\u{229b}"),
    ("ocir;", "\u{229a}"),
    ("ocirc", "\u{f4}"),
    ("ocirc;", "\u{f4}"),
    ("ocy;", "\u{43e}"),
    ("odash;", "\u{229d}"),
    ("odblac;", "\u{151}"),
    ("odiv;", "\u{2a38}"),
    ("odot;", "\u{2299}"),
    ("odsold;", "\u{29bc}"),
    ("oelig;", "\u{153}"),
    ("ofcir;", "\u{29bf}"),
    ("ofr;", "\u{1d52c}"),
    ("ogon;", "\u{2db}"),
    ("ograve", "\u{f2}"),
    ("ograve;", "\u{f2}"),
    ("ogt;", "\u{29c1}"),
    ("ohbar;", "\u{29b5}"),
    ("ohm;", "\u{3a9}"),
    ("oint;", "\u{222e}"),
    ("olarr;", "\u{21ba}"),
    ("olcir;", "\u{29be}"),
    ("olcross;", "\u{29bb}"),
    ("oline;", "\u{203e}"),
    ("olt;", "\u{29c0}"),
    ("omacr;", "\u{14d}"),
    ("omega;", "\u{3c9}"),
    ("omicron;", "\u{3bf}"),
    ("omid;", "\u{29b6}"),
    ("ominus;", "\u{2296}"),
    ("oopf;", "\u{1d560}"),
    ("opar;", "\u{29b7}"),
    ("operp;", "\u{29b9}"),
    ("oplus;", "\u{2295}"),
    ("or;", "\u{2228}"),
    ("orarr;", "\u{21bb}"),
    ("ord;", "\u{2a5d}"),
    ("order;", "\u{2134}"),
    ("orderof;", "\u{2134}"),
    ("ordf", "\u{aa}"),
    ("ordf;", "\u{aa}"),
    ("ordm", "\u{ba}"),
    ("ordm;", "\u{ba}"),
    ("origof;", "\u{22b6}"),
    ("oror;", "\u{2a56}"),
    ("orslope;", "\u{2a57}"),
    ("orv;", "\u{2a5b}"),
    ("oscr;", "\u{2134}"),
    ("oslash", "\u{f8}"),
    ("oslash;", "\u{f8}"),
    ("osol;", "\u{2298}"),
    ("otilde", "\u{f5}"),
    ("otilde;", "\u{f5}"),
    ("otimes;", "\u{2297}"),
    ("otimesas;", "\u{2a36}"),
    ("ouml", "\u{f6}"),
    ("ouml;", "\u{f6}"),
    ("ovbar;", "\u{233d}"),
    ("par;", "\u{2225}"),
    ("para", "\u{b6}"),
    ("para;", "\u{b6}"),
    ("parallel;", "\u{2225}"),
    ("parsim;", "\u{2af3}"),
    ("parsl;", "\u{2afd}"),
    ("part;", "\u{2202}"),
    ("pcy;", "\u{43f}"),
    ("percnt;", "%"),
    ("period;", "."),
    ("permil;", "\u{2030}"),
    ("perp;", "\u{22a5}"),
    ("pertenk;", "\u{2031}"),
    ("pfr;", "\u{1d52d}"),
    ("phi;", "\u{3c6}"),
    ("phiv;", "\u{3d5}"),
    ("phmmat;", "\u{2133}"),
    ("phone;", "\u{260e}"),
    ("pi;", "\u{3c0}"),
    ("pitchfork;", "\u{22d4}"),
    ("piv;", "\u{3d6}"),
    ("planck;", "\u{210f}"),
    ("planckh;", "\u{210e}"),
    ("plankv;", "\u{210f}"),
    ("plus;", "+"),
    ("plusacir;", "\u{2a23}"),
    ("plusb;", "\u{229e}"),
    ("pluscir;", "\u{2a22}"),
    ("plusdo;", "\u{2214}"),
    ("plusdu;", "\u{2a25}"),
    ("pluse;", "\u{2a72}"),
    ("plusmn", "\u{b1}"),
    ("plusmn;", "\u{b1}"),
    ("plussim;", "\u{2a26}"),
    ("plustwo;", "\u{2a27}"),
    ("pm;", "\u{b1}"),
    ("pointint;", "\u{2a15}"),
    ("popf;", "\u{1d561}"),
    ("pound", "\u{a3}"),
    ("pound;", "\u{a3}"),
    ("pr;", "\u{227a}"),
    ("prE;", "\u{2ab3}"),
    ("prap;", "\u{2ab7}"),
    ("prcue;", "\u{227c}"),
    ("pre;", "\u{2aaf}"),
    ("prec;", "\u{227a}"),
    ("precapprox;", "\u{2ab7}"),
    ("preccurlyeq;", "\u{227c}"),
    ("preceq;", "\u{2aaf}"),
    ("precnapprox;", "\u{2ab9}"),
    ("precneqq;", "\u{2ab5}"),
    ("precnsim;", "\u{22e8}"),
    ("precsim;", "\u{227e}"),
    ("prime;", "\u{2032}"),
    ("primes;", "\u{2119}"),
    ("prnE;", "\u{2ab5}"),
    ("prnap;", "\u{2ab9}"),
    ("prnsim;", "\u{22e8}"),
    ("prod;", "\u{220f}"),
    ("profalar;", "\u{232e}"),
    ("profline;", "\u{2312}"),
    ("profsurf;", "\u{2313}"),
    ("prop;", "\u{221d}"),
    ("propto;", "\u{221d}"),
    ("prsim;", "\u{227e}"),
    ("prurel;", "\u{22b0}"),
    ("pscr;", "\u{1d4c5}"),
    ("psi;", "\u{3c8}"),
    ("puncsp;", "\u{2008}"),
    ("qfr;", "\u{1d52e}"),
    ("qint;", "\u{2a0c}"),
    ("qopf;", "\u{1d562}"),
    ("qprime;", "\u{2057}"),
    ("qscr;", "\u{1d4c6}"),
    ("quaternions;", "\u{210d}"),
    ("quatint;", "\u{2a16}"),
    ("quest;", "?"),
    ("questeq;", "\u{225f}"),
    ("quot", "\u{22}"),
    ("quot;", "\u{22}"),
    ("rAarr;", "\u{21db}"),
    ("rArr;", "\u{21d2}"),
    ("rAtail;", "\u{291c}"),
    ("rBarr;", "\u{290f}"),
    ("rHar;", "\u{2964}"),
    ("race;", "\u{223d}\u{331}"),
    ("racute;", "\u{155}"),
    ("radic;", "\u{221a}"),
    ("raemptyv;", "\u{29b3}"),
    ("rang;", "\u{27e9}"),
    ("rangd;", "\u{2992}"),
    ("range;", "\u{29a5}"),
    ("rangle;", "\u{27e9}"),
    ("raquo", "\u{bb}"),
    ("raquo;", "\u{bb}"),
    ("rarr;", "\u{2192}"),
    ("rarrap;", "\u{2975}"),
    ("rarrb;", "\u{21e5}"),
    ("rarrbfs;", "\u{2920}"),
    ("rarrc;", "\u{2933}"),
    ("rarrfs;", "\u{291e}"),
    ("rarrhk;", "\u{21aa}"),
    ("rarrlp;", "\u{21ac}"),
    ("rarrpl;", "\u{2945}"),
    ("rarrsim;", "\u{2974}"),
    ("rarrtl;", "\u{21a3}"),
    ("rarrw;", "\u{219d}"),
    ("ratail;", "\u{291a}"),
    ("ratio;", "\u{2236}"),
    ("rationals;", "\u{211a}"),
    ("rbarr;", "\u{290d}"),
    ("rbbrk;", "\u{2773}"),
    ("rbrace;", "}"),
    ("rbrack;", "]"),
    ("rbrke;", "\u{298c}"),
    ("rbrksld;", "\u{298e}"),
    ("rbrkslu;", "\u{2990}"),
    ("rcaron;", "\u{159}"),
    ("rcedil;", "\u{157}"),
    ("rceil;", "\u{2309}"),
    ("rcub;", "}"),
    ("rcy;", "\u{440}"),
    ("rdca;", "\u{2937}"),
    ("rdldhar;", "\u{2969}"),
    ("rdquo;", "\u{201d}"),
    ("rdquor;", "\u{201d}"),
    ("rdsh;", "\u{21b3}"),
    ("real;", "\u{211c}"),
    ("realine;", "\u{211b}"),
    ("realpart;", "\u{211c}"),
    ("reals;", "\u{211d}"),
    ("rect;", "\u{25ad}"),
    ("reg", "\u{ae}"),
    ("reg;", "\u{ae}"),
    ("rfisht;", "\u{297d}"),
    ("rfloor;", "\u{230b}"),
    ("rfr;", "\u{1d52f}"),
    ("rhard;", "\u{21c1}"),
    ("rharu;", "\u{21c0}"),
    ("rharul;", "\u{296c}"),
    ("rho;", "\u{3c1}"),
    ("rhov;", "\u{3f1}"),
    ("rightarrow;", "\u{2192}"),
    ("rightarrowtail;", "\u{21a3}"),
    ("rightharpoondown;", "\u{21c1}"),
    ("rightharpoonup;", "\u{21c0}"),
    ("rightleftarrows;", "\u{21c4}"),
    ("rightleftharpoons;", "\u{21cc}"),
    ("rightrightarrows;", "\u{21c9}"),
    ("rightsquigarrow;", "\u{219d}"),
    ("rightthreetimes;", "\u{22cc}"),
    ("ring;", "\u{2da}"),
    ("risingdotseq;", "\u{2253}"),
    ("rlarr;", "\u{21c4}"),
    ("rlhar;", "\u{21cc}"),
    ("rlm;", "\u{200f}"),
    ("rmoust;", "\u{23b1}"),
    ("rmoustache;", "\u{23b1}"),
    ("rnmid;", "\u{2aee}"),
    ("roang;", "\u{27ed}"),
    ("roarr;", "\u{21fe}"),
    ("robrk;", "\u{27e7}"),
    ("ropar;", "\u{2986}"),
    ("ropf;", "\u{1d563}"),
    ("roplus;", "\u{2a2e}"),
    ("rotimes;", "\u{2a35}"),
    ("rpar;", ")"),
    ("rpargt;", "\u{2994}"),
    ("rppolint;", "\u{2a12}"),
    ("rrarr;", "\u{21c9}"),
    ("rsaquo;", "\u{203a}"),
    ("rscr;", "\u{1d4c7}"),
    ("rsh;", "\u{21b1}"),
    ("rsqb;", "]"),
    ("rsquo;", "\u{2019}"),
    ("rsquor;", "\u{2019}"),
    ("rthree;", "\u{22cc}"),
    ("rtimes;", "\u{22ca}"),
    ("rtri;", "\u{25b9}"),
    ("rtrie;", "\u{22b5}"),
    ("rtrif;", "\u{25b8}"),
    ("rtriltri;", "\u{29ce}"),
    ("ruluhar;", "\u{2968}"),
    ("rx;", "\u{211e}"),
    ("sacute;", "\u{15b}"),
    ("sbquo;", "\u{201a}"),
    ("sc;", "\u{227b}"),
    ("scE;", "\u{2ab4}"),
    ("scap;", "\u{2ab8}"),
    ("scaron;", "\u{161}"),
    ("sccue;", "\u{227d}"),
    ("sce;", "\u{2ab0}"),
    ("scedil;", "\u{15f}"),
    ("scirc;", "\u{15d}"),
    ("scnE;", "\u{2ab6}"),
    ("scnap;", "\u{2aba}"),
    ("scnsim;", "\u{22e9}"),
    ("scpolint;", "\u{2a13}"),
    ("scsim;", "\u{227f}"),
    ("scy;", "\u{441}"),
    ("sdot;", "\u{22c5}"),
    ("sdotb;", "\u{22a1}"),
    ("sdote;", "\u{2a66}"),
    ("seArr;", "\u{21d8}"),
    ("searhk;", "\u{2925}"),
    ("searr;", "\u{2198}"),
    ("searrow;", "\u{2198}"),
    ("sect", "\u{a7}"),
    ("sect;", "\u{a7}"),
    ("semi;", ";"),
    ("seswar;", "\u{2929}"),
    ("setminus;", "\u{2216}"),
    ("setmn;", "\u{2216}"),
    ("sext;", "\u{2736}"),
    ("sfr;", "\u{1d530}"),
    ("sfrown;", "\u{2322}"),
    ("sharp;", "\u{266f}"),
    ("shchcy;", "\u{449}"),
    ("shcy;", "\u{448}"),
    ("shortmid;", "\u{2223}"),
    ("shortparallel;", "\u{2225}"),
    ("shy", "\u{ad}"),
    ("shy;", "\u{ad}"),
    ("sigma;", "\u{3c3}"),
    ("sigmaf;", "\u{3c2}"),
    ("sigmav;", "\u{3c2}"),
    ("sim;", "\u{223c}"),
    ("simdot;", "\u{2a6a}"),
    ("sime;", "\u{2243}"),
    ("simeq;", "\u{2243}"),
    ("simg;", "\u{2a9e}"),
    ("simgE;", "\u{2aa0}"),
    ("siml;", "\u{2a9d}"),
    ("simlE;", "\u{2a9f}"),
    ("simne;", "\u{2246}"),
    ("simplus;", "\u{2a24}"),
    ("simrarr;", "\u{2972}"),
    ("slarr;", "\u{2190}"),
    ("smallsetminus;", "\u{2216}"),
    ("smashp;", "\u{2a33}"),
    ("smeparsl;", "\u{29e4}"),
    ("smid;", "\u{2223}"),
    ("smile;", "\u{2323}"),
    ("smt;", "\u{2aaa}"),
    ("smte;", "\u{2aac}"),
    ("smtes;", "\u{2aac}\u{fe00}"),
    ("softcy;", "\u{44c}"),
    ("sol;", "/"),
    ("solb;", "\u{29c4}"),
    ("solbar;", "\u{233f}"),
    ("sopf;", "\u{1d564}"),
    ("spades;", "\u{2660}"),
    ("spadesuit;", "\u{2660}"),
    ("spar;", "\u{2225}"),
    ("sqcap;", "\u{2293}"),
    ("sqcaps;", "\u{2293}\u{fe00}"),
    ("sqcup;", "\u{2294}"),
    ("sqcups;", "\u{2294}\u{fe00}"),
    ("sqsub;", "\u{228f}"),
    ("sqsube;", "\u{2291}"),
    ("sqsubset;", "\u{228f}"),
    ("sqsubseteq;", "\u{2291}"),
    ("sqsup;", "\u{2290}"),
    ("sqsupe;", "\u{2292}"),
    ("sqsupset;", "\u{2290}"),
    ("sqsupseteq;", "\u{2292}"),
    ("squ;", "\u{25a1}"),
    ("square;", "\u{25a1}"),
    ("squarf;", "\u{25aa}"),
    ("squf;", "\u{25aa}"),
    ("srarr;", "\u{2192}"),
    ("sscr;", "\u{1d4c8}"),
    ("ssetmn;", "\u{2216}"),
    ("ssmile;", "\u{2323}"),
    ("sstarf;", "\u{22c6}"),
    ("star;", "\u{2606}"),
    ("starf;", "\u{2605}"),
    ("straightepsilon;", "\u{3f5}"),
    ("straightphi;", "\u{3d5}"),
    ("strns;", "\u{af}"),
    ("sub;", "\u{2282}"),
    ("subE;", "\u{2ac5}"),
    ("subdot;", "\u{2abd}"),
    ("sube;", "\u{2286}"),
    ("subedot;", "\u{2ac3}"),
    ("submult;", "\u{2ac1}"),
    ("subnE;", "\u{2acb}"),
    ("subne;", "\u{228a}"),
    ("subplus;", "\u{2abf}"),
    ("subrarr;", "\u{2979}"),
    ("subset;", "\u{2282}"),
    ("subseteq;", "\u{2286}"),
    ("subseteqq;", "\u{2ac5}"),
    ("subsetneq;", "\u{228a}"),
    ("subsetneqq;", "\u{2acb}"),
    ("subsim;", "\u{2ac7}"),
    ("subsub;", "\u{2ad5}"),
    ("subsup;", "\u{2ad3}"),
    ("succ;", "\u{227b}"),
    ("succapprox;", "\u{2ab8}"),
    ("succcurlyeq;", "\u{227d}"),
    ("succeq;", "\u{2ab0}"),
    ("succnapprox;", "\u{2aba}"),
    ("succneqq;", "\u{2ab6}"),
    ("succnsim;", "\u{22e9}"),
    ("succsim;", "\u{227f}"),
    ("sum;", "\u{2211}"),
    ("sung;", "\u{266a}"),
    ("sup1", "\u{b9}"),
    ("sup1;", "\u{b9}"),
    ("sup2", "\u{b2}"),
    ("sup2;", "\u{b2}"),
    ("sup3", "\u{b3}"),
    ("sup3;", "\u{b3}"),
    ("sup;", "\u{2283}"),
    ("supE;", "\u{2ac6}"),
    ("supdot;", "\u{2abe}"),
    ("supdsub;", "\u{2ad8}"),
    ("supe;", "\u{2287}"),
    ("supedot;", "\u{2ac4}"),
    ("suphsol;", "\u{27c9}"),
    ("suphsub;", "\u{2ad7}"),
    ("suplarr;", "\u{297b}"),
    ("supmult;", "\u{2ac2}"),
    ("supnE;", "\u{2acc}"),
    ("supne;", "\u{228b}"),
    ("supplus;", "\u{2ac0}"),
    ("supset;", "\u{2283}"),
    ("supseteq;", "\u{2287}"),
    ("supseteqq;", "\u{2ac6}"),
    ("supsetneq;", "\u{228b}"),
    ("supsetneqq;", "\u{2acc}"),
    ("supsim;", "\u{2ac8}"),
    ("supsub;", "\u{2ad4}"),
    ("supsup;", "\u{2ad6}"),
    ("swArr;", "\u{21d9}"),
    ("swarhk;", "\u{2926}"),
    ("swarr;", "\u{2199}"),
    ("swarrow;", "\u{2199}"),
    ("swnwar;", "\u{292a}"),
    ("szlig", "\u{df}"),
    ("szlig;", "\u{df}"),
    ("target;", "\u{2316}"),
    ("tau;", "\u{3c4}"),
    ("tbrk;", "\u{23b4}"),
    ("tcaron;", "\u{165}"),
    ("tcedil;", "\u{163}"),
    ("tcy;", "\u{442}"),
    ("tdot;", "\u{20db}"),
    ("telrec;", "\u{2315}"),
    ("tfr;", "\u{1d531}"),
    ("there4;", "\u{2234}"),
    ("therefore;", "\u{2234}"),
    ("theta;", "\u{3b8}"),
    ("thetasym;", "\u{3d1}"),
    ("thetav;", "\u{3d1}"),
    ("thickapprox;", "\u{2248}"),
    ("thicksim;", "\u{223c}"),
    ("thinsp;", "\u{2009}"),
    ("thkap;", "\u{2248}"),
    ("thksim;", "\u{223c}"),
    ("thorn", "\u{fe}"),
    ("thorn;", "\u{fe}"),
    ("tilde;", "\u{2dc}"),
    ("times", "\u{d7}"),
    ("times;", "\u{d7}"),
    ("timesb;", "\u{22a0}"),
    ("timesbar;", "\u{2a31}"),
    ("timesd;", "\u{2a30}"),
    ("tint;", "\u{222d}"),
    ("toea;", "\u{2928}"),
    ("top;", "\u{22a4}"),
    ("topbot;", "\u{2336}"),
    ("topcir;", "\u{2af1}"),
    ("topf;", "\u{1d565}"),
    ("topfork;", "\u{2ada}"),
    ("tosa;", "\u{2929}"),
    ("tprime;", "\u{2034}"),
    ("trade;", "\u{2122}"),
    ("triangle;", "\u{25b5}"),
    ("triangledown;", "\u{25bf}"),
    ("triangleleft;", "\u{25c3}"),
    ("trianglelefteq;", "\u{22b4}"),
    ("triangleq;", "\u{225c}"),
    ("triangleright;", "\u{25b9}"),
    ("trianglerighteq;", "\u{22b5}"),
    ("tridot;", "\u{25ec}"),
    ("trie;", "\u{225c}"),
    ("triminus;", "\u{2a3a}"),
    ("triplus;", "\u{2a39}"),
    ("trisb;", "\u{29cd}"),
    ("tritime;", "\u{2a3b}"),
    ("trpezium;", "\u{23e2}"),
    ("tscr;", "\u{1d4c9}"),
    ("tscy;", "\u{446}"),
    ("tshcy;", "\u{45b}"),
    ("tstrok;", "\u{167}"),
    ("twixt;", "\u{226c}"),
    ("twoheadleftarrow;", "\u{219e}"),
    ("twoheadrightarrow;", "\u{21a0}"),
    ("uArr;", "\u{21d1}"),
    ("uHar;", "\u{2963}"),
    ("uacute", "\u{fa}"),
    ("uacute;", "\u{fa}"),
    ("uarr;", "\u{2191}"),
    ("ubrcy;", "\u{45e}"),
    ("ubreve;", "\u{16d}"),
    ("ucirc", "\u{fb}"),
    ("ucirc;", "\u{fb}"),
    ("ucy;", "\u{443}"),
    ("udarr;", "\u{21c5}"),
    ("udblac;", "\u{171}"),
    ("udhar;", "\u{296e}"),
    ("ufisht;", "\u{297e}"),
    ("ufr;", "\u{1d532}"),
    ("ugrave", "\u{f9}"),
    ("ugrave;", "\u{f9}"),
    ("uharl;", "\u{21bf}"),
    ("uharr;", "\u{21be}"),
    ("uhblk;", "\u{2580}"),
    ("ulcorn;", "\u{231c}"),
    ("ulcorner;", "\u{231c}"),
    ("ulcrop;", "\u{230f}"),
    ("ultri;", "\u{25f8}"),
    ("umacr;", "\u{16b}"),
    ("uml", "\u{a8}"),
    ("uml;", "\u{a8}"),
    ("uogon;", "\u{173}"),
    ("uopf;", "\u{1d566}"),
    ("uparrow;", "\u{2191}"),
    ("updownarrow;", "\u{2195}"),
    ("upharpoonleft;", "\u{21bf}"),
    ("upharpoonright;", "\u{21be}"),
    ("uplus;", "\u{228e}"),
    ("upsi;", "\u{3c5}"),
    ("upsih;", "\u{3d2}"),
    ("upsilon;", "\u{3c5}"),
    ("upuparrows;", "\u{21c8}"),
    ("urcorn;", "\u{231d}"),
    ("urcorner;", "\u{231d}"),
    ("urcrop;", "\u{230e}"),
    ("uring;", "\u{16f}"),
    ("urtri;", "\u{25f9}"),
    ("uscr;", "\u{1d4ca}"),
    ("utdot;", "\u{22f0}"),
    ("utilde;", "\u{169}"),
    ("utri;", "\u{25b5}"),
    ("utrif;", "\u{25b4}"),
    ("uuarr;", "\u{21c8}"),
    ("uuml", "\u{fc}"),
    ("uuml;", "\u{fc}"),
    ("uwangle;", "\u{29a7}"),
    ("vArr;", "\u{21d5}"),
    ("vBar;", "\u{2ae8}"),
    ("vBarv;", "\u{2ae9}"),
    ("vDash;", "\u{22a8}"),
    ("vangrt;", "\u{299c}"),
    ("varepsilon;", "\u{3f5}"),
    ("varkappa;", "\u{3f0}"),
    ("varnothing;", "\u{2205}"),
    ("varphi;", "\u{3d5}"),
    ("varpi;", "\u{3d6}"),
    ("varpropto;", "\u{221d}"),
    ("varr;", "\u{2195}"),
    ("varrho;", "\u{3f1}"),
    ("varsigma;", "\u{3c2}"),
    ("varsubsetneq;", "\u{228a}\u{fe00}"),
    ("varsubsetneqq;", "\u{2acb}\u{fe00}"),
    ("varsupsetneq;", "\u{228b}\u{fe00}"),
    ("varsupsetneqq;", "\u{2acc}\u{fe00}"),
    ("vartheta;", "\u{3d1}"),
    ("vartriangleleft;", "\u{22b2}"),
    ("vartriangleright;", "\u{22b3}"),
    ("vcy;", "\u{432}"),
    ("vdash;", "\u{22a2}"),
    ("vee;", "\u{2228}"),
    ("veebar;", "\u{22bb}"),
    ("veeeq;", "\u{225a}"),
    ("vellip;", "\u{22ee}"),
    ("verbar;", "|"),
    ("vert;", "|"),
    ("vfr;", "\u{1d533}"),
    ("vltri;", "\u{22b2}"),
    ("vnsub;", "\u{2282}\u{20d2}"),
    ("vnsup;", "\u{2283}\u{20d2}"),
    ("vopf;", "\u{1d567}"),
    ("vprop;", "\u{221d}"),
    ("vrtri;", "\u{22b3}"),
    ("vscr;", "\u{1d4cb}"),
    ("vsubnE;", "\u{2acb}\u{fe00}"),
    ("vsubne;", "\u{228a}\u{fe00}"),
    ("vsupnE;", "\u{2acc}\u{fe00}"),
    ("vsupne;", "\u{228b}\u{fe00}"),
    ("vzigzag;", "\u{299a}"),
    ("wcirc;", "\u{175}"),
    ("wedbar;", "\u{2a5f}"),
    ("wedge;", "\u{2227}"),
    ("wedgeq;", "\u{2259}"),
    ("weierp;", "\u{2118}"),
    ("wfr;", "\u{1d534}"),
    ("wopf;", "\u{1d568}"),
    ("wp;", "\u{2118}"),
    ("wr;", "\u{2240}"),
    ("wreath;", "\u{2240}"),
    ("wscr;", "\u{1d4cc}"),
    ("xcap;", "\u{22c2}"),
    ("xcirc;", "\u{25ef}"),
    ("xcup;", "\u{22c3}"),
    ("xdtri;", "\u{25bd}"),
    ("xfr;", "\u{1d535}"),
    ("xhArr;", "\u{27fa}"),
    ("xharr;", "\u{27f7}"),
    ("xi;", "\u{3be}"),
    ("xlArr;", "\u{27f8}"),
    ("xlarr;", "\u{27f5}"),
    ("xmap;", "\u{27fc}"),
    ("xnis;", "\u{22fb}"),
    ("xodot;", "\u{2a00}"),
    ("xopf;", "\u{1d569}"),
    ("xoplus;", "\u{2a01}"),
    ("xotime;", "\u{2a02}"),
    ("xrArr;", "\u{27f9}"),
    ("xrarr;", "\u{27f6}"),
    ("xscr;", "\u{1d4cd}"),
    ("xsqcup;", "\u{2a06}"),
    ("xuplus;", "\u{2a04}"),
    ("xutri;", "\u{25b3}"),
    ("xvee;", "\u{22c1}"),
    ("xwedge;", "\u{22c0}"),
    ("yacute", "\u{fd}"),
    ("yacute;", "\u{fd}"),
    ("yacy;", "\u{44f}"),
    ("ycirc;", "\u{177}"),
    ("ycy;", "\u{44b}"),
    ("yen", "\u{a5}"),
    ("yen;", "\u{a5}"),
    ("yfr;", "\u{1d536}"),
    ("yicy;", "\u{457}"),
    ("yopf;", "\u{1d56a}"),
    ("yscr;", "\u{1d4ce}"),
    ("yucy;", "\u{44e}"),
    ("yuml", "\u{ff}"),
    ("yuml;", "\u{ff}"),
    ("zacute;", "\u{17a}"),
    ("zcaron;", "\u{17e}"),
    ("zcy;", "\u{437}"),
    ("zdot;", "\u{17c}"),
    ("zeetrf;", "\u{2128}"),
    ("zeta;", "\u{3b6}"),
    ("zfr;", "\u{1d537}"),
    ("zhcy;", "\u{436}"),
    ("zigrarr;", "\u{21dd}"),
    ("zopf;", "\u{1d56b}"),
    ("zscr;", "\u{1d4cf}"),
    ("zwj;", "\u{200d}"),
    ("zwnj;", "\u{200c}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_format_like_python() {
        // outputs of Python 3.12 f"{v:,.2f}", f"{v:.0f}", f"{v:,.0f}", f"{v:.1f}"
        let cases: &[(f64, &str, &str, &str, &str)] = &[
            (0.125, "0.12", "0", "0", "0.1"),
            (0.375, "0.38", "0", "0", "0.4"),
            (2.5, "2.50", "2", "2", "2.5"),
            (0.5, "0.50", "0", "0", "0.5"),
            (1.5, "1.50", "2", "2", "1.5"),
            (
                1234567.125,
                "1,234,567.12",
                "1234567",
                "1,234,567",
                "1234567.1",
            ),
            (-0.0001, "-0.00", "-0", "-0", "-0.0"),
            (
                1e22,
                "10,000,000,000,000,000,000,000.00",
                "10000000000000000000000",
                "10,000,000,000,000,000,000,000",
                "10000000000000000000000.0",
            ),
        ];
        for (v, a, b, c, d) in cases {
            assert_eq!(fmt_f(*v, 2, true), *a, "{v}");
            assert_eq!(fmt_f(*v, 0, false), *b, "{v}");
            assert_eq!(fmt_f(*v, 0, true), *c, "{v}");
            assert_eq!(fmt_f(*v, 1, false), *d, "{v}");
        }
        assert_eq!(fmt_f(f64::NAN, 2, true), "nan");
        assert_eq!(
            centroid_words(-0.7994443439900589, 103.21224206034377),
            "the project boundary file (centroid 0.7994° S, 103.2122° E)"
        );
    }

    #[test]
    fn unescape_matches_python() {
        assert_eq!(py_str(&json!([1, "a"])), "[1, 'a']");
        assert_eq!(py_str(&json!(1e16)), "1e+16");
        assert_eq!(html_unescape("IP&amp;LC"), "IP&LC");
        assert_eq!(html_unescape("IP&LC or"), "IP&LC or");
        assert_eq!(
            html_unescape("&ampx &#65;&#x42 &#; &#xZ &notit; &#0; &#128;&#xD800;&#11;"),
            "&x AB &#; &#xZ ¬it; \u{fffd} €\u{fffd}"
        );
        assert_eq!(
            norm("  People:\n IP&amp;LC  community "),
            "People: IP&LC community"
        );
    }

    /// The `__main__` self-check of context.py.
    #[test]
    fn context_self_check() {
        let mut matrix = Map::new();
        for a in ["0-14", "15-44", "45-beyond"] {
            let mut by_edu = Map::new();
            for e in ["primary", "secondary", "higher"] {
                by_edu.insert(e.into(), json!({"male": 100, "female": 80, "other": 2}));
            }
            matrix.insert(a.into(), Value::Object(by_edu));
        }
        let form = json!({
            "seMatrix": matrix, "seHouseholds": "777", "seElectricity": "701", "seWash": "900",
            "seEmployment": {"employed": 611, "unemployed": 122, "economically-inactive": 233},
            "seIplc": [{"name": "Sundanese", "individuals": 555}],
            "seHhLivelihood": {"other": 44, "forestry-nbs": 42, "fishery-aquaculture": 0},
        });
        let ctx = build_context(None, &form, &json!({}), &[]).ok().unwrap();
        let t = |k: &str| ctx["tags"].get(k).cloned();
        assert_eq!(ctx["education_primary"], json!("546"));
        assert_eq!(
            t("People: Gender disaggregation within Project Area (% Male / % Female / % Other)"),
            Some(json!("54.9% Male / 44.0% Female / 1.1% Other"))
        );
        assert_eq!(
            t("People: Age Group Within Project Area"),
            Some(json!(
                "0–14: 546 (33.3%), 15–44: 546 (33.3%), 45+: 546 (33.3%)"
            ))
        );
        assert_eq!(t("People: employment%"), Some(json!("63.25%")));
        assert_eq!(t("People: Employment Status"), Some(json!("611")));
        assert_eq!(
            t("People: IP&LC or Ethnicity Identification"),
            Some(json!("Sundanese"))
        );
        assert_eq!(
            t("People: Household based on livelihood"),
            Some(json!("other (44 households), forestry/NbS (42 households)"))
        );
        assert_eq!(t("People: electricity%"), Some(json!("90.22")));
        assert_eq!(t("People: WASH%"), None);
        assert_eq!(
            t("People: Total Population in The Project Area"),
            Some(json!("1,638"))
        );

        // Manage-only run: untoggled Protect must not be listed as chosen.
        let card = json!({"label": "Forest", "interventions": [
            {"intervention": "Protect", "area_ha": 1.0, "activities": [{"activity_id": "1", "activity": "P1"}]},
            {"intervention": "Manage", "area_ha": 1.0, "activities": [{"activity_id": "2", "activity": "M1"}]}]});
        let a = Analyzer {
            site_information_json: json!({}),
            nature_json: json!({}),
            climate_json: json!({}),
            people_json: json!({}),
            threat_json: json!({}),
            intervention_eligibility_json: json!({"ecosystems": [card]}),
            benefit_json: json!({"assumptions": {"selections": {"forest": {
                "protect": false, "manage": true, "restore": false, "activities": []}}}}),
        };
        let ctx = build_context(
            Some(&a),
            &json!({}),
            &json!({"DD-Month-Year": "1 May", "x": null}),
            &[],
        )
        .ok()
        .unwrap();
        let t = |k: &str| ctx["tags"].get(k).cloned();
        assert_eq!(ctx["eco0"]["activities_all"], json!("M1"));
        assert_eq!(
            build_context(None, &json!({}), &json!({}), &[])
                .ok()
                .unwrap()["education_higher"],
            json!("[People: Education Level]")
        );
        assert_eq!(t("NbS Pathway: Chosen NbS Activities"), Some(json!("M1")));
        assert_eq!(
            t("NbS Pathway: Protect / Manage / Restore"),
            Some(json!("Manage"))
        );
        assert_eq!(t("User input: DD-Month-Year"), Some(json!("1 May")));

        let bad = Analyzer {
            site_information_json: json!([1]),
            ..a
        };
        assert_eq!(
            build_context(Some(&bad), &json!({}), &json!({}), &[]).err(),
            Some("'list' object has no attribute 'get'".into())
        );
    }
}
