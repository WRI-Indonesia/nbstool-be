"""
Component 2.6 Conservation Significance.

Where the project area sits in the global conservation priority ranking, on three weighting
scenarios: biodiversity alone, biodiversity with carbon, and biodiversity with water.

Data. NatureMap (Jung et al. 2021, Zenodo 10.5281/zenodo.5006332). These are RANKED layers, not
stocks: a cell's value is its percentile of global priority, 1 being the top 1% of the planet. The
archive contains no raw carbon or water rasters, so nothing here is a quantity of carbon or water,
and "Carbon" names a weighting of the ranking rather than a measurement.

What is reported, per axis, is the share of the AOI falling inside the world's top 10% and top 30%
ranked cells, plus the best (lowest) rank the AOI touches. This is NOT the quantity in the paper's
Fig. 1 triangle, which is the share of species targets met.

THIS IS A LANDSCAPE-CONTEXT STATEMENT, NOT A SITE MEASUREMENT, and the resolution is why. The
layers are 10 km, about 100 km2 a cell, and nothing is resampled. `all_touched=True` keeps every
cell the polygon so much as clips, so the denominator is a PIXEL ENVELOPE that is always larger
than the site and can be several times larger: a 637 km2 AOI sits inside an envelope of roughly
2,000 km2. Every percentage below is a share of that envelope, never of the project area. The
notebook warns under 20 valid cells and most project-scale AOIs are well under it, so that warning
is passed through as a flag rather than hidden.

Cell area is computed, not reprojected. The rasters stay in EPSG:4326 and `pixel_area_km2` works
out each row's true ground area from the latitude band it spans, so a cell near the equator counts
for more than one further north. This is the notebook's own rewrite of 2.6 and it is what removed
the component's last blocker: the earlier version needed `globalgrid_mollweide_10km.tif` for land
fraction, which is published in ESRI:54009 while the priority layers were republished in EPSG:4326,
and clipping the two returned arrays of different shapes.

Because of that this component does NOT use `load_raster_clipped`. Everything else in the tool
reprojects to the equal-area REFERENCE_CRS and lets a constant pixel area do the arithmetic; 2.6
must stay in 4326, because its area calculation is derived from the 4326 transform. Only the AOI
crosses the seam, reprojected to 4326 on the way in.

`true_km2` differs from the notebook's by about 0.7%. The notebook measures the polygon in
ESRI:54009 for this one number; here it comes from `aoi.area_ha`, the site area every other
component in the tool divides by, so one payload does not carry two different site areas. It
affects nothing that is reported as a percentage: those all divide by the envelope.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
import rasterio
from rasterio.mask import mask

try:
    from ...common import AOI, not_applicable
    from ...config import (
        EARTH_RADIUS_KM,
        NATUREMAP_BUDGETS,
        NATUREMAP_MIN_PIXELS,
        NATUREMAP_RASTERS,
    )
    from ...settings import layer_path
except ImportError:  # `python conservation_significance.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, not_applicable
    from config import (
        EARTH_RADIUS_KM,
        NATUREMAP_BUDGETS,
        NATUREMAP_MIN_PIXELS,
        NATUREMAP_RASTERS,
    )
    from settings import layer_path


@dataclass(frozen=True)
class PriorityShare:
    """One axis at one global budget: how much of the envelope is inside the world's top X%."""

    scenario: str
    budget_pct: int
    area_km2: float
    area_ha: float
    pct_of_envelope: float
    min_rank: float | None
    valid_px: int


def pixel_area_km2(transform, shape):
    """True ground area (km2) of every cell in a 4326 raster window.

    Longitude cells are equal width everywhere; latitude bands shrink toward the
    poles. Area of a cell spanning [lat1, lat2] x d_lon degrees:
        R^2 * d_lon_rad * (sin(lat2) - sin(lat1))
    Returns a 2-D array matching `shape` (rows x cols)."""
    d_lon = abs(transform.a)                 # degrees per pixel in x
    d_lat = abs(transform.e)                 # degrees per pixel in y
    n_rows, n_cols = shape

    # latitude of each row's top and bottom edge
    top = transform.f                        # y of the upper-left corner
    row_top = top - np.arange(n_rows) * d_lat
    row_bot = row_top - d_lat
    band = (np.sin(np.radians(row_top)) - np.sin(np.radians(row_bot)))  # per row
    d_lon_rad = np.radians(d_lon)
    row_area = (EARTH_RADIUS_KM ** 2) * d_lon_rad * band      # km2 per cell, per row
    return np.repeat(row_area[:, None], n_cols, axis=1)


def clip(path, geoms):
    """Clip raster to geometry. all_touched=True keeps any pixel the polygon
    intersects. Returns (values, area_km2) as 2-D arrays with NaN outside."""
    with rasterio.open(layer_path(path)) as src:
        arr, transform = mask(src, geoms, crop=True, all_touched=True,
                              nodata=src.nodata, filled=True)
        vals = arr[0].astype("float64")
        if src.nodata is not None:
            vals[vals == src.nodata] = np.nan
    area = pixel_area_km2(transform, vals.shape)
    area = np.where(np.isnan(vals), np.nan, area)   # mask area outside polygon too
    return vals, area


