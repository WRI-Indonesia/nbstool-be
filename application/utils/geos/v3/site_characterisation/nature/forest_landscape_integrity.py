"""
Component 2.1 Forest Landscape Integrity (FLII).

Reports the landscape integrity of the AOI forest as a headline mean score out of 10, with a
High / Medium / Low breakdown. Integrity is the degree to which a forest is still intact,
connected, and free of human pressure.

Data. TWO rasters. `flii_v3.tif` is the continuous 0 to 10 score, forest-masked upstream, and
gives the headline mean. A class mosaic (1 = Low, 2 = Medium, 3 = High) gives the shares.

NEEDS UPLOADING: only the continuous layer is in the GCS bucket. The class mosaic came with the
D:\\NBSTOOLV3 drop and has been copied into the local bucket folder, so this runs locally, but it
has to reach gs://assets-geo/v3/flii_class_v3.tif before it works on deploy. Deriving the classes
from the paper's breaks (High >= 9.6, Low <= 6.0) instead is deliberately NOT done here:
`classify_continuous` bins upper-exclusive, so a pixel at exactly 6.0 would land in Medium where
the paper puts it in Low. That is the same rule 1.4 follows -- a silently different legend is
worse than an absent card. See FLII_CLASS_RASTER in config.py.

Calibration warning. The 0 to 10 values are calibrated on the pooled SEA distribution, so they
are not one to one with the published global FLII product. Present them as the SEA forest
integrity layer, internally consistent within this run. Do not claim absolute global integrity.

Decisions locked.
- FLII is a property of forest, so the summary covers AOI forest area only. Denominator = AOI
  forest area, the same forest used in 1.5 and 1.6.
- Headline is the mean FLII score out of 10, a single big number for the frontend.
- The narrative reports the High, Medium and Low share and names the predominant class.

Downstream use. FLII is a biodiversity and ecosystem quality proxy feeding Triple Win Pillar 1,
and a pathway signal: high integrity favours PROTECT, low integrity favours RESTORE or MANAGE.
It also underpins the SCeNe high-integrity NbS criteria.
"""

from __future__ import annotations

import numpy as np

try:
    from ...common import (
        AOI,
        dominant,
        fmt_pct,
        forest_mask_2024,
        load_raster_clipped,
        not_applicable,
        sentences,
        tabulate_classes,
    )
    from ...config import FLII_CLASS_RASTER, FLII_CLASSES, FLII_FOREST_RASTER
except ImportError:  # `python forest_landscape_integrity.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        dominant,
        fmt_pct,
        forest_mask_2024,
        load_raster_clipped,
        not_applicable,
        sentences,
        tabulate_classes,
    )
    from config import FLII_CLASS_RASTER, FLII_CLASSES, FLII_FOREST_RASTER

FLII_LOW, FLII_MEDIUM, FLII_HIGH = 1, 2, 3

FLII_GLOSS = {
    FLII_HIGH: "indicating largely intact and well-connected forest under low human pressure",
    FLII_MEDIUM: (
        "indicating moderately modified forest with some fragmentation or human pressure"
    ),
    FLII_LOW: "indicating heavily modified and fragmented forest under high human pressure",
}


def _empty(reason: str) -> tuple[dict, dict]:
    """The not-applicable payload, in this component's shape."""
    empty = not_applicable("2.1 Forest Landscape Integrity", reason)
    results = {'narrative': empty.narrative, 'tables': {'integrity': []},
               'values': {}, 'flags': empty.flags, 'missing': empty.missing}
    view_results = {'flii_score': None, 'high_integrity_percentage': None,
                    'medium_integrity_percentage': None, 'low_integrity_percentage': None,
                    'dominant_integrity_class': None}
    return results, view_results


def analyze_flii(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.1. Forest landscape integrity over the AOI forest."""
    # The FLII rasters are already masked to forest upstream, so their valid extent defines the
    # forest here. forest_mask_2024 is loaded only to catch the "no forest at all" case early,
    # so the message matches 1.5 and 1.6 rather than saying "no FLII data".
    if forest_mask_2024(aoi).is_empty:
        return _empty(
            "No forest is present in this project area, so landscape integrity cannot be "
            "assessed."
        )

    score = load_raster_clipped(FLII_FOREST_RASTER, aoi, resampling="bilinear")
    classes = load_raster_clipped(FLII_CLASS_RASTER, aoi, resampling="nearest")

    forest_area_ha = classes.valid_area_ha
    if forest_area_ha <= 0 or score.valid_count == 0:
        return _empty(
            "The forest integrity layer does not cover the forest in this project area."
        )

    mean_flii = float(np.ma.mean(score.values))  # 0 to 10, one decimal on display

    rows = tabulate_classes(classes, FLII_CLASSES, denominator_ha=forest_area_ha)
    by_code = {r.code: r for r in rows}
    dom = dominant(rows)

    narrative = sentences(
        f"Of the forest in this area, {fmt_pct(by_code[FLII_HIGH].pct)} has high landscape "
        f"integrity, {fmt_pct(by_code[FLII_MEDIUM].pct)} medium, and "
        f"{fmt_pct(by_code[FLII_LOW].pct)} low.",
        f"The forest is predominantly {dom.label.lower()} integrity, {FLII_GLOSS[dom.code]}.",
    )

    results = {
        'narrative': narrative,
        'tables': {'integrity': rows},
        'values': {
            'mean_flii': mean_flii,          # headline big number
            'dominant_class': dom.code,
            'pct_high': by_code[FLII_HIGH].pct,
            'forest_area_ha': forest_area_ha,
        },
        'flags': [],
    }

    # The endpoint contract names the three shares individually rather than as a list, so the
    # class table is flattened here. `by_code` always has all three: tabulate_classes emits every
    # class in FLII_CLASSES, at zero when absent.
    view_results = {
        'flii_score': mean_flii,
        'high_integrity_percentage': by_code[FLII_HIGH].pct,
        'medium_integrity_percentage': by_code[FLII_MEDIUM].pct,
        'low_integrity_percentage': by_code[FLII_LOW].pct,
        # "High" / "Medium" / "Low", the class the narrative names. Was values-only (as a code);
        # promoted so templates get the integrity category without re-deriving it from the shares.
        'dominant_integrity_class': dom.label,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python forest_landscape_integrity.py [aoi path]
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
    results, view_results = analyze_flii(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
