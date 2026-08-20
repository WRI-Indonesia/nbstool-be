"""
Component 2.5 Key Species Presence.

Which key species have been recorded inside the project area, from GBIF occurrence points, with
a per species summary and a count per taxonomic class.

Data. `sea.key_species` in the GIS database: GBIF occurrence records with Darwin Core columns.
The notebook reads the same data as `key_species.shp` off a UNC share and a hardcoded dummy AOI.

THIS IS ITS OWN CARD, separate from 2.3, and the two share no field. The design has them side by
side answering different questions:

    HABITAT AREA                     "suitable habitat for a wide range of wildlife"
                                     four `*_number_of_species` counts + a total, from 2.3's
                                     modelled AoH stacks
    INDICATIVE KEY SPECIES PRESENCE  "keystone species throughout the project area"
                                     this component: a species list grouped by class, each with
                                     its occurrence count

So the counts are NOT emitted here. They belong to 2.3. An earlier version merged the two into one
list; the team's design keeps them apart, because "could live here" and "was recorded here" are
different claims and a reader has to be able to tell which they are looking at.

COVERAGE, and it is a real limit. The table holds Aves, Mammalia and Reptilia only -- 50, 90 and 7
species respectively across the whole of SEA. There are no Amphibia rows at all, so no amphibian is
ever recorded here; that is why the card's three headings are Aves, Mammalia and Reptilia.

Occurrence is not absence. These are opportunistic GBIF records, so a species missing from this
list may simply never have been recorded here. The list evidences presence; it does not evidence
absence, and it is not a species inventory of the site.

NO COMMON NAMES. The design shows "Bali Myna (Leucopsar rothschildi)". `sea.key_species` carries
`species`, `scientificName` and `verbatimScientificName` and no vernacular column at all, so the
common name cannot be filled from this source.

What this port carries over, and what it drops. The notebook's 2.5 is a top-level script, not an
`analyze_*(aoi)` function, so there was no function to keep identical. The AGGREGATION is carried
over exactly: group by species, sum `individualCount`, count records, take the latest `eventDate`
and the most frequent `basisOfRecord`. Two things are dropped, both because they exist only to
cope with reading a shapefile off disk:
  - `resolve_column`, which recovers Darwin Core names from a shapefile's 10-character
    truncation. The database stores the full names, so there is nothing to resolve.
  - the CRS-alignment and dissolve-then-sjoin block, which PostGIS does in the WHERE clause.
The script's `raise SystemExit` on an empty result is also dropped: no species recorded is a real
answer for a screening tool, not a reason to stop.
"""

from __future__ import annotations

from dataclasses import dataclass

import pandas as pd

try:
    from ...common import AOI
    from ...config import KEY_SPECIES_CLASS_FIELDS
    from ...db import load_key_species_intersecting
except ImportError:  # `python key_species_presence.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from config import KEY_SPECIES_CLASS_FIELDS
    from db import load_key_species_intersecting


@dataclass(frozen=True)
class SpeciesRecord:
    """One species summarised over its occurrence points inside the AOI."""

    species: str
    taxon_class: str
    family: str
    total_occurrence: float   # summed individualCount; 0 when every record omits a count
    record_count: int         # number of occurrence rows, always >= 1
    latest_encounter: str | None
    basis_of_record: str | None


def most_frequent(series):
    """The modal value, or None when every entry is null. Verbatim from the notebook."""
    modes = series.dropna().mode()
    return modes.iat[0] if not modes.empty else None


def _view(species: list[SpeciesRecord]) -> dict:
    """The INDICATIVE KEY SPECIES PRESENCE card: one entry per species recorded here.

    THE COUNTS ARE NOT HERE. `*_number_of_species` belongs to the HABITAT AREA card and is filled
    by 2.3 from the AoH stacks. This card groups its own list by class and shows an occurrence
    count per species, so it needs no totals of its own and shares no field with 2.3.

    Contract shape. `species_id` and `family_id` carry NAMES, not surrogate keys: GBIF's numeric
    taxonKey/speciesKey are in the table, but the frontend renders these strings directly and a key
    would need a lookup that does not exist yet.

    `family_id` is the CLASS -- Aves, Mammalia, Reptilia -- not the Darwin Core family. The card
    groups by exactly those three headings. The true family (Varanidae, Cercopithecidae,
    Accipitridae, ...) is a level too fine to group on -- AOI1 alone spans a dozen -- and stays on
    `results['tables']['species']`.

    `number_of_occurences` is the RECORD COUNT, not the summed individualCount. GBIF calls one row
    an occurrence, and individualCount is null on most records here (summing it reports 47
    individuals across 1,277 sightings, which reads as far rarer than the species is).
    `total_occurrence` stays in `results` for anything that wants the individual tally.
    """
    return {'key_species': [
        {'species_id': r.species,
         'number_of_occurences': r.record_count,
         'family_id': r.taxon_class}
        for r in species
    ]}


