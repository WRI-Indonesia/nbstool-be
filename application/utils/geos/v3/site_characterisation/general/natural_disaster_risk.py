"""
Component 1.7 Natural Disaster Risks.

Reports the natural disaster risks the AOI is exposed to, one card per risk, each with a
representative level.

Data. Five pre-classified risk rasters, `risk_*.tif`: cyclone, drought, fire, flood, landslide.
Values are 1..4 (Very Low / Low / Moderate / High) with 0 as nodata.

Risk, not hazard, and the distinction is the layer's rather than ours. These files fold exposure
and vulnerability in upstream, so what 1.7 reports is risk. The component takes them as given and
recombines nothing.

Resolution is wildly uneven across the five: flood and landslide are ~100 m, fire ~1 km, cyclone
~11 km, drought ~28 km. A screening-sized AOI can therefore sit inside a single cyclone or drought
cell and report exactly one class at 100%. That is the layer's resolution showing through, not a
bug, and it is part of why no composite index is computed: the five are not commensurable.

Decisions locked.
- Representative level is conservative: the highest class covering at least RISK_PRESENCE_PCT (20%)
  of the AOI's valid area for that layer. In risk screening a false negative costs more than a
  false positive.
- A risk with no coverage over the AOI is left out of `risk_cards` entirely rather than carried as
  an empty card, so the table lists what is actually present. The endpoint payload still names
  every field (see FE_RISK_FIELDS), so nothing the frontend renders disappears.
- No composite risk index.

History, because 1.7 moved. This port used to read `disaster_risks/hazard_*.tif` on a five-level
legend, and its numbers disagreed with the notebook by a non-uniform offset (landslide -1,
drought -1, fire -2), which no relabelling could reconcile because they were different products.
The notebook then moved 1.7 onto these `risk_*.tif` layers, which are the ones the v3 bucket had
published all along. Both sides now read the same five files, and the discrepancy is closed by
that move rather than by anything here. The Climate module (3.5) still reads the old fire hazard
raster on its 1..5 encoding, so 1.7 and 3.5 no longer share a fire layer.

Downstream use. Each level is read twice later: as permanence risk sensitivity, which constrains
activity design and durability, and as a disaster risk reduction co-benefit mapped to the Triple
Win pillars.
"""

from __future__ import annotations

from dataclasses import dataclass

try:
    from ...common import (
        AOI,
        ClassShare,
        load_raster_clipped,
        oxford_join,
        tabulate_classes,
    )
    from ...config import (
        RISK_KEYS,
        RISK_LEVELS,
        RISK_NO_DATA_KEY,
        RISK_PRESENCE_PCT,
        RISK_RASTERS,
    )
except ImportError:  # `python natural_disaster_risk.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        ClassShare,
        load_raster_clipped,
        oxford_join,
        tabulate_classes,
    )
    from config import (
        RISK_KEYS,
        RISK_LEVELS,
        RISK_NO_DATA_KEY,
        RISK_PRESENCE_PCT,
        RISK_RASTERS,
    )

# The endpoint names the cyclone card "typhoon"; everything else keeps the layer's own name.
# Every field is emitted on every response even when the layer does not reach the AOI, so the
# frontend never sees a key vanish.
#
# FIVE fields, one per layer in RISK_RASTERS. The written contract lists only four (flood,
# landslide, typhoon, drought), but `fire_risk_dict` is kept on the team's instruction: the
# contract is behind the frontend, not the other way round. `flashflood` is the one still absent,
# and only because no v3 raster exists for it -- add the layer to RISK_RASTERS and a line here and
# it works. 1.7 analyses every layer in RISK_RASTERS regardless of this dict, and
# `results['tables']['risk_cards']` always carries them all.
FE_RISK_FIELDS = {
    "flood": "flood_risk_dict",
    "landslide": "landslide_risk_dict",
    "cyclone": "typhoon_risk_dict",
    "drought": "drought_risk_dict",
    "fire": "fire_risk_dict",
}


@dataclass(frozen=True)
class RiskCard:
    risk: str
    level_code: int | None
    level_label: str
    distribution: list[ClassShare]


def _representative_level(rows: list[ClassShare]) -> int | None:
    """Highest risk class covering at least RISK_PRESENCE_PCT of the valid area.

    At least one class always qualifies when data exists: the shares sum to 100 over the classes
    present, so some class must reach 20 percent.
    """
    qualifying = [r.code for r in rows if r.pct >= RISK_PRESENCE_PCT]
    return max(qualifying) if qualifying else None


def analyze_natural_risk(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.7. One card per natural disaster risk, with a representative level each."""
    cards: list[RiskCard] = []

    for risk, path in RISK_RASTERS.items():
        raster = load_raster_clipped(path, aoi, resampling="nearest")
        if raster.valid_area_ha <= 0:
            # No coverage over this AOI: the risk is not "present", so it is left out of the list.
            continue
        rows = tabulate_classes(raster, RISK_LEVELS, denominator_ha=raster.valid_area_ha)
        code = _representative_level(rows)
        cards.append(
            RiskCard(
                risk=risk,
                level_code=code,
                level_label=RISK_LEVELS[code] if code else "No data",
                distribution=rows,
            )
        )

    # Any present risk is listed, ordered by level (highest first), then name for ties.
    present = [c for c in cards if c.level_code]
    present.sort(key=lambda c: (-c.level_code, c.risk))

    if present:
        narrative = "The selected area is susceptible to several natural disaster risks, including:"
    else:
        narrative = "The selected area has no natural disaster risk data for this location."

    # The notebook also keeps the clipped arrays for map display (`rasters=`). This port drops
    # them, as 1.4 and 1.8 already do: the endpoint streams numbers, and five clipped arrays held
    # alive per request is real memory for nothing.
    leveled = {c.risk: c for c in cards if c.level_code}

    flags: list[str] = []
    # ABSENCE, not degradation: the layer has no value here and never will for this AOI, so this
    # is `missing` and the card reports `failed` -- "this is the answer". See
    # pipeline.error_status.
    missing: list[str] = []
    no_data = [risk for risk in FE_RISK_FIELDS if risk not in leveled]
    if no_data:
        # 0 is the nodata value in these files, so a risk that is simply absent here and a layer
        # that does not reach the AOI look the same. Worth naming rather than rendering "No data"
        # with no explanation.
        missing.append(
            f"1.7: no risk value covers the AOI for {oxford_join(no_data)}. In these rasters 0 "
            "is nodata, so 'not exposed' and 'not mapped' cannot be told apart."
        )

    results = {
        'narrative': narrative,
        'tables': {'risk_cards': present},
        'values': {c.risk: c.level_code for c in present},
        'flags': flags,
        'missing': missing,
    }

    view_results = {
        field: (
            {'key': RISK_KEYS.get(leveled[risk].level_code), 'fallback': leveled[risk].level_label}
            if risk in leveled
            else {'key': RISK_NO_DATA_KEY, 'fallback': "No risk"}
        )
        for risk, field in FE_RISK_FIELDS.items()
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python natural_disaster_risk.py [aoi path]
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
    results, view_results = analyze_natural_risk(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
