"""
Component 3.7 Historical Burned Area.

How much of the AOI has burned over the GABAM record, 2014 to 2024, as an annual series (bar
chart) and one burned footprint.

Data. GABAM annual burned maps (`GABAM_RASTER_TEMPLATE`, `gabam/GABAM_<year>.tif` in the v3
bucket), one binary raster per year, value 1 = burned and 0 = NODATA, ~30 m, EPSG:4326. Eleven
years, 2014 to 2024.

TWO TOTALS, ON PURPOSE. A pixel can burn in more than one year, so:
- The headline is the UNION: the area that burned at least once. It is always at most the AOI
  area and reads as the true burned footprint. It fills the contract's `total_burned_area`.
- The bar chart is PER YEAR: a pixel that reburns is counted in each year it burns, which is what
  an annual series should show. It fills `historical_burned_areas`.
- The SUM of the annual areas is also kept in `values` (`burned_sum_ha`). It can exceed the AOI
  area and measures burn events, not footprint, so it must not be read as an area.

Decisions locked (the notebook's).
- Burned = the valid (non nodata) pixels, since GABAM encodes 0 as nodata.
- All years are aligned to the first year's grid (`like=`), so the union is an exact pixel-wise OR.
- Nearest resampling only, never interpolate a burned or not-burned flag.

History. Until 2026-08-22 this slot was filled by `burned_area.py`, a port of v2's
`get_climate_burned_area_data` over the MODIS layers under `assets-geo/baseline/` (2011-2020,
verified exact against v2), because the notebook had no burned-area section then. The notebook
gained 3.7 (commit `8144cc8`), the GABAM rasters reached the bucket, and the team switched this
component to it. Different product, different record, different resolution -- the numbers moved
by design. See git history of burned_area.py for the v2 parity version.

The port is the notebook's function unchanged; the only seam is that GABAM_RASTER_TEMPLATE is a
layer name resolved by `settings.layer_path` inside `load_raster_clipped`, where the notebook
formats a literal local path.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

try:
    from ...common import AOI, fmt_ha, load_raster_clipped
    from ...config import GABAM_RASTER_TEMPLATE, GABAM_YEARS
except ImportError:  # `python historical_burned_area.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, fmt_ha, load_raster_clipped
    from config import GABAM_RASTER_TEMPLATE, GABAM_YEARS


@dataclass(frozen=True)
class BurnedYear:
    year: int
    burned_ha: float


def analyze_historical_burned_area(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.7. Annual burned area from GABAM, 2014 to 2024, and the burned footprint.

    GABAM is a binary annual burned map (1 = burned, 0 = nodata). A pixel can burn in several
    years, so two totals are reported: the union (area burned at least once), used as the
    headline, and the sum of the annual areas, kept in values. The bar chart is per year, where a
    reburn is counted in each year it happens.
    """
    ref = None                    # first year fixes the grid; later years align to it via like=
    union = None                  # boolean, burned at least once, on the ref grid
    annual: list[BurnedYear] = []
    summed_ha = 0.0

    for year in GABAM_YEARS:
        path = GABAM_RASTER_TEMPLATE.format(year=year)
        raster = load_raster_clipped(path, aoi, resampling="nearest", like=ref)
        if ref is None:
            ref = raster
        burned = ~np.ma.getmaskarray(raster.values)   # unmasked = burned inside the AOI
        burned_ha = float(burned.sum()) * raster.pixel_area_ha
        annual.append(BurnedYear(year=year, burned_ha=burned_ha))
        summed_ha += burned_ha
        union = burned.copy() if union is None else (union | burned)

    union_ha = float(union.sum()) * ref.pixel_area_ha if union is not None else 0.0
    union_pct = (union_ha / aoi.area_ha * 100.0) if aoi.area_ha > 0 else 0.0

    if union_ha <= 0:
        narrative = "No burned area was detected in this project area between 2014 and 2024."
    else:
        narrative = (
            f"Between 2014 and 2024, {fmt_ha(union_ha)} of the project area burned at least once, "
            f"about {union_pct:.1f}% of the area."
        )

    results = {
        'narrative': narrative,
        'tables': {"annual_burned_area": annual},
        'values': {
            "burned_union_ha": union_ha,
            "burned_union_pct": union_pct,
            "burned_sum_ha": summed_ha,
            "years": [b.year for b in annual],
        },
        'flags': [],
    }

    view_results = {
        'total_burned_area': union_ha,
        'historical_burned_areas': [
            {'id': str(row.year), 'year': row.year, 'value': row.burned_ha}
            for row in annual
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python historical_burned_area.py [aoi path]
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
    results, view_results = analyze_historical_burned_area(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