def analyze_conservation_significance(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.6. Where the AOI sits in the global conservation priority ranking."""
    # The one seam: the AOI arrives in REFERENCE_CRS and these rasters are 4326, so it goes back.
    geoms = [aoi.geometry.to_crs(4326).union_all().__geo_interface__]
    true_km2 = aoi.area_ha / 100.0

    try:
        ref_vals, ref_area = clip(next(iter(NATUREMAP_RASTERS.values())), geoms)
    except ValueError:
        # rasterio raises when the polygon misses the raster entirely, which for a SEA-only extract
        # means an AOI outside the region rather than a fault.
        ref_vals = ref_area = None

    envelope = float(np.nansum(ref_area)) if ref_area is not None else 0.0
    n_px = int(np.sum(~np.isnan(ref_vals))) if ref_vals is not None else 0

    if n_px == 0 or envelope <= 0:
        empty = not_applicable(
            "2.6 Conservation Significance",
            "No global priority ranking is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'priority_shares': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        return results, {'conservation_significances': [], 'conservation_priority_rank': None}

    flags: list[str] = []
    if n_px < NATUREMAP_MIN_PIXELS:
        flags.append(
            f"2.6: the AOI covers {n_px} priority cells and the source warns below "
            f"{NATUREMAP_MIN_PIXELS}. At 10 km a cell is about 100 km2, so these shares move in "
            "large steps and should be read as landscape context, not as a site figure."
        )
    # A NOTE, NOT A FLAG: at 10 km the envelope is larger than the AOI on essentially every site,
    # so this would mark 2.6 `partial` on every request. Nothing is degraded and there is nothing
    # to retry -- it says which denominator the percentages use. See pipeline.error_status.
    notes: list[str] = []
    if envelope > true_km2:
        notes.append(
            f"2.6: percentages are shares of the {envelope:,.0f} km2 pixel envelope, "
            f"{100 * envelope / true_km2 - 100:+.0f}% larger than the {true_km2:,.0f} km2 project "
            "area, because a 10 km cell is kept whole wherever the polygon touches it."
        )

    rows: list[PriorityShare] = []
    min_ranks: dict[str, float | None] = {}
    for label, path in NATUREMAP_RASTERS.items():
        rank, area = clip(path, geoms)
        valid = int(np.sum(~np.isnan(rank)))
        rmin = float(np.nanmin(rank)) if valid else None
        min_ranks[label] = rmin

        for b in NATUREMAP_BUDGETS:
            sel = rank <= b
            km2 = float(np.nansum(area[sel]))
            rows.append(PriorityShare(
                scenario=label, budget_pct=b,
                area_km2=km2, area_ha=km2 * 100,
                pct_of_envelope=100 * km2 / envelope,
                min_rank=rmin, valid_px=valid,
            ))

    # The headline is the best rank the AOI touches on ANY axis: the strongest claim the data
    # supports is "part of this site is in the global top N%", and the axis it comes from is in
    # the table. Ranks are percentiles, so lower is better.
    ranked = [r for r in min_ranks.values() if r is not None]
    best_rank = min(ranked) if ranked else None

    if best_rank is not None:
        narrative = (
            f"Part of this project area falls within the top {best_rank:,.0f}% of global "
            f"conservation priority. Across the three weighting scenarios, "
            f"{max(r.pct_of_envelope for r in rows if r.budget_pct == 30):,.0f}% of the assessed "
            "area lies inside the global top 30%."
        )
    else:
        narrative = "No global priority ranking is available for this project area."

    results = {
        'narrative': narrative,
        'tables': {'priority_shares': rows},
        'values': {
            'chart_series': "priority_shares",
            'chart_unit': "%",
            'chart_axis_label': "Share of assessed area (%)",
            'min_ranks': min_ranks,
            'best_rank': best_rank,
            'valid_pixels': n_px,
            'envelope_km2': envelope,
            'true_km2': true_km2,
        },
        'flags': flags,
        'notes': notes,
    }

    # NO CONTRACT FIELD EXISTS YET for 2.6 -- the Nature contract the team supplied has nothing for
    # conservation significance. This shape follows 3.5's: one flat row per bar, plus the headline
    # rank on its own. It will need revisiting when the contract catches up.
    view_results = {
        'conservation_significances': [
            {'id': f"{row.scenario}_top{row.budget_pct}",
             'name': f"{row.scenario}, global top {row.budget_pct}%",
             'scenario_id': row.scenario,
             'budget_id': str(row.budget_pct),
             'area': row.area_ha,
             'percentage': row.pct_of_envelope}
            for row in rows
        ],
        'conservation_priority_rank': best_rank,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python conservation_significance.py [aoi path]
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_conservation_significance(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
