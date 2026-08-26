"""
Component 5.1 General Benefit (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL, body verbatim (F02-P5 Benefit.ipynb, 2026-08-25). Per pathway present
in the AOI: the activities that apply and the benefits they bring, split into the three Triple
Win pillars. Reads 4.2's `by_category` (whose activity rows carry the catalog's benefit columns);
the caller supplies the same stage shape run_benefit builds for 5.2.
"""

from __future__ import annotations

try:
    from ..common import ComponentResult, component_values, not_applicable
except ImportError:  # `python general_benefit.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import ComponentResult, component_values, not_applicable


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


def analyze_general_benefit(pathway_stage: dict) -> ComponentResult:
    """Component 5.1. Per pathway present in the AOI, list its activities and their benefits,
    split into nature, climate and people.

    Reads 4.2's `by_category` output. No new layer, no area weighting, no ranking.
    """
    component = "5.1 General Benefit"

    try:
        by_category = component_values(pathway_stage, "4.2")["by_category"]
    except KeyError:
        return not_applicable(
            component,
            "The activity list (4.2) is not available for this project area. Run F02-P4 for "
            "this AOI first.",
        )
    if not by_category:
        return not_applicable(
            component,
            "The activity list (4.2) found no categories in this project area, so there are no "
            "activities or benefits to list.",
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
        return not_applicable(
            component,
            "No activity in this project area declares a benefit, so no benefits can be listed.",
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

    return ComponentResult(
        component=component,
        applicable=True,
        narrative="",  # no render: by_pathway and the two tables carry everything
        tables={"activities": activity_rows, "benefits": benefit_rows},
        values={
            "by_pathway": by_pathway,
            "pathways_present": list(by_pathway),
        },
        flags=[],
    )
