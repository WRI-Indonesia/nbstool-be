# application/utils/document_generator/v3/context.py
#
# Build the docxtpl context for the v3 feasibility template: one lookup table from the template's
# bracket-tag text to a rendered value, plus the loop lists the converter wired in.
#
# Sources, in priority order per tag:
#   1. `user_input`  -- request payload, keyed by the exact text inside `[User input: ...]`
#                       (or any full tag text, which lets the frontend override anything)
#   2. `form`        -- the F03 socio-economic form (`se*` keys, see the frontend's SOCIO spec)
#   3. DataAnalyzer  -- the per-session v3 analysis results persisted by the feature endpoints
#                       (site_information_json / nature_json / climate_json / people_json /
#                        threat_json / intervention_eligibility_json)
#
# A tag with no value renders back as its literal bracket text, so the generated docx shows
# exactly what still needs manual fill -- the team's chosen workflow for user-input fields.
#
# Numbers are formatted here, not in the template: areas with thousands separators at 2 dp,
# percentages at 2 dp. Tag lookup is whitespace-normalised but otherwise exact, because the
# template's own spellings vary ("disturbed / degraded" vs "disturbed /degraded" both appear).

from __future__ import annotations

import re

_WS = re.compile(r"\s+")


def _norm(tag: str) -> str:
    return _WS.sub(" ", tag).strip()


def _ha(value) -> str | None:
    return f"{value:,.2f}" if isinstance(value, (int, float)) else None


def _pct(value) -> str | None:
    return f"{value:.2f}" if isinstance(value, (int, float)) else None


def _label(dict_obj) -> str | None:
    """The `fallback` of a frontend label object like {'key': ..., 'fallback': 'Lowland'}."""
    return dict_obj.get("fallback") if isinstance(dict_obj, dict) else None


def _join(items, sep=", ") -> str | None:
    items = [str(i) for i in items if i]
    return sep.join(items) if items else None


def _sum_numeric_leaves(value) -> float:
    """Total of every numeric leaf in a nested structure -- the form's matrix widgets."""
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, dict):
        return sum(_sum_numeric_leaves(v) for v in value.values())
    if isinstance(value, list):
        return sum(_sum_numeric_leaves(v) for v in value)
    try:
        return float(str(value).replace(",", "").replace(".", "", str(value).count(".") - 1))
    except (TypeError, ValueError):
        return 0.0


