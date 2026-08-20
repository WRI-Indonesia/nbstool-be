"""4.3 Pathway & Activities by Ecosystem - 4.1 and 4.2 regrouped with ECOSYSTEM as the primary axis.

For each ecosystem present in the AOI: its pathway mix and the activities that fall under it. This
recomputes nothing. It re-aggregates `results["4.2"].values["by_category"]`, so 4.1 and 4.2 remain
the source of truth and the F02-P5 activity join is unaffected.

Three display buckets: Dryland, Mangrove, Peatland. THE SAVANNA REFERENCE IS FOLDED INTO DRYLAND --
the tool reports three ecosystems and savanna stays an internal distinction. Folding changes only
the grouping header, never the pixel's cat_code, pathway, or activity, so a savanna cell keeps its
own row and a flag notes any activity label that still reads "savanna".

Pixels with no reference ecosystem (code 0) are not an ecosystem and stay in an Unclassified
residual, so the areas still sum to the whole AOI. Empty buckets are kept at 0 ha so the output has
the same shape for every AOI. Ineligible categories are listed with their area and a no-activity
marker, not dropped.

This is the component the Pathway Selection screen is built on; `run_pathway` shapes it into cards.
Body unchanged from the notebook.
"""

from __future__ import annotations

from dataclasses import dataclass

try:
    from ..common import ComponentResult, fmt_ha, not_applicable, safe_pct
    from ..config import PATHWAY_ECOSYSTEM_CODES
except ImportError:  # `python by_ecosystem.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import ComponentResult, fmt_ha, not_applicable, safe_pct
    from config import PATHWAY_ECOSYSTEM_CODES


# Ecosystem display buckets. Savanna (code 4) is folded into Dryland on purpose: the tool reports
# three ecosystems, and the savanna reference stays an internal distinction. Folding changes only
# the grouping header, never the pixel's cat_code, pathway, or activity.
ECOSYSTEM_BUCKETS = {          # display name -> ecosystem codes it absorbs
    "Dryland": (1, 4),
    "Mangrove": (2,),
    "Peatland": (3,),
}
_ECO_CODE_TO_BUCKET = {code: name for name, codes in ECOSYSTEM_BUCKETS.items() for code in codes}
_PATHWAY_MIX_ORDER = ["Protect", "Manage", "Restore", "Ineligible"]


@dataclass(frozen=True)
class EcosystemSummary:
    """One ecosystem bucket: total area, share of AOI, and the area split by pathway."""
    ecosystem: str
    area_ha: float
    pct: float
    protect_ha: float
    manage_ha: float
    restore_ha: float
    ineligible_ha: float
    present: bool


