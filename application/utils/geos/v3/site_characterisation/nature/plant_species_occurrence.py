"""
Component 2.7 Plant Species Occurrence.

Which plant species have been recorded inside the project area, from GBIF occurrence points, and
how many of them are listed under CITES Appendix I, II or III.

Data. `sea.plant_occurrence` in the GIS database (PLANT_OCCURRENCE_TABLE): 584k GBIF plant
records across SEA, 33,645 species, with `scientificName`, `citesAppendix` and `year`. The
notebook (commit `19d86df`) reads the same data as layer `SEA_plant_occurrence` of a file
geodatabase; its own config comment names this table as the PostGIS home.

Occurrence is not absence, same caveat as 2.5: these are opportunistic records, so the list
evidences presence and is not a flora inventory of the site.

NOT `endangered trees`. That component reports a mean modelled richness from
`tree_species_v3.tif`; this one counts species actually recorded. Different question, different
source, different field -- both are emitted.

What this port changes at the seam, and nowhere else:
  - it takes the prepared AOI, and the points arrive ALREADY FILTERED TO THE AOI by
    `db.load_plants_intersecting` (PostGIS ST_Intersects, the notebook's own predicate) rather
    than reading the whole layer and filtering in memory. The notebook's CRS check and its
    `plants.geometry.intersects(aoi_geom)` term are therefore gone; the year and species terms of
    the same filter stay;
  - the loader pins row order, because `get_cites` takes the first appendix it meets;
  - the config constants carry a PLANT_ prefix (the notebook's SPECIES / CITES / YEAR /
    START_YEAR / END_YEAR are bare module globals), aliased back below so the body reads as
    published;
  - the printed report is dropped; the values are returned instead.
"""

from __future__ import annotations

import pandas as pd

try:
    from ...common import AOI
    from ...config import (
        PLANT_CITES,
        PLANT_END_YEAR,
        PLANT_SPECIES,
        PLANT_START_YEAR,
        PLANT_YEAR,
    )
    from ...db import load_plants_intersecting
except ImportError:  # `python plant_species_occurrence.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from config import (
        PLANT_CITES,
        PLANT_END_YEAR,
        PLANT_SPECIES,
        PLANT_START_YEAR,
        PLANT_YEAR,
    )
    from db import load_plants_intersecting

# The notebook's names for the same constants.
SPECIES = PLANT_SPECIES
CITES = PLANT_CITES
YEAR = PLANT_YEAR
START_YEAR = PLANT_START_YEAR
END_YEAR = PLANT_END_YEAR


# Species summary
def get_cites(values):
    return next(
        (x for x in values if x in {"I", "II", "III"}),
        "Not listed"
    )


def analyze_plant_species(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.7. Plant species recorded inside the project area, with their CITES listing."""
    plants = load_plants_intersecting(aoi)

    # Filter by AOI and year
    plants[YEAR] = pd.to_numeric(
        plants[YEAR],
        errors="coerce"
    )

    plants = plants[
        plants[YEAR].between(START_YEAR, END_YEAR)
        & plants[SPECIES].notna()
    ].copy()

    # Clean attributes
    plants[SPECIES] = plants[SPECIES].astype(str).str.strip()

    plants[CITES] = (
        plants[CITES]
        .fillna("")
        .astype(str)
        .str.strip()
        .str.upper()
        .replace({
            "APPENDIX I": "I",
            "APPENDIX II": "II",
            "APPENDIX III": "III",
            "1": "I",
            "2": "II",
            "3": "III",
        })
    )

    species_table = (
        plants.groupby(SPECIES)
        .agg(
            citesAppendix=(CITES, get_cites),
            occurrence_count=(SPECIES, "size")
        )
        .reset_index()
        .sort_values(SPECIES)
    )

    total_species = len(species_table)

    counts = (
        species_table["citesAppendix"]
        .value_counts()
    )

    appendix_i = counts.get("I", 0)
    appendix_ii = counts.get("II", 0)
    appendix_iii = counts.get("III", 0)

    # Narrative
    narrative = (
        f"Based on GBIF occurrence records, the project area recorded occurrences "
        f"of {total_species:,} plant species between {START_YEAR} and {END_YEAR}. "
        f"Among these, {appendix_i:,} species are listed under CITES Appendix I, "
        f"{appendix_ii:,} under Appendix II, and {appendix_iii:,} under Appendix III."
    )

    species_rows = [
        {'scientific_name': str(row[SPECIES]),
         'cites_appendix': str(row["citesAppendix"]),
         'occurrence_count': int(row["occurrence_count"])}
        for row in species_table.to_dict(orient="records")
    ]

    results = {
        'narrative': narrative,
        'tables': {'species': species_rows},
        'values': {
            'total_species': int(total_species),
            'appendix_i': int(appendix_i),
            'appendix_ii': int(appendix_ii),
            'appendix_iii': int(appendix_iii),
            'record_count': int(len(plants)),
            'start_year': START_YEAR,
            'end_year': END_YEAR,
        },
        'flags': [],
    }

    # The PLANT SPECIES OCCURRENCE card: the four counts, the period they cover, and the list.
    view_results = {
        'plant_number_of_species': int(total_species),
        'plant_cites_appendix_i': int(appendix_i),
        'plant_cites_appendix_ii': int(appendix_ii),
        'plant_cites_appendix_iii': int(appendix_iii),
        'plant_period': {'from': START_YEAR, 'to': END_YEAR},
        'plant_species': species_rows,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own. Needs the GIS database, not the Flask app:
    #     python plant_species_occurrence.py [aoi path]
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
    results, view_results = analyze_plant_species(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", {**results, 'tables': {'species': results['tables']['species'][:8]}})
    print()
    dump("view_results", {**view_results, 'plant_species': view_results['plant_species'][:8]})