def _site_characterisation_tags(si: dict, pathway: dict) -> dict:
    tags: dict[str, str] = {}

    # Administrative location (1.2). Level 0..4 = country..village.
    tags["Site Characterisation: Level 0"] = si.get("country")
    tags["Site Characterisation: Level 1"] = si.get("province")
    tags["Site Characterisation: Level 2"] = si.get("district")
    tags["Site Characterisation: Level 3"] = si.get("subdistrict")
    tags["Site Characterisation: Level 4"] = si.get("village")
    tags["Site Characterisation: Country"] = si.get("country")
    location = _join([si.get("village"), si.get("subdistrict"), si.get("district"),
                      si.get("province")])
    tags["Site Characterisation: Villages, Sub-District, District, Province"] = location
    tags["Site Characterisation: Administrative Location"] = _join(
        [si.get("district"), si.get("province"), si.get("country")])

    # Areas. The AOI total comes from the pathway result; ecosystems (1.1) carry the shares.
    project_area = pathway.get("project_area_ha")
    tags["Site Characterisation: number of"] = _ha(project_area)
    tags["Site Characterisation: total AOI (ha)"] = _ha(project_area)
    tags["Site Characterisation: Total Project Area in Hectares"] = _ha(project_area)

    by_eco = {e.get("name"): e for e in si.get("ecosystems", []) if isinstance(e, dict)}
    tags["Site Characterisation: percentage of peatland number area"] = _pct(
        by_eco.get("Peatland", {}).get("percentage"))
    tags["Site Characterisation: percentage of mangrove number area"] = _pct(
        by_eco.get("Mangrove", {}).get("percentage"))
    dominant_eco = max(
        (e for e in si.get("ecosystems", []) if isinstance(e, dict) and e.get("name") != "Other/Unclassified"),
        key=lambda e: e.get("area") or 0, default=None)
    tags["Site Characterisation: dominant ecosystem type(s)"] = (
        dominant_eco.get("name") if dominant_eco else None)
    tags["Site Characterisation: dominant ecosystem type"] = (
        dominant_eco.get("name") if dominant_eco else None)

    # Terrain (1.4).
    tags["Site Characterisation: elevation class"] = _label(si.get("predominant_elevation_dict"))
    tags["Site Characterisation: percentage of slope number average"] = _pct(
        si.get("average_slope_percentage"))
    slopes = si.get("slopes") or []
    dominant_slope = max(slopes, key=lambda s: s.get("area") or 0, default=None)
    tags["Site Characterisation: slope categories"] = (
        _label(dominant_slope.get("dict")) if dominant_slope else None)

    # Land cover (1.8). The list arrives area-sorted; class 0 is "No data / other".
    classes = [c for c in si.get("land_cover_class", []) if isinstance(c, dict)]
    named = [c for c in classes if c.get("id") != "0"]
    tags["Site Characterisation: top 3 land cover class"] = _join(
        [c.get("name") for c in named[:3]])
    for position, ordinal in enumerate(("1st", "2nd", "3rd")):
        row = named[position] if position < len(named) else {}
        tags[f"Site Characterisation: {ordinal} land cover class"] = row.get("name")
        tags[f"Site Characterisation: {ordinal} land cover class area (ha)"] = _ha(row.get("area"))
        tags[f"Site Characterisation: {ordinal} land cover class area (% of AOI)"] = _pct(
            row.get("percentage"))
    forest_pct = sum(c.get("percentage") or 0 for c in classes if "forest" in
                     str(c.get("name", "")).lower())
    tags["Site Characterisation: percentage of forest coverage number area"] = (
        _pct(forest_pct) if classes else None)
    forest_ha = sum(c.get("area") or 0 for c in classes if "forest" in
                    str(c.get("name", "")).lower())
    tags["Site Characterisation: Forest Area Directly Involved (ha)"] = (
        _ha(forest_ha) if classes else None)

    # Change and risk (1.5, 1.7, burned area).
    tags["Site Characterisation: historical deforestation rate"] = _pct(
        si.get("historical_deforestation_percentage"))
    tags["Site Characterisation: rate_pct"] = _pct(si.get("historical_deforestation_percentage"))
    start = si.get("historical_deforestation_year_start")
    end = si.get("historical_deforestation_year_end")
    tags["Site Characterisation: time period"] = f"{start}–{end}" if start and end else None
    risks = _join([
        f"{name} ({_label(si.get(key))})"
        for name, key in (("Flood", "flood_risk_dict"), ("Tropical typhoon", "typhoon_risk_dict"),
                          ("Landslide", "landslide_risk_dict"), ("Drought", "drought_risk_dict"),
                          ("Fire", "fire_risk_dict"))
        if _label(si.get(key))
    ])
    tags["Site Characterisation: Flood/Tropical typhoon/Landslide/Drought/Fire risk"] = risks
    tags["Site Characterisation: Flood / Tropical typhoon / Landslide / Drought / Fire risk"] = risks

    return tags


def _burned_area_tags(climate: dict) -> dict:
    return {
        "Site Characterisation: Historical Burned Area": _ha(climate.get("total_burned_area")),
    }


