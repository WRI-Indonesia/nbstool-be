"""
Component 5.10 Reduced vulnerability to fire, pests, and disease -- Threatened Species Habitat
Benefit.

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, 2026-08-25), the cell under the 5.10 heading.
Script-style cell hoisted into a function per the 2.3/2.5/2.6 convention; the computation is the
notebook's own. Same grid discipline as 5.9: the deforestation-risk raster's native EPSG:4326
grid, WarpedVRT-aligned reads, geodesic per-row pixel areas -- deliberately NOT
load_raster_clipped.

Per threatened species (CR/EN/VU on the inventory's redlistCategory): suitable habitat inside
the AOI, and the share of it that projected deforestation would take, i.e. the avoidable loss.
"""

from __future__ import annotations

import math
from concurrent.futures import ThreadPoolExecutor

import geopandas as gpd
import numpy as np
import pandas as pd
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
        IUCN_MAP,
        PROB_RASTER,
        RASTER_COL,
        SPECIES_COL,
        STATUS_COL,
        THREAT_ECOSYSTEM,
    )
    from ..settings import layer_path
    from ..site_characterisation.nature.habitat_area import _load_inventory
except ImportError:  # `python threatened_species.py`: no package around it
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
        IUCN_MAP,
        PROB_RASTER,
        RASTER_COL,
        SPECIES_COL,
        STATUS_COL,
        THREAT_ECOSYSTEM,
    )
    from habitat_area import _load_inventory
    from settings import layer_path


