"""3.1 All Ecosystem (Overview) - total ecosystem and disturbed area across the three ecosystems.

Data. `ecosystem_v3.tif` for the classes, `forest_disturbance_v3.tif` where ANY pixel > 0 counts as
disturbed, following C. Bourgoin (2024) on JRC-TMF degraded/undisturbed forest. The scene approach
looks at structural decline using TCC and TCH; the drivers behind it are limited to selective
logging and forest fire.

Calibration warning, carried from the notebook. The 0 to 3 values are calibrated on the pooled SEA
distribution, so they are NOT one to one with the published JRC-TMF. Detection threshold is the 5th
percentile of the reference population height change, a nominal 5% false-positive rate, and the
reference population is forest >= 120 m from any disturbed forest. Per-pixel interpretation is not
supported at 30 m given a product RMSE of 6.6-9.1 m; these are area statistics only.

TWO RASTERS ON ONE GRID, and the body raises if they do not match after clipping rather than
comparing pixels that describe different ground. Preserved exactly.

AREA IS GEODESIC, row by row on the WGS84 ellipsoid, because these rasters stay in EPSG:4326 where
a pixel's ground area shrinks with latitude. This module therefore does NOT use
`common.load_raster_clipped`, which would reproject to REFERENCE_CRS -- the same deliberate
exception 2.3, 2.6 and burned area make.

Body unchanged from the notebook. The one seam change is step 1: the notebook opens a hardcoded
`USER_AOI` inside the function, so that block is replaced by the prepared `aoi` the endpoint
already holds. Nothing after it moves.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import rasterio
from pyproj import Geod
from rasterio.mask import raster_geometry_mask

try:
    from ..common import AOI
    from ..config import THREAT_DISTURBANCE, THREAT_ECOSYSTEM, THREAT_ECOSYSTEM_CLASSES, \
        THREAT_GEOD_ELLPS
    from ..settings import layer_path
except ImportError:  # `python all_ecosystems.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import THREAT_DISTURBANCE, THREAT_ECOSYSTEM, THREAT_ECOSYSTEM_CLASSES, \
        THREAT_GEOD_ELLPS
    from settings import layer_path

# Aliased to the notebook's own names so the body below reads exactly as published.
ECOSYSTEM = THREAT_ECOSYSTEM
DISTURBANCE = THREAT_DISTURBANCE
ECOSYSTEM_CLASSES = THREAT_ECOSYSTEM_CLASSES

# The notebook uses a module-level GEOD that nothing in its repo defines -- it imports Geod and
# reads GEOD_ELLPS from config but never instantiates it, so `pixel_area_by_row` raises NameError
# as published. Same bug as F02-P4's 2.3 cell. Instantiated here.
GEOD = Geod(ellps=THREAT_GEOD_ELLPS)


# The three helpers below are DELIBERATELY NOT SHARED with the other three sections. Cells 6, 8 and
# 10 each define their own reader with slightly different semantics -- `mask` versus
# `raster_geometry_mask`, different validity handling -- and unifying them would silently change
# numbers in whichever section lost its own version. Each section keeps the reader it was written
# with.

def pixel_area_by_row(transform, height):
    """
    Calculate pixel area in hectares by raster row.
    Suitable for EPSG:4326.
    """

    pixel_width = abs(transform.a)

    row_areas = np.zeros(height)

    for row in range(height):

        north = transform.f + row * transform.e
        south = north + transform.e

        west = transform.c
        east = west + pixel_width

        area_m2, _ = GEOD.polygon_area_perimeter(
            [west, east, east, west],
            [north, north, south, south]
        )

        row_areas[row] = abs(area_m2) / 10000

    return row_areas


def calculate_mask_area_ha(binary_mask, transform):
    """
    Calculate area of True pixels in hectares.
    """

    row_areas = pixel_area_by_row(
        transform,
        binary_mask.shape[0]
    )

    pixels_per_row = binary_mask.sum(axis=1)

    return float(
        np.sum(
            pixels_per_row * row_areas
        )
    )


def read_masked_raster(
    raster_path,
    aoi_geometry,
    aoi_crs
):
    """
    Read only the AOI window from a raster.
    """

    with rasterio.open(raster_path) as src:

        if src.crs is None:
            raise ValueError(
                f"Raster has no CRS: {raster_path}"
            )

        if src.crs != aoi_crs:

            raster_aoi = (
                gpd.GeoSeries(
                    [aoi_geometry],
                    crs=aoi_crs
                )
                .to_crs(src.crs)
                .iloc[0]
            )

        else:

            raster_aoi = aoi_geometry


        outside_mask, transform, window = (
            raster_geometry_mask(
                src,
                [raster_aoi.__geo_interface__],
                crop=True,
                all_touched=False
            )
        )


        data = src.read(
            1,
            window=window,
            masked=True
        )


        valid_mask = (
            ~outside_mask
            & ~np.ma.getmaskarray(data)
        )


        return (
            data.data,
            valid_mask,
            transform,
            src.crs
        )


# =============================================================================
# Main Analysis
# =============================================================================

def analyze_all_ecosystem(aoi: AOI):
    """3.1. Total ecosystem and disturbed area, and the per-ecosystem breakdown."""

    # -------------------------------------------------------------------------
    # 1. Read AOI
    # -------------------------------------------------------------------------
    # SEAM: the notebook opens a hardcoded USER_AOI here and validates it. The endpoint has
    # already done both in `common.prepare_aoi`, which also dissolved multi-part input so area is
    # counted once, so the prepared geometry is taken instead.

    aoi_geometry = aoi.geometry.union_all()
    aoi_crs = aoi.geometry.crs


    # -------------------------------------------------------------------------
    # 2. Read Ecosystem Raster
    # -------------------------------------------------------------------------

    (
        ecosystem_data,
        ecosystem_valid,
        ecosystem_transform,
        ecosystem_crs
    ) = read_masked_raster(
        layer_path(ECOSYSTEM),
        aoi_geometry,
        aoi_crs
    )


    # -------------------------------------------------------------------------
    # 3. Read Disturbance Raster
    # -------------------------------------------------------------------------

    (
        disturbance_data,
        disturbance_valid,
        disturbance_transform,
        disturbance_crs
    ) = read_masked_raster(
        layer_path(DISTURBANCE),
        aoi_geometry,
        aoi_crs
    )


    # -------------------------------------------------------------------------
    # IMPORTANT:
    # Both rasters must share the same clipped grid for direct boolean masking.
    # -------------------------------------------------------------------------

    if ecosystem_data.shape != disturbance_data.shape:

        raise ValueError(
            "Ecosystem and disturbance raster grids do not match "
            "after clipping."
        )


    if not np.allclose(
        ecosystem_transform,
        disturbance_transform
    ):

        raise ValueError(
            "Ecosystem and disturbance raster transforms do not match."
        )


    # -------------------------------------------------------------------------
    # 4. Total Ecosystem Mask
    # -------------------------------------------------------------------------

    total_ecosystem_mask = (
        ecosystem_valid
        & np.isin(
            ecosystem_data,
            list(
                ECOSYSTEM_CLASSES.keys()
            )
        )
    )


    total_ecosystem_area_ha = (
        calculate_mask_area_ha(
            total_ecosystem_mask,
            ecosystem_transform
        )
    )


    # -------------------------------------------------------------------------
    # 5. Total Disturbed Area
    # -------------------------------------------------------------------------

    disturbance_mask = (
        disturbance_valid
        & (disturbance_data > 0)
    )


    total_disturbed_mask = (
        total_ecosystem_mask
        & disturbance_mask
    )


    total_disturbed_area_ha = (
        calculate_mask_area_ha(
            total_disturbed_mask,
            ecosystem_transform
        )
    )


    total_disturbed_percentage = (
        total_disturbed_area_ha
        / total_ecosystem_area_ha
        * 100
        if total_ecosystem_area_ha > 0
        else 0
    )


    # -------------------------------------------------------------------------
    # 6. Ecosystem Breakdown
    # -------------------------------------------------------------------------

    ecosystem_results = {}


    for class_value, class_name in (
        ECOSYSTEM_CLASSES.items()
    ):

        ecosystem_mask = (
            ecosystem_valid
            & (ecosystem_data == class_value)
        )


        ecosystem_area_ha = (
            calculate_mask_area_ha(
                ecosystem_mask,
                ecosystem_transform
            )
        )


        ecosystem_percentage = (
            ecosystem_area_ha
            / total_ecosystem_area_ha
            * 100
            if total_ecosystem_area_ha > 0
            else 0
        )


        # ---------------------------------------------------------------------
        # Disturbed within ecosystem
        # ---------------------------------------------------------------------

        ecosystem_disturbed_mask = (
            ecosystem_mask
            & disturbance_mask
        )


        ecosystem_disturbed_area_ha = (
            calculate_mask_area_ha(
                ecosystem_disturbed_mask,
                ecosystem_transform
            )
        )


        ecosystem_disturbed_percentage = (
            ecosystem_disturbed_area_ha
            / ecosystem_area_ha
            * 100
            if ecosystem_area_ha > 0
            else 0
        )


        ecosystem_results[
            class_name
        ] = {

            "area_ha":
                round(
                    ecosystem_area_ha,
                    2
                ),

            "percentage_total":
                round(
                    ecosystem_percentage,
                    2
                ),

            "disturbed_area_ha":
                round(
                    ecosystem_disturbed_area_ha,
                    2
                ),

            "disturbed_percentage":
                round(
                    ecosystem_disturbed_percentage,
                    2
                )
        }


    # -------------------------------------------------------------------------
    # Return
    # -------------------------------------------------------------------------

    return {

        "total_ecosystem_area_ha":
            round(
                total_ecosystem_area_ha,
                2
            ),

        "total_disturbed_area_ha":
            round(
                total_disturbed_area_ha,
                2
            ),

        "total_disturbed_percentage":
            round(
                total_disturbed_percentage,
                2
            ),

        "ecosystems":
            ecosystem_results
    }


if __name__ == "__main__":
    # Run this section on its own, no Flask app and no stream:
    #     python all_ecosystems.py [aoi path]
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

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    print(json.dumps(to_jsonable(analyze_all_ecosystem(aoi)), indent=2, ensure_ascii=False))