def _threat_tags(threat: dict) -> dict:
    tags: dict[str, str] = {}
    overview = threat.get("all ecosystems", {})
    dryland = threat.get("dryland forest", {})
    mangrove = threat.get("mangrove", {})
    peatland = threat.get("peatland", {})

    ecosystems = overview.get("ecosystems") or []
    dominant = max((e for e in ecosystems if isinstance(e, dict)),
                   key=lambda e: e.get("area_ha") or 0, default=None)
    eco_label = dominant.get("label") if dominant else None
    tags["Threat: Forest / Mangrove / Peatland"] = eco_label
    tags["Threat: Forest / Mangrove  / Peatland"] = eco_label
    tags["Threat: ecosystem (forest / mangrove / peatland)"] = eco_label
    tags["Threat: total ecosystem area (ha)"] = _ha(overview.get("total_ecosystem_area_ha"))

    # Per-ecosystem figures are quoted for the DOMINANT ecosystem's tab.
    tab = {"Dryland forest": dryland, "Mangrove": mangrove, "Peatland": peatland}.get(
        eco_label or "", dryland) or {}
    tags["Threat: remaining intact cover (ha)"] = _ha(tab.get("remaining_forest_ha"))
    tags["Threat: disturbed / degraded area (ha)"] = _ha(tab.get("disturbed_area_ha"))
    tags["Threat: disturbed /degraded area (ha)"] = _ha(tab.get("disturbed_area_ha"))
    tags["Threat: percentage of disturbed area"] = _pct(tab.get("disturbed_percentage"))
    loss = dryland.get("forest_loss_ha")
    if loss is None:
        loss = peatland.get("converted_loss_ha")
    tags["Threat: forest disturbed and converted/loss (ha)"] = _ha(loss)
    tags["Site Characterisation: forest loss area"] = _ha(loss)

    drivers = (tab.get("drivers") or {})
    non_natural = drivers.get("non_natural") or []
    tags["Threat: Main pressure (Logging / Road access / Agriculture Expansion / Settlement / "
         "Mining / Coastal conversion / Drainage / risk)"] = (
        mangrove.get("main_pressure") if tab is mangrove else _join(non_natural))
    tags["Threat: driver 1"] = non_natural[0] if len(non_natural) > 0 else None
    tags["Threat: driver 2"] = non_natural[1] if len(non_natural) > 1 else None
    tags["Threat: natural drivers (flooding / forest fire / drought / typhoon / landslide / "
         "extreme climate)"] = _join(drivers.get("natural") or [])

    tags["Threat: canal proximity (High / Moderate / Low)"] = peatland.get("canal_proximity")
    tags["Threat: drainage pressure (High / Moderate / Low)"] = peatland.get("drainage_pressure")
    tags["Threat: fire risk (High / Moderate / Low)"] = peatland.get("fire_risk")
    return tags


def _pathway_tags(pathway: dict, threat: dict) -> dict:
    tags: dict[str, str] = {}
    dryland = threat.get("dryland forest", {})
    tags["NbS Pathway: hectare of remaining forest"] = _ha(dryland.get("remaining_forest_ha"))
    tags["NbS Pathway: percentage of remaining forest"] = _pct(
        dryland.get("remaining_forest_percentage"))

    # Aggregate protect/manage/restore over every ecosystem card.
    totals: dict[str, float] = {}
    activities: list[str] = []
    for eco in pathway.get("ecosystems", []):
        for intervention in eco.get("interventions", []):
            name = intervention.get("intervention")
            totals[name] = totals.get(name, 0.0) + (intervention.get("area_ha") or 0.0)
            for activity in intervention.get("activities", []):
                label = activity.get("activity")
                if label and label not in activities:
                    activities.append(label)
    project_area = pathway.get("project_area_ha") or 0
    for name in ("Protect", "Manage", "Restore"):
        tags[f"NbS Pathway: hectare area eligible to {name.lower()}"] = _ha(totals.get(name))
        if project_area:
            tags[f"NbS Pathway: percent area eligible to {name.lower()}"] = _pct(
                (totals.get(name) or 0) / project_area * 100)
    tags["NbS Pathway: List of NbS Activities"] = _join(activities)

    duration = pathway.get("duration_years") or {}
    if duration.get("default"):
        tags["NbS Pathway: Duration of protection (10–40 years, default 30)"] = (
            f"{duration['default']} years")
    return tags


