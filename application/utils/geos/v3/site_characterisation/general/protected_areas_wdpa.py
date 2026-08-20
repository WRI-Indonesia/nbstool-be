"""
Component 1.3 Protected Areas (WDPA).

Reports how much of the AOI overlaps protected areas and what those areas are designated for.
This is a key eligibility and additionality signal.

Data. `sea.wdpa_wdoecm` in the GIS database, the same source the v2 backend reads. The notebook
read `WDPA_polygon_4326.shp`; there is no WDPA object in the v3 bucket, so this component loads
from the database through `db.load_wdpa_intersecting`, which returns a GeoDataFrame carrying the
shapefile's column names. Only the load line differs from the notebook; the filters, the union
overlap and the narrative below are unchanged.

Decisions locked.
- Headline overlap uses the union of protected areas, because WDPA sites overlap each other and
  summing per site can exceed the AOI.
- No sliver threshold. Even a small overlap is legally meaningful.
- Filter: `STATUS` in {Designated, Inscribed, Established}; drop pure marine (`REALM == 'Marine'`),
  keep coastal for mangrove; polygon features only.
- The narrative names the designation type only, from `DESIG_ENG`. Site name, IUCN category and
  status are dropped from the prose. They stay in the per site table.
- Duplicate designation types are collapsed. Two national parks read as "National Park" once,
  ordered by total overlap area.
- The narrative opens with "Besides", so it assumes the frontend renders 1.2 and 1.3 as
  continuous prose, in that order.
- No percentage in the prose. Hectares only, matching the wording in 1.2.

Known limitation, accepted by the team. When the AOI does not overlap any protected area the
component emits an empty narrative, so nothing is rendered. This conflicts with the locked
decision in 1.7, which shows all five hazard cards precisely so that absence stays visible. The
practical cost here is larger than in 1.7: no overlap is a positive additionality argument, and a
missing card cannot be told apart from a WDPA layer that failed to load. The structured `values`
are still emitted (`protected_ha = 0`, `in_strict_pa = False`), so the pathway module keeps its
signal even with no prose.

Downstream use. A project inside a strict protected area (IUCN Ia, Ib, II) is hard to justify on
additionality, while overlap with unprotected land can strengthen the PROTECT pathway.
`in_strict_pa` carries that flag even though IUCN category is no longer narrated.
"""

from __future__ import annotations

from dataclasses import dataclass

try:
    from ...common import (
        AOI,
        oxford_join,
        per_feature_overlap_ha,
        safe_pct,
        sentences,
        sort_by_area,
        union_overlap_ha,
    )
    from ...config import WDPA_DROP_REALM, WDPA_KEEP_STATUS, WDPA_NO_DESIG, WDPA_STRICT_IUCN
    from ...db import load_wdpa_intersecting
except ImportError:  # `python protected_areas_wdpa.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        oxford_join,
        per_feature_overlap_ha,
        safe_pct,
        sentences,
        sort_by_area,
        union_overlap_ha,
    )
    from config import WDPA_DROP_REALM, WDPA_KEEP_STATUS, WDPA_NO_DESIG, WDPA_STRICT_IUCN
    from db import load_wdpa_intersecting


@dataclass(frozen=True)
class ProtectedSite:
    name: str
    designation: str
    iucn_category: str
    status: str
    area_ha: float


def _unique_designations(sites: list[ProtectedSite]) -> list[str]:
    """Distinct DESIG_ENG values, ordered by total overlap area descending.

    Two national parks in one AOI should read as "National Park" once, not twice. Ordering by
    summed area rather than first appearance keeps the dominant designation first even when a
    tiny site of another type happens to sort earlier.
    """
    totals: dict[str, float] = {}
    for s in sites:
        if s.designation.strip().lower() in WDPA_NO_DESIG:
            continue  # a site with no usable designation cannot fill the slot
        totals[s.designation] = totals.get(s.designation, 0.0) + s.area_ha
    return sorted(totals, key=totals.get, reverse=True)


def analyze_protected_areas(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.3. Legal protection status of the site."""
    gdf = load_wdpa_intersecting(aoi)

    if not gdf.empty:
        gdf = gdf[gdf["STATUS"].isin(WDPA_KEEP_STATUS)]
        gdf = gdf[gdf["REALM"].astype(str).str.strip() != WDPA_DROP_REALM]
        gdf = gdf[gdf.geometry.geom_type.isin(["Polygon", "MultiPolygon"])]

    if gdf.empty:
        # Team decision: render nothing when there is no overlap. Values are still emitted so
        # the pathway module keeps the additionality signal. See the note above.
        results = {
            'narrative': "",
            'tables': {'sites': []},
            'values': {'protected_ha': 0.0, 'protected_pct': 0.0, 'in_strict_pa': False},
            'flags': [],
        }
        view_results = {
            'overlapping_protected_area': 0.0,
            'overlapping_protected_name': None,
            'overlapping_protected_area_percentage': 0.0,
        }
        return results, view_results

    protected_ha = union_overlap_ha(aoi, gdf)
    protected_pct = safe_pct(protected_ha, aoi.area_ha)  # kept for the frontend, not narrated

    areas = per_feature_overlap_ha(aoi, gdf)
    sites = sort_by_area([
        ProtectedSite(
            name=str(gdf.iloc[i].get("NAME", "Unnamed site")),
            designation=str(gdf.iloc[i].get("DESIG_ENG", "Not Reported")),
            iucn_category=str(gdf.iloc[i].get("IUCN_CAT", "Not Reported")),
            status=str(gdf.iloc[i]["STATUS"]),
            area_ha=float(areas[i]),
        )
        for i in range(len(gdf))
    ])

    overlap_sentence = (
        f"Besides, this project area overlaps with {protected_ha:,.0f} hectares of "
        "protected areas."
    )

    flags: list[str] = []
    missing: list[str] = []
    designations = _unique_designations(sites)
    if designations:
        designation_sentence = (
            "The protected area within the polygon is designated for "
            f"{oxford_join(designations)}."
        )
    else:
        # Every overlapping site lacks a usable DESIG_ENG. Drop the second sentence rather than
        # print "designated for Not Reported".
        designation_sentence = ""
        # ABSENCE: the source rows carry no designation, so there is nothing to show and nothing
        # to fix. `missing`, not `flags`.
        missing.append(
            "1.3: overlapping WDPA sites carry no usable DESIG_ENG. The designation sentence "
            "is omitted."
        )

    # Additionality constraint, read by the pathway module even though it is no longer narrated.
    in_strict_pa = any(s.iucn_category in WDPA_STRICT_IUCN for s in sites)

    results = {
        'narrative': sentences(overlap_sentence, designation_sentence),
        'tables': {'sites': sites},
        'values': {
            'protected_ha': protected_ha,
            'protected_pct': protected_pct,
            'in_strict_pa': in_strict_pa,
            'designations': designations,
        },
        'flags': flags,
        'missing': missing,
    }

    view_results = {
        'overlapping_protected_area': protected_ha,
        # Site names, not designations: the prose already carries the designation, and the card
        # names the place. Several overlapping sites are joined, dominant first.
        'overlapping_protected_name': ", ".join(s.name for s in sites),
        'overlapping_protected_area_percentage': protected_pct,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python protected_areas_wdpa.py [aoi path]
    # The AOI is any file geopandas reads: a zipped shapefile, .shp, .geojson, .gpkg.
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\thailand.zip"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_protected_areas(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
