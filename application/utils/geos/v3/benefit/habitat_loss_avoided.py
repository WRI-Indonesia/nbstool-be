"""
Component 5.9 Enhanced biodiversity and ecosystem function -- Habitat Loss Avoided.

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, 2026-08-25), the cell under the 5.9 heading.
The notebook writes it as a top-level script with hardcoded USER_AOI / RATE_PCT /
PROJECT_DURATION / ECOSYSTEM_CLASS; per the porting convention for script-style cells (2.3, 2.5,
2.6 precedent) the body is hoisted into a function taking the prepared AOI and those parameters,
with NOTHING inside the computation changed.

DELIBERATELY NOT load_raster_clipped: the cell works on the deforestation-risk raster's own
EPSG:4326 grid (window over the AOI, WarpedVRT-aligned reads, geodesic per-row pixel areas).
Reprojecting to REFERENCE_CRS would move its numbers.

Two seams beyond parameterisation, both output-identical:
  - habitat rasters come from the species inventory's bbox prefilter instead of `rglob` over the
    four class folders. A raster whose footprint misses the AOI window reads as all zeros here,
    contributing nothing to the union or the species count, so skipping it changes nothing.
  - reads run inside `rasterio.Env(**AOH_GDAL_OPTIONS)` (sidecar probing off), byte-identical
    and several times faster over /vsicurl -- the 2.3 trick.
"""

from __future__ import annotations

import math
from concurrent.futures import ThreadPoolExecutor

import geopandas as gpd
import numpy as np
import rasterio
from pyproj import Geod
from rasterio.features import geometry_mask, geometry_window
from rasterio.vrt import WarpedVRT
from rasterio.warp import Resampling

try:
    from ..common import AOI, ComponentResult, not_applicable
    from ..config import (
        AOH_GDAL_OPTIONS,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        ECOSYSTEM_CLASS,
        ECOSYSTEM_NAMES,
        PROB_RASTER,
        RASTER_COL,
        THREAT_ECOSYSTEM,
    )
    from ..settings import layer_path
    from ..site_characterisation.nature.habitat_area import _load_inventory
except ImportError:  # `python habitat_loss_avoided.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]
                           / "site_characterisation" / "nature"))
    from common import AOI, ComponentResult, not_applicable
    from config import (
        AOH_GDAL_OPTIONS,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        ECOSYSTEM_CLASS,
        ECOSYSTEM_NAMES,
        PROB_RASTER,
        RASTER_COL,
        THREAT_ECOSYSTEM,
    )
    from habitat_area import _load_inventory
    from settings import layer_path