def _nature_tags(nature: dict) -> dict:
    tags: dict[str, str] = {}
    tags["Nature: FLII score"] = _pct(nature.get("flii_score"))
    tags["Nature: Mean FLII/10"] = _pct(nature.get("flii_score"))
    tags["Nature: average of Forest Landscape Integrity Index score"] = _pct(
        nature.get("flii_score"))
    tags["Nature: % High FLII"] = _pct(nature.get("high_integrity_percentage"))
    tags["Nature: % Medium FLII"] = _pct(nature.get("medium_integrity_percentage"))
    tags["Nature: % Low FLII"] = _pct(nature.get("low_integrity_percentage"))
    integrity = nature.get("dominant_integrity_class")
    tags["Nature: Integrity Category"] = integrity
    tags["Nature: Integrity Category (High/Medium/Low/No forest cover)"] = integrity
    tags["Nature: forest integrity class (high / medium / low / no forest)"] = integrity

    tags["Nature: total wildlife species"] = nature.get("total_wildlife_species")
    tags["Nature: amphibian species count"] = nature.get("amphibian_number_of_species")
    tags["Nature: [amphibian species count"] = nature.get("amphibian_number_of_species")
    tags["Nature: bird species count"] = nature.get("bird_number_of_species")
    tags["Nature: mammal species count"] = nature.get("mammal_number_of_species")
    tags["Nature: reptile species count"] = nature.get("reptile_number_of_species")
    tags["Nature: endangered tree species"] = nature.get("endangered_tree_number_of_species")
    tags["Nature: number of endangered tree species"] = nature.get(
        "endangered_tree_number_of_species")

    key_species = [s.get("species_id") for s in nature.get("key_species", [])
                   if isinstance(s, dict)]
    tags["Nature: keystone species"] = _join(key_species)
    if len(key_species) > 1:
        tags["Nature: keystone species names (comma-separated, with \"and\" before the final "
             "species)"] = _join(key_species[:-1]) + " and " + key_species[-1]
    else:
        tags["Nature: keystone species names (comma-separated, with \"and\" before the final "
             "species)"] = _join(key_species)

    summary = nature.get("iucn_summary") or {}
    endangered = sum(summary.get(code, 0) for code in ("CR", "EN", "VU"))
    tags["Nature: number of endangered species"] = endangered if summary else None

    kba_names = [k.get("overlapping_key_biodiversity_name")
                 for k in nature.get("overlapping_key_biodiversity_areas", [])
                 if isinstance(k, dict)]
    tags["Nature: KBA name(s) / count"] = _join(kba_names)
    tags["Nature: KBA overlap area"] = _ha(
        nature.get("overlapping_key_biodiversity_area_total_size"))
    tags["Nature: KBA overlap %"] = _pct(
        nature.get("overlapping_key_biodiversity_area_percentage"))
    tags["Nature: in percent of total AOI area"] = _pct(
        nature.get("overlapping_key_biodiversity_area_percentage"))
    return tags


