"""4.2 Activity List - the activities that apply to the AOI, derived from 4.1.

Bands 3 (`cat_code`) and 2 (`ecosystem`) are read ON THE SAME GRID and cross-tabulated into the
unique `(cat_code, ecosystem)` pairs present, with area. `like=catcode` is what makes that safe:
each pixel pairs its own category with its own ecosystem, which is meaningless if the two reads
land on different grids.

Each pair is joined to the canonical_v3 catalog (`config.ACTIVITY_TABLE`, keyed on
`(cat_code, ecosystem)`). Ineligible categories carry no activity BY DESIGN; a category with no
matching catalog row is flagged, not dropped, so a catalog that falls behind the raster stays
visible instead of silently losing area.

The full activity rows, including the Triple Win benefits and the two carbon QB flags, travel in
`values["by_category"]` for F02-P5; the displayed table shows the activity list only.

Body unchanged from the notebook. `load_activity_table` takes a layer NAME here rather than a
local path, because the CSV lives beside the rasters under V3_BUCKET.
"""

from __future__ import annotations

import numpy as np

try:
    from ..common import AOI, ComponentResult, fmt_ha, load_activity_table, \
        load_raster_clipped, not_applicable
    from ..config import (
        ACTIVITY_TABLE,
        PATHWAY_CATCODE_BAND,
        PATHWAY_CATCODE_LABELS,
        PATHWAY_CATCODE_TO_PATHWAY,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_RASTER,
    )
except ImportError:  # `python activity_list.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI, ComponentResult, fmt_ha, load_activity_table, \
        load_raster_clipped, not_applicable
    from config import (
        ACTIVITY_TABLE,
        PATHWAY_CATCODE_BAND,
        PATHWAY_CATCODE_LABELS,
        PATHWAY_CATCODE_TO_PATHWAY,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_RASTER,
    )


def analyze_activity_list(aoi: AOI) -> ComponentResult:
    """Component 4.2. Activities per (cat_code, ecosystem) category present in the AOI."""
    # Bands 3 and 2 on one shared grid, so each pixel pairs its own cat_code with its ecosystem.
    catcode = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                                  band=PATHWAY_CATCODE_BAND)
    ecosystem = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                                    band=PATHWAY_ECOSYSTEM_BAND, like=catcode)

    cat, eco = catcode.values, ecosystem.values
    valid = ~np.ma.getmaskarray(cat) & ~np.ma.getmaskarray(eco)
    if not valid.any():
        return not_applicable(
            "4.2 Activity List",
            "The pathway layer does not cover this project area, so no activities apply.",
        )

    # Unique (cat_code, ecosystem) pairs present, with pixel count -> area.
    pairs = np.stack([cat[valid].astype(int), eco[valid].astype(int)], axis=1)
    uniq, counts = np.unique(pairs, axis=0, return_counts=True)
    px_area = catcode.pixel_area_ha

    table = load_activity_table(ACTIVITY_TABLE)

    # Dominant categories first.
    order = np.argsort(counts)[::-1]
    rows: list[dict] = []
    by_category: dict[str, dict] = {}
    flags: list[str] = []

    for idx in order:
        cc, ec = int(uniq[idx][0]), int(uniq[idx][1])
        if cc == 0:
            continue  # mask
        area_ha = int(counts[idx]) * px_area
        pathway_code = PATHWAY_CATCODE_TO_PATHWAY.get(cc)
        pathway = PATHWAY_CODES.get(pathway_code, "Unknown")
        cat_label = PATHWAY_CATCODE_LABELS.get(cc, f"cat {cc}")
        eco_label = PATHWAY_ECOSYSTEM_CODES.get(ec, f"ecosystem {ec}")

        if ec == 0:
            acts = []
            flags.append(
                f"4.2: {cat_label} appears with ecosystem 0 (no reference) on "
                f"{fmt_ha(area_ha)}; the pathway script should have masked these pixels."
            )
        elif pathway_code == 4:
            acts = []  # Ineligible: no activity by design
        else:
            acts = table.get((cc, ec), [])
            if not acts:
                flags.append(
                    f"4.2: no catalog row for ({cat_label}, {eco_label}); "
                    f"{fmt_ha(area_ha)} left without an activity."
                )

        if acts:
            for a in acts:
                rows.append({
                    "pathway": pathway, "category": cat_label, "ecosystem": eco_label,
                    "area_ha": round(area_ha, 1),
                    "activity_id": a["activity_id"], "activity": a["activity"],
                })
        else:
            note = "(ineligible, no activity)" if pathway_code == 4 else "(no catalog match)"
            rows.append({
                "pathway": pathway, "category": cat_label, "ecosystem": eco_label,
                "area_ha": round(area_ha, 1), "activity_id": "", "activity": note,
            })

        by_category[f"{cat_label} | {eco_label}"] = {
            "pathway": pathway, "area_ha": area_ha, "activities": acts,
        }

    return ComponentResult(
        component="4.2 Activity List",
        applicable=True,
        narrative="",
        tables={"activities": rows},   # one row per activity, category by category
        values={
            "by_category": by_category,        # full rows incl. benefits + QB flags, for F02-P5
            "category_count": len(by_category),
            "activity_count": sum(1 for r in rows if r["activity_id"]),
        },
        flags=flags,
    )
