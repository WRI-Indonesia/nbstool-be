"""
Component 1.1 Ecosystem Type.

Reports the ecosystem setting of the AOI as area and share per ecosystem, for a pie chart, plus
a composition narrative. The result is also the Axis 3 reference ecosystem that drives
downstream logic.

Data. Derived from the pathway raster's ecosystem band (band 2), not a separate layer. Band-2
codes are remapped to three Axis 3 classes: dryland forest (1) and savanna (4) both become
Dryland, mangrove (2) and peatland (3) keep their own class, and 0 (none) falls into Other.
Merging dryland forest and savanna is the team's choice.

Decisions locked.
- Percentage denominator = total AOI area. An "Other/Unclassified" slice absorbs nodata and
  non ecosystem pixels so the pie sums to 100.
- Composition uses pure presence. A class counts as present if its area is above zero, with no
  threshold. Open risk: a single stray edge pixel can flip "single" to "combination".

Open item. Three classes give 2^3 = 8 subsets, and the spec listed only the 7 non empty ones. An
AOI that is entirely water or unclassified land falls into the eighth case. It is handled below
with a default narrative and an UNRESOLVED flag. Team decision needed: reject such an AOI, or
continue with reduced output. Without Axis 3 the pathway module cannot run.

Downstream use. `present_set` is the Axis 3 reference ecosystem. It drives the Cat 8 ecosystem
conditional in the pathway module and the Ecosystem Applicability filter in the Activity Catalog.
"""

from __future__ import annotations

import numpy as np

try:
    from ...common import AOI, ClassShare, ComponentResult, load_raster_clipped, safe_pct
    from ...config import (
        ECOSYSTEM_CLASSES,
        ECOSYSTEM_COLORS,
        ECOSYSTEM_KEYS,
        PATHWAY_ECO_TO_AXIS3,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )
except ImportError:  # `python ecosystem_type.py`: no package around it, so import by path
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, ClassShare, ComponentResult, load_raster_clipped, safe_pct
    from config import (
        ECOSYSTEM_CLASSES,
        ECOSYSTEM_COLORS,
        ECOSYSTEM_KEYS,
        PATHWAY_ECO_TO_AXIS3,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
    )

DRYLAND, MANGROVE, PEATLAND = 1, 2, 3

ECOSYSTEM_NARRATIVE: dict[frozenset[int], str] = {
    frozenset({DRYLAND}): (
        "This area sits on mineral soil that is not regularly flooded, unlike peatland or "
        "mangrove. Its natural reference ecosystem is dryland forest, dominated by trees "
        "growing on well-drained land."
    ),
    frozenset({MANGROVE}): (
        "This area sits in a coastal zone with salt or brackish water shaped by the tides. Its "
        "natural reference ecosystem is mangrove, made up of trees and shrubs adapted to "
        "waterlogged, salty ground."
    ),
    frozenset({PEATLAND}): (
        "This area sits on peat, a soil built up from layers of organic material that stays wet "
        "for most of the year. Its natural reference ecosystem is peatland, a waterlogged "
        "system shaped by its deep organic soil."
    ),
    frozenset({MANGROVE, PEATLAND}): (
        "This area is a coastal zone with tidal salt or brackish water that also sits on peat "
        "soil. It combines mangrove and peatland features in the same place."
    ),
    frozenset({DRYLAND, PEATLAND}): (
        "This area includes both mineral-soil ground and peat soil. Part of it follows a "
        "dryland reference, and part follows a peatland reference shaped by its wet, organic "
        "soil."
    ),
    frozenset({DRYLAND, MANGROVE}): (
        "This area spans both non-flooded mineral soil inland and a tidal coastal zone. It "
        "combines dryland and mangrove references across different parts of the site."
    ),
    frozenset({DRYLAND, MANGROVE, PEATLAND}): (
        "This area spans all three settings: non-flooded mineral soil inland, a tidal coastal "
        "zone with salt or brackish water, and ground built on wet peat soil. It combines "
        "dryland, mangrove, and peatland references across different parts of the site."
    ),
    # Eighth case, see the Open item above.
    frozenset(): (
        "This area does not fall within any mapped ecosystem type. It may be open water or "
        "land outside the mapped extent. No reference ecosystem can be assigned."
    ),
}


def analyze_ecosystem_type(aoi: AOI) -> dict:
    """Component 1.1. Ecosystem composition and the Axis 3 reference ecosystem."""
    # Ecosystem is derived from the pathway raster's ecosystem band (band 2), not a separate
    # layer, and remapped to the 3-class Axis 3 scheme: dryland forest (1) and savanna (4) both
    # become Dryland. Denominator is the whole site, so the pie can carry an Other slice.
    band = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                               band=PATHWAY_ECOSYSTEM_BAND)
    vals = band.values.filled(-1)
    rows: list[ClassShare] = []
    for code, label in ECOSYSTEM_CLASSES.items():
        src_codes = [s for s, d in PATHWAY_ECO_TO_AXIS3.items() if d == code]
        area_ha = int(np.isin(vals, src_codes).sum()) * band.pixel_area_ha
        rows.append(ClassShare(code=code, label=label, area_ha=area_ha,
                               pct=safe_pct(area_ha, aoi.area_ha)))

    mapped_ha = sum(r.area_ha for r in rows)
    other_ha = max(0.0, aoi.area_ha - mapped_ha)
    rows.append(
        ClassShare(
            code="other",
            label="Other/Unclassified",
            area_ha=other_ha,
            pct=safe_pct(other_ha, aoi.area_ha),
        )
    )

    # Pure presence. "Other" is never part of present_set; it does not drive ecosystem logic.
    present_set = frozenset(r.code for r in rows if r.code != "other" and r.area_ha > 0)

    flags: list[str] = []
    # ABSENCE: there is no ecosystem mapped here, which is an answer rather than a fault.
    missing: list[str] = []
    if not present_set:
        missing.append(
            "UNRESOLVED 1.1: AOI has no mapped ecosystem. Downstream modules that require an "
            "Axis 3 reference ecosystem cannot run."
        )
    
    results = {
        'narrative': ECOSYSTEM_NARRATIVE[present_set],
        'tables': {'ecosystem_composition': rows},
        'values': {'present_set': present_set, 'mapped_ha': mapped_ha, 'other_ha': other_ha},
        'flags': flags,
        'missing': missing,
    }
    
    view_results = {
        'ecosystems': [
            {
                'id': str(r.code),
                'dict': {'key': ECOSYSTEM_KEYS.get(r.code), 'fallback': r.label},
                'name': r.label,
                'area': r.area_ha,
                'color': ECOSYSTEM_COLORS.get(r.code),
                'percentage': r.pct,
            }
            for r in rows
        ]
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python ecosystem_type.py [aoi path]
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
    results, view_results = analyze_ecosystem_type(aoi)

    # to_jsonable first: the rows are ClassShare dataclasses and present_set is a frozenset,
    # neither of which json.dumps takes. ensure_ascii=False keeps non-English labels readable.
    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
