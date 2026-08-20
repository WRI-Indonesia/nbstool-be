"""run_pathway.py - F02-P4 as ONE JSON response, shaped for the Pathway Selection screen.

Runs 4.1 -> 4.2 -> 4.3 in sequence. They are NOT independent and there is no thread pool here:
4.2 needs the same grid 4.1 established, and 4.3 is pure re-aggregation of 4.2's `by_category`. All
three read ONE raster, so the whole thing is a handful of band reads rather than the 25 component
fan-out that site characterisation needs.

A SINGLE JSON RESPONSE, not the NDJSON envelope. The screen needs every ecosystem card at once
before it can render anything -- a user picking interventions cannot act on a partial answer the
way they can read a half-filled characterisation report -- and the run is short enough that
streaming would buy nothing. So a failure here is a 500, not a degraded card.

WHAT THE SCREEN NEEDS THAT 4.3 DOES NOT PRODUCE, and where each comes from:

  disturbed area      F02-P3 THREAT PROFILE OWNS THIS, and it is not wired up yet: the three
                      fields are served as 0 / 0 / false placeholders so the card renders and the
                      contract is stable. F02-P4 has no notion of disturbance at all.
                      An earlier build derived it here as "everything that is not Cat 1"; that was
                      REMOVED rather than left dormant, because a wrong source that still returns
                      plausible numbers is worse than an obvious zero.
  duration, risk      product constants, served from config so the frontend does not hardcode
                      numbers the backend will later be asked to honour.

THE THREE INTERVENTIONS PARTITION THE ECOSYSTEM. Every pixel carries exactly one pathway, so
Protect + Manage + Restore + Ineligible = the ecosystem's area, and the three shares sum to at most
100. The design mock showed 70/40/40, which this raster cannot produce; confirmed with the team
2026-08-10 that the partition is what was meant and the mock's numbers were placeholders.
"""

from __future__ import annotations

try:
    from ..common import AOI, safe_pct
    from ..config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_DISPLAY_NAMES,
        INTERVENTION_DURATION_DEFAULT_YEARS,
        INTERVENTION_DURATION_MAX_YEARS,
        INTERVENTION_DURATION_MIN_YEARS,
        INTERVENTION_DURATION_STEP_YEARS,
        PATHWAY_CODES,
        PATHWAY_ELIGIBLE_CODES,
    )
    from .activity_list import analyze_activity_list
    from .by_ecosystem import ECOSYSTEM_BUCKETS, analyze_by_ecosystem
    from .pathway_distribution import analyze_pathway_distribution
except ImportError:  # `python run_pathway.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from activity_list import analyze_activity_list
    from by_ecosystem import ECOSYSTEM_BUCKETS, analyze_by_ecosystem
    from common import AOI, safe_pct
    from config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_DISPLAY_NAMES,
        INTERVENTION_DURATION_DEFAULT_YEARS,
        INTERVENTION_DURATION_MAX_YEARS,
        INTERVENTION_DURATION_MIN_YEARS,
        INTERVENTION_DURATION_STEP_YEARS,
        PATHWAY_CODES,
        PATHWAY_ELIGIBLE_CODES,
    )
    from pathway_distribution import analyze_pathway_distribution


# The three selectable interventions, in the order the cards show them. Ineligible is the fourth
# value of the same band but is not an intervention -- it is reported as the ecosystem's
# non-eligible remainder, not as a fourth toggle.
INTERVENTIONS = [PATHWAY_CODES[code] for code in PATHWAY_ELIGIBLE_CODES]

def _activities_for(categories: dict, pathway: str) -> list[dict]:
    """The distinct activities under one intervention within one ecosystem.

    Several categories can map to the same pathway and carry the same activity -- Cat 3B and Cat 4B
    are both Restore on dryland -- so the same checkbox would otherwise appear two or three times
    on one card. Deduplicated by `activity_id`, largest category first, so the order a user reads
    is the order of how much of their site each activity applies to.
    """
    seen: set[str] = set()
    out: list[dict] = []
    for _key, info in sorted(categories.items(), key=lambda kv: kv[1]["area_ha"], reverse=True):
        if info["pathway"] != pathway:
            continue
        for activity in info.get("activities", ()):
            if activity["activity_id"] in seen:
                continue
            seen.add(activity["activity_id"])
            out.append({"activity_id": activity["activity_id"],
                        "activity": activity["activity"]})
    return out


