"""
Component 7.2 Historical Burned Area (was Climate 3.7).

How much of the AOI burned in each year of the GABAM record, the cumulative and average annual
burned area, and the peak year.

Data. GABAM annual burned maps (`GABAM_RASTER_TEMPLATE`, `gabam/GABAM_<year>.tif` in the v3
bucket), one binary raster per year, value 1 = burned and 0 = NODATA, ~30 m, EPSG:4326.

RE-PORTED 2026-11-23 from notebook `67ad949` ("Updated Burned Area calculations"), where the
section moved from Climate 3.7 to Threat 7.2 and the calculation changed:
  - NO UNION FOOTPRINT. The headline is now the CUMULATIVE burned area: the sum of the annual
    areas. A pixel that burns in two years is counted twice, so the figure measures burn events
    and CAN EXCEED THE AOI AREA. `total_burned_area` therefore changed meaning (it was the area
    burned at least once).
  - NATIVE GRID, GEODESIC AREA. Each year is masked on its own EPSG:4326 grid and measured row
    by row on the WGS84 ellipsoid, instead of being reprojected to REFERENCE_CRS and counted at
    one nominal pixel area. No `like=` alignment is needed without the union.
  - Burned is `== BURNED_VALUE` on valid pixels, stated explicitly.
  - New outputs: average annual burned area, the peak year and its area, each year's share of
    the AOI.
  - THE YEARS ARE 2014..2023. The notebook's `YEARS = range(2014, 2024)` stops at 2023 while its
    own narrative says "between 2014 and 2024" and the bucket holds GABAM_2024.tif. Followed as
    published -- the narrative strings below are verbatim, 2024 included. See GABAM_YEARS.

IT STAYS IN THE CLIMATE STREAM under the wire name `burned area`, so the card and the document tag
that read `climate.total_burned_area` keep working; only the notebook's heading moved.

What this port changes at the seam, and nowhere else:
  - the raster path is a layer name resolved by `settings.layer_path`, where the notebook joins a
    UNC folder;
  - the notebook's free names `aoi` (a GeoDataFrame) and `aoi_area_ha` -- neither is defined
    anywhere in the Threat notebook -- are the prepared AOI's geometry and `aoi.area_ha`;
  - `GEOD` is built here from GABAM_GEOD_ELLPS (the notebook's config calls `Geod` without
    importing it);
  - the printed table is dropped; its 2 dp rounding was display only and is not applied.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import pandas as pd
import rasterio
from pyproj import Geod
from rasterio.mask import mask

try:
    from ...common import AOI
    from ...config import (
        GABAM_BURNED_VALUE,
        GABAM_GEOD_ELLPS,
        GABAM_RASTER_TEMPLATE,
        GABAM_YEARS,
    )
    from ...settings import layer_path
except ImportError:  # `python historical_burned_area.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from config import (
        GABAM_BURNED_VALUE,
        GABAM_GEOD_ELLPS,
        GABAM_RASTER_TEMPLATE,
        GABAM_YEARS,
    )
    from settings import layer_path

# The notebook's names for the same constants.
YEARS = GABAM_YEARS
BURNED_VALUE = GABAM_BURNED_VALUE
GEOD = Geod(ellps=GABAM_GEOD_ELLPS)


def analyze_historical_burned_area(aoi: AOI) -> tuple[dict, dict]:
    """Component 7.2. Annual burned area from GABAM, with its cumulative, average and peak."""
    # SEAM: the notebook's `aoi` is a GeoDataFrame and `aoi_area_ha` a bare global.
    aoi_area_ha = aoi.area_ha
    aoi = gpd.GeoDataFrame(geometry=aoi.geometry)

    records = []

    for year in YEARS:

        path = layer_path(GABAM_RASTER_TEMPLATE.format(year=year))

        with rasterio.open(path) as src:

            geom = aoi.to_crs(src.crs).geometry

            data, transform = mask(
                src,
                geom,
                crop=True,
                filled=False
            )

            burned = (
                ~np.ma.getmaskarray(data[0])
                & (data[0].data == BURNED_VALUE)
            )

            # EPSG:4326 / geographic raster
            if src.crs.is_geographic:

                burned_ha = 0

                for row in range(burned.shape[0]):

                    north = transform.f + row * transform.e
                    south = north + transform.e

                    area, _ = GEOD.polygon_area_perimeter(
                        [
                            transform.c,
                            transform.c + transform.a,
                            transform.c + transform.a,
                            transform.c
                        ],
                        [north, north, south, south]
                    )

                    burned_ha += (
                        burned[row].sum()
                        * abs(area)
                        / 10000
                    )

            else:

                pixel_ha = (
                    abs(transform.a * transform.e)
                    / 10000
                )

                burned_ha = (
                    burned.sum()
                    * pixel_ha
                )

        records.append({
            "year": year,
            "burned_area_ha": burned_ha,
            "burned_pct_aoi": burned_ha / aoi_area_ha * 100
        })

    # =============================================================================
    # SUMMARY
    # =============================================================================

    annual = pd.DataFrame(records)

    cumulative_ha = annual["burned_area_ha"].sum()
    average_ha = annual["burned_area_ha"].mean()

    peak = annual.loc[
        annual["burned_area_ha"].idxmax()
    ]

    # =============================================================================
    # NARRATIVE
    # =============================================================================

    if cumulative_ha > 0:

        narrative = (
            f"The project area recorded a cumulative burned area of "
            f"{cumulative_ha:,.2f} ha between 2014 and 2024, "
            f"with an average annual burned area of {average_ha:,.2f} ha. "
            f"The highest annual burned area was recorded in "
            f"{int(peak['year'])}, when {peak['burned_area_ha']:,.2f} ha "
            f"was affected by fire."
        )

    else:

        narrative = (
            "No burned area was detected within the project area "
            "between 2014 and 2024."
        )

    rows = [
        {"year": int(r["year"]), "burned_area_ha": float(r["burned_area_ha"]),
         "burned_pct_aoi": float(r["burned_pct_aoi"])}
        for r in records
    ]
    # With nothing burned every year ties at 0 and idxmax names the first; no year is a peak then.
    burned_any = bool(cumulative_ha > 0)

    results = {
        'narrative': narrative,
        'tables': {"annual_burned_area": rows},
        'values': {
            "cumulative_ha": float(cumulative_ha),
            "average_ha": float(average_ha),
            "peak_year": int(peak["year"]) if burned_any else None,
            "peak_ha": float(peak["burned_area_ha"]) if burned_any else 0.0,
            "years": [r["year"] for r in rows],
        },
        'flags': [],
    }

    view_results = {
        'total_burned_area': float(cumulative_ha),
        'average_annual_burned_area': float(average_ha),
        'peak_burned_year': results['values']["peak_year"],
        'peak_burned_area': results['values']["peak_ha"],
        'historical_burned_areas': [
            {'id': str(r["year"]), 'year': r["year"], 'value': r["burned_area_ha"],
             'percentage': r["burned_pct_aoi"]}
            for r in rows
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python historical_burned_area.py [aoi path]
    import json
    import os
    import sys

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
