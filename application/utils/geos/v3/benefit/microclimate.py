"""
Component 5.13 Microclimate regulation (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, commits `83a3f2d`/`e5c80e5`, 2026-08-26).

NAMESPACE INTERPRETATION, surfaced: the cell is five statements that read `read`, `inside_aoi`,
`restore`, `change`, `pixel_ha`, `ECOSYSTEM` and `PROJECT_DURATION_YEARS` from EARLIER CELLS'
namespaces -- 5.11 most plausibly (its grid carries `restore` and `change`), with the ecosystem
layer read the way 5.9/5.10 read it. This port therefore rebuilds that state self-contained ON
THE PATHWAY GRID (5.11's), reads the threat ecosystem layer onto it, and then runs the cell's
own statements verbatim. `ECOSYSTEM_LABELS` does not exist in either config; the notebook's
`ECOSYSTEM_NAMES` (forest/mangrove/peatland) is the label source. Re-check when the notebook
makes the cell self-contained.

Projected tree-cover gain on the Restore area of one ecosystem: historical gain rate x duration,
capped at the Restore area itself.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import rasterio

try:
    from ..common import AOI
    from ..config import (
        AOH_GDAL_OPTIONS,
        ECOSYSTEM_CLASS,
        ECOSYSTEM_NAMES,
        FOREST_CHANGE_RASTER,
        FOREST_CHANGE_YEARS,
        GAIN_CODE,
        PATHWAY_RASTER,
        RESTORE_CODE,
        THREAT_ECOSYSTEM,
    )
    from ..settings import layer_path
    from .biodiversity_uplift import pixel_area, read_like, read_reference
except ImportError:  # `python microclimate.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from biodiversity_uplift import pixel_area, read_like, read_reference
    from common import AOI
    from config import (
        AOH_GDAL_OPTIONS,
        ECOSYSTEM_CLASS,
        ECOSYSTEM_NAMES,
        FOREST_CHANGE_RASTER,
        FOREST_CHANGE_YEARS,
        GAIN_CODE,
        PATHWAY_RASTER,
        RESTORE_CODE,
        THREAT_ECOSYSTEM,
    )
    from settings import layer_path

from rasterio.features import geometry_mask


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to measure -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def analyze_microclimate(aoi: AOI, duration_years: int,
                         ecosystem_class: int = ECOSYSTEM_CLASS) -> tuple[dict, dict]:
    """Component 5.13. Projected tree-cover gain on the ecosystem's Restore area."""
    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # State the cell inherits from 5.11: the pathway grid, restore mask, change layer.
        pathway, transform, crs, aoi_r = read_reference(
            layer_path(PATHWAY_RASTER), aoi.geometry, band=1)
        shape = pathway.shape
        inside_aoi = geometry_mask(aoi_r.geometry, out_shape=shape, transform=transform,
                                   invert=True)
        pixel_ha = pixel_area(transform, shape)
        restore = (inside_aoi
                   & ~np.ma.getmaskarray(pathway)
                   & (pathway.data == RESTORE_CODE))
        change = read_like(layer_path(FOREST_CHANGE_RASTER), transform, crs, shape)

        def read(path):
            return read_like(path, transform, crs, shape)

        # ---- THE CELL, verbatim from here ----
        ecosystem = read(layer_path(THREAT_ECOSYSTEM))

        ecosystem_mask = (
            inside_aoi
            & (ecosystem == ecosystem_class)
        )

        # Restore area within selected ecosystem
        restore_ecosystem = (
            restore
            & ecosystem_mask
        )

        restore_ecosystem_ha = pixel_ha[restore_ecosystem].sum()

        if restore_ecosystem_ha <= 0:
            return _na(
                f"This project area has no Restore-pathway "
                f"{ECOSYSTEM_NAMES.get(ecosystem_class, 'target')} area, so there is no "
                "tree-cover gain to project."
            )

        # Historical tree-cover gain within selected ecosystem
        gain_ecosystem = (
            restore_ecosystem
            & (change == GAIN_CODE)
        )
        # Refer to 5.11 section
        gain_10yr_ha = pixel_ha[gain_ecosystem].sum()

        # Annual gain as fraction of eligible Restore area
        gain_rate = (
            gain_10yr_ha
            / restore_ecosystem_ha
            / FOREST_CHANGE_YEARS
        )

        # Project gain
        projected_gain_ha = min(
            gain_rate
            * duration_years
            * restore_ecosystem_ha,
            restore_ecosystem_ha
        )

        projected_gain_pct = min(
            gain_rate
            * duration_years
            * 100,
            100
        )

        ecosystem_label = ECOSYSTEM_NAMES[ecosystem_class]

    narrative = (
        f"Restoring this {ecosystem_label} ecosystem, with an estimated "
        f"{projected_gain_ha:,.2f} ha ({projected_gain_pct:.1f}%) of tree cover increase, "
        f"could help regulate the local microclimate by moderating temperatures "
        f"through shading and evapotranspiration and buffering the surrounding area "
        f"against heat extremes over the project's {duration_years}-year duration."
    )

    values = {
        "duration_years": duration_years,
        "ecosystem": ecosystem_label,
        "restore_ecosystem_ha": float(restore_ecosystem_ha),
        "gain_10yr_ha": float(gain_10yr_ha),
        "gain_rate_pct_per_year": float(gain_rate * 100),
        "projected_gain_ha": float(projected_gain_ha),
        "projected_gain_pct": float(projected_gain_pct),
    }
    results = {'narrative': narrative, 'tables': {}, 'values': values, 'flags': []}
    # The card contract: projected tree-cover gain (headline) on the ecosystem's Restore area.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'ecosystem', 'duration_years', 'restore_ecosystem_ha',
            'projected_gain_ha', 'projected_gain_pct')},
    }
    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python microclimate.py [aoi path] [duration]
    import json
    import os
    import sys
    import time

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    results, view_results = analyze_microclimate(aoi, duration)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False, default=str))
