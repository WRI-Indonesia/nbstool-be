"""
7.3 Land Degradation (F02-P3 Threat, notebook commits `0e7f3cc` / `a6d8ae8` / `7069954`).

How much of the project area is degraded, stable or improved between 2014 and 2024, and how the
degraded part splits between forest and non-forest land.

Method, upstream of this file. UNCCD SDG 15.3.1: three sub-indicators (land cover change,
productivity, soil organic carbon) combined One-Out-All-Out into ONE categorical raster. The cell
does none of that maths; it clips that raster, splits it by a 2024 forest mask and counts pixels:

    1 Degraded   2 Stable   3 Improved   (nodata otherwise)

THREE CLASSES SINCE NOTEBOOK `7069954` (2026-11-23). The first version of the cell counted five,
with 4 and 5 as "potential false positive / negative", and reported a total over 1 + 4 + 5. Those
are gone: degraded is class 1 and nothing else, the table has three rows and no Total row, and
its hectares are no longer rounded.

Data. `THREAT_LAND_DEGRADATION` and `THREAT_FOREST_2024_54034`, BOTH IN ESRI:54034: the cell reads
pixel area straight off the forest transform and never reprojects the AOI, which is already in
that CRS here. Both reached the bucket on 2026-11-23 and sit on one 29.43 m grid. The degradation
raster's nodata is 15, not 0, and that is harmless: the clip reads it masked and fills it with 0
like everything outside the polygon. Where the layer has no data the three classes therefore add
up to less than the AOI (77% on one coastal test site).

The body is the notebook's cell. What changed at the seam, and nowhere else:
  - it takes the prepared AOI instead of reading `aoi_path`;
  - the rasters are opened through `settings.layer_path`;
  - THE THREE GeoTIFF WRITES AND THE CSV ARE DROPPED, with the profile bookkeeping that only fed
    them. Their paths (`forest_out`, `inside_out`, `outside_out`, `csv_out`) are defined nowhere
    in the notebook repo, so the cell raises NameError at its first write as published;
  - a CRS check, because a layer uploaded in another CRS would otherwise clip the wrong place and
    report square degrees as square metres without raising;
  - the prints are dropped and the values returned.
"""

from __future__ import annotations

import numpy as np
import pandas as pd
import rasterio
from rasterio.mask import mask
from rasterio.warp import Resampling, reproject

try:
    from ..common import AOI
    from ..config import THREAT_FOREST_2024_54034, THREAT_LAND_DEGRADATION
    from ..settings import layer_path
except ImportError:  # `python land_degradation.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import THREAT_FOREST_2024_54034, THREAT_LAND_DEGRADATION
    from settings import layer_path

# Aliased to the notebook's own names so the body below reads exactly as published.
forest_path = THREAT_FOREST_2024_54034
degradation_path = THREAT_LAND_DEGRADATION


