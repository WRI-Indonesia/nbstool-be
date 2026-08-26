"""
Component 5.11 Improved forest productivity and regeneration -- Potential biodiversity uplift
(ex-ante).

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, commits `02da862`..`881e2f4`, 2026-08-26).
Script-style cell hoisted per the 5.9/5.10 convention; the computation is the notebook's own.

Method (notebook markdown): a spatially explicit adaptation of the area-adjusted Condition
approach of the SD VISta Nature Framework v1.0 -- condition uplift = (historical gain rate +
degradation rate) x duration, capped at 1, applied to the Restore area, giving
CONDITION-ADJUSTED HECTARES; then per species, the share of its AOI habitat overlapping the
Restore area, condition-adjusted.

GRID NOTE: unlike 5.9/5.10 (deforestation-risk grid), this cell works on the PATHWAY raster's
own grid -- `rasterio.mask` crop for the reference, WarpedVRT alignment for everything else,
geodesic per-row pixel areas. Ported as-is.

Seams, output-identical: layer paths through `layer_path`; the species loop runs on a thread
pool with per-worker GDAL Env (the 2.3 trick), results collected in candidate order.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import geopandas as gpd
import numpy as np
import pandas as pd
import rasterio
from pyproj import Geod
from rasterio.features import geometry_mask
from rasterio.mask import mask
from rasterio.vrt import WarpedVRT
from rasterio.warp import Resampling

try:
    from ..common import AOI
    from ..config import (
        AOH_GDAL_OPTIONS,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        DEGRADATION_CODE,
        FOREST_CHANGE_RASTER,
        FOREST_CHANGE_YEARS,
        GAIN_CODE,
        IUCN_MAP,
        PATHWAY_RASTER,
        RASTER_COL,
        RESTORE_CODE,
        SPECIES_COL,
        STATUS_COL,
    )
    from ..settings import layer_path
    from ..site_characterisation.nature.habitat_area import _load_inventory
except ImportError:  # `python biodiversity_uplift.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]
                           / "site_characterisation" / "nature"))
    from common import AOI
    from config import (
        AOH_GDAL_OPTIONS,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        DEGRADATION_CODE,
        FOREST_CHANGE_RASTER,
        FOREST_CHANGE_YEARS,
        GAIN_CODE,
        IUCN_MAP,
        PATHWAY_RASTER,
        RASTER_COL,
        RESTORE_CODE,
        SPECIES_COL,
        STATUS_COL,
    )
    from habitat_area import _load_inventory
    from settings import layer_path


# ---- RASTER HELPERS (notebook body) ----

def pixel_area(transform, shape):
    geod = Geod(ellps="WGS84")
    areas = []

    for row in range(shape[0]):
        north = transform.f + row * transform.e
        south = north + transform.e
        west = transform.c
        east = west + transform.a

        area, _ = geod.polygon_area_perimeter(
            [west, east, east, west],
            [north, north, south, south]
        )

        areas.append(abs(area) / 10000)

    return np.broadcast_to(
        np.array(areas)[:, None],
        shape
    )


def read_reference(path, aoi, band=1):
    with rasterio.open(path) as src:
        aoi = aoi.to_crs(src.crs)

        data, transform = mask(
            src,
            aoi.geometry,
            crop=True,
            filled=False,
            indexes=band
        )

        return (
            data,
            transform,
            src.crs,
            aoi
        )


def read_like(path, transform, crs, shape, band=1):
    with rasterio.open(path) as src:
        with WarpedVRT(
            src,
            crs=crs,
            transform=transform,
            width=shape[1],
            height=shape[0],
            resampling=Resampling.nearest
        ) as vrt:

            return vrt.read(
                band,
                masked=True
            ).filled(0)


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to measure -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def analyze_biodiversity_uplift(aoi: AOI, duration_years: int) -> tuple[dict, dict]:
    """Component 5.11. Condition-adjusted habitat gain on the Restore area, per species."""
    with rasterio.Env(**AOH_GDAL_OPTIONS):
        # ---- AOI + RESTORATION AREA (notebook body; USER_AOI read -> prepared AOI) ----
        pathway, transform, crs, aoi_r = read_reference(
            layer_path(PATHWAY_RASTER),
            aoi.geometry,
            band=1
        )

        shape = pathway.shape

        inside_aoi = geometry_mask(
            aoi_r.geometry,
            out_shape=shape,
            transform=transform,
            invert=True
        )

        pixel_ha = pixel_area(
            transform,
            shape
        )

        restore = (
            inside_aoi
            & ~np.ma.getmaskarray(pathway)
            & (pathway.data == RESTORE_CODE)
        )

        restore_ha = pixel_ha[
            restore
        ].sum()

        if restore_ha <= 0:
            return _na(
                "No area of this project falls under the Restore pathway, so there is no "
                "restoration to project a biodiversity uplift from."
            )

        # ---- TREE COVER GAIN + DEGRADATION ----
        change = read_like(
            layer_path(FOREST_CHANGE_RASTER),
            transform,
            crs,
            shape
        )

        gain = (
            restore
            & (change == GAIN_CODE)
        )

        degradation = (
            restore
            & (change == DEGRADATION_CODE)
        )

        gain_10yr_ha = pixel_ha[gain].sum()
        degradation_10yr_ha = pixel_ha[degradation].sum()

        gain_rate = (
            gain_10yr_ha
            / restore_ha
            / FOREST_CHANGE_YEARS
        )

        degradation_rate = (
            degradation_10yr_ha
            / restore_ha
            / FOREST_CHANGE_YEARS
        )

        # ---- POTENTIAL BIODIVERSITY UPLIFT ----
        condition_uplift = min(
            1.0,
            (gain_rate + degradation_rate)
            * duration_years
        )

        biodiversity_uplift_ha = (
            restore_ha
            * condition_uplift
        )

        # ---- SPECIES INVENTORY ----
        inventory = _load_inventory()

        aoi_inventory = aoi.geometry.to_crs(
            inventory.crs
        ).union_all()

        candidates = inventory[
            inventory.intersects(
                aoi_inventory
            )
        ].copy()

        root = layer_path(AOH_RASTER_ROOT)

        # ---- HABITAT BENEFIT PER SPECIES ----
        # Seam: reads overlap on a thread pool (per-worker Env); collected in candidate order.
        def _species_row(sp):
            with rasterio.Env(**AOH_GDAL_OPTIONS):
                habitat = read_like(
                    f"{root}/{sp[RASTER_COL]}",
                    transform,
                    crs,
                    shape
                )

            habitat_aoi = (
                inside_aoi
                & (habitat == 1)
            )

            habitat_ha = pixel_ha[
                habitat_aoi
            ].sum()

            if habitat_ha <= 0:
                return None

            # Habitat overlapping restoration area
            restore_habitat = (
                habitat_aoi
                & restore
            )

            restore_habitat_ha = pixel_ha[
                restore_habitat
            ].sum()

            # Condition-adjusted habitat gain
            habitat_gain_ha = (
                restore_habitat_ha
                * condition_uplift
            )

            habitat_gain_pct = (
                habitat_gain_ha
                / habitat_ha
                * 100
            )

            status = IUCN_MAP.get(
                str(sp[STATUS_COL])
                .strip()
                .upper()
            )

            return {
                "species": sp[SPECIES_COL],
                "iucn": status,
                "habitat_aoi_ha": round(habitat_ha, 2),
                "restoration_overlap_ha": round(restore_habitat_ha, 2),
                "habitat_gain_ha": round(habitat_gain_ha, 2),
                "habitat_gain_pct": round(habitat_gain_pct, 2)
            }

        rows = []
        with ThreadPoolExecutor(max_workers=AOH_MAX_WORKERS) as pool:
            for row in pool.map(_species_row, [sp for _, sp in candidates.iterrows()]):
                if row is not None:
                    rows.append(row)

    species_df = pd.DataFrame(rows)

    # Species that actually overlap restoration
    benefiting = species_df[
        species_df["habitat_gain_ha"] > 0
    ].copy() if not species_df.empty else species_df

    # ---- SUMMARY ----
    species_count = len(benefiting)

    threatened = benefiting[
        benefiting["iucn"].isin(
            ["CR", "EN", "VU"]
        )
    ] if species_count else benefiting

    threatened_count = len(threatened)

    threatened_pct = (
        threatened_count
        / species_count
        * 100
        if species_count else 0
    )

    cr = (threatened["iucn"] == "CR").sum() if species_count else 0
    en = (threatened["iucn"] == "EN").sum() if species_count else 0
    vu = (threatened["iucn"] == "VU").sum() if species_count else 0

    # Mean species-level habitat increase
    mean_habitat_gain_pct = (
        benefiting["habitat_gain_pct"].mean()
        if species_count else 0
    )

    # Area-weighted habitat increase
    weighted_habitat_gain_pct = (
        benefiting["habitat_gain_ha"].sum()
        / benefiting["habitat_aoi_ha"].sum()
        * 100
        if species_count else 0
    )

    narrative = (
        f"By restoring degraded areas, this project may expand habitat for "
        f"{species_count} species, including {threatened_pct:.1f}% threatened species: "
        f"{cr} CR, {en} EN, and {vu} VU. "
        f"Over the project\u2019s {duration_years} year duration, the intervention may "
        f"increase suitable habitat by {mean_habitat_gain_pct:.2f}% , with an "
        f"area-weighted habitat gain of {weighted_habitat_gain_pct:.2f}%"
    )

    benefiting_sorted = (benefiting.sort_values("habitat_gain_pct", ascending=False)
                         if species_count else benefiting)

    species_rows = benefiting_sorted.to_dict(orient="records")
    values = {
        "duration_years": duration_years,
        "restore_ha": float(restore_ha),
        "gain_10yr_ha": float(gain_10yr_ha),
        "degradation_10yr_ha": float(degradation_10yr_ha),
        "gain_rate_pct_per_year": float(gain_rate * 100),
        "degradation_rate_pct_per_year": float(degradation_rate * 100),
        "condition_uplift_pct": float(condition_uplift * 100),
        "biodiversity_uplift_ha": float(biodiversity_uplift_ha),
        "species_benefiting": int(species_count),
        "threatened_count": int(threatened_count),
        "threatened_pct": float(threatened_pct),
        "cr": int(cr),
        "en": int(en),
        "vu": int(vu),
        "mean_habitat_gain_pct": float(mean_habitat_gain_pct),
        "weighted_habitat_gain_pct": float(weighted_habitat_gain_pct),
    }
    results = {'narrative': narrative, 'tables': {"species": species_rows}, 'values': values,
               'flags': []}
    # The card contract: species expanded (headline), the uplift figures and per-species rows.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'duration_years', 'restore_ha', 'condition_uplift_pct', 'biodiversity_uplift_ha',
            'species_benefiting', 'threatened_count', 'threatened_pct', 'cr', 'en', 'vu',
            'mean_habitat_gain_pct', 'weighted_habitat_gain_pct')},
        'species': species_rows,
    }
    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python biodiversity_uplift.py [aoi path] [duration]
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
    results, view_results = analyze_biodiversity_uplift(aoi, duration)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    out = to_jsonable(view_results)
    out["species"] = out.get("species", [])[:6]
    print(json.dumps(out, indent=2, ensure_ascii=False, default=str))
