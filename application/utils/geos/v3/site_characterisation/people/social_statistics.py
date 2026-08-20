"""
Component 6.3 Social Statistics.

Employment, education, economy, health and housing for the administrative area the project sits
in, from the national statistics tables in `se_v3`.

NOT A NOTEBOOK COMPONENT. The People notebook covers 6.1 and 6.2 only and says outright that
"Household estimation is excluded". Everything here is the endpoint's contract, filled from the
tables the data team loaded and specified by the sample queries in `_test/nbs/sosial_query/*.sql`.
The mapping from contract field to table, level, category and subgroup is entirely in
`config.SOCIAL_INDICATORS`; this module is only the machinery that runs it.

THE SCALE IS NOT THE AOI. 6.1 and 6.2 measure the polygon the user drew. Every number here
describes a whole province, district or country, because that is the finest unit a statistics
office publishes. A project of 674 ha inside a province of two million people gets that province's
unemployment rate, not its own. Each section therefore carries the name of the area it describes,
so the payload never implies a precision it does not have.

THE PAYLOAD SHAPE IS PER COUNTRY. Eleven countries, eleven contract types, because eleven
statistics offices publish different things. A country whose table does not exist gets the field
as null with a flag naming it -- never a zero, which would read as a measurement of nothing rather
than as an absence of data.

WHICH AREA, when the AOI spans several. The dominant one by area, as computed by 1.2, the same
rule 1.6 already uses for its country. That is what the contract's single
`administrative_area_name` string can carry. When the AOI crosses a boundary, the flag says so and
names the area that was used.
"""

from __future__ import annotations

from collections import Counter

try:
    from ...config import (
        COUNTRY_ISO3,
        SOCIAL_AREA_NAME_FIELDS,
        SOCIAL_INDICATORS,
        SOCIAL_LEVEL_SOURCES,
        SOCIAL_NO_AREA_NAME,
    )
    from ...db import load_social_rows, load_statistical_area
except ImportError:  # `python social_statistics.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from config import (
        COUNTRY_ISO3,
        SOCIAL_AREA_NAME_FIELDS,
        SOCIAL_INDICATORS,
        SOCIAL_LEVEL_SOURCES,
        SOCIAL_NO_AREA_NAME,
    )
    from db import load_social_rows, load_statistical_area

# The admin level an indicator asks for, and which of 1.2's names answers it. Level 0 is
# country-wide and needs no name at all.
_LEVEL_KEYS = {
    0: None,
    1: "dominant_province",
    2: "dominant_district",
    3: "dominant_subdistrict",
}

# Fields whose absence is expected rather than a fault: the country simply has no such table.
# Listing them keeps the flag honest -- "Brunei publishes no household count" is a data fact, and
# repeating it on every request as if it were a failure would train readers to ignore flags.
_KNOWN_ABSENT = {
    "brn": ("household_number",),
    "idn": ("household_number", "students_enrolled_number"),
    "vnm": ("household_number", "permanent_reserved_forest_area"),
    "sgp": ("permanent_reserved_forest_area",),
    # Withdrawn by the data team 2026-08-08: there is no Philippine education-level data. The
    # table that carried the name held Vietnamese provinces.
    "phl": ("population_education_levels",),
}


def _section_level(fields: dict) -> int:
    """The admin level a section names itself at: the one most of its indicators use.

    Five sections draw on two levels at once -- Brunei's employment is provincial except for its
    industry breakdown, Cambodia's is national except for its sectors, Laos' housing mixes the two
    -- and the contract gives each section a single area name. Naming it after the level that
    carries most of the section is the least misleading of the available answers, and every
    indicator's own area is recorded per field in `results` regardless.

    Ties go to the COARSER level, because overstating how local a figure is would be the worse
    error of the two.
    """
    counts = Counter(f['level'] for f in fields.values())
    return min(counts, key=lambda level: (-counts[level], level))


