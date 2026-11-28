"""
Component 4.4 Activities by IUCN Conservation Actions Classification Scheme (notebook `3d99e09`,
summary by activity since `8d2b0d4`).

For the charismatic species whose Area of Habitat covers MORE THAN 90% of the project area: which
IUCN conservation actions apply, how many species share each action, and the actions per species.

IT IS A LOOKUP ON 2.3's SPECIES TABLE, not a spatial analysis and not a join to the NbS activity
catalog. Despite the heading, nothing here touches 4.2's activities: "Conservation Activities" is
a free-text column of the workbook.

Data. `CONSERVATION_FILE`, one sheet: Scientific name, Class, code, Conservation Action,
Conservation Activities, Details Conservation Activities, redlistCategory, Action Relevance.
582 rows, 126 species, 16 codes, each with one action name and one activity sentence.

SINCE NOTEBOOK `8d2b0d4` THE SUMMARY IS BY ACTIVITY: grouped on code + "Conservation Activities"
and `conservation_action_count` counts distinct ACTIVITY sentences. Codes can share a sentence
(3.1.1 Harvest management and 3.1.2 Trade management do), so the count can be smaller than the
number of rows. The cell's per-species list and its duplicate check still use the action NAME;
only that table's printed heading was renamed. Followed as published.

STAGE-FED. The notebook's cell reads `result`, the species table that the NATURE notebook's 2.3
cell leaves in the kernel -- so it is a NameError in a fresh Pathway kernel as published. Here it
is a dependent of `habitat area` in the union stream (run_analysis) and receives that component's
output; it reads no raster of its own.

THE >90% CUT MAKES THIS EMPTY ON MOST SITES. A species must have suitable habitat over more than
90% of the AOI AND be in the charismatic workbook. Zero species is a real answer.

THE SPECIES MATCHED DEPEND ON WHICH AoH INVENTORY 2.3 READS. On the default `habitat_area` set
59 of the workbook's 126 species exist at all; on `habitat_area_v3` 120 do.

The body is the notebook's cell. What changed at the seam, and nowhere else:
  - `result` is rebuilt from 2.3's `tables.species`, with `class` renamed to the `Class` the cell
    reads; the workbook is read through `settings.layer_path` and cached per process;
  - `code` is read as the cell reads it and turned to TEXT ONLY ON THE WAY OUT. The sheet mixes
    numeric cells (2.1) with text ones ("3.1.1"); pandas sorts such a column numbers first, then
    text, so within a species 4.3 comes BEFORE 3.1.1. That is the notebook's published order and
    it is kept. Only the emitted value is stringified, so the wire carries one type;
  - the two display frames built only for `print` are dropped, with the prints;
  - a 2.3 run that could not read its inventory, or lost some species rasters, makes this
    component say so and offer a retry instead of reporting a confident zero.
"""

from __future__ import annotations

import io
import urllib.request
from functools import lru_cache

import pandas as pd

try:
    from ..common import AOI
    from ..config import CONSERVATION_FILE, OVERLAP_THRESHOLD
    from ..settings import layer_path
except ImportError:  # `python conservation_actions.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import CONSERVATION_FILE, OVERLAP_THRESHOLD
    from settings import layer_path


@lru_cache(maxsize=4)
def _read_conservation(path: str) -> pd.DataFrame:
    """The workbook, once per process and per resolved path. Callers copy before mutating."""
    if path.startswith("http"):
        with urllib.request.urlopen(path, timeout=60) as response:
            source = io.BytesIO(response.read())
    else:
        source = path
    return pd.read_excel(source)


# =============================================================================
# NORMALIZE SPECIES NAME
# =============================================================================

def species_key(value):
    return (
        str(value)
        .strip()
        .lower()
        .replace("_", " ")
    )