def _ecosystem_card(name: str, info: dict, project_area_ha: float) -> dict:
    """One ecosystem card. Emitted for all three buckets even when absent, so the screen has the
    same shape on every AOI and a missing ecosystem reads as 0 ha rather than as a layout change.
    """
    area_ha = info["area_ha"]
    categories = info["categories"]
    mix = info["pathway_mix"]

    return {
        "ecosystem": name,                                   # analysis key: Dryland/Mangrove/Peatland
        "label": ECOSYSTEM_DISPLAY_NAMES.get(name, name),    # what the card prints
        "present": area_ha > 0,
        "total_area_ha": area_ha,
        "total_area_percentage": safe_pct(area_ha, project_area_ha),
        # PLACEHOLDERS UNTIL F02-P3 THREAT PROFILE LANDS. Nothing in F02-P4 measures disturbance,
        # so these are served as zeros rather than derived from something that merely correlates:
        # a card reading "0 ha disturbed" is obviously unpopulated, whereas a plausible wrong
        # number would be believed. The keys are here so the contract does not change when P3
        # fills them.
        "disturbed_area_ha": 0.0,
        "disturbed_percentage": 0.0,
        "is_disturbed": False,
        "interventions": [
            {
                "intervention": pathway,
                "area_ha": mix.get(pathway, 0.0),
                # Share OF THIS ECOSYSTEM, not of the project area: the card reads
                # "X% of forest is eligible to protect".
                "percentage": safe_pct(mix.get(pathway, 0.0), area_ha),
                "eligible": mix.get(pathway, 0.0) > 0,
                "activities": _activities_for(categories, pathway),
            }
            for pathway in INTERVENTIONS
        ],
        # The rest of the ecosystem: real area that no intervention can be selected on.
        "ineligible_area_ha": mix.get("Ineligible", 0.0),
        "ineligible_percentage": safe_pct(mix.get("Ineligible", 0.0), area_ha),
    }


def run_pathway(aoi: AOI) -> dict:
    """The Pathway Selection payload for one AOI: one card per ecosystem, plus the run's defaults.

    Raises rather than degrading. The three components are one raster read chain, so a failure is
    a failure of the whole answer, and this endpoint has no per-card error channel to report it
    through.
    """
    results = {}
    results["4.1"] = analyze_pathway_distribution(aoi)
    results["4.2"] = analyze_activity_list(aoi)
    results["4.3"] = analyze_by_ecosystem(aoi, results)

    distribution, by_ecosystem = results["4.1"], results["4.3"]
    buckets = by_ecosystem.values.get("by_ecosystem", {})

    # Every flag and absence the three components raised, in one list. The screen has no per-card
    # error slot, so they travel together and a client can show them above the cards or not at all.
    messages: list[str] = []
    for component in results.values():
        messages.extend(component.flags)
        messages.extend(component.missing)

    return {
        "project_area_ha": aoi.area_ha,
        "applicable": by_ecosystem.applicable,
        # Headline from 4.1: how much of the site qualifies for any pathway at all.
        "eligible_area_ha": distribution.values.get("eligible_ha", 0.0),
        "eligible_percentage": distribution.values.get("eligible_pct", 0.0),
        "unclassified_area_ha": by_ecosystem.values.get("unclassified_ha", 0.0),
        "unclassified_percentage": by_ecosystem.values.get("unclassified_pct", 0.0),
        "ecosystems": [
            _ecosystem_card(name, buckets.get(
                name, {"area_ha": 0.0, "pathway_mix": {}, "categories": {}}), aoi.area_ha)
            for name in ECOSYSTEM_BUCKETS
        ],
        "duration_years": {
            "default": INTERVENTION_DURATION_DEFAULT_YEARS,
            "min": INTERVENTION_DURATION_MIN_YEARS,
            "max": INTERVENTION_DURATION_MAX_YEARS,
            "step": INTERVENTION_DURATION_STEP_YEARS,
        },
        "carbon_risk": dict(CARBON_RISK_DEFAULTS),
        "messages": messages,
    }


if __name__ == "__main__":
    # Run on a file and print the payload the endpoint sends, no Flask app:
    #     python run_pathway.py [aoi path]
    import json
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    _aoi = prepare_aoi(gpd.read_file(aoi_path))
    _t0 = time.perf_counter()
    print(json.dumps(to_jsonable(run_pathway(_aoi)), indent=2))
    print(f"\n[{time.perf_counter() - _t0:.1f}s]", file=sys.stderr)