def _percentage_of(rows: list[tuple], exclude: tuple) -> list[dict]:
    """[{id, percentage}] for each category.

    A table already published in percent keeps its own numbers, so the values are what the
    statistics office stated. A table published in counts is converted to each category's share of
    the categories present. The two are not interchangeable: a percent table's rows need not sum
    to 100 (Malaysia publishes one facility type and nothing else), and forcing them to would
    invent a denominator.
    """
    kept = [(c, u, v) for c, u, v in rows if c not in exclude]
    if not kept:
        return []

    if all(u == "percent" for _, u, _ in kept):
        return [{'id': c, 'percentage': v} for c, _, v in kept]

    total = sum(v for _, _, v in kept)
    return [
        {'id': c, 'percentage': (v / total * 100.0) if total else None}
        for c, _, v in kept
    ]


def _read(rows: list[tuple], kind: str, exclude: tuple):
    """Turn the rows of one indicator table into the value its contract field carries."""
    kept = [(c, u, v) for c, u, v in rows if c not in exclude]

    if kind == "value":
        # No rows is not zero. A missing indicator must not read as "measured, and it was none".
        return sum(v for _, _, v in kept) if kept else None

    if kind == "shares":
        return _percentage_of(rows, exclude)

    if kind == "category_values":
        return [{'id': c, 'value': v} for c, _, v in kept]

    if kind == "diseases":
        # `contagious_id` is null on purpose: no `*_top5_common_diseases` table carries
        # contagiousness in any column, and inferring it from a disease name would be a clinical
        # claim this tool is in no position to make.
        return [{'id': c, 'contagious_id': None} for c, _, v in kept]

    if kind in ("top_id", "top_value"):
        if not kept:
            return None
        category, _, value = max(kept, key=lambda r: r[2])
        return category if kind == "top_id" else value

    if kind == "coverage_pct":
        # `exclude` here names the categories that record the ABSENCE of the thing being counted,
        # so the denominator is every row and the numerator is the rest.
        total = sum(v for _, _, v in rows)
        return (sum(v for _, _, v in kept) / total * 100.0) if total else None

    raise ValueError(f"unknown read kind {kind!r}")