def analyze_conservation_actions(aoi: AOI, habitat: tuple[dict, dict]) -> tuple[dict, dict]:
    """Component 4.4. `habitat` is 2.3's `(results, view_results)`."""
    habitat_results, _ = habitat
    species_rows = (habitat_results.get('tables') or {}).get('species')

    if species_rows is None or not habitat_results.get('values') \
            or habitat_results.get('retryable'):
        # 2.3 raised, could not read its inventory, or lost species rasters to the network. A
        # lookup on that table would report "no charismatic species" as if it were a finding.
        # The view is EMPTY, not a null field: an empty `data` is never persisted, so a failed
        # retry cannot overwrite a result an earlier run stored.
        return {
            'narrative': "", 'tables': {}, 'values': {}, 'flags': [],
            'missing': ["4.4: the habitat area analysis did not complete, so no conservation "
                        "actions can be listed."],
            'retryable': True,
        }, {}

    # SEAM: the notebook's `result` is 2.3's table, which spells the column `Class`.
    result = pd.DataFrame(
        species_rows, columns=["species", "class", "iucn_status", "percentage_aoi"]
    ).rename(columns={"class": "Class"})

    conservation = _read_conservation(layer_path(CONSERVATION_FILE)).copy()

    # Remove old column if it still exists
    conservation = conservation.drop(
        columns=["Conservation Action Count"],
        errors="ignore"
    )

    result["_key"] = (
        result["species"]
        .map(species_key)
    )

    conservation["_key"] = (
        conservation["Scientific name"]
        .map(species_key)
    )

    # =============================================================================
    # SPECIES WITH >90% HABITAT OVERLAP
    # =============================================================================

    over_90 = result[
        result["percentage_aoi"] > OVERLAP_THRESHOLD
    ].copy()

    # =============================================================================
    # MATCH CHARISMATIC SPECIES
    # =============================================================================

    species_actions = over_90.merge(
        conservation,
        on="_key",
        how="inner",
        suffixes=("", "_conservation")
    )

    # Remove duplicate species-action combinations
    species_actions = species_actions.drop_duplicates(
        subset=[
            "species",
            "code",
            "Conservation Action"
        ]
    )

    # =============================================================================
    # KEEP OUTPUT FIELDS
    # =============================================================================

    species_actions = species_actions[
        [
            "species",
            "Class",
            "iucn_status",
            "percentage_aoi",
            "code",
            "Conservation Action",
            "Conservation Activities"
        ]
    ].sort_values(
        [
            "species",
            "code"
        ]
    ).reset_index(drop=True)

    # =============================================================================
    # PROJECT-LEVEL CONSERVATION ACTION SUMMARY
    # =============================================================================

    action_summary = (
        species_actions
        .groupby(
            [
                "code",
                "Conservation Activities"
            ],
            as_index=False
        )
        .agg(
            species_count=(
                "species",
                "nunique"
            ),
            species=(
                "species",
                lambda x: ", ".join(
                    sorted(set(x))
                )
            )
        )
        .sort_values(
            [
                "species_count",
                "code"
            ],
            ascending=[
                False,
                True
            ]
        )
        .reset_index(drop=True)
    )

    # =============================================================================
    # SUMMARY
    # =============================================================================

    charismatic_species_count = (
        species_actions["species"]
        .nunique()
    )

    conservation_action_count = (
        species_actions["Conservation Activities"]
        .nunique()
    )

    # 1. Conservation action summary: one row per code, the species sharing it. The cell's
    #    summary carries the activity sentence; the action NAME is kept beside it (one per code
    #    in the workbook) so the field the wire already had does not disappear.
    action_names = species_actions.drop_duplicates("code").set_index("code")["Conservation Action"]
    actions = [
        {'code': str(row["code"]),
         'conservation_action': action_names[row["code"]],
         'conservation_activity': row["Conservation Activities"],
         'species_count': int(row["species_count"]),
         'species': [name.replace("_", " ") for name in row["species"].split(", ")]}
        for row in action_summary.to_dict(orient="records")
    ] if len(action_summary) else []

    # 2. Conservation actions grouped by species, first-seen order as the notebook's
    #    `dict.fromkeys`. Names cleaned for display, percentage left as measured.
    species = [
        {'scientific_name': name.replace("_", " "),
         'taxon_class': taxon_class,
         'iucn_status': iucn_status,
         'percentage_aoi': float(percentage_aoi),
         'conservation_actions': list(dict.fromkeys(group["Conservation Action"]))}
        for (name, taxon_class, iucn_status, percentage_aoi), group in species_actions.groupby(
            [
                "species",
                "Class",
                "iucn_status",
                "percentage_aoi"
            ]
        )
    ]

    results = {
        'narrative': "",
        'tables': {'species_actions': species_actions.astype(object)
                   .where(species_actions.notna(), None).to_dict(orient="records")},
        'values': {
            'charismatic_species_count': int(charismatic_species_count),
            'conservation_action_count': int(conservation_action_count),
            'over_90_species_count': int(len(over_90)),
            'overlap_threshold': OVERLAP_THRESHOLD,
        },
        'flags': [],
    }

    view_results = {'conservation_actions': {
        'overlap_threshold': OVERLAP_THRESHOLD,
        'charismatic_species_count': int(charismatic_species_count),
        'conservation_action_count': int(conservation_action_count),
        'actions': actions,
        'species': species,
    }}

    return results, view_results


if __name__ == "__main__":
    # Run 2.3 on a file, then this component on its output, no Flask app:
    #     python conservation_actions.py [aoi path] [overlap threshold, to see a non-empty result]
    import json
    import os
    import sys

    import geopandas as gpd

    _here = pathlib.Path(__file__).resolve()
    sys.path.insert(0, str(_here.parents[1] / "site_characterisation" / "nature"))
    from common import prepare_aoi, to_jsonable
    from habitat_area import analyze_habitat_area

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if len(sys.argv) > 2:
        OVERLAP_THRESHOLD = float(sys.argv[2])
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    _aoi = prepare_aoi(gpd.read_file(aoi_path))
    _results, _view = analyze_conservation_actions(_aoi, analyze_habitat_area(_aoi))

    print(f"AOI: {_aoi.area_ha:,.0f} ha, threshold > {OVERLAP_THRESHOLD}%\n", file=sys.stderr)
    print(json.dumps(to_jsonable(_view), indent=2, ensure_ascii=False))
    print(json.dumps(to_jsonable(_results['values']), indent=2), file=sys.stderr)
