"""
Component 3.x Burned Area History.

How much of the project area has burned, and in which years.

NOT A NOTEBOOK COMPONENT. The v3 Climate notebook has no burned-area section at all. This follows
the V2 backend's `get_climate_burned_area_data` (utils/geos/current_condition.py), on the team's
instruction to use the v2 version, and it reads v2's layers under `assets-geo/baseline/` rather
than anything in the v3 bucket.

Two figures, from two different layers, and the difference between them matters:

  `total_burned_area`      the area of every cell that burned AT LEAST ONCE, from the frequency
                           raster. A cell that burned four times contributes its area once.
  `historical_burned_areas` ten per-year areas, 2011-2020, from ten 0/1 masks.

THEY DO NOT ADD UP, and that is correct. The annual series counts place-years; the total counts
places. Wherever the same ground burned in two different years the annual series counts it twice
and the total counts it once, so summing the chart will normally exceed the headline. On the
Indonesian test AOI the annual series totals 710 burned cells against 685 distinct ones.

`burn_frequency` is v2's own summary of the frequency raster: the midpoint of its minimum and
maximum inside the AOI, rounded. It is a crude statistic -- a range midpoint, not a mean -- and it
is kept because it is what v2 reports and what the existing narrative quotes.

Units. The annual areas are a pixel count times a NOMINAL 250 m cell, exactly as v2 computes them,
rather than by reprojecting to an equal-area CRS. The total is measured on the equal-area
REFERENCE_CRS grid through `load_raster_clipped`, as everything else in this module is. The two
therefore rest on slightly different area definitions; that is v2's behaviour and changing it
would move the numbers away from the ones already in production.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
import rasterio
from rasterio.mask import mask
from rasterio.transform import array_bounds
from rasterio.warp import Resampling, calculate_default_transform, reproject

try:
    from ...common import AOI, not_applicable
    from ...config import (
        BURNED_ANNUAL_PIXEL_M,
        BURNED_ANNUAL_RASTER,
        BURNED_FREQUENCY_RASTER,
        BURNED_YEARS,
        REFERENCE_CRS,
    )
except ImportError:  # `python burned_area.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, not_applicable
    from config import (
        BURNED_ANNUAL_PIXEL_M,
        BURNED_ANNUAL_RASTER,
        BURNED_FREQUENCY_RASTER,
        BURNED_YEARS,
        REFERENCE_CRS,
    )

M2_PER_HA = 10_000.0


# ============================ V2'S OWN RASTER PATH ============================
# These two helpers reproduce `clip_raster_to_aoi` + `reproject_raster` +
# `calculate_raster_area` / `calculate_stats_pixel_value` from utils/geos/current_condition.py.
# They deliberately do NOT go through `common.load_raster_clipped`, and that is the whole point:
# an earlier version of this component did, and its figures came out 2 to 8 per cent away from
# v2's. Two reasons, both in the grid rather than the arithmetic.
#
#   THE TOTAL. v2 clips in the source CRS, writes the clip out, and derives the destination grid
#   from the CLIPPED bounds. `load_raster_clipped` derives it from the FULL source bounds and then
#   translates to the AOI window. Same CRS, different pixel origin, so a different number of cells
#   straddle the polygon edge and the area moves.
#
#   THE ANNUAL SERIES. v2 never reprojects it at all -- it counts burned cells in native EPSG:4326
#   and multiplies by a NOMINAL 250 m. Reprojecting a 0/1 mask to an equal-area CRS resamples it,
#   which changes the count outright: 1,281 ha against v2's 1,225 ha for 2011 on indonesia_3.
#
# So this matches v2 grid for grid. It is not the more rigorous way to measure an area -- the
# nominal 250 m cell is not the true one, and the total's grid depends on where the AOI happens to
# fall -- but parity with the figures already in production is what was asked for.


def _clip_native(path, geoms):
    """v2's clip_raster_to_aoi, without the write: the raster cropped to the AOI, still in its
    own CRS, with everything outside the polygon set to the source nodata."""
    with rasterio.open(path) as src:
        out_image, out_transform = mask(src, geoms, crop=True)
        return out_image[0], out_transform, src.crs, src.nodata


def _reproject_clip(values, transform, src_crs, dst_crs):
    """v2's reproject_raster: destination grid derived from the CLIPPED raster's own bounds.

    v2 writes the clip to a GeoTIFF and reprojects that file, so the destination starts as zeros
    and reproject only fills what the source covers. `np.zeros` reproduces exactly that, which
    matters because the area test below is `>= 1`: an untouched destination cell must not count.
    """
    height, width = values.shape
    bounds = array_bounds(height, width, transform)
    dst_transform, dst_w, dst_h = calculate_default_transform(
        src_crs, dst_crs, width, height, *bounds
    )
    dst = np.zeros((dst_h, dst_w), dtype=values.dtype)
    reproject(
        source=values, destination=dst,
        src_transform=transform, src_crs=src_crs,
        dst_transform=dst_transform, dst_crs=dst_crs,
        resampling=Resampling.nearest,
    )
    return dst, dst_transform


@dataclass(frozen=True)
class BurnedYear:
    year: int
    burned_ha: float


def analyze_burned_area(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.x. Burned area over the AOI, total and by year, reproducing v2 exactly."""
    # v2 passes the AOI as it stands in EPSG:4326; ours arrives in REFERENCE_CRS.
    geoms = list(aoi.geometry.to_crs(4326))

    try:
        clipped, transform, src_crs, src_nodata = _clip_native(BURNED_FREQUENCY_RASTER, geoms)
    except ValueError:
        clipped = None

    if clipped is None or clipped.size == 0:
        empty = not_applicable(
            "3.x Burned Area",
            "No burned area data is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'burned_years': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        return results, {'total_burned_area': 0.0, 'historical_burned_areas': []}

    # v2 reprojects the CLIP, then measures on that grid.
    warped, dst_transform = _reproject_clip(clipped, transform, src_crs, REFERENCE_CRS)

    # v2's calculate_raster_area: every cell that burned at least once, times the destination
    # cell size. `>= 1` is v2's own test, and it is what makes this a count of PLACES rather than
    # of burn events -- a cell that burned four times contributes its area once.
    burned_cells = int(np.sum(warped >= 1))
    pixel_area_ha = abs(dst_transform.a * dst_transform.e) / M2_PER_HA
    total_burned_ha = burned_cells * pixel_area_ha

    # v2's burn_freq: the midpoint of the observed range, rounded. Not a mean. Taken over the
    # reprojected raster with nodata excluded, exactly as calculate_stats_pixel_value does.
    stats_values = warped[warped != src_nodata] if src_nodata is not None else warped.ravel()
    burn_frequency = (
        int(round((float(stats_values.min()) + float(stats_values.max())) / 2))
        if stats_values.size else 0
    )

    rows: list[BurnedYear] = []
    flags: list[str] = []
    notes: list[str] = []
    # Only a failed layer read is worth a retry button. The two quirks below are v2's arithmetic
    # reproduced on purpose and give the same answer every time.
    retryable = False

    # The minimum is taken over EVERY valid cell, and an unburned cell is a valid 0 rather than
    # nodata, so on any site that is mostly unburned the minimum is 0 and the midpoint collapses.
    # On a site whose cells burned at most once it rounds to 0 outright, and the narrative then
    # reads "average frequency 0 occurrences" beside a non-zero burned area. That is v2's
    # arithmetic reproduced exactly, not a fault here, but it is not a usable statistic.
    if burned_cells and burn_frequency < 1:
        flags.append(
            "3.x: burn frequency reports 0 beside a non-zero burned area. It is the midpoint of "
            "the minimum and maximum over all cells, and unburned cells count as 0, so it "
            "collapses on any site that is mostly unburned. Carried over from v2 unchanged."
        )
    for year in BURNED_YEARS:
        try:
            # NOT reprojected: v2 counts in native EPSG:4326 and converts with a nominal 250 m
            # cell. Warping the 0/1 mask first would resample it and change the count.
            values, _, _, nodata = _clip_native(BURNED_ANNUAL_RASTER.format(year=year), geoms)
        except Exception as e:      # one missing year must not cost the other nine
            # Type only: the message carries the layer URL and this string reaches the browser.
            flags.append(f"3.x: the {year} burned-area layer could not be read "
                         f"({type(e).__name__}).")
            retryable = True
            continue
        # v2's calculate_stats_pixel_value sums the band with nodata dropped. The masks hold 0
        # and 1, so the sum is the burned cell count.
        cells = values[values != nodata] if nodata is not None else values.ravel()
        burned = float(cells.sum()) if cells.size else 0.0
        rows.append(BurnedYear(
            year=year,
            burned_ha=round(burned * BURNED_ANNUAL_PIXEL_M ** 2 / M2_PER_HA, 1),
        ))

    annual_total = sum(row.burned_ha for row in rows)
    # A NOTE: the total and the series answer different questions and both are correct. Nothing is
    # degraded, so this must not report `partial`.
    if annual_total > total_burned_ha:
        notes.append(
            f"3.x: the annual series sums to {annual_total:,.0f} ha against a total of "
            f"{total_burned_ha:,.0f} ha. Ground that burned in more than one year is counted once "
            "in the total and once per year in the series; the two answer different questions."
        )

    if total_burned_ha > 0:
        narrative = (
            f"Over the past ten years, fires have affected this project location, impacting up to "
            f"{total_burned_ha:,.1f} hectares of burned area, with an average frequency "
            f"{burn_frequency:,.0f} occurrences in several areas."
        )
    else:
        narrative = ("There are no historical burned area detected within this project area over "
                     "the past ten years.")

    results = {
        'narrative': narrative,
        'tables': {'burned_years': rows},
        'values': {
            'chart_series': "burned_years",
            'chart_unit': "ha",
            'chart_axis_label': "Burned area (ha)",
            'total_burned_ha': total_burned_ha,
            'burn_frequency': burn_frequency,
            'burned_cells': burned_cells,
            'assessed_ha': int(np.sum(warped != src_nodata)) * pixel_area_ha,
            'annual_total_ha': annual_total,
        },
        'flags': flags,
        'notes': notes,
        'retryable': retryable,
    }

    view_results = {
        'total_burned_area': total_burned_ha,
        'historical_burned_areas': [
            {'id': str(row.year), 'year': row.year, 'value': row.burned_ha}
            for row in rows
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python burned_area.py [aoi path]
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
    results, view_results = analyze_burned_area(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
