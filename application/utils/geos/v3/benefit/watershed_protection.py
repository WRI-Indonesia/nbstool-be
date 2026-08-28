"""
Component 5.8 Protection of watershed function (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, commit `6684860`, 2026-08-27). Script-style
cell hoisted per the 5.9-5.13 convention; the computation is the notebook's own.

A SIMPLIFIED NARRATIVE BENEFIT by design (the notebook's own markdown): per ecosystem present
on the site, a fixed watershed-function narrative plus its area, measured on the pathway
raster's ecosystem band (savanna folds into forest). The launch version is expected to bring
InVEST SDR/SWY modelling; until then this is the whole method.

Seams, output-identical: the notebook's `NAMES` is config's `ECOSYSTEM_NAMES` (same mapping);
the layer path goes through `layer_path`; band 2 is `PATHWAY_ECOSYSTEM_BAND`.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import rasterio
from pyproj import Geod
from rasterio.mask import mask

try:
    from ..common import AOI
    from ..config import (
        AOH_GDAL_OPTIONS,
        ECOSYSTEM_NAMES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )
    from ..settings import layer_path
except ImportError:  # `python watershed_protection.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import (
        AOH_GDAL_OPTIONS,
        ECOSYSTEM_NAMES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )
    from settings import layer_path

# The notebook's own names, verbatim (its `NAMES` is the same mapping as ECOSYSTEM_NAMES).
NAMES = ECOSYSTEM_NAMES

# ---- THE CELL'S NARRATIVES, verbatim (including the missing spaces the forest text joins
# across its f-string line breaks) ----
NARRATIVES = {
    "forest":
        "Safeguarding this forest ecosystem helps maintain the natural capacity "
        "of the landscape to capture, store, filter, and slowly release water."
        "Keeping the site under healthy natural cover support more stable river flow,"
        "lower erosion and sediment loads, and better water quality downstream.",

    "mangrove":
        "Safeguarding this mangrove ecosystem helps maintain hydrological "
        "connectivity between land and coast, supporting sediment retention, "
        "nutrient regulation and water-quality functions while stabilizing "
        "the coastal landscape.",

    "peatland":
        "Safeguarding this peatland ecosystem helps maintain natural water "
        "storage and water-table regulation. Protecting peat soils and "
        "hydrological connectivity can reduce drainage, erosion and sediment "
        "losses while supporting downstream water quality and more resilient "
        "watershed function."
}


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to describe -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def analyze_watershed_protection(aoi: AOI) -> tuple[dict, dict]:
    """Component 5.8. Watershed-function narrative and area per ecosystem present."""
    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # ---- THE CELL, verbatim from here (USER_AOI read -> prepared AOI) ----
        with rasterio.open(layer_path(PATHWAY_RASTER)) as src:
            aoi_r = aoi.geometry.to_crs(src.crs)
            eco, transform = mask(
                src,
                aoi_r.geometry,
                crop=True,
                filled=False,
                indexes=PATHWAY_ECOSYSTEM_BAND
            )

        eco = eco.filled(0).astype(int)
        eco[eco == 4] = 1  # Savanna -> Forest

        geod = Geod(ellps="WGS84")
        row_ha = []

        for row in range(eco.shape[0]):
            north = transform.f + row * transform.e
            south = north + transform.e
            west = transform.c
            east = west + transform.a

            area, _ = geod.polygon_area_perimeter(
                [west, east, east, west],
                [north, north, south, south]
            )

            row_ha.append(abs(area) / 10000)

        pixel_ha = np.broadcast_to(
            np.array(row_ha)[:, None],
            eco.shape
        )

        areas = {
            name: pixel_ha[eco == code].sum()
            for code, name in NAMES.items()
        }

        present = {
            name: area
            for name, area in areas.items()
            if area > 0
        }

    if not present:
        return _na(
            "No forest, mangrove or peatland ecosystem is mapped on this project area, so "
            "there is no watershed function to describe."
        )

    narrative = "\n\n".join(NARRATIVES[name] for name in present)

    values = {name: float(area) for name, area in areas.items()}
    results = {'narrative': narrative, 'tables': {}, 'values': values, 'flags': []}
    # The card contract: one narrative per present ecosystem, with its area behind it.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        'ecosystems': [{'name': name.title(), 'area_ha': float(area)}
                       for name, area in present.items()],
    }
    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python watershed_protection.py [aoi path]
    import json
    import os
    import sys
    import time

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    results, view_results = analyze_watershed_protection(aoi)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False, default=str))
