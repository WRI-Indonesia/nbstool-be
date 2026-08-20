"""
Endangered tree species richness.

Fills the endpoint's `endangered_tree_number_of_species`.

NOT A NOTEBOOK COMPONENT. The v3 Nature notebook has no endangered-tree section, so there is no
`analyze_*` body to keep identical. The logic is carried over from the V2 BACKEND instead, from
`get_nature_richness_data` in utils/geos/current_condition.py, which is the only existing
implementation and the one the frontend already renders. That function clips
`tree_species_richness.tif` to the AOI, calls `calculate_stats_pixel_value`, and reports
`round(stats[0])` -- and `stats[0]` is the MEAN of non-nodata pixels. Same layer in v3, published
as `tree_species_v3.tif`.

WHAT THE NUMBER MEANS, because the field name oversells it. The raster is species RICHNESS: each
pixel holds a count of endangered tree species occurring there. Averaging those counts gives the
typical richness at a point inside the AOI, so a value of 7 means "a point in this area typically
has 7 endangered tree species", NOT "there are 7 endangered tree species in this area". The
distinct-species count across the whole AOI would be the union of the species behind each pixel,
which a richness raster does not carry and cannot be recovered from. v2 has always reported the
mean under this name; this port keeps that rather than silently changing what the card means.

One deliberate difference from v2. v2 masks with `array != nodata` after `read(masked=True)`,
which double counts the nodata test and, when the raster declares no nodata at all, compares
against None and keeps everything. `load_raster_clipped` already returns a masked array covering
both nodata and outside-the-polygon, so the mean here is over exactly the AOI's valid pixels.
On a layer with a declared nodata value the two agree.
"""

from __future__ import annotations

import numpy as np

try:
    from ...common import AOI, load_raster_clipped
    from ...config import TREE_SPECIES_RASTER
except ImportError:  # `python endangered_trees.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, load_raster_clipped
    from config import TREE_SPECIES_RASTER


def analyze_endangered_trees(aoi: AOI) -> tuple[dict, dict]:
    """Endangered tree species richness over the AOI, as v2 reports it."""
    raster = load_raster_clipped(TREE_SPECIES_RASTER, aoi, resampling="average")

    if raster.valid_count == 0:
        results = {
            'narrative': "No endangered tree species data covers this project area.",
            'tables': {},
            'values': {'endangered_tree_richness': None, 'covered_ha': 0.0},
            'flags': [],
        }
        return results, {'endangered_tree_number_of_species': None}

    mean_richness = float(np.ma.mean(raster.values))
    reported = int(round(mean_richness))

    results = {
        # v2's wording, kept so the two backends do not describe the same number differently.
        'narrative': f"The project area is home to {reported:.0f} of endangered trees.",
        'tables': {},
        'values': {
            'endangered_tree_richness': mean_richness,   # unrounded, for anything downstream
            'endangered_tree_reported': reported,
            'covered_ha': raster.valid_area_ha,
        },
        'flags': [],
    }

    return results, {'endangered_tree_number_of_species': reported}


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python endangered_trees.py [aoi path]
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
    results, view_results = analyze_endangered_trees(aoi)

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    print(json.dumps(to_jsonable(results), indent=2, ensure_ascii=False))
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False))