def analyze_key_species(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.5. Key species recorded inside the project area."""
    points = load_key_species_intersecting(aoi)

    # Records with no species name cannot be grouped meaningfully.
    if not points.empty:
        points = points.dropna(subset=["species"])

    if points.empty:
        results = {
            'narrative': "No key species occurrence records fall inside this project area.",
            'tables': {'species': []},
            'values': {'species_count': 0, 'record_count': 0},
            'flags': [],
        }
        return results, _view([])

    # individualCount -> numeric (nulls become NaN and are ignored by sum).
    points["individual_count"] = pd.to_numeric(points["individual_count"], errors="coerce")
    # eventDate -> datetime so "max" is chronological, not lexical.
    # utc=True is not cosmetic: GBIF eventDate mixes offset-aware ("...T16:32+07:00") and naive
    # ("...T16:32") strings in the same column, which parses to an object column that `max`
    # refuses to reduce ("can't compare offset-naive and offset-aware datetimes"). Normalising
    # to UTC gives one dtype and makes "latest" well defined across records from any timezone.
    points["event_date"] = pd.to_datetime(
        points["event_date"], errors="coerce", format="mixed", utc=True
    )

    grouped = (
        points
        .groupby("species")
        .agg(
            taxon_class=("taxon_class", most_frequent),
            family=("family", most_frequent),
            total_occurrence=("individual_count", "sum"),
            record_count=("species", "count"),
            latest_encounter=("event_date", "max"),
            basis_of_record=("basis_of_record", most_frequent),
        )
        .reset_index()
        .sort_values(["record_count", "species"], ascending=[False, True])
    )

    species = [
        SpeciesRecord(
            species=str(r.species),
            taxon_class=str(r.taxon_class) if r.taxon_class else "",
            family=str(r.family) if r.family else "",
            total_occurrence=float(r.total_occurrence) if pd.notna(r.total_occurrence) else 0.0,
            record_count=int(r.record_count),
            latest_encounter=(
                r.latest_encounter.strftime("%d %B %Y") if pd.notna(r.latest_encounter) else None
            ),
            basis_of_record=str(r.basis_of_record) if r.basis_of_record else None,
        )
        for r in grouped.itertuples(index=False)
    ]

    # Kept in `results` for narratives and checking, NOT emitted: the endpoint's
    # `*_number_of_species` fields are the HABITAT AREA card and 2.3 owns them.
    counts = {field: 0 for field in KEY_SPECIES_CLASS_FIELDS.values()}
    for record in species:
        field = KEY_SPECIES_CLASS_FIELDS.get(record.taxon_class)
        if field:
            counts[field] += 1

    unmapped = sorted({r.taxon_class for r in species if r.taxon_class not in KEY_SPECIES_CLASS_FIELDS})
    flags: list[str] = []
    if unmapped:
        # The card groups by class, so a class with no heading would render loose or not at all.
        flags.append(
            f"2.5: {', '.join(unmapped)} recorded in the AOI but the card has no heading for that "
            "class, so those species may not render."
        )

    named = ", ".join(r.species for r in species[:5])
    more = f", and {len(species) - 5} more" if len(species) > 5 else ""
    results = {
        'narrative': (
            f"{len(species)} key species have been recorded in this project area, including "
            f"{named}{more}."
        ),
        'tables': {'species': species},
        'values': {
            'species_count': len(species),
            'record_count': int(sum(r.record_count for r in species)),
            'class_counts': counts,          # recorded occurrences only, not the endpoint's counts
        },
        'flags': flags,
    }

    return results, _view(species)


if __name__ == "__main__":
    # Run this component on its own. Needs the GIS database, not the Flask app:
    #     python key_species_presence.py [aoi path]
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_key_species(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
