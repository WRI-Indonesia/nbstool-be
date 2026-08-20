"""3.2 Dryland Forest Ecosystem - remaining forest, disturbance, loss and gain, and the drivers.

DRYLAND HERE INCLUDES SAVANNA. `ecosystem_v3.tif` class 1 covers both; the layer does not separate
them, verified against the pathway raster where 100% of its savanna pixels fall in this class. So
there is no savanna fold to perform, unlike F02-P4.

Data. `drivers_disturbance_v3.tif`, values 1 to 11, from Bart Slagter et al. 2026
(https://doi.org/10.21203/rs.3.rs-7424252/v1), which classifies the KEY drivers of forest
disturbance and may not include every cause of deforestation.

Calibration warning, carried from the notebook. Post-processing identified where fire coincided
with clearing of agricultural land, from VIIRS fire alerts within a 500 m buffer plus a low
post-disturbance normalized burn ratio in the following month's Sentinel-2 composite. It also masks
out area outside `forest_disturbance_v3.tif` and recalibrates against ADPC disaster risk -- and that
recalibration counts ONLY high and very high disaster risk as forest disturbance.

DRIVERS ARE PRESENCE ONLY. Each list is the driver names that occur somewhere in the disturbed
area, with no area and no ranking, because the notebook computes none. The design's narrative
"driven mainly by [driver 1] and [driver 2]" cannot be filled from this.

Body unchanged from the notebook. Three seam changes, all wiring:
  - the cell is a top-level script reading a hardcoded AOI; it is hoisted into
    `analyze_dryland_forest(aoi)` and the AOI block replaced by the prepared AOI.
  - `read`, `area_ha` and `percentage` are nested inside that function because the notebook's
    versions close over module-level `aoi`, `geometry` and `total_area`.
  - `read` resolves layer names through `settings.layer_path`.
Nothing between them changes, including the duplicate `GEOD` assignment the cell contains.
"""

from __future__ import annotations

import numpy as np
import rasterio
from pyproj import Geod
from rasterio.mask import mask

try:
    from ..common import AOI
    from ..config import (
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DRIVERS,
        THREAT_DRYLAND,
        THREAT_ECOSYSTEM,
        THREAT_FOREST_2024,
        THREAT_FOREST_DRIVER_CLASSES,
        THREAT_FOREST_GAIN,
        THREAT_FOREST_GAIN_VALUE,
        THREAT_FOREST_LOSS,
        THREAT_HISTORICAL,
        THREAT_NATURAL_DRIVERS,
        THREAT_REMAINING_FOREST,
    )
    from ..settings import layer_path
except ImportError:  # `python dryland_forest.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import (
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DRIVERS,
        THREAT_DRYLAND,
        THREAT_ECOSYSTEM,
        THREAT_FOREST_2024,
        THREAT_FOREST_DRIVER_CLASSES,
        THREAT_FOREST_GAIN,
        THREAT_FOREST_GAIN_VALUE,
        THREAT_FOREST_LOSS,
        THREAT_HISTORICAL,
        THREAT_NATURAL_DRIVERS,
        THREAT_REMAINING_FOREST,
    )
    from settings import layer_path

# Aliased to the notebook's own names so the body below reads exactly as published. FOREST_DRIVERS
# and DRIVERS_DISTURBANCE are two config names for ONE file; the cell imports both and uses
# FOREST_DRIVERS for the driver read while NATURAL_DRIVERS reaches the same file for forest fire.
ECOSYSTEM = THREAT_ECOSYSTEM
HISTORICAL = THREAT_HISTORICAL
FOREST_2024 = THREAT_FOREST_2024
DISTURBANCE = THREAT_DISTURBANCE
FOREST_GAIN = THREAT_FOREST_GAIN
FOREST_DRIVERS = THREAT_DRIVERS
DRIVERS_DISTURBANCE = THREAT_DRIVERS
DRYLAND = THREAT_DRYLAND
REMAINING_FOREST = THREAT_REMAINING_FOREST
FOREST_LOSS = THREAT_FOREST_LOSS
CURRENT_FOREST = THREAT_CURRENT_FOREST
FOREST_GAIN_VALUE = THREAT_FOREST_GAIN_VALUE
FOREST_DRIVER_CLASSES = THREAT_FOREST_DRIVER_CLASSES
NATURAL_DRIVERS = THREAT_NATURAL_DRIVERS

GEOD = Geod(ellps="WGS84")