def analyze_habitat_loss_avoided(aoi: AOI, duration_years: int, rate_pct: float | None,
                                 ecosystem_class: int = ECOSYSTEM_CLASS) -> ComponentResult:
    """Component 5.9. Suitable habitat whose projected loss the project avoids, in hectares."""
    component = "5.9 Enhanced biodiversity and ecosystem function"

    if rate_pct is None:
        return not_applicable(
            component,
            "No historical deforestation rate is available for this project area, so projected "
            "habitat loss cannot be estimated.",
        )

    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # ---- REFERENCE GRID + AOI (notebook body, USER_AOI read -> prepared AOI) ----
        risk_src = rasterio.open(layer_path(PROB_RASTER))

        geom = aoi.geometry.to_crs(risk_src.crs).union_all()

        window = geometry_window(
            risk_src,
            [geom.__geo_interface__]
        )

        transform = risk_src.window_transform(window)

        risk = risk_src.read(
            1,
            window=window,
            masked=True
        ).filled(0)

        inside_aoi = geometry_mask(
            [geom.__geo_interface__],
            risk.shape,
            transform,
            invert=True
        )

        # ---- READ RASTER ON SAME GRID ----
        def read(path):
            with rasterio.open(path) as src:
                with WarpedVRT(
                    src,
                    crs=risk_src.crs,
                    transform=risk_src.transform,
                    width=risk_src.width,
                    height=risk_src.height,
                    resampling=Resampling.nearest
                ) as vrt:
                    return vrt.read(
                        1,
                        window=window,
                        masked=True
                    ).filled(0)

        # ---- PIXEL AREA (HA) FOR EPSG:4326 ----
        geod = Geod(ellps="WGS84")

        row_ha = []

        for row in range(risk.shape[0]):

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
            risk.shape
        )

        # ---- ECOSYSTEM + PROJECTED LOSS ----
        ecosystem = read(layer_path(THREAT_ECOSYSTEM))

        ecosystem_mask = (
            inside_aoi
            & (ecosystem == ecosystem_class)
        )

        risk_pool = (
            ecosystem_mask
            & (risk > 1)
        )

        ecosystem_area = pixel_ha[
            ecosystem_mask
        ].sum()

        risk_area = pixel_ha[
            risk_pool
        ].sum()

        if ecosystem_area <= 0:
            risk_src.close()
            return not_applicable(
                component,
                f"This project area contains no {ECOSYSTEM_NAMES.get(ecosystem_class, 'target')} "
                "ecosystem, so there is no habitat to assess.",
            )

        projected_loss = (
            risk_area
            * (
                1 - math.exp(
                    -(rate_pct / 100)
                    * duration_years
                )
            )
        )

        # ---- ALLOCATE LOSS: HIGHEST RISK FIRST ----
        def allocate_loss(mask, target_ha):

            allocation = np.zeros(
                risk.shape,
                dtype="float32"
            )

            idx = np.flatnonzero(mask)

            idx = idx[
                np.argsort(
                    -risk.ravel()[idx],
                    kind="stable"
                )
            ]

            areas = pixel_ha.ravel()[idx]
            cumulative = np.cumsum(areas)

            n = np.searchsorted(
                cumulative,
                target_ha,
                side="right"
            )

            allocation.ravel()[idx[:n]] = 1

            used = areas[:n].sum()

            if n < len(idx):
                allocation.ravel()[idx[n]] = min(
                    (target_ha - used) / areas[n],
                    1
                )

            return allocation

        loss = allocate_loss(
            risk_pool,
            projected_loss
        )

        # ---- HABITAT UNION ----
        # Seam: the notebook rglobs every tif under the four class folders; the inventory's bbox
        # prefilter skips only rasters that would read all-zero here. Output-identical.
        habitat_union = np.zeros(
            risk.shape,
            dtype=bool
        )

        species_count = 0

        inventory = _load_inventory()
        aoi_4326 = aoi.geometry.to_crs(inventory.crs).union_all()
        candidates = inventory[inventory.intersects(aoi_4326)]
        root = layer_path(AOH_RASTER_ROOT)

        # Seam: the reads overlap on a thread pool (2.3 precedent, GDAL config is thread-local so
        # the Env is entered per worker). Results are reduced in candidate order; union and count
        # are order-independent anyway, so the output is identical to the serial loop.
        def _habitat_of(raster_path):
            with rasterio.Env(**AOH_GDAL_OPTIONS):
                return ecosystem_mask & (read(f"{root}/{raster_path}") == 1)

        with ThreadPoolExecutor(max_workers=AOH_MAX_WORKERS) as pool:
            for habitat in pool.map(_habitat_of,
                                    [sp[RASTER_COL] for _, sp in candidates.iterrows()]):

                habitat_union |= habitat

                if np.any(
                    habitat
                    & (loss > 0)
                ):
                    species_count += 1

        # ---- RESULT ----
        current_habitat = pixel_ha[
            habitat_union
        ].sum()

        habitat_avoided = (
            pixel_ha
            * loss
            * habitat_union
        ).sum()

        name = ECOSYSTEM_NAMES[
            ecosystem_class
        ]

        risk_src.close()

    narrative = (
        f"Conserving this {name} ecosystem is estimated to avoid "
        f"the loss of {habitat_avoided:,.2f} hectares of suitable habitat "
        f"over the project's {duration_years}-year duration."
    )

    return ComponentResult(
        component=component,
        applicable=True,
        narrative=narrative,
        values={
            "ecosystem": name,
            "duration_years": duration_years,
            "rate_pct": rate_pct,
            "ecosystem_area_ha": float(ecosystem_area),
            "risk_area_ha": float(risk_area),
            "projected_loss_ha": float(projected_loss),
            "current_habitat_ha": float(current_habitat),
            "habitat_loss_avoided_ha": float(habitat_avoided),
            "species_affected": int(species_count),
            "candidate_species": int(len(candidates)),
        },
    )


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python habitat_loss_avoided.py [aoi path] [duration] [rate_pct]
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
    rate = float(sys.argv[3]) if len(sys.argv) > 3 else 1.23
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    result = analyze_habitat_loss_avoided(aoi, duration, rate)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(result), indent=2, ensure_ascii=False, default=str))
