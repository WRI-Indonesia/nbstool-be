"""What the Pathway Selection payload has to guarantee, checked without a network.

The card shaping is where the notebook's output becomes the screen's contract, and it is the part
with no notebook to compare against -- disturbance, the badge, the dedupe and the partition are all
decisions made here. So they are checked on stub buckets rather than on a live raster: this runs in
a second, anywhere, with no bucket and no database.

    python check_pathway.py             stub checks only
    python check_pathway.py --live      stub checks, then a real AOI end to end

The live pass needs V3_BUCKET reachable and is the only way to confirm the raster still carries the
bands 4.1 expects; the stub pass is the one that must never break.
"""

from __future__ import annotations

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from run_pathway import INTERVENTIONS, _activities_for, _ecosystem_card  # noqa: E402

PASS = FAIL = 0


def check(label: str, condition: bool) -> None:
    global PASS, FAIL
    if condition:
        PASS += 1
        print(f"  PASS  {label}")
    else:
        FAIL += 1
        print(f"  FAIL  {label}")


def cat(pathway: str, area: float, *activities: tuple[str, str]) -> dict:
    return {"pathway": pathway, "area_ha": area,
            "activities": tuple({"activity_id": i, "activity": a} for i, a in activities)}


# A 100 ha dryland bucket: 40 Protect (Cat 1, the only undisturbed category), 20 Manage,
# 30 Restore split across two categories that SHARE an activity, 10 Ineligible.
DRYLAND = {
    "area_ha": 100.0,
    "pathway_mix": {"Protect": 40.0, "Manage": 20.0, "Restore": 30.0, "Ineligible": 10.0},
    "categories": {
        "Cat 1 | Dryland forest": cat("Protect", 40.0, ("11", "Establish protected areas")),
        "Cat 6 | Dryland forest": cat("Manage", 20.0, ("61", "Protect regenerating stands")),
        "Cat 3B | Dryland forest": cat("Restore", 20.0, ("321", "ANR"), ("101", "Bio-engineering")),
        "Cat 4B | Dryland forest": cat("Restore", 10.0, ("321", "ANR")),
        "Cat 2 | Dryland forest": cat("Ineligible", 10.0),
    },
}
EMPTY = {"area_ha": 0.0, "pathway_mix": {}, "categories": {}}


print("== the partition, which is the whole contract with the frontend")
card = _ecosystem_card("Dryland", DRYLAND, project_area_ha=200.0)
by_name = {i["intervention"]: i for i in card["interventions"]}
total = sum(i["area_ha"] for i in card["interventions"]) + card["ineligible_area_ha"]
check("Protect + Manage + Restore + Ineligible == the ecosystem area",
      abs(total - card["total_area_ha"]) < 1e-9)
check("the three intervention shares never exceed 100",
      sum(i["percentage"] for i in card["interventions"]) <= 100.0 + 1e-9)
check("each share is of the ECOSYSTEM, not the project area",
      abs(by_name["Protect"]["percentage"] - 40.0) < 1e-9)
check("total_area_percentage is of the PROJECT area",
      abs(card["total_area_percentage"] - 50.0) < 1e-9)

print("\n== disturbance: a placeholder until F02-P3, and it must STAY a placeholder")
check("the three keys are present, so P3 changes no contract",
      {"disturbed_area_ha", "disturbed_percentage", "is_disturbed"} <= set(card))
check("zero, not derived from the pathway categories",
      card["disturbed_area_ha"] == 0.0 and card["disturbed_percentage"] == 0.0)
check("the badge is off for every ecosystem", card["is_disturbed"] is False)
check("and stays zero on an ecosystem that is entirely non-Cat-1 -- the old rule would have "
      "reported 100% here",
      _ecosystem_card("Peatland",
                      {"area_ha": 50.0, "pathway_mix": {"Restore": 50.0},
                       "categories": {"Cat 3B | Peatland": cat("Restore", 50.0, ("321", "ANR"))}},
                      200.0)["disturbed_percentage"] == 0.0)

print("\n== activities")
restore = by_name["Restore"]["activities"]
check("the shared activity appears once, not once per category",
      [a["activity_id"] for a in restore].count("321") == 1)
check("largest category first", restore[0]["activity_id"] == "321")
check("an ineligible category contributes none",
      _activities_for(DRYLAND["categories"], "Ineligible") == [])
check("every activity carries an id and a label",
      all(a["activity_id"] and a["activity"] for i in card["interventions"]
          for a in i["activities"]))

print("\n== an ecosystem the AOI does not contain")
absent = _ecosystem_card("Mangrove", EMPTY, project_area_ha=200.0)
check("still emitted, so the screen keeps its shape", absent["ecosystem"] == "Mangrove")
check("present is False", absent["present"] is False)
check("no NaN from dividing by a zero area",
      absent["disturbed_percentage"] == 0.0
      and all(i["percentage"] == 0.0 for i in absent["interventions"]))
check("every intervention reads non-eligible",
      all(i["eligible"] is False for i in absent["interventions"]))

print("\n== card shape is identical whatever the AOI")
for name, info in (("Dryland", DRYLAND), ("Mangrove", EMPTY)):
    c = _ecosystem_card(name, info, 200.0)
    check(f"{name}: three interventions, always in Protect/Manage/Restore order",
          [i["intervention"] for i in c["interventions"]] == INTERVENTIONS)
check("the display label differs from the analysis key", card["label"] == "Forest")
check("eligible tracks area", by_name["Protect"]["eligible"] is True)

if "--live" in sys.argv:
    print("\n== live: a real AOI end to end")
    import json

    import geopandas as gpd

    from common import prepare_aoi, to_jsonable
    from run_pathway import run_pathway

    aoi = prepare_aoi(gpd.read_file(r"D:\Documents\ALL\_test\nbs\AOI1.shp"))
    payload = to_jsonable(run_pathway(aoi))

    check("serialises to JSON", isinstance(json.dumps(payload), str))
    check("three ecosystem cards", len(payload["ecosystems"]) == 3)
    covered = sum(e["total_area_ha"] for e in payload["ecosystems"])
    check("ecosystems + unclassified == the project area",
          abs(covered + payload["unclassified_area_ha"] - payload["project_area_ha"]) < 1.0)
    for e in payload["ecosystems"]:
        if not e["present"]:
            continue
        got = sum(i["area_ha"] for i in e["interventions"]) + e["ineligible_area_ha"]
        check(f"{e['label']}: partition holds on real data",
              abs(got - e["total_area_ha"]) < 1e-6)
    check("the defaults the screen renders are present",
          payload["duration_years"]["default"] == 30 and len(payload["carbon_risk"]) == 3)

print(f"\n{PASS}/{PASS + FAIL} checks passed")
sys.exit(0 if FAIL == 0 else 1)