def analyze_dryland_forest(aoi: AOI):
    """3.2. Dryland forest (incl. savanna): areas, and which drivers are present."""

    # =============================================================================
    # Read AOI
    # =============================================================================
    # SEAM: the notebook opens config.AOI here -- a name its config does not even define -- and
    # validates it. `common.prepare_aoi` has already done that, so the prepared AOI is used.

    aoi = aoi.geometry.to_frame("geometry").set_crs(aoi.geometry.crs)

    if aoi.empty:
        raise ValueError("AOI contains no features.")

    if aoi.crs is None:
        raise ValueError("AOI has no CRS.")


    geometry = [
        geom.__geo_interface__
        for geom in aoi.geometry
        if geom is not None and not geom.is_empty
    ]


    # =============================================================================
    # Read raster clipped to AOI
    # =============================================================================

    def read(path):

        with rasterio.open(layer_path(path)) as src:

            shapes = geometry

            if aoi.crs != src.crs:

                projected = aoi.to_crs(
                    src.crs
                )

                shapes = [
                    geom.__geo_interface__
                    for geom in projected.geometry
                    if geom is not None
                    and not geom.is_empty
                ]

            data, transform = mask(
                src,
                shapes,
                crop=True,
                filled=False
            )

            return (
                data[0],
                transform
            )


    # =============================================================================
    # Pixel area for EPSG:4326
    # =============================================================================

    def area_ha(mask_array, transform):

        mask_array = np.ma.filled(
            mask_array,
            False
        )

        total = 0.0

        for row in range(mask_array.shape[0]):

            count = np.count_nonzero(
                mask_array[row]
            )

            if count == 0:
                continue

            north = transform.f + row * transform.e
            south = north + transform.e

            west = transform.c
            east = west + transform.a

            pixel_m2, _ = GEOD.polygon_area_perimeter(
                [west, east, east, west],
                [north, north, south, south]
            )

            total += count * abs(pixel_m2)

        return total / 10000


    # =============================================================================
    # Load core rasters
    # =============================================================================

    eco, transform = read(
        ECOSYSTEM
    )

    historical, _ = read(
        HISTORICAL
    )

    forest2024, _ = read(
        FOREST_2024
    )

    disturbance, _ = read(
        DISTURBANCE
    )

    gain, _ = read(
        FOREST_GAIN
    )

    drivers, _ = read(
        FOREST_DRIVERS
    )


    # =============================================================================
    # Core masks
    # =============================================================================

    dryland = (
        eco == DRYLAND
    )


    remaining = (
        dryland
        & (
            historical
            == REMAINING_FOREST
        )
    )


    forest_loss = (
        dryland
        & (
            historical
            == FOREST_LOSS
        )
    )


    current_forest = (
        dryland
        & (
            forest2024
            == CURRENT_FOREST
        )
    )


    disturbed = (
        current_forest
        & (
            disturbance > 0
        )
    )


    forest_gain = (
        dryland
        & (
            gain
            == FOREST_GAIN_VALUE
        )
    )


    # =============================================================================
    # Areas
    # =============================================================================

    total_area = area_ha(
        dryland,
        transform
    )

    remaining_area = area_ha(
        remaining,
        transform
    )

    disturbed_area = area_ha(
        disturbed,
        transform
    )

    loss_area = area_ha(
        forest_loss,
        transform
    )

    gain_area = area_ha(
        forest_gain,
        transform
    )


    # =============================================================================
    # Percentages
    # =============================================================================

    def percentage(area):

        if total_area == 0:
            return 0

        return (
            area
            / total_area
            * 100
        )


    # =============================================================================
    # Non-natural + explicit other forest drivers
    # =============================================================================

    driver_values = np.unique(
        drivers[disturbed]
    )


    non_natural = []
    other = []


    for value in driver_values:

        # Ignore masked/nodata values
        if np.ma.is_masked(value):
            continue

        value = int(value)

        if value not in FOREST_DRIVER_CLASSES:
            continue


        driver_name = (
            FOREST_DRIVER_CLASSES[
                value
            ]
        )


        if (
            driver_name
            == "Non-productive conversion"
        ):

            other.append(
                driver_name
            )

        else:

            non_natural.append(
                driver_name
            )


    # =============================================================================
    # Natural drivers
    # =============================================================================

    natural = []

    # Used later so pixels explained by natural drivers
    # are not classified as Unknown.
    natural_presence = np.zeros(
        disturbed.shape,
        dtype=bool
    )


    for (
        driver_name,
        config
    ) in NATURAL_DRIVERS.items():

        driver_raster, _ = read(
            config["raster"]
        )


        driver_mask = (
            disturbed
            & np.isin(
                driver_raster,
                config["values"]
            )
        )


        if np.any(
            driver_mask
        ):

            natural.append(
                driver_name
            )

            natural_presence |= (
                driver_mask
            )


    # =============================================================================
    # Unknown driver
    # =============================================================================

    known_forest_driver = (
        disturbed
        & np.isin(
            drivers,
            list(
                FOREST_DRIVER_CLASSES.keys()
            )
        )
    )


    unknown = (
        disturbed
        & ~known_forest_driver
        & ~natural_presence
    )


    if np.any(
        unknown
    ):

        other.append(
            "Unknown"
        )


    # =============================================================================
    # Backend result
    # =============================================================================

    result = {

        "total_area_ha":
            round(
                total_area,
                2
            ),

        "remaining_forest": {
            "area_ha":
                round(
                    remaining_area,
                    2
                ),

            "percentage":
                round(
                    percentage(
                        remaining_area
                    ),
                    2
                )
        },

        "disturbed": {
            "area_ha":
                round(
                    disturbed_area,
                    2
                ),

            "percentage":
                round(
                    percentage(
                        disturbed_area
                    ),
                    2
                )
        },

        "forest_loss": {
            "area_ha":
                round(
                    loss_area,
                    2
                ),

            "percentage":
                round(
                    percentage(
                        loss_area
                    ),
                    2
                )
        },

        "forest_gain": {
            "area_ha":
                round(
                    gain_area,
                    2
                ),

            "percentage":
                round(
                    percentage(
                        gain_area
                    ),
                    2
                )
        },

        "drivers": {

            "non_natural":
                non_natural,

            "natural":
                natural,

            "other":
                other
        }
    }

    return result


if __name__ == "__main__":
    # Run this section on its own, no Flask app and no stream:
    #     python dryland_forest.py [aoi path]
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
    print(json.dumps(to_jsonable(analyze_dryland_forest(aoi)), indent=2, ensure_ascii=False))
