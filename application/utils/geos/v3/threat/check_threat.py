"""What the Threat Profile stream has to guarantee, checked without a network.

The four analysis bodies are the notebook's and are verified by diffing against it. What is NOT in
the notebook is the shaping: flattening its nested `{area_ha, percentage}` blocks into card fields,
splitting Other out of the Overview, reporting an absent ecosystem as `failed` rather than as a
healthy row of zeros, and the envelope itself. That is what this checks, on stub sections, so it
runs in a second with no bucket and no database.

    python check_threat.py            stub checks only
    python check_threat.py --live     stub checks, then a real AOI end to end
"""

from __future__ import annotations

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import run_threat as R                                        # noqa: E402
from config import THREAT_ECOSYSTEM_CLASSES                   # noqa: E402
from pipeline import error_status                             # noqa: E402

PASS = FAIL = 0


def check(label: str, condition: bool) -> None:
    global PASS, FAIL
    if condition:
        PASS += 1
        print(f"  PASS  {label}")
    else:
        FAIL += 1
        print(f"  FAIL  {label}")


class FakeAOI:
    """Only `area_ha` is read by the shaping; the analysis is stubbed out."""
    area_ha = 1000.0


def graded(area, pct):
    return {"area_ha": area, "percentage": pct}


# ---- stub sections, shaped exactly as each notebook cell returns -------------------------------
OVERVIEW_RAW = {
    "total_ecosystem_area_ha": 800.0,
    "total_disturbed_area_ha": 200.0,
    "total_disturbed_percentage": 25.0,
    "ecosystems": {
        "Dryland": {"area_ha": 400.0, "percentage_total": 50.0,
                    "disturbed_area_ha": 120.0, "disturbed_percentage": 30.0},
        "Mangrove": {"area_ha": 200.0, "percentage_total": 25.0,
                     "disturbed_area_ha": 50.0, "disturbed_percentage": 25.0},
        "Peatland": {"area_ha": 200.0, "percentage_total": 25.0,
                     "disturbed_area_ha": 30.0, "disturbed_percentage": 15.0},
    },
}
DRYLAND_RAW = {
    "total_area_ha": 400.0,
    "remaining_forest": graded(300.0, 75.0),
    "disturbed": graded(120.0, 30.0),
    "forest_loss": graded(40.0, 10.0),
    "forest_gain": graded(12.0, 3.0),
    "drivers": {"non_natural": ["Mining"], "natural": ["Flooding"], "other": ["Unknown"]},
}
MANGROVE_RAW = {"mangrove": {
    "total_area_ha": 200.0,
    "remaining_forest": graded(150.0, 75.0),
    "disturbed": graded(50.0, 25.0),
    "main_pressure": "Commodities",
    "drivers": {"non_natural": ["Commodities"], "natural": [], "other": ["Other"]},
}}
PEATLAND_RAW = {"peatland": {
    "total_area_ha": 100.0,
    "remaining_forest": graded(25.0, 25.0),
    "disturbed": graded(30.0, 30.0),
    "converted_loss": graded(20.0, 20.0),
    "drivers": {"canal_proximity": "High", "canal_distance_m": 0.0,
                "drainage_pressure": "Medium", "fire_risk": "High"},
}}

R.analyze_all_ecosystem = lambda aoi: OVERVIEW_RAW
R.analyze_dryland_forest = lambda aoi: DRYLAND_RAW
R.analyze_mangrove = lambda aoi: MANGROVE_RAW
R.analyze_peatland = lambda aoi: PEATLAND_RAW

aoi = FakeAOI()

print("== the Overview tab")
_res, ov = R._overview(aoi)
check("three cards, always Dryland / Mangrove / Peatland",
      [c["ecosystem"] for c in ov["ecosystems"]] == ["Dryland", "Mangrove", "Peatland"])
check("three classes only -- no Other card and no Other field",
      len(ov["ecosystems"]) == 3 and "other_area_ha" not in ov)
check("the three cards sum to exactly 100 of the ecosystem area",
      sum(c["percentage_total"] for c in ov["ecosystems"]) == 100.0)
check("analysis key is the layer's label, card prints the design's",
      THREAT_ECOSYSTEM_CLASSES[1] == "Dryland"
      and ov["ecosystems"][0]["ecosystem"] == "Dryland"
      and ov["ecosystems"][0]["label"] == "Dryland forest")
check("3.1's own disturbed figures never reach the wire -- they use a different mask",
      all("disturbed_area_ha" not in c for c in ov["ecosystems"])
      and "total_disturbed_area_ha" not in ov)
check("total_ecosystem_percentage is of the PROJECT area, not of itself",
      ov["total_ecosystem_percentage"] == 80.0)
check("a clean overview reports no error_status", error_status(_res) is None)

print("\n== the nested {area_ha, percentage} blocks are flattened for the cards")
_res, fo = R._dryland(aoi)
check("forest: five tiles present",
      {"total_area_ha", "remaining_forest_ha", "disturbed_area_ha", "forest_loss_ha",
       "forest_gain_ha"} <= set(fo))
check("forest: no nested block survives",
      not any(isinstance(v, dict) for k, v in fo.items() if k not in ("period", "drivers")))
check("forest: percentages travel beside their areas",
      fo["remaining_forest_percentage"] == 75.0 and fo["forest_gain_percentage"] == 3.0)
check("forest: the narrative period is stated, not derived from a raster",
      fo["period"] == {"from": 2015, "to": 2024})