# F03 form key -> template tag(s). Scalars only; compound widgets are handled in _form_tags.
_FORM_SCALARS = {
    "seYearSource": ["People: Year of Data Source"],
    "seHouseholds": ["People: Household population and number of households in The Project Area"],
    "seDisabilities": ["People: Disabilities"],
    "seSchools": ["People: Number of Schools within project area"],
    "seHealthFac": ["People: Health Facility within project area"],
    "seDiseases": ["People: Endemic Infectious Diseases"],
    "seSocialForestry": ["People: Social Forestry Beneficiaries"],
    "seIncome": ["People: Average Household income"],
    "seExpenditure": ["People: Average Household expenditure"],
    "sePoverty": ["People: Number of Household in poverty"],
    "seUndernourished": ["People: Number of undernourished Household"],
    "seElectricity": ["People: Number of Households with Access to electricity"],
    "seWash": ["People: Number of Households with Access to WASH"],
    "seLegal": ["People: Forestry Legal Framework", "People: forestry legal framework"],
    "seBenefit": ["People: Benefit Sharing Mechanism", "People: benefit sharing mechanism"],
    "seTenure": ["People: Indigenous Land Tenure"],
    "seMrv": ["People: MRV Institutional Capacity", "People: MRV institutional capacity level"],
    "seSafeguard": ["People: Safeguard Information System"],
    "seGrievance": ["People: Grievance Redress Mechanism"],
    "seAdaptive": ["People: Adaptive Management Response Time"],
    "seEmployment": ["People: Employment Status"],
    "seLivelihood": ["People: Type of livelihood (Agriculture, Forestry, Fishery)"],
    "seHhLivelihood": ["People: Household based on livelihood"],
    "seIplc": ["People: IP&LC or Ethnicity Identification",
               "People: identified IP&LC or ethnicity name",
               "People: identified IP&LC community"],
}


def _form_tags(form: dict) -> dict:
    tags: dict[str, str] = {}
    for key, tag_names in _FORM_SCALARS.items():
        value = form.get(key)
        if value in (None, "", [], {}):
            continue
        if isinstance(value, (list, dict)):
            value = _join(value) if isinstance(value, list) and all(
                isinstance(v, str) for v in value) else None
        if value is None:
            continue
        for tag in tag_names:
            tags[tag] = value

    matrix = form.get("seMatrix")
    if matrix:
        total = _sum_numeric_leaves(matrix)
        if total:
            tags["People: Total Population in The Project Area"] = f"{total:,.0f}"

    villages = form.get("seVillages")
    if isinstance(villages, list) and villages:
        names = [v.get("name") if isinstance(v, dict) else str(v) for v in villages]
        tags["People: Number of Villages"] = len([n for n in names if n])
    elif villages not in (None, "", []):
        tags["People: Number of Villages"] = villages
    return tags


def _people_tags(people: dict) -> dict:
    """Regional analyser figures (6.1-6.3), read from the persisted People sections."""
    demo = people.get("social_demography", {})
    tags = {
        "People: Total Population": (
            f"{demo['total_population']:,.0f}" if isinstance(
                demo.get("total_population"), (int, float)) and demo.get("total_population")
            else None),
        "People: Household population and number of households": demo.get("household_number"),
        "People: Physical Vulnerability": demo.get("physical_vulnerability_level_id"),
        "People: Environmental Vulnerability": demo.get("environmental_vulnerability_level_id"),
        "People: Economic Vulnerability": demo.get("economic_vulnerability_level_id"),
        "People: Social Vulnerability": demo.get("social_vulnerability_level_id"),
    }
    male = demo.get("male_population_percentage")
    female = demo.get("female_population_percentage")
    if male is not None and female is not None:
        tags["People: Gender disaggregation (% Male / % Female)"] = (
            f"{male:.1f}% Male / {female:.1f}% Female")
        tags["People: Gender disaggregation within Project Area (% Male / % Female / % Other)"] = (
            f"{male:.1f}% Male / {female:.1f}% Female")
    return tags


def _benefit_pillar_counts(benefit: dict) -> dict:
    """Distinct 5.1 benefits per Triple Win pillar, across every pathway -- the template's
    "x Nature / x People / x Climate sub-components scored". Unfilled -> literal bracket text."""
    by_pathway = ((benefit.get("general_benefit") or {}).get("values", {})
                  .get("by_pathway") or {})
    counts = {}
    for pillar in ("nature", "people", "climate"):
        distinct: set = set()
        for slot in by_pathway.values():
            distinct.update((slot.get("benefits") or {}).get(pillar) or [])
        counts[f"benefit_{pillar}_count"] = (
            str(len(distinct)) if by_pathway else "[Potential Benefit: x]")
    return counts


