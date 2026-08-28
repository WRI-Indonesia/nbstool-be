"""3.4 Peatland Ecosystem - remaining peat forest, disturbance, conversion, and hydrological drivers.

THE ONLY SECTION WHOSE DRIVERS ARE NOT A DRIVER RASTER. Peat degradation is hydrological, so instead
of naming drivers it reports three graded indicators: canal proximity, drainage pressure and fire
risk. It also reports CONVERTED / LOSS where 3.2 reports forest loss and gain.

Calibration warning, carried from the notebook. The drainage canal dataset (Dadap et al. 2021)
captures canals across both peat and non-peat areas, so it was masked with `ecosystem_v3.tif` to
keep only canals inside mapped SEA peatland. DRAINAGE PRESSURE IS DELIBERATELY NOT CLIPPED TO THE
PROJECT AREA: canals outside the boundary still influence conditions inside it, because Astiani et
al. (2017) found canals affect water table up to 500 m away and Wedeux et al. (2020) found canal
networks affect biomass growth up to 1 km. Those two distances are exactly the High / Medium / Low
thresholds below.

CANAL PROXIMITY IS A VECTOR DISTANCE, not a raster one: both the current peat forest mask and the
canal mask are polygonised with `rasterio.features.shapes`, reprojected to the AOI's own UTM zone
for metre distance, and measured with shapely. On a large AOI that polygonisation is the expensive
part of the whole endpoint -- `peat_canal.tif` is 1.28 GB before clipping.

NOTE the grade vocabularies are wider than the other sections': canal proximity and drainage
pressure can be "Not identified", and fire risk has five levels including "Very low" and "No risk".

Body unchanged from the notebook, including its mid-body imports of `shapes` and `shape`. Hoisted
into `analyze_peatland(aoi)` with the AOI block replaced and the helpers nested so they still close
over `aoi` and the loaded rasters; layer names resolved through `settings.layer_path`. The reader
follows notebook commit `85af76e` verbatim: `filled=False` masked reads, and a layer with no
coverage over the AOI falls back to zeros on the ecosystem reference grid ("no value status")
instead of raising.
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
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DISTURBANCE_THRESHOLD,
        THREAT_ECOSYSTEM,
        THREAT_FIRE_RISK,
        THREAT_FOREST_2024,
        THREAT_FOREST_LOSS,
        THREAT_HISTORICAL,
        THREAT_PEAT_CANAL,
        THREAT_PEAT_CANALS_DENSITY,
        THREAT_PEATLAND,
        THREAT_REMAINING_FOREST,
    )
    from ..settings import layer_path
except ImportError:  # `python peatland.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import (
        THREAT_CURRENT_FOREST,
        THREAT_DISTURBANCE,
        THREAT_DISTURBANCE_THRESHOLD,
        THREAT_ECOSYSTEM,
        THREAT_FIRE_RISK,
        THREAT_FOREST_2024,
        THREAT_FOREST_LOSS,
        THREAT_HISTORICAL,
        THREAT_PEAT_CANAL,
        THREAT_PEAT_CANALS_DENSITY,
        THREAT_PEATLAND,
        THREAT_REMAINING_FOREST,
    )
    from settings import layer_path

# Aliased to the notebook's own names so the body below reads exactly as published.
ECOSYSTEM = THREAT_ECOSYSTEM
HISTORICAL = THREAT_HISTORICAL
FOREST_2024 = THREAT_FOREST_2024
DISTURBANCE = THREAT_DISTURBANCE
PEAT_CANALS_DENSITY = THREAT_PEAT_CANALS_DENSITY
FIRE_RISK = THREAT_FIRE_RISK
DRAINAGE_CANALS = THREAT_PEAT_CANAL
PEATLAND = THREAT_PEATLAND
REMAINING_FOREST = THREAT_REMAINING_FOREST
FOREST_LOSS = THREAT_FOREST_LOSS
CURRENT_FOREST = THREAT_CURRENT_FOREST
DISTURBANCE_THRESHOLD = THREAT_DISTURBANCE_THRESHOLD


def analyze_peatland(aoi: AOI):
    """3.4. Peatland: areas, plus canal proximity, drainage pressure and fire risk."""

    # SEAM: the notebook opens a hardcoded USER_AOI here. `common.prepare_aoi` has already done
    # that, so the prepared AOI is used and the checks below stay as written.
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


    # =============================================================================
    # READ RASTER
    # =============================================================================

    # Notebook commit `85af76e` ("no value status"), VERBATIM including `filled=False` (team
    # call 2026-08-28): a layer whose footprint misses the AOI entirely -- the disturbance
    # raster stops at ~10N, the canal layers only exist over peat regions -- reads as ZEROS on
    # the reference grid instead of raising. The ONE deviation is a make-it-run fix of the
    # notebook's own NameError (the cell dropped the `raster_crs`/`drainage_crs` bindings its
    # polygonise step still reads); reported upstream, same class as the GEOD precedent.
    def read_raster(
        path,
        reference=None,
        reference_transform=None,
        reference_crs=None
    ):

        with rasterio.open(layer_path(path)) as src:

            polygon = (
                aoi.to_crs(src.crs)
                if aoi.crs != src.crs
                else aoi
            )

            try:

                data, raster_transform = mask(
                    src,
                    polygon.geometry,
                    crop=True,
                    filled=False
                )

                return (
                    data[0],
                    raster_transform,
                    src.crs
                )

            except ValueError as e:

                if (
                    "Input shapes do not overlap raster" not in str(e)
                    or reference is None
                ):
                    raise

                # No raster coverage inside AOI -> return zero
                data = np.ma.array(
                    np.zeros(
                        reference.shape,
                        dtype="float32"
                    ),
                    mask=False
                )

                return (
                    data,
                    reference_transform,
                    reference_crs
                )

    # =============================================================================
    # AREA CALCULATION
    # EPSG:4326 -> hectares
    # =============================================================================

    GEOD = Geod(
        ellps="WGS84"
    )


    def area_ha(
        binary_mask,
        transform
    ):

        binary_mask = np.asarray(
            binary_mask,
            dtype=bool
        )

        total_m2 = 0.0

        pixel_width = abs(
            transform.a
        )

        for row in range(
            binary_mask.shape[0]
        ):

            count = np.count_nonzero(
                binary_mask[row]
            )

            if count == 0:
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

            pixel_m2, _ = (
                GEOD.polygon_area_perimeter(
                    [west, east, east, west],
                    [north, north, south, south]
                )
            )

            total_m2 += (
                count
                * abs(pixel_m2)
            )

        return (
            total_m2 / 10000
        )


    # =============================================================================
    # LOAD RASTERS
    # =============================================================================

    ecosystem, transform, raster_crs = read_raster(
        ECOSYSTEM
    )

    historical, historical_transform, _ = read_raster(
        HISTORICAL,
        ecosystem,
        transform,
        raster_crs
    )

    forest2024, forest2024_transform, _ = read_raster(
        FOREST_2024,
        ecosystem,
        transform,
        raster_crs
    )

    disturbance, disturbance_transform, _ = read_raster(
        DISTURBANCE,
        ecosystem,
        transform,
        raster_crs
    )

    canal_density, canal_transform, _ = read_raster(
        PEAT_CANALS_DENSITY,
        ecosystem,
        transform,
        raster_crs
    )

    fire_risk, fire_transform, _ = read_raster(
        FIRE_RISK,
        ecosystem,
        transform,
        raster_crs
    )

    drainage_canals, drainage_transform, drainage_crs = read_raster(
        DRAINAGE_CANALS,
        ecosystem,
        transform,
        raster_crs
    )

    # =============================================================================
    # 1. TOTAL PEATLAND
    #
    # AOI
    # -> ecosystem_v3 == 3
    # =============================================================================

    peatland_mask = (
        ecosystem == PEATLAND
    )

    total_area = area_ha(
        peatland_mask,
        transform
    )


    # =============================================================================
    # 2. REMAINING PEATLAND FOREST
    #
    # Peatland
    # -> historical_deforestation_v3 == 1
    # =============================================================================

    remaining_mask = (
        peatland_mask
        & (
            historical
            == REMAINING_FOREST
        )
    )

    remaining_area = area_ha(
        remaining_mask,
        transform
    )


    # =============================================================================
    # 3. CURRENT PEATLAND FOREST
    #
    # Peatland
    # -> forest_2024_v3 == 1
    # =============================================================================

    current_peatland_mask = (
        peatland_mask
        & (
            forest2024
            == CURRENT_FOREST
        )
    )


    # =============================================================================
    # 4. DISTURBED PEATLAND
    #
    # Current peatland forest
    # -> forest_disturbance_v3 > 0
    # =============================================================================

    disturbed_mask = (
        current_peatland_mask
        & (
            disturbance
            > DISTURBANCE_THRESHOLD
        )
    )

    disturbed_area = area_ha(
        disturbed_mask,
        transform
    )


    # =============================================================================
    # 5. CONVERTED / LOSS
    #
    # Peatland
    # -> historical_deforestation_v3 == 2
    # =============================================================================

    loss_mask = (
        peatland_mask
        & (
            historical
            == FOREST_LOSS
        )
    )

    loss_area = area_ha(
        loss_mask,
        transform
    )


    # =============================================================================
    # PERCENTAGES
    # =============================================================================

    def percentage(area):

        if total_area == 0:
            return 0

        return (
            area
            / total_area
            * 100
        )


    remaining_percentage = percentage(
        remaining_area
    )

    disturbed_percentage = percentage(
        disturbed_area
    )

    loss_percentage = percentage(
        loss_area
    )


    # =============================================================================
    # CANAL PROXIMITY
    #
    # drainage_canal_v3.tif
    # pixel 1 = drainage canal
    #
    # Current peatland forest -> nearest canal
    #
    # <= 500 m       = High
    # > 500-1000 m   = Medium
    # > 1000 m       = Low
    # =============================================================================

    from rasterio.features import shapes
    from shapely.geometry import shape


    def mask_to_geometry(
        binary_mask,
        transform,
        crs
    ):

        geometries = [
            shape(geom)
            for geom, value in shapes(
                binary_mask.astype(
                    np.uint8
                ),
                mask=binary_mask,
                transform=transform
            )
            if value == 1
        ]

        if not geometries:
            return None

        return (
            gpd.GeoSeries(
                geometries,
                crs=crs
            )
            .union_all()
        )


    def canal_proximity_level():

        # Current peatland forest
        if not np.any(
            current_peatland_mask
        ):
            return "Not identified", None


        # Drainage canal pixel = 1
        canal_mask = (
            drainage_canals == 1
        )


        if not np.any(
            canal_mask
        ):
            return "Low", None


        # Convert raster masks to geometry
        peat_forest_geometry = mask_to_geometry(
            current_peatland_mask,
            transform,
            raster_crs
        )

        canal_geometry = mask_to_geometry(
            canal_mask,
            drainage_transform,
            drainage_crs
        )


        if (
            peat_forest_geometry is None
            or canal_geometry is None
        ):
            return "Low", None


        # Use projected CRS for metre distance
        metric_crs = (
            aoi.estimate_utm_crs()
        )


        peat_metric = (
            gpd.GeoSeries(
                [peat_forest_geometry],
                crs=raster_crs
            )
            .to_crs(metric_crs)
            .iloc[0]
        )


        canal_metric = (
            gpd.GeoSeries(
                [canal_geometry],
                crs=drainage_crs
            )
            .to_crs(metric_crs)
            .iloc[0]
        )


        # Nearest distance
        distance_m = (
            peat_metric.distance(
                canal_metric
            )
        )


        if distance_m <= 500:

            level = "High"

        elif distance_m <= 1000:

            level = "Medium"

        else:

            level = "Low"


        return (
            level,
            float(distance_m)
        )


    canal_proximity, canal_distance_m = (
        canal_proximity_level()
    )
    # =============================================================================
    # 7. DRAINAGE PRESSURE
    #
    # Peatland
    # -> peat_canals_density_v3
    #
    # 3 = High
    # 2 = Medium
    # 1 = Low
    #
    # UI uses highest class present.
    # =============================================================================

    drainage_values = np.unique(
        canal_density[
            peatland_mask
        ]
    )


    if 3 in drainage_values:

        drainage_pressure = (
            "High"
        )

    elif 2 in drainage_values:

        drainage_pressure = (
            "Medium"
        )

    elif 1 in drainage_values:

        drainage_pressure = (
            "Low"
        )

    else:

        drainage_pressure = (
            "Not identified"
        )


    # =============================================================================
    # 8. FIRE RISK
    #
    # Peatland
    # -> fire_risk_v3
    #
    # 4 = High
    # 3 = Medium
    # 2 = Low
    # 1 = Very low
    # 0 / NoData = No risk
    #
    # UI uses highest class present.
    # =============================================================================

    fire_values = np.unique(
        fire_risk[
            peatland_mask
        ]
    )


    if 4 in fire_values:

        fire_risk_level = (
            "High"
        )

    elif 3 in fire_values:

        fire_risk_level = (
            "Medium"
        )

    elif 2 in fire_values:

        fire_risk_level = (
            "Low"
        )

    elif 1 in fire_values:

        fire_risk_level = (
            "Very low"
        )

    else:

        fire_risk_level = (
            "No risk"
        )


    # =============================================================================
    # BACKEND RESULT
    # =============================================================================

    result = {

        "peatland": {

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

            "converted_loss": {

                "area_ha":
                    round(
                        loss_area,
                        2
                    ),

                "percentage":
                    round(
                        loss_percentage,
                        2
                    )
            },

            "drivers": {

                "canal_proximity":
                    canal_proximity,

                "canal_distance_m":
                    (
                        round(
                            canal_distance_m,
                            2
                        )
                        if canal_distance_m
                        is not None
                        else None
                    ),

                "drainage_pressure":
                    drainage_pressure,

                "fire_risk":
                    fire_risk_level
            }
        }
    }

    return result


if __name__ == "__main__":
    # Run this section on its own, no Flask app and no stream:
    #     python peatland.py [aoi path]
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
    print(json.dumps(to_jsonable(analyze_peatland(aoi)), indent=2, ensure_ascii=False))
