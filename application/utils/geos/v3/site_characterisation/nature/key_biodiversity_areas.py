"""
Component 2.2 Key Biodiversity Areas (KBA).

Reports whether the AOI overlaps Key Biodiversity Areas, with overlap area, share and a
narrative.

A KBA is not a protected area. It is a site that contributes significantly to the global
persistence of biodiversity (IUCN KBA Standard 2016). It may or may not be legally protected.
This is a different lens from 1.3 (WDPA, legal status) and the two complement each other. The
narrative emphasises biodiversity importance, not protection.

Data. World Database of Key Biodiversity Areas, BirdLife International and the KBA Partnership.
The notebook reads `SouthEast_Asia_KBA.shp`; no v3 bucket object was published and the backend
already holds the layer, so this reads `sea.key_biodiversity_area` in the GIS database.
`db.load_kba_intersecting` renames the table's `intname` back to the shapefile's `IntName`, so
the component below is unchanged.

Decisions locked.
- Mirrors 1.3 mechanics: headline overlap = union of KBA polygons, so overlapping or nested
  sites are not double counted. No sliver threshold, because any KBA overlap is material.
- Denominator = total AOI area. A KBA concerns the whole site, not only its forest.
- The narrative gives the KBA name only, no criteria or type.
- "No overlap" is applicable, not missing. An AOI outside every KBA is a real answer.

Downstream use. Feeds Triple Win Pillar 1 and acts as a safeguard and eligibility signal. A KBA
that is not also under WDPA protection (1.3) is a biodiversity important but unprotected site,
which is a strong PROTECT and additionality rationale.
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
    from ...db import load_kba_intersecting
except ImportError:  # `python key_biodiversity_areas.py`: no package around it
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
    from db import load_kba_intersecting

KBA_DEFINITION = (
    "Key Biodiversity Areas are sites that contribute significantly to the global persistence "
    "of biodiversity."
)


@dataclass(frozen=True)
class KbaSite:
    name: str
    area_ha: float


def analyze_kba(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.2. Overlap with Key Biodiversity Areas."""
    # The notebook calls load_vector_intersecting(KBA_POLYGON, aoi) against a shapefile. The
    # database loader returns the same frame with the same column names, so what the body below
    # receives is unchanged.
    gdf = load_kba_intersecting(aoi)

    if gdf.empty:
        results = {
            'narrative': "This project area does not overlap any Key Biodiversity Areas.",
            'tables': {'sites': []},
            'values': {'kba_ha': 0.0, 'kba_pct': 0.0, 'kba_site_count': 0},
            'flags': [],
        }
        view_results = {
            'overlapping_key_biodiversity_areas': [],
            'overlapping_key_biodiversity_area_total_size': 0.0,
            'overlapping_key_biodiversity_area_percentage': 0.0,
        }
        return results, view_results

    kba_ha = union_overlap_ha(aoi, gdf)
    kba_pct = safe_pct(kba_ha, aoi.area_ha)

    areas = per_feature_overlap_ha(aoi, gdf)
    sites = sort_by_area([
        KbaSite(name=str(gdf.iloc[i].get("IntName", "Unnamed KBA")), area_ha=float(areas[i]))
        for i in range(len(gdf))
    ])

    if len(sites) == 1:
        head = (
            f"This project area overlaps {fmt_ha(kba_ha)} ({fmt_pct(kba_pct)}) of a Key "
            f"Biodiversity Area, {sites[0].name}."
        )
    else:
        largest, *rest = sites
        rest_text = oxford_join(f"{s.name} ({fmt_ha(s.area_ha)})" for s in rest)
        head = (
            f"This project area overlaps {fmt_ha(kba_ha)} ({fmt_pct(kba_pct)}) of Key "
            f"Biodiversity Areas, across {len(sites)} sites. The largest is {largest.name} "
            f"({fmt_ha(largest.area_ha)}), followed by {rest_text}."
        )

    results = {
        'narrative': sentences(head, KBA_DEFINITION),
        'tables': {'sites': sites},
        'values': {'kba_ha': kba_ha, 'kba_pct': kba_pct, 'kba_site_count': len(sites)},
        'flags': [],
    }

    # The headline total is the DEDUPLICATED UNION, promoted alongside the per-site list because
    # summing the list double counts wherever two KBA polygons overlap -- a consumer must use the
    # total fields for a headline, never the sum of the rows.
    view_results = {
        'overlapping_key_biodiversity_areas': [
            {'overlapping_key_biodiversity_name': s.name,
             'overlapping_key_biodiversity_area_size': s.area_ha}
            for s in sites
        ],
        'overlapping_key_biodiversity_area_total_size': kba_ha,
        'overlapping_key_biodiversity_area_percentage': kba_pct,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own. Needs the GIS database, not the Flask app:
    #     python key_biodiversity_areas.py [aoi path]
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
    results, view_results = analyze_kba(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
