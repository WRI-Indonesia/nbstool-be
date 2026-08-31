"""
Component 5.7 Maintenance of ecological connectivity (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL (F02-P5 Benefit.ipynb, commit `7114a57`, 2026-08-31 -- the restructured
notebook, whose cells are written in the backend's own idiom). Body verbatim; packaging converted
to the house `(results, view_results)` dicts like every benefit sibling, and the cell's
`rasters=` map exports (the clipped MSPA class raster) dropped the way 5.2's port drops its --
the endpoint streams numbers.

Structural connectivity from a GuidosToolbox MSPA raster (2024), reported per pathway:
PROTECT = intact block + corridor share, MANAGE = connector/edge habitat to strengthen,
RESTORE = separate habitat patches restoration would re-link. Qualitative co-benefit, no carbon.

The 2024 MSPA layer reached the bucket 2026-08-31 (briefly as `lc_2024...`, renamed to the
notebook's own `fc_2024_mspa_4326_v2.tif` by the data team the same day; the object was verified
to be the genuine MSPA product on the same grid as fc_2014). Wired into the benefit stream.
"""

from __future__ import annotations

import geopandas as gpd
import numpy as np
from rasterio.features import shapes as _rio_shapes

try:
    from ..common import (
        AOI,
        fmt_ha,
        fmt_pct,
        load_raster_clipped,
        oxford_join,
        safe_pct,
    )
    from ..config import (
        CONNECTIVITY_ECO_WORDS,
        MANAGE_CODE,
        MSPA_2024_RASTER,
        MSPA_BACKGROUND_CODES,
        MSPA_CLASS_CODES,
        MSPA_CONNECTOR_CLASSES,
        MSPA_CORE_CLASSES,
        MSPA_EDGE_CLASSES,
        MSPA_HABITAT_CLASSES,
        PATHWAY_BAND,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
        PROTECT_CODE,
        RESTORE_CODE,
    )
except ImportError:  # `python ecological_connectivity.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import (
        AOI,
        fmt_ha,
        fmt_pct,
        load_raster_clipped,
        oxford_join,
        safe_pct,
    )
    from config import (
        CONNECTIVITY_ECO_WORDS,
        MANAGE_CODE,
        MSPA_2024_RASTER,
        MSPA_BACKGROUND_CODES,
        MSPA_CLASS_CODES,
        MSPA_CONNECTOR_CLASSES,
        MSPA_CORE_CLASSES,
        MSPA_EDGE_CLASSES,
        MSPA_HABITAT_CLASSES,
        PATHWAY_BAND,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_RASTER,
        PROTECT_CODE,
        RESTORE_CODE,
    )


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to assess -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def _count_habitat_patches(core_mask: np.ndarray, transform) -> int:
    """Number of separate habitat-core components, 8-connectivity, via rasterio.features.shapes."""
    if not core_mask.any():
        return 0
    arr = core_mask.astype("uint8")
    return sum(1 for _geom, val in _rio_shapes(arr, mask=core_mask, transform=transform,
                                               connectivity=8) if val == 1)


