"""
Component 1.4 Terrain (Slope and Elevation).

Reports slope and elevation as two distributions, each a bar chart, plus a short narrative.

Data. ONE continuous DEM in metres (`SEA_ELEVATION_54034.tif`). Slope is not a layer: it is the
horizontal gradient of that DEM, computed by `common.slope_percent_from_dem`, which corrects the
per-pixel run for the latitude distortion of the equal-area reference CRS. Both are then binned
by this component, elevation at ELEVATION_BREAKS (500 / 1000 / 2000 m) and slope at SLOPE_BREAKS
(8 / 15 / 25 / 40 percent).

| Slope code | Label            | | Elevation code | Label             |
|---|---|---|---|---|
| 1 | Flat             | | 1 | Lowland           |
| 2 | Gently sloping   | | 2 | Submontane / hill |
| 3 | Moderately steep | | 3 | Montane           |
| 4 | Steep            | | 4 | Upper montane     |
| 5 | Very steep       | |   |                   |

Decisions locked.
- Denominator is the raster's valid area, so the shares of each distribution sum to 100 over the
  pixels that carry data. Bins are upper-exclusive.
- The narrative quotes exact metres ("ranges from 12 to 1043 m asl"), taken from the continuous
  DEM before binning, and names the predominant class alongside them. An earlier version quoted
  class labels at both ends ("from Lowland to Montane"), which threw away the precision the DEM
  actually carries.

History, because the numbers moved. An earlier version of this port read two PRE-CLASSIFIED
rasters, `srtm_elevation_v3.tif` (codes 1..7 plus 15 for water) and `srtm_slope_v3.tif` (codes
1..5), and tallied them the way the v2 backend does, with the denominator set to pixels greater
than zero. That was not a formatting difference: it is a different definition of every class. On
AOI1 it reported Flat at 6.2% where the notebook reports 24.2%. Those rasters are no longer read.
If the DEM is missing, 1.4 fails rather than falling back to them, because a silent legend swap is
worse than an absent card.

Downstream use. Terrain is a design constraint, not only a description. Steep slopes limit some
activities and raise erosion risk; elevation guides species and forest type choice. F02-P5 benefit
5.3 montane zoning reads the metre DEM directly.
"""

from __future__ import annotations

try:
    from ...common import (
        AOI,
        classify_continuous,
        dominant,
        load_raster_clipped,
        not_applicable,
        sentences,
        slope_percent_from_dem,
        tabulate_classes,
    )
    from ...config import (
        ELEVATION_BREAKS,
        ELEVATION_CLASSES,
        ELEVATION_KEYS,
        ELEVATION_RASTER,
        SLOPE_BREAKS,
        SLOPE_CLASSES,
        SLOPE_COLORS,
        SLOPE_KEYS,
    )
except ImportError:  # `python terrain_slope_elevation.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        classify_continuous,
        dominant,
        load_raster_clipped,
        not_applicable,
        sentences,
        slope_percent_from_dem,
        tabulate_classes,
    )
    from config import (
        ELEVATION_BREAKS,
        ELEVATION_CLASSES,
        ELEVATION_KEYS,
        ELEVATION_RASTER,
        SLOPE_BREAKS,
        SLOPE_CLASSES,
        SLOPE_COLORS,
        SLOPE_KEYS,
    )


def _elevation_dict(code: int | None) -> dict | None:
    """One frontend label object, or None when there is no class to name."""
    if code is None:
        return None
    return {'key': ELEVATION_KEYS.get(code), 'fallback': ELEVATION_CLASSES[code]}


def analyze_terrain(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.4. Slope and elevation profile."""
    # Only elevation is read (continuous, metres). Slope is DERIVED from it, because a standalone
    # slope raster was not available. Both are then binned into class codes.
    elev_c = load_raster_clipped(ELEVATION_RASTER, aoi, resampling="nearest")
    if elev_c.valid_area_ha <= 0:
        empty = not_applicable(
            "1.4 Terrain", "No elevation data is available for this project area."
        )
        results = {'narrative': empty.narrative, 'tables': {'slope': [], 'elevation': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'slopes': [], 'min_elevation': None, 'max_elevation': None,
                        'predominant_elevation_dict': None}
        return results, view_results

    slope_c = slope_percent_from_dem(elev_c, aoi)
    slope = classify_continuous(slope_c, SLOPE_BREAKS)
    elev = classify_continuous(elev_c, ELEVATION_BREAKS)

    slope_rows = tabulate_classes(slope, SLOPE_CLASSES, denominator_ha=slope.valid_area_ha)
    elev_rows = tabulate_classes(elev, ELEVATION_CLASSES, denominator_ha=elev.valid_area_ha)

    dom_slope = dominant(slope_rows)
    dom_elev = dominant(elev_rows)

    # Min and max in actual metres, from the continuous elevation raster (the class raster
    # cannot give metres). elev_c.values is masked, so min and max ignore nodata.
    vmin = int(round(float(elev_c.values.min())))
    vmax = int(round(float(elev_c.values.max())))

    elev_clause = ""
    if dom_elev:
        # Flat AOI (a single elevation value): avoid "ranges from 12 to 12 m".
        elev_clause = (
            f"Elevation is {vmin} m above sea level (asl), predominantly {dom_elev.label}."
            if vmin == vmax
            else f"Elevation ranges from {vmin} to {vmax} m above sea level (asl), "
                 f"predominantly {dom_elev.label}."
        )
    slope_clause = f"Slopes are predominantly {dom_slope.label}." if dom_slope else ""

    results = {
        'narrative': sentences(elev_clause, slope_clause),
        'tables': {'slope': slope_rows, 'elevation': elev_rows},
        'values': {
            'dominant_slope': dom_slope.code if dom_slope else None,
            'dominant_elevation': dom_elev.code if dom_elev else None,
        },
        'flags': [],
    }

    view_results = {
        'slopes': [
            {
                'id': str(row.code),
                'dict': {'key': SLOPE_KEYS.get(row.code), 'fallback': row.label},
                'area': row.area_ha,
                'percentage': row.pct,
                'color': SLOPE_COLORS.get(row.code),
            }
            for row in slope_rows
        ],
        # Exact metres from the continuous DEM, the same figures the narrative quotes.
        'min_elevation': vmin,
        'max_elevation': vmax,
        'predominant_elevation_dict': _elevation_dict(dom_elev.code if dom_elev else None),
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python terrain_slope_elevation.py [aoi path]
    # The AOI is any file geopandas reads: a zipped shapefile, .shp, .geojson, .gpkg.
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
    results, view_results = analyze_terrain(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