def build_context(analyzer, form: dict | None, user_input: dict | None) -> dict:
    """The docxtpl context: `t` lookup, loop lists and condition flags.

    `analyzer` is the session's DataAnalyzer row (may be None); `form` the F03 socio-economic
    payload; `user_input` free overrides keyed by tag text, with or without the `User input: `
    prefix.
    """
    si = (analyzer.site_information_json if analyzer else None) or {}
    nature = (analyzer.nature_json if analyzer else None) or {}
    climate = (analyzer.climate_json if analyzer else None) or {}
    people = (analyzer.people_json if analyzer else None) or {}
    threat = (analyzer.threat_json if analyzer else None) or {}
    pathway = ((analyzer.intervention_eligibility_json if analyzer else None) or {})
    benefit = (getattr(analyzer, "benefit_json", None) if analyzer else None) or {}

    # F02-P5 carbon figures. Net (after carbon-risk deductions) when the project ran as an NbS
    # carbon project, else the gross component totals. Missing entirely -> tags stay unfilled.
    def _carbon(net_key, gross_key):
        net = (benefit.get(net_key) or {}).get("values", {}).get("net_tco2e")
        if net is not None:
            return net
        gross = benefit.get(gross_key) or {}
        return gross.get("values", {}).get("total_tco2e") if gross.get("applicable") else None

    benefit_avoided = _carbon("net_emission_reduction", "avoided_emissions")
    benefit_sequestered = _carbon("net_carbon_removal", "arr_sequestration")
    # 5.6's combined Net ERRs is the headline total when present (buffer taken on the adjusted
    # figure there, so it is NOT the sum of the 5.4/5.5 nets).
    net_errs_total = (benefit.get("net_errs") or {}).get("values", {}).get("net_tco2e")

    tags: dict[str, object] = {}
    if net_errs_total is not None:
        tags["Potential Benefit: total carbon stock stored"] = _ha(net_errs_total)
    elif benefit_avoided is not None or benefit_sequestered is not None:
        tags["Potential Benefit: total carbon stock stored"] = _ha(
            (benefit_avoided or 0.0) + (benefit_sequestered or 0.0))
    tags.update(_site_characterisation_tags(si, pathway))
    tags.update(_burned_area_tags(climate))
    tags.update(_threat_tags(threat))
    tags.update(_pathway_tags(pathway, threat))
    tags.update(_nature_tags(nature))
    tags.update(_people_tags(people))
    tags.update(_form_tags(form or {}))

    for key, value in (user_input or {}).items():
        if value in (None, ""):
            continue
        key = _norm(str(key))
        # Bare keys fill `[User input: <key>]`; a full "Category: name" key overrides any tag.
        tags[key if ": " in key else f"User input: {key}"] = value

    lookup = {_norm(k): v for k, v in tags.items() if v is not None}

    def t(tag_text: str) -> str:
        value = lookup.get(_norm(tag_text))
        return str(value) if value is not None else f"[{tag_text}]"

    named = [c for c in si.get("land_cover_class", [])
             if isinstance(c, dict) and c.get("id") != "0"]
    land_cover_rest = [
        {"name": c.get("name"), "area": _ha(c.get("area")), "pct": _pct(c.get("percentage"))}
        for c in named[3:]
    ]

    # Per-ecosystem blocks for the Monitoring Plan tables (see convert.MONITORING_OCCURRENCES):
    # always exactly 3 entries in the pathway card order Forest, Mangrove, Peatland.
    #
    # Activities and indicator rows come from the F05 monitoring form when it was saved
    # (`form.mpPlan`, the frontend's collectPlan shape: ecos[].activities[].{name, pw, groups[]
    # .{benefit, cat, items[].{label, source, freq, unit?}}}). Without a plan, the pathway's
    # AVAILABLE activity list per intervention stands in and the indicator tables stay empty;
    # a missing pathway run leaves literal bracket text, same as any unfilled tag.
    plan_ecos = {}
    for eco in ((form or {}).get("mpPlan") or {}).get("ecos") or []:
        if isinstance(eco, dict):
            plan_ecos[str(eco.get("id") or eco.get("name") or "").lower()] = eco

    # Pathway-screen choices, persisted with the benefit run: {forest|mangrove|peatland:
    # {protect|manage|restore: bool, activities: [activity ids]}}. Used when the F05 plan has
    # not been saved yet -- the plan is the richer, later source and wins.
    selections = benefit.get("selections") or {}

    ecos = []
    cards = {e.get("label"): e for e in pathway.get("ecosystems", []) if isinstance(e, dict)}
    for label, plan_key in (("Forest", "forest"), ("Mangrove", "mangrove"),
                            ("Peatland", "peatland")):
        card = cards.get(label) or {}
        interventions = {i.get("intervention"): i for i in card.get("interventions", [])}
        plan = plan_ecos.get(plan_key) or {}
        plan_activities = [a for a in plan.get("activities", []) if isinstance(a, dict)]

        entry = {"label": label}
        names: list = []
        for name in ("Protect", "Manage", "Restore"):
            intervention = interventions.get(name) or {}
            entry[f"{name.lower()}_ha"] = (
                _ha(intervention.get("area_ha"))
                or "[NbS Pathway: hectare area eligible to " + name.lower() + "]")
            chosen = [a.get("name") for a in plan_activities
                      if str(a.get("pw", "")).upper() == name.upper() and a.get("name")]
            available_entries = [a for a in intervention.get("activities", [])
                                 if a.get("activity")]
            available = [a.get("activity") for a in available_entries]
            if not chosen:
                selection = selections.get(plan_key) or {}
                if selection.get(name.lower()):
                    ids = {str(i) for i in (selection.get("activities") or [])}
                    chosen = [a["activity"] for a in available_entries
                              if not ids or str(a.get("activity_id")) in ids]
            acts = chosen or available
            entry[f"activities_{name.lower()}"] = _join(acts) or "[NbS Pathway: Chosen NbS Activities]"
            names.extend(a for a in acts if a not in names)
        entry["activities_all"] = _join(names) or "[NbS Pathway: Chosen NbS Activities]"

        entry["indicator_rows"] = [
            {
                "category": group.get("cat") or "",
                "benefit": group.get("benefit") or "",
                "indicator": item.get("label") or "",
                "unit": item.get("unit") or "",
                "freq": item.get("freq") or "",
                "source": item.get("source") or "",
            }
            for activity in plan_activities
            for group in activity.get("groups", []) if isinstance(group, dict)
            for item in group.get("items", []) if isinstance(item, dict)
        ]
        ecos.append(entry)

    species_rows = [
        {"taxon_class": s.get("taxon_class"), "scientific_name": s.get("scientific_name"),
         "redlist_category": s.get("redlist_category")}
        for s in nature.get("species_list", []) if isinstance(s, dict)
    ]

    return {
        "t": t,
        "land_cover_rest": land_cover_rest,
        "species_rows": species_rows,
        "keystone_present": bool(nature.get("key_species")),
        # Flat names, not a subscripted list: the converter's expressions must stay bracket-free
        # (see convert._eco_field).
        "eco0": ecos[0],
        "eco1": ecos[1],
        "eco2": ecos[2],
        # Occurrence-routed by the feasibility converter: odd `X tonnes` = avoided, even =
        # sequestered. Unfilled -> the literal bracket text, same as any other tag.
        "benefit_avoided_tco2e": (
            _ha(benefit_avoided) if benefit_avoided is not None
            else "[Potential Benefit: X tonnes]"),
        "benefit_sequestered_tco2e": (
            _ha(benefit_sequestered) if benefit_sequestered is not None
            else "[Potential Benefit: X tonnes]"),
        **_benefit_pillar_counts(benefit),
    }
