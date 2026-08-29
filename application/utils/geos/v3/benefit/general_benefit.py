"""
Component 5.1 General Benefit (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL, body verbatim (F02-P5 Benefit.ipynb, 2026-08-25). Per pathway present
in the AOI: the activities that apply and the benefits they bring, split into the three Triple
Win pillars. Reads 4.2's `by_category` (whose activity rows carry the catalog's benefit columns);
the caller supplies the same stage shape run_benefit builds for 5.2.
"""

from __future__ import annotations

try:
    from ..common import component_values
except ImportError:  # `python general_benefit.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import component_values


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to list -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


# 5.1 General Benefit -------------------------------------------------------------------
# For each pathway present in the AOI: the activities that apply, and the benefits they bring,
# split into the three Triple Win pillars. Benefits are deduplicated so each distinct benefit is
# listed once per pathway.

# Benefit column -> short pillar key, in the order the tool lists them (nature, climate, people).
PILLAR_KEYS = {
    "benefit_nature":  "nature",
    "benefit_climate": "climate",
    "benefit_people":  "people",
}

# The pathways that carry activities, in report order. Ineligible is absent by design: 4.2 gives
# it no activities, so it never reaches this component.
PATHWAY_REPORT_ORDER = ("Protect", "Manage", "Restore")


def _split_benefit_phrases(cell: str) -> list[str]:
    """Split one benefit cell into normalized phrases.

    The catalog stores several benefits per cell, separated by ';'. Normalizing collapses
    whitespace and drops a trailing period, so the same benefit written slightly differently
    merges instead of being listed twice.
    """
    out: list[str] = []
    for part in cell.split(";"):
        phrase = " ".join(part.split()).strip().rstrip(".").strip()
        if phrase:
            out.append(phrase)
    return out


def analyze_general_benefit(pathway_stage: dict) -> tuple[dict, dict]:
    """Component 5.1. Per pathway present in the AOI, list its activities and their benefits,
    split into nature, climate and people.

    Reads 4.2's `by_category` output. No new layer, no area weighting, no ranking.
    """
    try:
        by_category = component_values(pathway_stage, "4.2")["by_category"]
    except KeyError:
        return _na(
            "The activity list (4.2) is not available for this project area. Run F02-P4 for "
            "this AOI first."
        )
    if not by_category:
        return _na(
            "The activity list (4.2) found no categories in this project area, so there are no "
            "activities or benefits to list."
        )

    # pathway -> {"activities": {activity_id: text}, "benefits": {pillar: set(phrases)}}
    collected: dict[str, dict] = {}
    for info in by_category.values():
        activities = info.get("activities", [])
        if not activities:
            continue  # Ineligible / unclassified: no activity, no benefit
        pathway = info.get("pathway", "Unknown")
        slot = collected.setdefault(pathway, {
            "activities": {},
            "benefits": {pillar: set() for pillar in PILLAR_KEYS.values()},
        })
        for activity in activities:
            slot["activities"].setdefault(activity.get("activity_id", ""), activity.get("activity", ""))
            for col, pillar in PILLAR_KEYS.items():
                for phrase in _split_benefit_phrases(activity.get(col, "") or ""):
                    slot["benefits"][pillar].add(phrase)

    if not collected:
        return _na(
            "No activity in this project area declares a benefit, so no benefits can be listed."
        )

    by_pathway: dict[str, dict] = {}
    activity_rows: list[dict] = []
    benefit_rows: list[dict] = []
    for pw in PATHWAY_REPORT_ORDER:
        if pw not in collected:
            continue
        # Activities keep the order 4.2 gave them, dominant category first. Benefits are sorted
        # alphabetically for a stable, predictable order.
        activities = [
            {"activity_id": aid, "activity": text}
            for aid, text in collected[pw]["activities"].items()
        ]
        benefits = {
            pillar: sorted(collected[pw]["benefits"][pillar])
            for pillar in PILLAR_KEYS.values()
        }
        by_pathway[pw] = {"activities": activities, "benefits": benefits}

        for row in activities:
            activity_rows.append({"pathway": pw, **row})
        for pillar in PILLAR_KEYS.values():
            for benefit in benefits[pillar]:
                benefit_rows.append({"pathway": pw, "pillar": pillar, "benefit": benefit})

    results = {
        'narrative': "",  # no render: by_pathway and the two row lists carry everything
        'tables': {"activities": activity_rows, "benefits": benefit_rows},
        'values': {
            "by_pathway": by_pathway,
            "pathways_present": list(by_pathway),
        },
        'flags': [],
    }
    # The card's tag row and pillar grouping come straight from the two row lists.
    view_results = {
        'applicable': True,
        'narrative': "",
        'pathways_present': list(by_pathway),
        'activities': activity_rows,
        'benefits': benefit_rows,
    }
    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python general_benefit.py [aoi path]
    # 5.1 reads the 4.2 activity table, so 4.2 is run here the same way run_benefit does.
    import json
    import os
    import pathlib
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "pathway"))
        from activity_list import analyze_activity_list
    except ImportError:
        from ..common import prepare_aoi, to_jsonable
        from ..pathway.activity_list import analyze_activity_list

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    stage = {"components": {"4.2": to_jsonable(analyze_activity_list(aoi))}}

    t0 = time.perf_counter()
    results, view_results = analyze_general_benefit(stage)
    print(f"[{time.perf_counter() - t0:.1f}s]")
    print(json.dumps(to_jsonable(view_results), indent=2, ensure_ascii=False, default=str))