_res, mg = R._mangrove(aoi)
check("mangrove: unwrapped from the notebook's `mangrove` key", mg["total_area_ha"] == 200.0)
check("mangrove: main_pressure carried through", mg["main_pressure"] == "Commodities")
_res, pt = R._peatland(aoi)
check("peatland: converted/loss replaces loss+gain",
      "converted_loss_ha" in pt and "forest_gain_ha" not in pt)
check("peatland: the three indicators are flat strings",
      (pt["canal_proximity"], pt["drainage_pressure"], pt["fire_risk"]) == ("High", "Medium", "High"))

print("\n== an ecosystem the AOI does not contain")
R.analyze_mangrove = lambda aoi: {"mangrove": dict(
    MANGROVE_RAW["mangrove"], total_area_ha=0.0, main_pressure="Not identified",
    remaining_forest=graded(0.0, 0), disturbed=graded(0.0, 0),
    drivers={"non_natural": [], "natural": [], "other": []})}
res, mg0 = R._mangrove(aoi)
st = error_status(res)
check("reports a state rather than a healthy row of zeros", st is not None)
check("and it is `failed` -- asking again cannot grow a mangrove",
      st["state"] == "failed" and st["retryable"] is False)
check("with a message naming the ecosystem", "mangrove" in st["messages"][0].lower())
check("the tab still gets its full shape", mg0["total_area_ha"] == 0.0 and "drivers" in mg0)
R.analyze_mangrove = lambda aoi: MANGROVE_RAW

print("\n== the envelope, which is site-characterisation's")
lines = [json.loads(x) for x in R.stream_threat(aoi)]
check("six lines: preparation, four tabs, end", len(lines) == 6)
check("first is preparation carrying the plan",
      lines[0]["process"] == "preparation" and "processes" in lines[0]["data"])
check("last is end with a null next",
      lines[-1]["process"] == "end" and lines[-1]["next"] is None)
check("emitted order matches the plan",
      [ln["process"] for ln in lines] == [p["name"] for p in R.processes])
check("cheapest tab first, so Overview lands before the slow ones",
      [p["name"] for p in R.processes[1:-1]] ==
      sorted((p["name"] for p in R.processes[1:-1]),
             key=lambda n: next(p["w"] for p in R.processes if p["name"] == n)))
check("`w` is this run's own total",
      abs(lines[0]["w"] - round(sum(p["w"] for p in R.processes), 2)) < 1e-9)
check("every line carries the envelope keys",
      all({"process", "data", "error_status", "w", "a", "next"} == set(ln) for ln in lines))

print("\n== a retry of one tab")
sub = [json.loads(x) for x in R.stream_threat(aoi, ["peatland"])]
check("three lines: preparation, the tab, end", len(sub) == 3)
check("only the tab asked for is emitted", sub[1]["process"] == "peatland")
check("`w` describes THAT run, so the progress bar is self-contained",
      abs(sub[0]["w"] - round(0.1 + 12.0 + 0.1, 2)) < 1e-9)
check("an unknown name resolves to nothing rather than guessing", R.resolve(["nope"]) == [])

print("\n== a section that raises loses its tab, not the response")
def boom(_aoi):
    raise RuntimeError("postgresql://user:hunter2@10.0.0.5:5432/gis is not reachable")
R.analyze_peatland = boom
crashed = [json.loads(x) for x in R.stream_threat(aoi, ["peatland"])]
st = crashed[1]["error_status"]
check("the stream still completes with `end`", crashed[-1]["process"] == "end")
check("the tab reports `partial`, so a retry is offered",
      st["state"] == "partial" and st["retryable"] is True)
check("and the exception text never reaches the wire",
      "hunter2" not in json.dumps(crashed) and "RuntimeError" in st["messages"][0])
R.analyze_peatland = lambda aoi: PEATLAND_RAW

if "--live" in sys.argv:
    print("\n== live: a real AOI end to end")
    import importlib

    import geopandas as gpd

    import settings
    settings._cache.setdefault("V3_BUCKET", "https://storage.googleapis.com/assets-geo/v3")
    importlib.reload(R)
    from common import prepare_aoi

    # muara_merang is the only test AOI with real peatland, so it is the one that exercises the
    # canal-proximity path. A zipped shapefile needs the zip:// prefix and forward slashes.
    import os
    zipped = os.path.abspath(r"D:\Documents\ALL\_test\nbs\muara_merang_4326.zip")
    live = prepare_aoi(gpd.read_file("zip://" + zipped.replace("\\", "/")))
    got = [json.loads(x) for x in R.stream_threat(live)]
    by = {ln["process"]: ln["data"] for ln in got}
    check("six lines", len(got) == 6)
    ovl = by["all ecosystems"]
    check("the three cards sum to the total ecosystem area",
          abs(sum(c["area_ha"] for c in ovl["ecosystems"])
              - ovl["total_ecosystem_area_ha"]) < 1.0)
    check("total ecosystem never exceeds the project area",
          ovl["total_ecosystem_percentage"] <= 100.0 + 1e-6)
    for tab in ("dryland forest", "mangrove", "peatland"):
        d = by[tab]
        check(f"{tab}: disturbed <= total",
              d["disturbed_area_ha"] <= d["total_area_ha"] + 1e-6)
    pk = by["peatland"]
    check("peatland: indicators are from the documented vocabularies",
          pk["canal_proximity"] in ("High", "Medium", "Low", "Not identified")
          and pk["drainage_pressure"] in ("High", "Medium", "Low", "Not identified")
          and pk["fire_risk"] in ("High", "Medium", "Low", "Very low", "No risk"))

print(f"\n{PASS}/{PASS + FAIL} checks passed")
sys.exit(0 if FAIL == 0 else 1)