def analyze_land_degradation(aoi: AOI) -> dict:
    """7.3. Land condition classes over the AOI, inside and outside forest."""

    # =============================================================================
    # READ AOI
    # =============================================================================

    # SEAM: the notebook reads a hardcoded `aoi_path`; the prepared AOI is already ESRI:54034.
    aoi_crs = aoi.geometry.crs
    aoi_geom = [geom for geom in aoi.geometry]

    # =============================================================================
    # STEP 1: CLIP FOREST COVER
    # =============================================================================

    with rasterio.open(layer_path(forest_path)) as src:
        if src.crs != aoi_crs:
            raise RuntimeError(f"{forest_path} is {src.crs}, expected {aoi_crs}.")
        forest_arr, forest_transform = mask(
            src,
            aoi_geom,
            crop=True,
            nodata=0,
            filled=True
        )

        forest_crs = src.crs

    forest_arr = forest_arr[0].astype("float32")

    # =============================================================================
    # CLIP LAND DEGRADATION
    # =============================================================================

    with rasterio.open(layer_path(degradation_path)) as src:
        if src.crs != aoi_crs:
            raise RuntimeError(f"{degradation_path} is {src.crs}, expected {aoi_crs}.")
        deg_arr, deg_transform = mask(
            src,
            aoi_geom,
            crop=True,
            nodata=0,
            filled=True
        )

        deg_crs = src.crs

    deg_arr = deg_arr[0].astype("float32")

    # =============================================================================
    # ALIGN DEGRADATION TO FOREST GRID
    # =============================================================================

    deg_aligned = np.zeros(
        forest_arr.shape,
        dtype="float32"
    )

    reproject(
        source=deg_arr,
        destination=deg_aligned,
        src_transform=deg_transform,
        src_crs=deg_crs,
        src_nodata=0,
        dst_transform=forest_transform,
        dst_crs=forest_crs,
        dst_nodata=0,
        resampling=Resampling.nearest,
    )

    # =============================================================================
    # STEP 2: LAND CONDITION INSIDE / OUTSIDE FOREST
    # =============================================================================

    forest_1 = forest_arr == 1

    deg_inside = np.where(
        forest_1 & (deg_aligned > 0),
        deg_aligned,
        0
    ).astype("float32")

    deg_outside = np.where(
        ~forest_1 & (deg_aligned > 0),
        deg_aligned,
        0
    ).astype("float32")

    # =============================================================================
    # STEP 3: AREA BY CLASS
    #
    # 1 = Degraded
    # 2 = Stable
    # 3 = Improved
    # =============================================================================

    pixel_area_ha = (
        abs(
            forest_transform.a
            * forest_transform.e
        )
        / 10000
    )

    rows = []

    for value in [1, 2, 3]:

        inside_count = int(
            (deg_inside == value).sum()
        )

        outside_count = int(
            (deg_outside == value).sum()
        )

        rows.append({
            "class": value,

            "inside_forest_pixels":
                inside_count,

            "inside_forest_ha":
                inside_count * pixel_area_ha,

            "outside_forest_pixels":
                outside_count,

            "outside_forest_ha":
                outside_count * pixel_area_ha,
        })

    df = pd.DataFrame(rows)

    # =============================================================================
    # STEP 4: LAND CONDITION SUMMARY
    # =============================================================================

    degraded_ha = (
        (deg_aligned == 1).sum()
        * pixel_area_ha
    )

    stable_ha = (
        (deg_aligned == 2).sum()
        * pixel_area_ha
    )

    improved_ha = (
        (deg_aligned == 3).sum()
        * pixel_area_ha
    )

    # =============================================================================
    # DEGRADATION INSIDE / OUTSIDE FOREST
    # =============================================================================

    degraded_inside_forest_ha = (
        (deg_inside == 1).sum()
        * pixel_area_ha
    )

    degraded_outside_forest_ha = (
        (deg_outside == 1).sum()
        * pixel_area_ha
    )

    # =============================================================================
    # NARRATIVE
    # =============================================================================

    narrative = (
        f"A total of {degraded_ha:,.2f} ha within the project area "
        f"showed signs of land degradation between 2014 and 2024. "
        f"Of this degraded area, {degraded_inside_forest_ha:,.2f} ha "
        f"occurred within forest areas, while "
        f"{degraded_outside_forest_ha:,.2f} ha occurred outside forest areas."
    )

    if improved_ha > 0:

        narrative += (
            "\n\n"
            f"In contrast, {improved_ha:,.2f} ha within the project area "
            f"showed positive changes in land condition between 2014 and 2024."
        )

    if stable_ha > 0:

        narrative += (
            "\n\n"
            f"A total of {stable_ha:,.2f} ha was classified as stable, "
            f"indicating no significant improvement or deterioration in the "
            f"assessed land condition indicators between 2014 and 2024."
        )

    return {
        "degraded_ha": float(degraded_ha),
        "stable_ha": float(stable_ha),
        "improved_ha": float(improved_ha),
        "degraded_inside_forest_ha": float(degraded_inside_forest_ha),
        "degraded_outside_forest_ha": float(degraded_outside_forest_ha),
        "table": df.to_dict(orient="records"),
        "narrative": narrative,
    }


if __name__ == "__main__":
    # Run on a file and print the result, no Flask app:
    #     python land_degradation.py [aoi path]
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    _aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {_aoi.area_ha:,.0f} ha\n", file=sys.stderr)
    print(json.dumps(to_jsonable(analyze_land_degradation(_aoi)), indent=2, ensure_ascii=False))
