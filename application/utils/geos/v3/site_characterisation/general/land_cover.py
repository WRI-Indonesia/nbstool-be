"""
Component 1.8 Land Cover.

Land cover composition of the project area, from the 2024 20-class RLCMS map, as a bar chart
ordered by area.

Data. `SEA_LC2024.tif`, 20 classes plus 0 for no data. This is NOT the forest mask: 1.5 reads
`SEA_FC2024.tif` for that, and the two answer different questions. Reading LC2024 to derive forest
would apply a different forest definition from the one 1.5 uses, so the two components would
disagree about the same AOI.

Decisions locked.
- Denominator is the whole site, and a "No data / other" row absorbs code 0 plus anything outside
  the raster, so the full table sums to 100.
- Snow, Water and Other land stay their own classes. They are real land cover, not absence of it,
  and folding them into No-data would hide a lake.
- Two tables are emitted: `land_cover_full` (every class present, sums to 100) and
  `land_cover_top6` (the six largest real classes, no No-data row). The chart reads the full
  table; the top 6 exists for a compact card.

Downstream use. Land cover is the starting state every intervention is measured against, and
`dominant_class` is what the endpoint's `land_cover_class` reports.
"""

from __future__ import annotations

try:
    from ...common import (
        AOI,
        ClassShare,
        load_raster_clipped,
        not_applicable,
        safe_pct,
        sort_by_area,
        tabulate_classes,
    )
    from ...config import (
        LAND_COVER_COLORS,
        LAND_COVER_KEYS,
        LC2024_CLASSES,
        LC2024_RASTER,
        LC_TOP_N,
    )
except ImportError:  # `python land_cover.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        ClassShare,
        load_raster_clipped,
        not_applicable,
        safe_pct,
        sort_by_area,
        tabulate_classes,
    )
    from config import (
        LAND_COVER_COLORS,
        LAND_COVER_KEYS,
        LC2024_CLASSES,
        LC2024_RASTER,
        LC_TOP_N,
    )

LC_NODATA_LABEL = "No data / other"


def _row_view(row: ClassShare) -> dict:
    """One bar of the chart, in the shape 1.1 and 1.4 already use for their distributions."""
    # Contract shape: id, dict, area, percentage. `name` and `color` are carried too -- the
    # contract does not ask for them, but 1.1 and 1.4 send both and dropping them here would make
    # land cover the one distribution the frontend cannot colour.
    return {
        'id': str(row.code),
        'dict': {'key': LAND_COVER_KEYS.get(row.code), 'fallback': row.label},
        'name': row.label,
        'area': row.area_ha,
        'percentage': row.pct,
        'color': LAND_COVER_COLORS.get(row.code),
    }


def analyze_land_cover(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.8. Land cover composition of the AOI, from the 2024 20-class map."""
    lc = load_raster_clipped(LC2024_RASTER, aoi, resampling="nearest")
    if lc.valid_area_ha <= 0:
        empty = not_applicable(
            "1.8 Land Cover", "No land cover data is available for this project area."
        )
        results = {'narrative': empty.narrative,
                   'tables': {'land_cover_full': [], 'land_cover_top6': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'land_cover_class': []}
        return results, view_results

    # Denominator is the whole site; a No-data/other row absorbs code 0 so the full table sums
    # to 100. Snow, Water and Other land are kept as their own classes, not folded into No-data.
    rows = tabulate_classes(lc, LC2024_CLASSES, denominator_ha=aoi.area_ha)
    mapped_ha = sum(r.area_ha for r in rows)
    other_ha = max(0.0, aoi.area_ha - mapped_ha)
    rows.append(ClassShare(code=0, label=LC_NODATA_LABEL, area_ha=other_ha,
                           pct=safe_pct(other_ha, aoi.area_ha)))

    full = sort_by_area([r for r in rows if r.area_ha > 0])              # full table, sums to 100
    top6 = sort_by_area([r for r in full if r.code != 0])[:LC_TOP_N]     # six largest LC classes
    dom = top6[0] if top6 else None

    results = {
        'narrative': "The major land cover categories in the selected area are:",
        'tables': {'land_cover_full': full, 'land_cover_top6': top6},
        'values': {
            'chart_series': "land_cover_full",
            'chart_unit': "%",
            'chart_axis_label': "Share of project area (%)",
            'dominant_class': dom.label if dom else None,
            'class_count': sum(1 for r in full if r.code != 0),
        },
        'flags': [],
    }

    # The contract's `land_cover_class` is the whole distribution, not the dominant class: one
    # row per class present, ordered by area. `dom` stays in `results` for the narrative.
    view_results = {'land_cover_class': [_row_view(row) for row in full]}

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python land_cover.py [aoi path]
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
    results, view_results = analyze_land_cover(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
