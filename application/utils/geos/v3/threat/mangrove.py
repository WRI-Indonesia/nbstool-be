"""3.3 Mangrove Ecosystem - remaining mangrove forest, disturbance, and the drivers behind it.

Data and calibration warning are the same as 3.2: `drivers_disturbance_v3.tif` from Bart Slagter et
al. 2026, recalibrated against ADPC disaster risk counting only high and very high risk.

WHERE THIS DIFFERS FROM 3.2, and it is not just the ecosystem code:
  - the driver raster is COLLAPSED. Classes 1-4 all become one "Commodities" pressure and 5 becomes
    "Settlement", rather than 3.2's seven named human drivers.
  - it reports a MAIN PRESSURE, the larger of Commodities and Settlement by area, "Commodities and
    Settlement" when they tie above zero, and "Not identified" when neither occurs. This is the one
    place in F02-P3 that ranks drivers by area -- and it never leaves the section, because the
    design's dryland narrative needs a ranking that only exists here.
  - the only natural driver is storm risk, on classes 4 AND 5, where 3.2's storm check uses 4 only.
  - "Other" is disturbed area not overlapping driver classes 1-5. Storm risk is INDEPENDENT and does
    not remove a pixel from Other, so a pixel can be both natural and other.
  - every raster is checked against the ecosystem grid by name through `check_grid`, so a mismatch
    says which layer failed.

Body unchanged from the notebook. The cell is a top-level script reading a hardcoded `USER_AOI`
that it never imports; it is hoisted into `analyze_mangrove(aoi)` with the AOI block replaced, the
helpers nested so they still close over `aoi`, and layer names resolved through
`settings.layer_path`. The notebook's result is nested under a "mangrove" key; that is preserved.
"""

from __future__ import annotations

import numpy as np
import rasterio
from pyproj import Geod
from rasterio.mask import mask

try:
    from ..common import AOI
    from ..config import (
        THREAT_COMMODITY_CLASSES,
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DISTURBANCE_THRESHOLD,
        THREAT_DRIVERS,
        THREAT_ECOSYSTEM,
        THREAT_FOREST_2024,
        THREAT_HISTORICAL,
        THREAT_MANGROVE,
        THREAT_REMAINING_FOREST,
        THREAT_SETTLEMENT_CLASS,
        THREAT_STORM_RISK,
        THREAT_STORM_RISK_CLASSES,
    )
    from ..settings import layer_path
except ImportError:  # `python mangrove.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import (
        THREAT_COMMODITY_CLASSES,
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DISTURBANCE_THRESHOLD,
        THREAT_DRIVERS,
        THREAT_ECOSYSTEM,
        THREAT_FOREST_2024,
        THREAT_HISTORICAL,
        THREAT_MANGROVE,
        THREAT_REMAINING_FOREST,
        THREAT_SETTLEMENT_CLASS,
        THREAT_STORM_RISK,
        THREAT_STORM_RISK_CLASSES,
    )
    from settings import layer_path

# Aliased to the notebook's own names so the body below reads exactly as published.
ECOSYSTEM = THREAT_ECOSYSTEM
HISTORICAL = THREAT_HISTORICAL
FOREST_2024 = THREAT_FOREST_2024
DISTURBANCE = THREAT_DISTURBANCE
DRIVERS = THREAT_DRIVERS
STORM_RISK = THREAT_STORM_RISK
MANGROVE_CLASS = THREAT_MANGROVE
REMAINING_FOREST_CLASS = THREAT_REMAINING_FOREST
CURRENT_FOREST_CLASS = THREAT_CURRENT_FOREST
DISTURBANCE_THRESHOLD = THREAT_DISTURBANCE_THRESHOLD
COMMODITY_CLASSES = THREAT_COMMODITY_CLASSES
SETTLEMENT_CLASS = THREAT_SETTLEMENT_CLASS
STORM_RISK_CLASSES = THREAT_STORM_RISK_CLASSES