def analyze_by_ecosystem(aoi, results) -> "ComponentResult":
    """Component 4.3. Regroup 4.2 with ecosystem as the primary axis: for each ecosystem, its
    pathway mix and the activity list under it. Pure re-aggregation of results['4.2'], so 4.1/4.2
    stay the source of truth and F02-P5 is unaffected.
    """
    src = results.get("4.2")
    if src is None or not src.applicable:
        return not_applicable(
            "4.3 Pathway & Activities by Ecosystem",
            "The activity list (4.2) is not available for this project area, so it cannot be "
            "grouped by ecosystem.",
        )
    by_category = src.values.get("by_category", {})

    _label_to_code = {label: code for code, label in PATHWAY_ECOSYSTEM_CODES.items()}

    # Accumulate per bucket.
    buckets: dict[str, dict] = {
        name: {"area_ha": 0.0, "pathway_mix": {}, "rows": [], "categories": {}}
        for name in ECOSYSTEM_BUCKETS
    }
    savanna_ha = 0.0
    savanna_activity_labels: set[str] = set()
    off_bucket_ha = 0.0            # entries with ecosystem 0 / unknown: not a reported ecosystem
    flags: list[str] = []          # 4.3 has its own flags; 4.2 already surfaced its own

    for key, info in by_category.items():
        cat_label, eco_label = [s.strip() for s in key.split("|")]
        eco_code = _label_to_code.get(eco_label)
        bucket = _ECO_CODE_TO_BUCKET.get(eco_code)
        area_ha = info["area_ha"]
        pathway = info["pathway"]
        acts = info.get("activities", [])

        if bucket is None:            # ecosystem 0 (no reference) or unknown: stays unclassified
            off_bucket_ha += area_ha
            continue

        if eco_code == 4:             # savanna folded into Dryland
            savanna_ha += area_ha
            for a in acts:
                if "savanna" in a.get("activity", "").lower():
                    savanna_activity_labels.add(a["activity"])

        b = buckets[bucket]
        b["area_ha"] += area_ha
        b["pathway_mix"][pathway] = b["pathway_mix"].get(pathway, 0.0) + area_ha
        b["categories"][f"{cat_label} | {eco_label}"] = {
            "pathway": pathway, "area_ha": area_ha, "activities": acts,
        }
        if acts:
            for a in acts:
                b["rows"].append({
                    "ecosystem": bucket, "pathway": pathway, "category": cat_label,
                    "area_ha": round(area_ha, 1),
                    "activity_id": a["activity_id"], "activity": a["activity"],
                })
        else:
            note = "(ineligible, no activity)" if pathway == "Ineligible" else "(no catalog match)"
            b["rows"].append({
                "ecosystem": bucket, "pathway": pathway, "category": cat_label,
                "area_ha": round(area_ha, 1), "activity_id": "", "activity": note,
            })

    # Ordered summary + activity tables. Empty buckets are kept so every AOI has the same shape.
    summary_rows: list[EcosystemSummary] = []
    activity_rows: list[dict] = []
    by_ecosystem: dict[str, dict] = {}
    for name in ECOSYSTEM_BUCKETS:
        b = buckets[name]
        mix = b["pathway_mix"]
        summary_rows.append(EcosystemSummary(
            ecosystem=name, area_ha=b["area_ha"], pct=safe_pct(b["area_ha"], aoi.area_ha),
            protect_ha=mix.get("Protect", 0.0), manage_ha=mix.get("Manage", 0.0),
            restore_ha=mix.get("Restore", 0.0), ineligible_ha=mix.get("Ineligible", 0.0),
            present=b["area_ha"] > 0,
        ))
        activity_rows.extend(sorted(b["rows"], key=lambda r: r["area_ha"], reverse=True))
        by_ecosystem[name] = {
            "area_ha": b["area_ha"], "pct": safe_pct(b["area_ha"], aoi.area_ha),
            "pathway_mix": {pw: mix[pw] for pw in _PATHWAY_MIX_ORDER if pw in mix},
            "categories": b["categories"],
        }

    if off_bucket_ha > 0:
        flags.append(
            f"4.3: {fmt_ha(off_bucket_ha)} has a category but no reference ecosystem; "
            "counted as Unclassified here, not under any ecosystem."
        )

    classified_ha = sum(b["area_ha"] for b in buckets.values())
    unclassified_ha = max(0.0, aoi.area_ha - classified_ha - off_bucket_ha) + off_bucket_ha
    unclassified_pct = safe_pct(unclassified_ha, aoi.area_ha)

    if savanna_ha > 0:
        flags.append(
            f"4.3: {fmt_ha(savanna_ha)} of savanna reference is reported under Dryland. "
            "cat_code and pathway are unchanged."
        )
        if savanna_activity_labels:
            flags.append(
                "4.3: these activity labels still read 'savanna' under the Dryland group: "
                + "; ".join(sorted(savanna_activity_labels))
            )

    return ComponentResult(
        component="4.3 Pathway & Activities by Ecosystem",
        applicable=True,
        narrative="",
        tables={
            "ecosystem_summary": summary_rows,   # one row per ecosystem, area + pathway split
            "ecosystem_activities": activity_rows,  # activities grouped under each ecosystem
        },
        values={
            "by_ecosystem": by_ecosystem,
            "ecosystems_present": [n for n in ECOSYSTEM_BUCKETS if buckets[n]["area_ha"] > 0],
            "unclassified_ha": unclassified_ha,
            "unclassified_pct": unclassified_pct,
        },
        flags=flags,
    )
