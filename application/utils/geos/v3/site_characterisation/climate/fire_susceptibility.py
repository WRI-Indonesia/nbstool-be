"""
Component 3.5 Fire Susceptibility.

The distribution of fire susceptibility classes over the AOI, as a bar chart. The narrative is
FIXED TEXT, not derived from the data: the finding is the chart, and a generated sentence would
only restate it less precisely.

Data. The notebook reads `disaster_risks/hazard_fire.tif` on a 1..5 encoding. That file is not in
the v3 bucket. The only fire layer published is `risk_fire_v3.tif`, the same raster 1.7 reads, on
a 1..4 encoding, so config aliases FIRE_HAZARD_RASTER to it.

WHAT THAT CHANGES: four bars here where the notebook shows five, and the labels top out at High
rather than Very High. The component logic is untouched -- same tabulation, same denominator,
same dominant-class rule -- only the layer and its legend differ, because no other fire layer
exists. DATA_STATUS in the notebook records the same thing from its side: 3.5 is to be reconciled
when it is rebuilt.

3.5 vs 1.7, same raster read two different ways. 1.7 reports a REPRESENTATIVE level using the
conservative rule "highest class covering at least 20% of the area", because a screening tool
should not miss a real risk. 3.5 reports the DOMINANT class, simply the largest by area, because
a chart of the distribution wants its biggest bar named. The two can disagree on the same site and
that is correct, not a bug.

Decisions locked.
- Denominator is the valid raster area, so the bars sum to 100. Same base as 1.7.
- Every class is shown, including those at zero, so an absent class is visible as absent.
"""

from __future__ import annotations

try:
    from ...common import AOI, dominant, load_raster_clipped, not_applicable, tabulate_classes
    from ...config import FIRE_HAZARD_RASTER, FIRE_LEVELS
except ImportError:  # `python fire_susceptibility.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, dominant, load_raster_clipped, not_applicable, tabulate_classes
    from config import FIRE_HAZARD_RASTER, FIRE_LEVELS

FIRE_NARRATIVE = (
    "This shows how likely the land is to burn under baseline conditions, based on factors "
    "such as land cover, dryness, and climate. It is not a forecast of current fire danger."
)


def analyze_fire_susceptibility(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.5. Fire susceptibility distribution over the AOI.

    The narrative is fixed text, not derived from the data. The finding is the chart.
    """
    raster = load_raster_clipped(FIRE_HAZARD_RASTER, aoi, resampling="nearest")

    if raster.valid_area_ha <= 0:
        empty = not_applicable(
            "3.5 Fire Susceptibility",
            "No fire susceptibility data is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'fire_susceptibility': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'fire_susceptibilities': [], 'fire_susceptibility_class': None}
        return results, view_results

    # Denominator is the valid hazard area, so the bars sum to 100. Same base as 1.7.
    rows = tabulate_classes(raster, FIRE_LEVELS, denominator_ha=raster.valid_area_ha)
    dom = dominant(rows)

    results = {
        'narrative': FIRE_NARRATIVE,
        'tables': {'fire_susceptibility': rows},   # one bar per class, always all of them
        'values': {
            # Chart metadata travels with the series so the frontend does not hardcode units.
            'chart_series': "fire_susceptibility",
            'chart_unit': "%",
            'chart_axis_label': "Share of project area (%)",
            # Largest class by area. This is NOT the representative level in 1.7, which uses the
            # conservative "highest class covering at least 20%" rule. See the module docstring.
            'dominant_class': dom.code if dom else None,
            'dominant_label': dom.label if dom else None,
            'assessed_ha': raster.valid_area_ha,
        },
        'flags': [],
    }

    view_results = {
        'fire_susceptibilities': [
            {'id': str(row.code), 'name': row.label,
             'area': row.area_ha, 'percentage': row.pct}
            for row in rows
        ],
        'fire_susceptibility_class': (
            {'id': str(dom.code), 'name': dom.label} if dom else None
        ),
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python fire_susceptibility.py [aoi path]
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
    results, view_results = analyze_fire_susceptibility(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
