"""
Component Indigenous Territory.

Reports which indigenous / ethnolinguistic areas overlap the AOI, leading with the majority one
(the largest overlap), so report templates can fill "name of majority indigenous territory".

NOT A NOTEBOOK COMPONENT. The notebook has no counterpart; this was added for the report
templates (2026-08-22, team decision: build a component rather than leave it to user input).
Mechanics mirror 2.2 KBA: overlap measured against the whole AOI, per-area rows plus a
deduplicated union headline.

Data. INDIGENOUS_TABLE in the GIS database -- today `sea.indigenous_ethnicity`, an
ethnolinguistic homelands map, the only polygon layer covering all eleven countries. See
config.py for the candidates considered and why. The names are peoples (Dayak, Pakpak), the
boundaries are INDICATIVE, not legal recognitions of customary tenure: the narrative and any
report using this must say "indicative", and field verification / FPIC remains the real source.

Decisions locked.
- Denominator = total AOI area, same as 2.2: a territory concerns the whole site.
- Sliver threshold INDIGENOUS_SLIVER_PCT (1%), same rule as 1.2 and for the same reason:
  the boundary linework is coarse, and a fraction of a percent of overlap is noise, not a
  resident group.
- Same-named areas are dissolved into one row (the source stores one people as many polygons).
- "No overlap" is applicable, not missing: the layer covers the whole region, so an AOI outside
  every mapped area is a real answer.
"""

from __future__ import annotations

from dataclasses import dataclass

try:
    from ...common import (
        AOI,
        fmt_ha,
        fmt_pct,
        oxford_join,
        per_feature_overlap_ha,
        safe_pct,
        sentences,
        sort_by_area,
        union_overlap_ha,
    )
    from ...config import INDIGENOUS_MAX_SOURCE_HA, INDIGENOUS_SLIVER_PCT
    from ...db import load_indigenous_intersecting
except ImportError:  # `python indigenous_territory.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        fmt_ha,
        fmt_pct,
        oxford_join,
        per_feature_overlap_ha,
        safe_pct,
        sentences,
        sort_by_area,
        union_overlap_ha,
    )
    from config import INDIGENOUS_MAX_SOURCE_HA, INDIGENOUS_SLIVER_PCT
    from db import load_indigenous_intersecting

INDICATIVE_CAVEAT = (
    "Boundaries are indicative ethnolinguistic areas, not legally recognised customary tenure; "
    "field verification and community consultation remain essential."
)


@dataclass(frozen=True)
class IndigenousArea:
    name: str
    area_ha: float
    pct: float


def analyze_indigenous_territory(aoi: AOI) -> tuple[dict, dict]:
    """Overlap with indigenous / ethnolinguistic areas, majority first."""
    gdf = load_indigenous_intersecting(aoi)

    if not gdf.empty:
        # Drop nationwide population-distribution polygons (Han Chinese spans 57M ha of
        # Indonesia and would claim every AOI at 100%). `source_ha` is the group's WHOLE mapped
        # extent, computed by the loader over the full table -- the loaded fragment alone would
        # understate it. See INDIGENOUS_MAX_SOURCE_HA.
        gdf = gdf[gdf["source_ha"] <= INDIGENOUS_MAX_SOURCE_HA]
        # One people, many polygons in the source: dissolve to one row per name so the overlap
        # is measured once and the list does not repeat a name per fragment.
        if not gdf.empty:
            gdf = gdf.dissolve(by="name", as_index=False)

    empty_view = {
        'majority_indigenous_territory': None,
        'overlapping_indigenous_territories': [],
        'overlapping_indigenous_territory_total_size': 0.0,
        'overlapping_indigenous_territory_percentage': 0.0,
    }

    if gdf.empty:
        results = {
            'narrative': "No mapped indigenous or ethnolinguistic areas overlap this project "
                         "area.",
            'tables': {'territories': []},
            'values': {'indigenous_ha': 0.0, 'indigenous_pct': 0.0, 'territory_count': 0},
            'flags': [],
            'notes': [INDICATIVE_CAVEAT],
        }
        return results, empty_view

    overlaps = per_feature_overlap_ha(aoi, gdf)
    areas = sort_by_area([
        IndigenousArea(
            name=str(gdf.iloc[i]["name"]),
            area_ha=float(overlaps[i]),
            pct=safe_pct(float(overlaps[i]), aoi.area_ha),
        )
        for i in range(len(gdf))
    ])
    kept = [a for a in areas if a.pct >= INDIGENOUS_SLIVER_PCT]

    if not kept:
        results = {
            'narrative': "No mapped indigenous or ethnolinguistic area overlaps more than "
                         f"{INDIGENOUS_SLIVER_PCT:g}% of this project area.",
            'tables': {'territories': []},
            'values': {'indigenous_ha': 0.0, 'indigenous_pct': 0.0, 'territory_count': 0},
            'flags': [],
            'notes': [INDICATIVE_CAVEAT],
        }
        return results, empty_view

    total_ha = union_overlap_ha(aoi, gdf)
    total_pct = safe_pct(total_ha, aoi.area_ha)
    majority = kept[0]

    if len(kept) == 1:
        head = (
            f"This project area overlaps the indicative area of the {majority.name} "
            f"({fmt_ha(majority.area_ha)}, {fmt_pct(majority.pct)} of the site)."
        )
    else:
        rest_text = oxford_join(f"{a.name} ({fmt_ha(a.area_ha)})" for a in kept[1:])
        head = (
            f"This project area overlaps the indicative areas of {len(kept)} indigenous or "
            f"ethnolinguistic groups, predominantly the {majority.name} "
            f"({fmt_ha(majority.area_ha)}, {fmt_pct(majority.pct)} of the site), followed by "
            f"{rest_text}."
        )

    results = {
        'narrative': sentences(head, INDICATIVE_CAVEAT),
        'tables': {'territories': kept},
        'values': {
            'indigenous_ha': total_ha,
            'indigenous_pct': total_pct,
            'territory_count': len(kept),
        },
        'flags': [],
        'notes': [INDICATIVE_CAVEAT],
    }

    # The headline total is the DEDUPLICATED UNION over every intersecting polygon, so it can
    # exceed the sum of the kept rows' share where slivers were dropped, and the rows must not be
    # summed for a headline where areas overlap each other.
    view_results = {
        'majority_indigenous_territory': majority.name,
        'overlapping_indigenous_territories': [
            {'indigenous_territory_name': a.name,
             'indigenous_territory_area_size': a.area_ha,
             'indigenous_territory_percentage': a.pct}
            for a in kept
        ],
        'overlapping_indigenous_territory_total_size': total_ha,
        'overlapping_indigenous_territory_percentage': total_pct,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own. Needs the GIS database, not the Flask app:
    #     python indigenous_territory.py [aoi path]
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
    results, view_results = analyze_indigenous_territory(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