def analyze_ecological_connectivity(aoi: AOI, duration_years: int) -> tuple[dict, dict]:
    """Component 5.7. Structural connectivity (MSPA) reported per pathway. No carbon number."""
    pathway = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest", band=PATHWAY_BAND)
    mspa = load_raster_clipped(MSPA_2024_RASTER, aoi, resampling="nearest", like=pathway)
    ecosystem = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                                    band=PATHWAY_ECOSYSTEM_BAND, like=pathway)
    pix = pathway.pixel_area_ha

    code = mspa.values.filled(0).astype(int)
    mspa_valid = ~np.ma.getmaskarray(mspa.values)

    def cls_mask(names):
        codes = [c for c, n in MSPA_CLASS_CODES.items() if n in names]
        return np.isin(code, codes) & mspa_valid

    habitat = cls_mask(MSPA_HABITAT_CLASSES)
    core = cls_mask(MSPA_CORE_CLASSES)
    connector = cls_mask(MSPA_CONNECTOR_CLASSES)
    edge = cls_mask(MSPA_EDGE_CLASSES)

    if not habitat.any():
        return _na(
            "No mapped MSPA habitat covers this project area, so connectivity cannot be assessed."
        )

    pw = pathway.values.filled(0).astype(int)
    protect = pw == PROTECT_CODE
    manage = pw == MANAGE_CODE
    restore = pw == RESTORE_CODE

    # Permanent methodology caveats -- `notes`, never `flags`: they must not drive error_status.
    notes: list[str] = []

    # Verify the MSPA legend: any foreground code present but not mapped, and not a known
    # background value, is surfaced so a legend mismatch is caught on the first run.
    present = set(np.unique(code[mspa_valid]).tolist())
    known = set(MSPA_CLASS_CODES) | set(MSPA_BACKGROUND_CODES)
    unmapped = sorted(present - known)
    if unmapped:
        unm_ha = int(np.isin(code, unmapped)[mspa_valid].sum()) * pix
        notes.append(
            f"5.7: the MSPA raster carries code(s) {unmapped} ({fmt_ha(unm_ha)}) not in "
            "MSPA_CLASS_CODES. Verify the GuidosToolbox legend and add them; connector/core areas "
            "may be understated until then."
        )

    def eco_word(area_mask):
        codes, counts = np.unique(ecosystem.values.filled(0)[area_mask].astype(int),
                                  return_counts=True)
        ha = {int(c): int(n) * pix for c, n in zip(codes, counts) if c in CONNECTIVITY_ECO_WORDS}
        words = [CONNECTIVITY_ECO_WORDS[c] for c in sorted(ha, key=ha.get, reverse=True)]
        return oxford_join(words) or "natural"

    values: dict = {}
    sections: list = []   # (LABEL, sentence) for each pathway present

    # PROTECT: intact block + corridor within it.
    protect_hab = protect & habitat
    if protect_hab.any():
        block_ha = int(protect_hab.sum()) * pix
        corridor_ha = int((protect & connector).sum()) * pix
        corridor_pct = safe_pct(corridor_ha, block_ha)
        word = eco_word(protect_hab)
        sentence = (
            f"Conserving this {word} ecosystem keeps an estimated {fmt_ha(block_ha)} block of "
            "natural habitat intact and safeguards its role as a connector in the surrounding "
            f"landscape. Based on the site structure, {fmt_ha(corridor_ha)} "
            f"({fmt_pct(corridor_pct)}) of the area functions as a corridor linking separate "
            "habitat cores. Protecting it prevents the fragmentation that would likely occur under "
            f"current deforestation pressure over the {duration_years} year project period."
        )
        values["protect"] = {"block_ha": block_ha, "corridor_ha": corridor_ha,
                             "corridor_pct": corridor_pct, "ecosystem": word,
                             "narrative": sentence}
        sections.append(("PROTECT", sentence))

    # MANAGE: connector + edge habitat whose condition can be improved.
    manage_hab = manage & habitat
    if manage_hab.any():
        conn_edge_ha = int((manage & (connector | edge)).sum()) * pix
        word = eco_word(manage_hab)
        sentence = (
            f"Managing this {word} ecosystem safeguards and strengthens the physical links between "
            f"habitat patches across the landscape. The site includes {fmt_ha(conn_edge_ha)} of "
            "connector and edge habitat whose condition, if improved, supports continued species "
            "movement and genetic exchange between nearby cores."
        )
        values["manage"] = {"connector_edge_ha": conn_edge_ha, "ecosystem": word,
                            "narrative": sentence}
        sections.append(("MANAGE", sentence))

    # RESTORE: number of separate habitat patches restoration would re-link.
    if restore.any():
        n_patches = _count_habitat_patches(core, mspa.transform)
        word = eco_word(restore)
        sentence = (
            f"Restoring this {word} ecosystem re-establishes physical links between {n_patches} "
            "separate habitat patches that are currently fragmented, improving the potential for "
            f"species movement over the {duration_years} year project period."
        )
        values["restore"] = {"patch_count": n_patches, "ecosystem": word,
                            "narrative": sentence}
        sections.append(("RESTORE", sentence))

    if not sections:
        return _na(
            "No Protect, Manage or Restore area with mapped habitat is present, so no "
            "connectivity benefit can be reported."
        )

    # Class-area table for the frontend, over the whole AOI habitat.
    class_rows = []
    for c, name in MSPA_CLASS_CODES.items():
        area = int((code == c)[mspa_valid].sum()) * pix
        if area > 0:
            class_rows.append({"mspa_class": name, "code": c, "area_ha": round(area, 1),
                               "pct_of_habitat": round(safe_pct(area, int(habitat.sum()) * pix), 1)})
    class_rows = sorted(class_rows, key=lambda r: -r["area_ha"])

    values.update({
        "connector_ha_total": int(connector.sum()) * pix,
        "core_ha_total": int(core.sum()) * pix,
        "habitat_ha_total": int(habitat.sum()) * pix,
        "pathways_reported": [k for k in ("protect", "manage", "restore") if k in values],
        "mspa_year": 2024,
    })
    # Narrative separated per pathway: a dict for the frontend, and a labelled string for display.
    values["narratives"] = {label: sentence for label, sentence in sections}

    narrative = "\n\n".join(f"{label}: {sentence}" for label, sentence in sections)

    results = {
        'narrative': narrative,
        'tables': {"mspa_classes": class_rows},
        'values': values,
        'flags': [],
        'notes': notes,
    }
    # The card contract: the per-pathway figures with their sentences, and the MSPA split. The
    # `narratives` dict is NOT on the wire (team call 2026-09-01) -- it only duplicated the
    # sentence each pathway sub-dict already carries.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'pathways_reported', 'habitat_ha_total', 'core_ha_total',
            'connector_ha_total', 'mspa_year')},
        **{k: values[k] for k in ('protect', 'manage', 'restore') if k in values},
        'mspa_classes': class_rows,
    }
    if notes:
        view_results['notes'] = notes
    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python ecological_connectivity.py [aoi path] [duration]
    import json
    import os
    import sys
    import time

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    results, view_results = analyze_ecological_connectivity(aoi, duration)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False, default=str))
