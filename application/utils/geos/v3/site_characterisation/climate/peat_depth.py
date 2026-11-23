"""
Component Peat Depth (F02-P2 Climate, notebook commit `139dea8`; the section carries no number).

How much of the project area is peatland, how deep the peat is, and how the peat splits between
shallow, medium and deep.

Data. `PEAT_DEPTH_RASTER` (`peat_depth_54034_v3.tif`): PEATGRIDS peat thickness in CENTIMETRES,
float32, ESRI:54034, about 1 km per pixel (981 m), nodata outside peat. 160,747 peat pixels across
SEA, 2 to 1,007 cm.

    Shallow  under 100 cm      Medium  100 to 300 cm      Deep  over 300 cm

The shares are OF THE PEATLAND, not of the project area: the three add up to 100.

ONE KILOMETRE PIXELS. A pixel counts when its centre is inside the polygon, and each one is 96 ha,
so on a small site the whole answer rests on a handful of pixels and the peat area moves in 96 ha
steps.

THE NARRATIVE'S UNIT IS THE NOTEBOOK'S. It prints the raw range as "meters" while the values and
its own class sentences are centimetres ("less than 100 cm"). Carried verbatim and reported
upstream; the endpoint's fields state centimetres.

The body is the notebook's cell. What changed at the seam, and nowhere else:
  - it takes the prepared AOI instead of reading `USER_POLYGON`, and the raster is opened through
    `settings.layer_path`;
  - NO PEAT IS AN ANSWER. The cell raises ValueError when the polygon holds no peat pixel, and
    `rasterio.mask` raises when the polygon misses the raster altogether. Most sites have no
    peat, so both return an empty result with the reason in `missing`, never a crashed card;
  - the prints are dropped and the values returned.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import rasterio
from rasterio.mask import mask

try:
    from ...common import AOI
    from ...config import PEAT_DEPTH_RASTER
    from ...settings import layer_path
except ImportError:  # `python peat_depth.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from config import PEAT_DEPTH_RASTER
    from settings import layer_path

_NO_PEAT = "No peat depth data found within the project area."


def _no_peat() -> tuple[dict, dict]:
    results = {'narrative': _NO_PEAT, 'tables': {'peat_depth': []}, 'values': {},
               'flags': [], 'missing': [_NO_PEAT]}
    view_results = {'peat_area_ha': 0.0, 'peat_depth_min_cm': None, 'peat_depth_max_cm': None,
                    'peat_depths': []}
    return results, view_results


def analyze_peat_depth(aoi: AOI) -> tuple[dict, dict]:
    """Peat depth. Peatland area, depth range, and the shallow / medium / deep split."""
    # SEAM: the notebook reads a hardcoded `USER_POLYGON`.
    aoi = gpd.GeoDataFrame(geometry=aoi.geometry)

    with rasterio.open(layer_path(PEAT_DEPTH_RASTER)) as src:

        aoi = aoi.to_crs(src.crs)

        try:
            peat, transform = mask(
                src,
                aoi.geometry,
                crop=True,
                nodata=0,
                filled=True
            )
        except ValueError:      # the polygon does not reach the raster at all
            return _no_peat()

    peat = peat[0]

    pixel_area_ha = (
        abs(transform.a * transform.e)
        / 10000
    )

    # =============================================================================
    # VALID PEAT
    # =============================================================================

    valid = peat > 0

    if not np.any(valid):
        return _no_peat()       # the notebook raises ValueError here

    # =============================================================================
    # DEPTH AND CLASSES
    # =============================================================================

    min_depth = peat[valid].min()
    max_depth = peat[valid].max()

    shallow = valid & (peat < 100)
    medium = valid & (peat >= 100) & (peat <= 300)
    deep = valid & (peat > 300)

    def area_ha(mask):
        return np.count_nonzero(mask) * pixel_area_ha

    peatland_ha = area_ha(valid)

    shallow_ha = area_ha(shallow)
    medium_ha = area_ha(medium)
    deep_ha = area_ha(deep)

    shallow_pct = shallow_ha / peatland_ha * 100
    medium_pct = medium_ha / peatland_ha * 100
    deep_pct = deep_ha / peatland_ha * 100

    # =============================================================================
    # NARRATIVE
    # =============================================================================

    narrative = (
        f"This area contains {peatland_ha:,.2f} ha of peatland, "
        f"with peat depth ranging from {min_depth:.2f} to {max_depth:.2f} meters."
    )

    if shallow_ha > 0:
        narrative += (
            "\n\n"
            f"{shallow_pct:.2f}% of peatland within the project area has shallow peat "
            f"with a thickness of less than 100 cm. These areas may be suitable for "
            f"crop cultivation with traditional systems and wet-tolerant crop species "
            f"to minimize peat degradation and fire risk."
        )

    if medium_ha > 0:
        narrative += (
            "\n\n"
            f"{medium_pct:.2f}% of peatland within the project area has medium peat "
            f"with a thickness ranging from 100 to 300 cm. These areas may be cultivated "
            f"with sustainable water management and soil conservation to enhance peat "
            f"ecosystem services."
        )

    if deep_ha > 0:
        narrative += (
            "\n\n"
            f"{deep_pct:.2f}% of peatland within the project area has deep peat deposits "
            f"exceeding 300 cm. These areas should be conserved to preserve existing "
            f"carbon stocks, maintain hydrological functions, and sustain ecosystem services."
        )

    rows = [
        {'id': 'shallow', 'name': "Shallow", 'area': float(shallow_ha),
         'percentage': float(shallow_pct)},
        {'id': 'medium', 'name': "Medium", 'area': float(medium_ha),
         'percentage': float(medium_pct)},
        {'id': 'deep', 'name': "Deep", 'area': float(deep_ha), 'percentage': float(deep_pct)},
    ]

    results = {
        'narrative': narrative,
        'tables': {'peat_depth': rows},
        'values': {
            'peatland_ha': float(peatland_ha),
            'min_depth': float(min_depth),
            'max_depth': float(max_depth),
            'pixel_area_ha': float(pixel_area_ha),
            'peat_pixels': int(np.count_nonzero(valid)),
        },
        'flags': [],
    }

    # The PEAT DEPTH card: the peat area, its depth range in centimetres, and three class rows
    # whose percentages are shares of the peatland.
    view_results = {
        'peat_area_ha': float(peatland_ha),
        'peat_depth_min_cm': float(min_depth),
        'peat_depth_max_cm': float(max_depth),
        'peat_depths': rows,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python peat_depth.py [aoi path]
    import json
    import os
    import sys

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = (sys.argv[1] if len(sys.argv) > 1
                else r"D:\Documents\ALL\_test\nbs\muara_merang_4326.zip")
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    _aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_peat_depth(_aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {_aoi.area_ha:,.0f} ha, supplied in {_aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
