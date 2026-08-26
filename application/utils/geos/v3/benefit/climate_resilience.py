"""
Component 5.12 Enhance resilience to climate hazards (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, commit `bfe9500`, 2026-08-26). Script-style
cell hoisted per the 5.9-5.11 convention; the computation is the notebook's own.

Population exposed to High/Very High hazard (classes 4 and 5 of the threat risk layers:
flood, fire, landslide, typhoon/storm) at the project's END YEAR, using the FuturePop raster
(one band per 5 years, 2025-2100; the nearest band to base year + duration is read with sum
resampling). The flood risk layer is the reference grid, per the notebook.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
import rasterio
from pyproj import Geod
from rasterio.features import geometry_mask, geometry_window
from rasterio.vrt import WarpedVRT
from rasterio.warp import Resampling

try:
    from ..common import AOI
    from ..config import (
        AOH_GDAL_OPTIONS,
        FUTUREPOP_RASTER,
        FUTUREPOP_YEARS,
        POP_BASE_YEAR,
        THREAT_FIRE_RISK,
        THREAT_FLOOD_RISK,
        THREAT_LANDSLIDE_RISK,
        THREAT_STORM_RISK,
    )
    from ..settings import layer_path
except ImportError:  # `python climate_resilience.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI
    from config import (
        AOH_GDAL_OPTIONS,
        FUTUREPOP_RASTER,
        FUTUREPOP_YEARS,
        POP_BASE_YEAR,
        THREAT_FIRE_RISK,
        THREAT_FLOOD_RISK,
        THREAT_LANDSLIDE_RISK,
        THREAT_STORM_RISK,
    )
    from settings import layer_path


def analyze_climate_resilience(aoi: AOI, duration_years: int,
                               pop_base_year: int = POP_BASE_YEAR) -> tuple[dict, dict]:
    """Component 5.12. Hazard-exposed area and projected population at the project end year."""
    HAZARDS = {
        "Flood": layer_path(THREAT_FLOOD_RISK),
        "Fire": layer_path(THREAT_FIRE_RISK),
        "Landslide": layer_path(THREAT_LANDSLIDE_RISK),
        "Typhoon": layer_path(THREAT_STORM_RISK),
    }

    # ---- PROJECT YEAR -> WORLDPOP BAND (notebook body) ----
    project_year = pop_base_year + duration_years

    # FuturePop is every 5 years
    years = FUTUREPOP_YEARS

    # Choose nearest available FuturePop year
    population_year = min(
        years,
        key=lambda y: abs(y - project_year)
    )

    population_band = years.index(population_year) + 1

    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # ---- REFERENCE GRID ----
        # Use flood raster as reference grid
        reference_path = HAZARDS["Flood"]

        ref = rasterio.open(reference_path)

        geom = aoi.geometry.to_crs(ref.crs).union_all()

        window = geometry_window(
            ref,
            [geom.__geo_interface__]
        )

        transform = ref.window_transform(window)
        shape = (
            int(window.height),
            int(window.width)
        )

        inside_aoi = geometry_mask(
            [geom.__geo_interface__],
            out_shape=shape,
            transform=transform,
            invert=True
        )

        # ---- READ RASTER ON REFERENCE GRID ----
        def read_grid(path, band=1, resampling=Resampling.nearest):

            with rasterio.open(path) as src:

                with WarpedVRT(
                    src,
                    crs=ref.crs,
                    transform=ref.transform,
                    width=ref.width,
                    height=ref.height,
                    resampling=resampling
                ) as vrt:

                    return vrt.read(
                        band,
                        window=window,
                        masked=True
                    ).filled(0)

        # ---- PIXEL AREA (HA) ----
        geod = Geod(ellps="WGS84")
        row_ha = []

        for row in range(shape[0]):

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
            shape
        )

        # ---- FUTURE POPULATION ----
        population = read_grid(
            layer_path(FUTUREPOP_RASTER),
            band=population_band,
            resampling=Resampling.sum
        )

        population = np.where(
            inside_aoi,
            population,
            0
        )

        # ---- HAZARD EXPOSURE ----
        results = {}
        multi_hazard = np.zeros(shape, dtype=bool)

        for name, path in HAZARDS.items():

            hazard = read_grid(path)

            # 4 = High, 5 = Very High
            exposed = (
                inside_aoi
                & np.isin(hazard, [4, 5])
            )

            multi_hazard |= exposed

            exposed_area_ha = pixel_ha[
                exposed
            ].sum()

            exposed_population = population[
                exposed
            ].sum()

            results[name] = {
                "area_ha": float(exposed_area_ha),
                "population": float(exposed_population)
            }

        # ---- UNIQUE MULTI-HAZARD EXPOSURE ----
        unique_area_ha = pixel_ha[
            multi_hazard
        ].sum()

        unique_population = population[
            multi_hazard
        ].sum()

        ref.close()

    narrative = (
        f"Implementing NbS in this ecosystem can reduce disaster exposure across "
        f"an estimated {unique_area_ha:,.2f} ha, helping to lower risk for an "
        f"estimated {unique_population:,.0f} people over the project's "
        f"{duration_years}-year duration."
    )

    hazard_rows = [{"hazard": name, **vals} for name, vals in results.items()]
    values = {
        "duration_years": duration_years,
        "population_base_year": pop_base_year,
        "population_year": population_year,
        "population_band": population_band,
        "unique_hazard_area_ha": float(unique_area_ha),
        "projected_population": float(unique_population),
        "by_hazard": results,
    }
    component_results = {'narrative': narrative, 'tables': {"hazards": hazard_rows},
                         'values': values, 'flags': []}
    # The card contract: multi-hazard union (headline) and the per-hazard exposure rows.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'duration_years', 'population_year', 'unique_hazard_area_ha',
            'projected_population')},
        'hazards': hazard_rows,
    }
    return component_results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python climate_resilience.py [aoi path] [duration]
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
    results, view_results = analyze_climate_resilience(aoi, duration)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False, default=str))
