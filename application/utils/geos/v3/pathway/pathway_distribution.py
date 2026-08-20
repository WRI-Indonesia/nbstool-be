"""4.1 Pathway Distribution - area and share of the AOI for each NBS pathway.

Data. `sea_nbs_pathway_v3.tif`, three bands, canonical_v3 layout.

| Band | Name | Codes |
|---|---|---|
| 1 | `pathway` | 0 no data, 1 Protect, 2 Manage, 3 Restore, 4 Ineligible |
| 2 | `ecosystem` | 0 none, 1 dryland forest, 2 mangrove, 3 peatland, 4 savanna |
| 3 | `cat_code` | 1 to 17 category index |

Code 4 Ineligible is not a pathway (established plantation, forest-lost savanna, stable savanna,
settlement); it is listed but excluded from the eligible headline. Denominator is the total AOI
area, with an Unclassified row for code 0 and nodata, so the table sums to 100. Bands 2 and 3 are
not tabulated; their codes-present pass through `values` for the F02-P5 activity join on
`(cat_code, ecosystem)`.

Body unchanged from the notebook, with one seam removal: the notebook returns the clipped raster
under `rasters=` for later cells to reuse. `ComponentResult` here has no such field and nothing in
the backend consumes pixel arrays, so it is dropped rather than carried to the wire.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

try:
    from ..common import AOI, ComponentResult, load_raster_clipped, not_applicable, safe_pct, \
        sort_by_area
    from ..config import (
        PATHWAY_BAND,
        PATHWAY_CATCODE_BAND,
        PATHWAY_CATCODE_LABELS,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_ELIGIBLE_CODES,
        PATHWAY_RASTER,
        PATHWAY_UNCLASSIFIED_WARN_PCT,
    )
except ImportError:  # `python pathway_distribution.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import AOI, ComponentResult, load_raster_clipped, not_applicable, safe_pct, \
        sort_by_area
    from config import (
        PATHWAY_BAND,
        PATHWAY_CATCODE_BAND,
        PATHWAY_CATCODE_LABELS,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_ELIGIBLE_CODES,
        PATHWAY_RASTER,
        PATHWAY_UNCLASSIFIED_WARN_PCT,
    )


@dataclass(frozen=True)
class PathwayShare:
    """One row of the pathway breakdown."""

    code: int
    label: str
    area_ha: float
    pct: float          # share of the TOTAL AOI area, not of the classified area
    is_pathway: bool    # False for Ineligible and Unclassified


UNCLASSIFIED_LABEL = "Unclassified"


def _codes_present(aoi: AOI, band: int, labels: dict[int, str]) -> dict:
    """Read one pass-through band and return which codes occur, with labels and areas.

    Used for the ecosystem and cat_code bands. They are not tabulated into the headline, but the
    activity generator needs to know which categories the AOI contains, so the set of present
    codes and their areas travel in `values`.
    """
    raster = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest", band=band)
    codes, counts = np.unique(raster.values.compressed(), return_counts=True)
    area_by_code = {
        int(c): int(n) * raster.pixel_area_ha
        for c, n in zip(codes.tolist(), counts.tolist())
        if int(c) != 0  # 0 is mask / none, not a category
    }
    return {
        "codes": sorted(area_by_code),
        "labels": [labels.get(c, f"Unknown code {c}") for c in sorted(area_by_code)],
        "area_ha": {labels.get(c, f"Unknown code {c}"): area_by_code[c]
                    for c in sorted(area_by_code)},
    }


def analyze_pathway_distribution(aoi: AOI) -> ComponentResult:
    """Component 4.1. Area and share of the AOI per primary pathway, canonical_v3."""
    primary = load_raster_clipped(
        PATHWAY_RASTER, aoi, resampling="nearest", band=PATHWAY_BAND
    )

    if primary.valid_area_ha <= 0:
        return not_applicable(
            "4.1 Pathway Distribution",
            "The pathway layer does not cover this project area, so no pathway can be "
            "recommended.",
        )

    # Denominator is the whole site. Code 0 and nodata are absorbed by an Unclassified row, so
    # the table sums to 100 of the AOI rather than of the covered part.
    rows: list[PathwayShare] = []
    classified_ha = 0.0
    for code, label in PATHWAY_CODES.items():
        if code == 0:
            continue  # 0 is folded into Unclassified below, together with nodata
        area_ha = int((primary.values == code).sum()) * primary.pixel_area_ha
        classified_ha += area_ha
        rows.append(
            PathwayShare(
                code=code,
                label=label,
                area_ha=area_ha,
                pct=safe_pct(area_ha, aoi.area_ha),
                is_pathway=code in PATHWAY_ELIGIBLE_CODES,
            )
        )

    unclassified_ha = max(0.0, aoi.area_ha - classified_ha)
    rows.append(
        PathwayShare(
            code=0,
            label=UNCLASSIFIED_LABEL,
            area_ha=unclassified_ha,
            pct=safe_pct(unclassified_ha, aoi.area_ha),
            is_pathway=False,
        )
    )

    eligible_ha = sum(r.area_ha for r in rows if r.is_pathway)
    eligible_pct = safe_pct(eligible_ha, aoi.area_ha)

    flags: list[str] = []
    unclassified_pct = safe_pct(unclassified_ha, aoi.area_ha)
    if unclassified_pct > PATHWAY_UNCLASSIFIED_WARN_PCT:
        flags.append(
            f"4.1: {unclassified_pct:.0f}% of the AOI carries no pathway value. Every share "
            "below describes only the remainder of the site."
        )

    # Bands 2 and 3 pass through untabulated, for the activity generator in F02-P5.
    ecosystem = _codes_present(aoi, PATHWAY_ECOSYSTEM_BAND, PATHWAY_ECOSYSTEM_CODES)
    catcode = _codes_present(aoi, PATHWAY_CATCODE_BAND, PATHWAY_CATCODE_LABELS)

    pathway_rows = sort_by_area([r for r in rows if r.is_pathway and r.area_ha > 0])
    return ComponentResult(
        component="4.1 Pathway Distribution",
        applicable=True,
        narrative="",  # no rendered sentence: downstream reads values, the human reads the table
        tables={"pathway_distribution": rows},  # every code plus Unclassified, sums to 100
        values={
            "chart_series": "pathway_distribution",
            "chart_unit": "%",
            "chart_axis_label": "Share of project area (%)",
            "eligible_ha": eligible_ha,
            "eligible_pct": eligible_pct,
            "pathway_ha": {r.label: r.area_ha for r in rows if r.is_pathway},
            "dominant_pathway": pathway_rows[0].label if pathway_rows else None,
            "unclassified_pct": unclassified_pct,
            # Band 2 ecosystem and band 3 cat_code, for the F02-P5 activity join on
            # (cat_code, ecosystem). Four class ecosystem, NOT the three class layer of 1.1.
            "reference_ecosystem_codes": ecosystem["codes"],
            "reference_ecosystem_labels": ecosystem["labels"],
            "cat_code_codes": catcode["codes"],
            "cat_code_labels": catcode["labels"],
        },
        flags=flags,
    )