def analyze_mangrove(aoi: AOI):
    """3.3. Mangrove: areas, drivers, and the main pressure."""

    # SEAM: the notebook opens a hardcoded USER_AOI here and validates it. `common.prepare_aoi`
    # has already done that, so the prepared AOI is used and the checks below stay as written.
    aoi = aoi.geometry.to_frame("geometry").set_crs(aoi.geometry.crs)

    if aoi.empty:
        raise ValueError(
            "AOI contains no features."
        )

    if aoi.crs is None:
        raise ValueError(
            "AOI has no CRS."
        )

    aoi = aoi[
        aoi.geometry.notna()
        & ~aoi.geometry.is_empty
    ].copy()

    if aoi.empty:
        raise ValueError(
            "AOI contains no valid geometry."
        )


    # =============================================================================
    # READ RASTER CLIPPED TO USER AOI
    # =============================================================================

    def read_raster(
        raster_path
    ):

        with rasterio.open(
            layer_path(raster_path)
        ) as src:

            if src.crs is None:
                raise ValueError(
                    f"Raster has no CRS: {raster_path}"
                )

            polygon = (
                aoi.to_crs(src.crs)
                if aoi.crs != src.crs
                else aoi
            )

            geometries = [
                geom.__geo_interface__
                for geom in polygon.geometry
            ]

            data, transform = mask(
                src,
                geometries,
                crop=True,
                filled=False
            )

            return (
                data[0],
                transform,
                src.crs
            )


    # =============================================================================
    # CHECK GRID ALIGNMENT
    # =============================================================================

    def check_grid(
        reference,
        reference_transform,
        data,
        transform,
        name
    ):

        if reference.shape != data.shape:

            raise ValueError(
                f"Grid size mismatch: {name}"
            )

        if not np.allclose(
            reference_transform,
            transform
        ):

            raise ValueError(
                f"Grid alignment mismatch: {name}"
            )


    # =============================================================================
    # AREA CALCULATION
    # EPSG:4326 -> geodesic hectare
    # =============================================================================

    GEOD = Geod(
        ellps="WGS84"
    )


    def area_ha(
        binary_mask,
        transform
    ):

        # Avoid MaskError from masked arrays
        binary_mask = np.ma.filled(
            binary_mask,
            False
        ).astype(bool)

        total_m2 = 0.0

        pixel_width = abs(
            transform.a
        )

        for row in range(
            binary_mask.shape[0]
        ):

            pixel_count = np.count_nonzero(
                binary_mask[row]
            )

            if pixel_count == 0:
                continue

            north = (
                transform.f
                + row * transform.e
            )

            south = (
                north
                + transform.e
            )

            west = transform.c
            east = west + pixel_width

            pixel_area_m2, _ = (
                GEOD.polygon_area_perimeter(
                    [west, east, east, west],
                    [north, north, south, south]
                )
            )

            total_m2 += (
                pixel_count
                * abs(pixel_area_m2)
            )

        return (
            total_m2 / 10000
        )


    # =============================================================================
    # LOAD RASTERS
    # =============================================================================

    ecosystem, transform, crs = (
        read_raster(
            ECOSYSTEM
        )
    )

    historical, historical_transform, _ = (
        read_raster(
            HISTORICAL
        )
    )

    forest2024, forest2024_transform, _ = (
        read_raster(
            FOREST_2024
        )
    )

    disturbance, disturbance_transform, _ = (
        read_raster(
            DISTURBANCE
        )
    )

    drivers, drivers_transform, _ = (
        read_raster(
            DRIVERS
        )
    )

    storm, storm_transform, _ = (
        read_raster(
            STORM_RISK
        )
    )


    # =============================================================================
    # VALIDATE GRIDS
    # =============================================================================

    check_grid(
        ecosystem,
        transform,
        historical,
        historical_transform,
        "historical_deforestation_v3.tif"
    )

    check_grid(
        ecosystem,
        transform,
        forest2024,
        forest2024_transform,
        "forest_2024_v3.tif"
    )

    check_grid(
        ecosystem,
        transform,
        disturbance,
        disturbance_transform,
        "forest_disturbance_v3.tif"
    )

    check_grid(
        ecosystem,
        transform,
        drivers,
        drivers_transform,
        "drivers_disturbance_v3.tif"
    )

    check_grid(
        ecosystem,
        transform,
        storm,
        storm_transform,
        "risk_storm_v3.tif"
    )


    # =============================================================================
    # VALID PIXELS
    # =============================================================================

    ecosystem_valid = (
        ~np.ma.getmaskarray(
            ecosystem
        )
    )

    historical_valid = (
        ~np.ma.getmaskarray(
            historical
        )
    )

    forest2024_valid = (
        ~np.ma.getmaskarray(
            forest2024
        )
    )

    disturbance_valid = (
        ~np.ma.getmaskarray(
            disturbance
        )
    )

    drivers_valid = (
        ~np.ma.getmaskarray(
            drivers
        )
    )

    storm_valid = (
        ~np.ma.getmaskarray(
            storm
        )
    )


    # =============================================================================
    # 1. TOTAL MANGROVE AREA
    #
    # User AOI
    # -> ecosystem_v3 == 2
    # =============================================================================

    mangrove_mask = (
        ecosystem_valid
        & (
            ecosystem.data
            == MANGROVE_CLASS
        )
    )

    total_mangrove_area = area_ha(
        mangrove_mask,
        transform
    )


    # =============================================================================
    # 2. REMAINING MANGROVE FOREST
    #
    # Mangrove mask
    # -> historical_deforestation_v3 == 1
    # =============================================================================

    remaining_mask = (
        mangrove_mask
        & historical_valid
        & (
            historical.data
            == REMAINING_FOREST_CLASS
        )
    )

    remaining_area = area_ha(
        remaining_mask,
        transform
    )


    # =============================================================================
    # 3. CURRENT MANGROVE FOREST
    #
    # Mangrove mask
    # -> forest_2024_v3 == 1
    # =============================================================================

    current_mangrove_mask = (
        mangrove_mask
        & forest2024_valid
        & (
            forest2024.data
            == CURRENT_FOREST_CLASS
        )
    )


    # =============================================================================
    # 4. DISTURBED MANGROVE
    #
    # Current mangrove
    # -> forest_disturbance_v3 > 0
    #
    # This becomes the MASTER MASK for all driver analysis.
    # =============================================================================

    disturbed_mangrove_mask = (
        current_mangrove_mask
        & disturbance_valid
        & (
            disturbance.data
            > DISTURBANCE_THRESHOLD
        )
    )

    disturbed_area = area_ha(
        disturbed_mangrove_mask,
        transform
    )


    # =============================================================================
    # PERCENTAGES
    # =============================================================================

    remaining_percentage = (
        remaining_area
        / total_mangrove_area
        * 100
        if total_mangrove_area > 0
        else 0
    )

    disturbed_percentage = (
        disturbed_area
        / total_mangrove_area
        * 100
        if total_mangrove_area > 0
        else 0
    )


    # =============================================================================
    # 5. NON-NATURAL DRIVERS
    #
    # Disturbed mangrove
    # -> drivers_disturbance_v3
    #
    # 1,2,3,4 = Commodities
    # 5       = Settlement
    # =============================================================================

    commodities_mask = (
        disturbed_mangrove_mask
        & drivers_valid
        & np.isin(
            drivers.data,
            COMMODITY_CLASSES
        )
    )

    settlement_mask = (
        disturbed_mangrove_mask
        & drivers_valid
        & (
            drivers.data
            == SETTLEMENT_CLASS
        )
    )


    commodities_area = area_ha(
        commodities_mask,
        transform
    )

    settlement_area = area_ha(
        settlement_mask,
        transform
    )


    non_natural_drivers = []

    if np.any(
        commodities_mask
    ):

        non_natural_drivers.append(
            "Commodities"
        )

    if np.any(
        settlement_mask
    ):

        non_natural_drivers.append(
            "Settlement"
        )


    # =============================================================================
    # 6. MAIN PRESSURE
    #
    # Largest overlap area between:
    # - Commodities
    # - Settlement
    # =============================================================================

    if commodities_area > settlement_area:

        main_pressure = (
            "Commodities"
        )

    elif settlement_area > commodities_area:

        main_pressure = (
            "Settlement"
        )

    elif (
        commodities_area > 0
        and settlement_area > 0
    ):

        main_pressure = (
            "Commodities and Settlement"
        )

    else:

        main_pressure = (
            "Not identified"
        )


    # =============================================================================
    # 7. NATURAL DRIVER
    #
    # Disturbed mangrove
    # -> risk_storm_v3
    # -> pixel 4 or 5
    # =============================================================================

    storm_mask = (
        disturbed_mangrove_mask
        & storm_valid
        & np.isin(
            storm.data,
            STORM_RISK_CLASSES
        )
    )


    natural_drivers = []

    if np.any(
        storm_mask
    ):

        natural_drivers.append(
            "Extreme climate event"
        )


    # =============================================================================
    # 8. OTHER DRIVER
    #
    # Disturbed mangrove pixels that DO NOT overlap
    # known drivers_disturbance_v3 classes 1-5.
    #
    # Natural storm risk is independent and does not remove
    # a pixel from the "Other" classification.
    # =============================================================================

    known_driver_mask = (
        disturbed_mangrove_mask
        & drivers_valid
        & np.isin(
            drivers.data,
            [1, 2, 3, 4, 5]
        )
    )

    other_mask = (
        disturbed_mangrove_mask
        & ~known_driver_mask
    )


    other_drivers = []

    if np.any(
        other_mask
    ):

        other_drivers.append(
            "Other"
        )


    result = {

        "mangrove": {

            "total_area_ha":
                round(
                    total_mangrove_area,
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
                        remaining_percentage,
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
                        disturbed_percentage,
                        2
                    )
            },

            "main_pressure":
                main_pressure,

            "drivers": {

                "non_natural":
                    non_natural_drivers,

                "natural":
                    natural_drivers,

                "other":
                    other_drivers
            }
        }
    }

    return result


if __name__ == "__main__":
    # Run this section on its own, no Flask app and no stream:
    #     python mangrove.py [aoi path]
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
    print(json.dumps(to_jsonable(analyze_mangrove(aoi)), indent=2, ensure_ascii=False))