def analyze_threatened_species_habitat(aoi: AOI, duration_years: int, rate_pct: float | None,
                                       ecosystem_class: int = ECOSYSTEM_CLASS) -> ComponentResult:
    """Component 5.10. Threatened species whose habitat overlaps projected deforestation."""
    component = "5.10 Reduced vulnerability to fire, pests, and disease"

    if rate_pct is None:
        return not_applicable(
            component,
            "No historical deforestation rate is available for this project area, so projected "
            "habitat loss cannot be estimated.",
        )

    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # ---- AOI + DEFORESTATION RISK GRID (notebook body) ----
        risk_src = rasterio.open(layer_path(PROB_RASTER))

        geom = aoi.geometry.to_crs(risk_src.crs).union_all()

        window = geometry_window(risk_src, [geom.__geo_interface__])
        transform = risk_src.window_transform(window)

        risk = risk_src.read(1, window=window, masked=True).filled(0)

        inside_aoi = geometry_mask(
            [geom.__geo_interface__],
            risk.shape,
            transform,
            invert=True
        )

        def read(path):
            """Read raster aligned to the deforestation-risk grid."""
            with rasterio.open(path) as src:
                with WarpedVRT(
                    src,
                    crs=risk_src.crs,
                    transform=risk_src.transform,
                    width=risk_src.width,
                    height=risk_src.height,
                    resampling=Resampling.nearest
                ) as vrt:
                    return vrt.read(1, window=window, masked=True).filled(0)

        # ---- PIXEL AREA (HA) ----
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

        # ---- PROJECTED DEFORESTATION ----
        ecosystem = read(layer_path(THREAT_ECOSYSTEM))

        ecosystem_mask = (
            inside_aoi
            & (ecosystem == ecosystem_class)
        )

        risk_pool = (
            ecosystem_mask
            & (risk > 1)
        )

        risk_area = pixel_ha[risk_pool].sum()

        projected_loss = (
            risk_area
            * (1 - math.exp(-(rate_pct / 100) * duration_years))
        )

        def allocate_loss(mask, target_ha):

            allocation = np.zeros(risk.shape, dtype="float32")
            idx = np.flatnonzero(mask)

            idx = idx[
                np.argsort(-risk.ravel()[idx], kind="stable")
            ]

            areas = pixel_ha.ravel()[idx]
            cumulative = np.cumsum(areas)

            n = np.searchsorted(cumulative, target_ha, side="right")

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

        # ---- SPECIES INVENTORY ----
        inventory = _load_inventory()

        aoi_inventory = aoi.geometry.to_crs(inventory.crs).union_all()

        # Only species whose raster footprint intersects the AOI
        candidates = inventory[
            inventory.intersects(aoi_inventory)
        ].copy()

        root = layer_path(AOH_RASTER_ROOT)

        # ---- HABITAT BENEFIT PER SPECIES ----
        # Seam: reads overlap on a thread pool (2.3 precedent, per-worker Env); results are
        # collected in candidate order, so even the pre-sort row order matches the serial loop.
        rows = []
        total_species = 0

        def _species_row(sp):
            with rasterio.Env(**AOH_GDAL_OPTIONS):
                habitat = (
                    inside_aoi
                    & (read(f"{root}/{sp[RASTER_COL]}") == 1)
                )

            habitat_area = pixel_ha[habitat].sum()

            if habitat_area <= 0:
                return None

            status_raw = str(sp[STATUS_COL]).strip().upper()
            status = IUCN_MAP.get(status_raw)

            if status is None:
                return "counted"

            avoided_ha = (
                pixel_ha
                * loss
                * habitat
            ).sum()

            avoided_habitat_loss_pct = (
                avoided_ha
                / habitat_area
                * 100
            )

            return {
                "species": sp[SPECIES_COL],
                "iucn": status,
                "habitat_aoi_ha": round(habitat_area, 2),
                "habitat_loss_avoided_ha": round(avoided_ha, 2),
                "avoided_habitat_loss_pct": round(avoided_habitat_loss_pct, 2),
            }

        with ThreadPoolExecutor(max_workers=AOH_MAX_WORKERS) as pool:
            for outcome in pool.map(_species_row, [sp for _, sp in candidates.iterrows()]):
                if outcome is None:
                    continue
                total_species += 1
                if outcome != "counted":
                    rows.append(outcome)

        risk_src.close()

    # ---- SUMMARY ----
    species_df = pd.DataFrame(rows)

    if not species_df.empty:
        species_df = species_df.sort_values(
            "avoided_habitat_loss_pct",
            ascending=False
        )

    threatened_count = len(species_df)

    cr = (species_df["iucn"] == "CR").sum() if threatened_count else 0
    en = (species_df["iucn"] == "EN").sum() if threatened_count else 0
    vu = (species_df["iucn"] == "VU").sum() if threatened_count else 0

    threatened_pct = (
        threatened_count / total_species * 100
        if total_species else 0
    )

    # Threatened species whose habitat overlaps projected deforestation
    benefiting_species = (
        (species_df["habitat_loss_avoided_ha"] > 0).sum()
        if threatened_count else 0
    )

    # Overall avoided habitat loss percentage across threatened species
    if threatened_count:
        total_threatened_habitat = species_df["habitat_aoi_ha"].sum()
        total_avoided_habitat = species_df["habitat_loss_avoided_ha"].sum()

        avoided_habitat_loss_pct = (
            total_avoided_habitat
            / total_threatened_habitat
            * 100
            if total_threatened_habitat > 0 else 0
        )
    else:
        avoided_habitat_loss_pct = 0

    narrative = (
        f"By protecting at-risk habitat, this project supports "
        f"habitat for {total_species} species, including {threatened_pct:.1f}% "
        f"threatened species: {cr} CR, {en} EN, and {vu} VU. "
        f"Over the project's {duration_years}-year duration, the intervention "
        f"may reduce threat levels by protecting at least {avoided_habitat_loss_pct:.1f}% "
        f"of habitat"
        f"for {benefiting_species} threatened species."
    )

    return ComponentResult(
        component=component,
        applicable=True,
        narrative=narrative,
        tables={"species": species_df.to_dict(orient="records")},
        values={
            "duration_years": duration_years,
            "rate_pct": rate_pct,
            "projected_loss_ha": float(projected_loss),
            "species_with_habitat": int(total_species),
            "threatened_count": int(threatened_count),
            "threatened_pct": float(threatened_pct),
            "cr": int(cr),
            "en": int(en),
            "vu": int(vu),
            "avoided_habitat_loss_pct": float(avoided_habitat_loss_pct),
            "benefiting_species": int(benefiting_species),
        },
    )


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python threatened_species.py [aoi path] [duration] [rate_pct]
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
    result = analyze_threatened_species_habitat(aoi, duration, rate)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    out = to_jsonable(result)
    out["tables"]["species"] = out["tables"]["species"][:8]
    print(json.dumps(out, indent=2, ensure_ascii=False, default=str))