def analyze_social_statistics(aoi, admin_values: dict | None = None) -> tuple[dict, dict]:
    """Component 6.3. National statistics for the administrative area the AOI sits in.

    `admin_values` is 1.2's `results['values']`: it supplies the country and the dominant province,
    district and sub-district. Without it nothing can be looked up, because every table is keyed by
    administrative name and the AOI's names live in the GIS database, not in this one.

    The AOI is used only by countries in SOCIAL_LEVEL_SOURCES, whose statistics are published at a
    tier the boundary layer does not carry and so must be resolved against a second layer.
    """
    admin_values = admin_values or {}
    country = admin_values.get('dominant_country')
    flags: list[str] = []
    # ABSENCE: a table that does not exist and a name with no row are permanent answers, not
    # faults, so they report `failed` -- "this is the answer" -- and never a retry button. 6.3 is
    # where most of the absence in this tool lives: the whole romanisation crosswalk gap is here.
    absent: list[str] = []
    # Kept apart from `flags` because they never mean "something went wrong": see
    # pipeline.error_status. `flags` decides whether the card reports a problem, `notes` does not.
    notes: list[str] = []
    # A retry re-runs 1.2 as well, so a lookup that failed for want of an administrative name or
    # because the database was unreachable is worth a button. A name that simply has no row is not:
    # the crosswalk gap gives the same answer forever.
    retryable = False

    if not country:
        results = {
            'narrative': "",
            'tables': {},
            'values': {'country': None, 'iso3': None},
            # No `retryable` here: an AOI in open water resolves to no country every time. When
            # the country is missing because 1.2 CRASHED, `pipeline.after` marks it retryable --
            # it is the only caller that can see which of the two happened.
            'missing': ["6.3: no administrative area was resolved for this AOI, so no national "
                        "statistics could be looked up."],
        }
        return results, {}

    iso3 = COUNTRY_ISO3.get(country)
    if iso3 is None or iso3 not in SOCIAL_INDICATORS:
        results = {
            'narrative': "",
            'tables': {},
            'values': {'country': country, 'iso3': iso3},
            'missing': [f"6.3: no social statistics are loaded for {country}."],
        }
        return results, {}

    if admin_values.get('transboundary'):
        flags.append(
            f"6.3: the AOI crosses a national border; every figure below is for {country}, the "
            "country holding the largest share of it."
        )
    provinces = admin_values.get('provinces') or []
    if len(provinces) > 1:
        flags.append(
            f"6.3: the AOI spans {len(provinces)} provinces "
            f"({', '.join(provinces)}); province-level figures are for "
            f"{admin_values.get('dominant_province')}, the largest share."
        )

    # The name each admin level answers to, resolved once. Normally these are 1.2's, straight from
    # the boundary layer. A country in SOCIAL_LEVEL_SOURCES overrides some of them, because its
    # statistics are published at tiers the boundary layer does not line up with -- see the config
    # entry, which spells out the Philippine case.
    # `candidates` because one level can have more than one right answer: the Philippines has two
    # regional vintages and which one a table answers to is a property of the TABLE, not of the
    # AOI. Candidates are tried in order and the first that returns rows wins -- see the config
    # entry. Every other country has exactly one candidate per level.
    candidates = {level: ([admin_values.get(key)] if key and admin_values.get(key) else [])
                  for level, key in _LEVEL_KEYS.items()}
    area_sources = {level: "1.2" for level in _LEVEL_KEYS}

    for level, source in SOCIAL_LEVEL_SOURCES.get(iso3, {}).items():
        if isinstance(source, str):          # another of 1.2's names, one tier over
            name = admin_values.get(source)
            candidates[level] = [name] if name else []
            area_sources[level] = f"1.2's {source.removeprefix('dominant_')}"
            continue

        resolved: list[str] = []             # second boundary layers, resolved spatially
        for table, column in source:
            try:
                name = load_statistical_area(aoi, table, column)
            except Exception as e:
                # Type only: a psycopg2 message can carry the connection string, and this text is
                # rendered in a browser.
                flags.append(
                    f"6.3: {table} could not be read ({type(e).__name__}), so a level-{level} "
                    f"figure for {country} may be null. Falling back to 1.2's name would not help: "
                    "it names a different administrative tier and matches no row."
                )
                retryable = True
                continue
            if name and name not in resolved:
                resolved.append(name)
        candidates[level] = resolved
        area_sources[level] = " then ".join(t for t, _ in source)

    # The name to print when nothing was looked up at that level -- the preferred candidate.
    area_names = {level: (names[0] if names else None) for level, names in candidates.items()}

    # Both of the following are NOTES: they explain which area each figure describes. Nothing is
    # missing and nothing is degraded, and the second one fires on every single Philippine request.
    if len(candidates.get(1, [])) > 1:
        notes.append(
            f"6.3: this area sits in the part of {country} that was re-districted, so the "
            f"statistics tables disagree about which region it is in: {candidates[1][0]!r} in the "
            f"newer tables, {candidates[1][1]!r} in the older ones. Each figure below is labelled "
            "with the region its own table uses."
        )

    if iso3 in SOCIAL_LEVEL_SOURCES:
        notes.append(
            f"6.3: the statistics for {country} are published one administrative tier above the "
            f"boundary layer, so the areas below are not the ones the ADMINISTRATIVE BOUNDARIES "
            f"card names. Level 1 is {area_names[1]!r} and level 2 is {area_names[2]!r}, against "
            f"1.2's province {admin_values.get('dominant_province')!r} and district "
            f"{admin_values.get('dominant_district')!r}. Both are correct at their own tier."
        )

    sections: dict[str, dict] = {}
    values: dict[str, dict] = {}
    missing: dict[str, str] = {}   # field -> the area whose row was not found

    for section, fields in SOCIAL_INDICATORS[iso3].items():
        payload: dict = {}
        sources: dict = {}
        used: dict[int, str] = {}      # level -> the name this section's tables actually answered to

        for field, spec in fields.items():
            level = spec['level']
            names = candidates[level] if level else [None]

            if level and not names:
                payload[field] = None
                missing[field] = (f"no {_LEVEL_KEYS[level].removeprefix('dominant_')} from "
                                  f"{area_sources[level]}")
                continue

            # One candidate for every country but the Philippines, where the loop stops at the
            # first regional vintage this table recognises.
            for area_name in names:
                year, rows = load_social_rows(iso3, spec['table'], level, area_name, spec['where'])
                if rows:
                    break

            payload[field] = _read(rows, spec['read'], tuple(spec['exclude']))
            # The year belongs with the number. It is not always the same across a section -- see
            # load_social_rows -- and a rate with no year attached cannot be checked by anyone.
            # `area` is the name that ANSWERED, which on Negros is not always the preferred one.
            sources[field] = {'table': f"{iso3}_{spec['table']}",
                              'area': area_name or country, 'year': year}
            if level and rows:
                used.setdefault(level, area_name)

            if payload[field] in (None, []):
                missing[field] = (f"{iso3}_{spec['table']} has no row for "
                                  f"{' or '.join(repr(n) for n in names)}")

        # The area every figure in this section describes. Named per the country's own contract:
        # Indonesia spells out which level and names two, Singapore names none because all of its
        # tables are country-wide.
        #
        # `used` before `area_names`: on Negros the preferred region is NIR, but a section whose
        # tables predate the split answered to Region VI or VII, and the name shown has to be the
        # one its numbers actually describe. No Philippine section mixes the two vintages -- each
        # draws on a single table per level -- so one name per section stays correct.
        if iso3 not in SOCIAL_NO_AREA_NAME:
            name_fields = SOCIAL_AREA_NAME_FIELDS.get(iso3, {}).get(
                section, {'administrative_area_name': _section_level(fields)}
            )
            for name_field, level in name_fields.items():
                payload[name_field] = (used.get(level) or area_names[level]) if level else country

        years = {s['year'] for s in sources.values() if s['year'] is not None}
        # A NOTE: every figure is the newest its own table publishes, which is correct, not partial.
        if len(years) > 1:
            notes.append(
                f"6.3: the {section} figures are not all from the same year "
                f"({', '.join(str(y) for y in sorted(years))}); each field's year is in "
                "`values` because the newest published year differs between these tables."
            )

        sections[section] = payload
        values[section] = sources

    known_absent = _KNOWN_ABSENT.get(iso3, ())
    if known_absent:
        absent.append(
            f"6.3: {country} publishes no {', '.join(known_absent)}; the field is null because "
            "the table does not exist, not because the value is zero."
        )

    # Anything else that came back empty is a LOOKUP failure, not an absent table, and almost
    # always the same cause: the boundary layer and the statistics office spell the same
    # administrative unit differently. Naming the table and the exact string that missed is what
    # makes that fixable -- "lao_number_of_households has no row for 'Attapu'" points straight at
    # the crosswalk entry needed, where "no data" would not.
    for field, reason in sorted(missing.items()):
        if field not in known_absent:
            absent.append(f"6.3: {field} is null -- {reason}.")

    results = {
        'narrative': "",
        'tables': {},
        'values': dict(values, country=country, iso3=iso3),
        'flags': flags,
        'missing': absent,
        'notes': notes,
        'retryable': retryable,
    }

    # The contract nests these under People rather than flattening them, because a section is what
    # the frontend renders as one card and several sections repeat field names across countries.
    return results, sections


if __name__ == "__main__":
    # Run this component on its own. Needs both databases, not the Flask app:
    #     python social_statistics.py [aoi path]
    import json
    import os
    import pathlib
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    # 1.2 lives in the General package and supplies the names every lookup here is keyed by.
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "general"))
    from administrative_boundaries import analyze_admin_boundaries

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    admin_results, _ = analyze_admin_boundaries(aoi)
    results, view_results = analyze_social_statistics(aoi, admin_results['values'])

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
