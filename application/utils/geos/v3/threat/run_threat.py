"""run_threat.py - F02-P3 as ONE NDJSON stream, shaped for the four tabs of the Threat Profile.

SAME ENVELOPE AS SITE CHARACTERISATION, and the same code: `pipeline.stream`, `pipeline.safe` and
`pipeline.error_status` are imported, not reimplemented, so `process` / `data` / `error_status` /
`w` / `a` / `next` behave identically and a client parses this with what it already has. `?process=`
retries work the same way too.

STREAMED, unlike F02-P4 Pathway which returns one JSON document. The reason is cost and shape: the
four sections map one-to-one onto the four tabs, the user lands on Overview, and Overview is the
cheapest section at two rasters while the whole profile touches twelve. A single document would make
the first tab wait for the slowest.

NO DEPENDENCIES BETWEEN SECTIONS. All four run concurrently on one pool. The design does show the
peat forest / peat non-forest split on the Overview tab's Peatland card, which 3.1 does not compute
-- but wiring Overview to wait for Peatland would hand the landing tab the slowest section's latency,
so it is left to the client to fill that bar from the `peatland` line when it arrives. Both are on
one screen.

EMISSION ORDER IS CHEAPEST FIRST, for the same reason as site characterisation: a finished line
cannot be written until every line before it has been. Overview first is both the cheapest and the
tab the user is looking at.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import rasterio
from rasterio.coords import disjoint_bounds

try:
    from ..common import AOI
    from ..config import (
        THREAT_DISTURBANCE,
        THREAT_ECOSYSTEM,
        THREAT_ECOSYSTEM_CLASSES,
        THREAT_ECOSYSTEM_DISPLAY_NAMES,
        THREAT_PEAT_CANAL,
        THREAT_PEAT_CANALS_DENSITY,
        THREAT_PERIOD_FROM,
        THREAT_PERIOD_TO,
    )
    from ..pipeline import error_status, safe, stream
    from ..settings import layer_path
    from .all_ecosystems import analyze_all_ecosystem
    from .dryland_forest import analyze_dryland_forest
    from .mangrove import analyze_mangrove
    from .peatland import analyze_peatland
except ImportError:  # `python run_threat.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from all_ecosystems import analyze_all_ecosystem
    from common import AOI
    from config import (
        THREAT_DISTURBANCE,
        THREAT_ECOSYSTEM,
        THREAT_ECOSYSTEM_CLASSES,
        THREAT_ECOSYSTEM_DISPLAY_NAMES,
        THREAT_PEAT_CANAL,
        THREAT_PEAT_CANALS_DENSITY,
        THREAT_PERIOD_FROM,
        THREAT_PERIOD_TO,
    )
    from dryland_forest import analyze_dryland_forest
    from mangrove import analyze_mangrove
    from peatland import analyze_peatland
    from pipeline import error_status, safe, stream
    from settings import layer_path


# The labels `ecosystem_v3.tif` carries, which the analysis keys on. Class 1 is "Dryland" and it
# INCLUDES SAVANNA -- the layer does not separate them. The Overview card prints
# THREAT_ECOSYSTEM_DISPLAY_NAMES instead ("Dryland forest"), same split as F02-P4.
_DRYLAND_LABEL = THREAT_ECOSYSTEM_CLASSES[1]
_MANGROVE_LABEL = THREAT_ECOSYSTEM_CLASSES[2]
_PEATLAND_LABEL = THREAT_ECOSYSTEM_CLASSES[3]

_PERIOD = {"from": THREAT_PERIOD_FROM, "to": THREAT_PERIOD_TO}

# The layers each section needs to have coverage for. Every section depends on the disturbance
# layer, which is the one with the restricted footprint; peatland additionally needs the two peat
# layers, which are narrower again. See `_guard`.
_CORE_LAYERS = (THREAT_ECOSYSTEM, THREAT_DISTURBANCE)
_PEAT_LAYERS = _CORE_LAYERS + (THREAT_PEAT_CANALS_DENSITY, THREAT_PEAT_CANAL)

_G0 = {"area_ha": 0.0, "percentage": 0}
_NO_DRIVERS = {"non_natural": [], "natural": [], "other": []}

# The shape each section returns when its layers do not reach the AOI, so the tab still renders
# every field the contract promises instead of arriving empty.
_ZERO_OVERVIEW = {
    "total_ecosystem_area_ha": 0.0, "total_disturbed_area_ha": 0.0,
    "total_disturbed_percentage": 0,
    "ecosystems": {label: {"area_ha": 0.0, "percentage_total": 0,
                           "disturbed_area_ha": 0.0, "disturbed_percentage": 0}
                   for label in THREAT_ECOSYSTEM_CLASSES.values()},   # three, no Other
}   # 3.1 still computes disturbed; `_overview` drops it from the wire, see there.
_ZERO_DRYLAND = {"total_area_ha": 0.0, "remaining_forest": _G0, "disturbed": _G0,
                 "forest_loss": _G0, "forest_gain": _G0, "drivers": _NO_DRIVERS}
_ZERO_MANGROVE = {"mangrove": {"total_area_ha": 0.0, "remaining_forest": _G0, "disturbed": _G0,
                               "main_pressure": "Not identified", "drivers": _NO_DRIVERS}}
_ZERO_PEATLAND = {"peatland": {
    "total_area_ha": 0.0, "remaining_forest": _G0, "disturbed": _G0, "converted_loss": _G0,
    "drivers": {"canal_proximity": "Not identified", "canal_distance_m": None,
                "drainage_pressure": "Not identified", "fire_risk": "No risk"}}}

_NO_COVERAGE = ("The threat layers do not cover this project area, so no disturbance can be "
                "reported. The forest disturbance layer reaches only 10 degrees north, which "
                "excludes Thailand, Laos, Myanmar, northern Vietnam and the northern Philippines.")

# `w` IS PROVISIONAL. Site characterisation's weights are profiled medians; these are not, because
# nothing has yet timed this endpoint against the bucket. They are ordered by raster count -- 2, 10,
# 6, and 7-plus-polygonisation reads respectively -- which gets the ORDER right even if the
# magnitudes are wrong. Profile and replace them before anyone reads the progress bar as a promise.
processes = [
    {'name': 'preparation', 'w': 0.1},
    {'name': 'all ecosystems', 'w': 3.0},
    {'name': 'mangrove', 'w': 5.0},
    {'name': 'dryland forest', 'w': 8.0},
    {'name': 'peatland', 'w': 12.0},
    {'name': 'end', 'w': 0.1},
]


def _uncovered(aoi: AOI, layers: tuple[str, ...]) -> list[str]:
    """Which of `layers` have a footprint that does not reach this AOI.

    Only ever called AFTER a section has already failed, so the happy path pays nothing for it.
    Each check is a header read, no pixels.
    """
    out = []
    for layer in layers:
        try:
            with rasterio.open(layer_path(layer)) as src:
                bounds = tuple(aoi.geometry.to_crs(src.crs).total_bounds)
                if disjoint_bounds(bounds, src.bounds):
                    out.append(layer)
        except Exception:       # noqa: BLE001 -- a layer we cannot even open is not "uncovered"
            continue
    return out


def _guard(fn, aoi: AOI, layers: tuple[str, ...], zero: dict) -> tuple[dict, bool]:
    """Run a section, turning "the layer does not reach here" into an ANSWER rather than a crash.

    THE LAYER FOOTPRINTS ARE NOT ALL THE SAME, and one of them is much smaller than the rest:
    `forest_disturbance_v3.tif` spans latitude -15 to +10 where every other layer reaches 28.5 N.
    So an AOI in Thailand, Laos, Myanmar, northern Vietnam or the northern Philippines makes
    `rasterio.mask` raise "Input shapes do not overlap raster" -- and the notebook raises there too,
    identically. The two peat layers are narrower again.

    Left alone, `pipeline.safe` would catch that and report `partial`, offering a retry button for a
    condition no retry can change. So a genuine coverage gap is converted into `missing`, which the
    wire reports as `failed`: this IS the answer. Any OTHER failure is re-raised untouched and still
    becomes a retryable `partial`.

    Returns `(raw, covered)`; on a gap, `raw` is the section's zero-filled shape so the tab still
    renders every field it promised.
    """
    try:
        return fn(aoi), True
    except ValueError:
        if _uncovered(aoi, layers):
            return zero, False
        raise


def _absent(label: str, area_ha: float) -> list[str]:
    """`missing` for an ecosystem the AOI does not contain.

    Not a fault and never retryable: the site simply has no mangrove. Without this the tab reports
    zeros with `error_status: null`, indistinguishable from a mangrove that is present and pristine.
    """
    if area_ha > 0:
        return []
    return [f"This project area contains no {label.lower()}, so it has nothing to report."]


def _overview(aoi: AOI) -> tuple[dict, dict]:
    """3.1 shaped into the Overview tab -- AREAS ONLY.

    3.1's own disturbed figures are DELIBERATELY NOT PUT ON THE WIRE. It masks
    `ecosystem AND disturbance>0`, while the three per-ecosystem tabs mask
    `ecosystem AND forest_2024 AND disturbance>0` -- so on the data team's own run the two differ by
    up to 2600x for the same ecosystem (mangrove 962.32 ha here against 0.36 ha on its tab). Showing
    both on one screen would read as a bug. The data team CONFIRMED the PER-TAB definition
    (2026-08-19), so the client fills each card's disturbed figure, and the "Total Disturbed area"
    headline, from the three tab lines.

    Overview is NOT made to wait for those tabs: it is the landing tab and the cheapest section, and
    a dependency would hand it the slowest section's latency. Its areas render immediately and the
    disturbed figures fill in as the tabs arrive, on the same screen.

    THREE ECOSYSTEMS, NO "OTHER". `total_ecosystem_area_ha` counts only classes 1-3, so the three
    cards sum to exactly 100% of it; land that is none of them shows as the shortfall in
    `total_ecosystem_percentage` against the project area.
    """
    raw, covered = _guard(analyze_all_ecosystem, aoi, _CORE_LAYERS, _ZERO_OVERVIEW)
    by_label = raw['ecosystems']

    cards = []
    for key, label in (('Dryland', _DRYLAND_LABEL),
                       ('Mangrove', _MANGROVE_LABEL),
                       ('Peatland', _PEATLAND_LABEL)):
        row = by_label.get(label, {})
        cards.append({
            'ecosystem': key,
            'label': THREAT_ECOSYSTEM_DISPLAY_NAMES.get(key, label),
            'area_ha': row.get('area_ha', 0.0),
            'percentage_total': row.get('percentage_total', 0.0),
        })

    total = raw['total_ecosystem_area_ha']
    view = {
        'total_ecosystem_area_ha': total,
        'total_ecosystem_percentage': round(100.0 * total / aoi.area_ha, 2) if aoi.area_ha else 0.0,
        'ecosystems': cards,
    }

    missing = []
    if not covered:
        missing.append(_NO_COVERAGE)
    elif total <= 0:
        missing.append("The ecosystem layer does not cover this project area, so no threat can be "
                       "reported.")
    results = {'narrative': "", 'tables': {}, 'values': raw, 'flags': [], 'missing': missing}
    return results, view


def _graded(raw: dict, key: str) -> tuple[float, float]:
    """`{area_ha, percentage}` sub-dict flattened. The notebook nests these; the cards do not."""
    block = raw.get(key) or {}
    return block.get('area_ha', 0.0), block.get('percentage', 0.0)


def _dryland(aoi: AOI) -> tuple[dict, dict]:
    """3.2 shaped into the Forest tab. Dryland here is forest AND savanna."""
    raw, covered = _guard(analyze_dryland_forest, aoi, _CORE_LAYERS, _ZERO_DRYLAND)
    remaining_ha, remaining_pct = _graded(raw, 'remaining_forest')
    disturbed_ha, disturbed_pct = _graded(raw, 'disturbed')
    loss_ha, loss_pct = _graded(raw, 'forest_loss')
    gain_ha, gain_pct = _graded(raw, 'forest_gain')

    view = {
        'total_area_ha': raw['total_area_ha'],
        'remaining_forest_ha': remaining_ha,
        'remaining_forest_percentage': remaining_pct,
        'disturbed_area_ha': disturbed_ha,
        'disturbed_percentage': disturbed_pct,
        'forest_loss_ha': loss_ha,
        'forest_loss_percentage': loss_pct,
        'forest_gain_ha': gain_ha,
        'forest_gain_percentage': gain_pct,
        'period': dict(_PERIOD),
        'drivers': raw['drivers'],
    }
    # A PERMANENT METHODOLOGY CAVEAT, so a note rather than a flag: it is true on every request and
    # routing it through error_status would mark this tab every single time.
    notes = ["3.2: the drivers are presence only -- no area and no ranking is computed, so the "
             "narrative cannot name a dominant driver. Drought and Typhoon can never appear at "
             "all: no layer supplies them."]
    missing = [_NO_COVERAGE] if not covered else _absent(_DRYLAND_LABEL, raw['total_area_ha'])
    results = {'narrative': "", 'tables': {}, 'values': raw, 'flags': [],
               'missing': missing, 'notes': notes}
    return results, view


def _mangrove(aoi: AOI) -> tuple[dict, dict]:
    """3.3 shaped into the Mangrove tab. The notebook nests its result under a `mangrove` key."""
    wrapped, covered = _guard(analyze_mangrove, aoi, _CORE_LAYERS, _ZERO_MANGROVE)
    raw = wrapped['mangrove']
    remaining_ha, remaining_pct = _graded(raw, 'remaining_forest')
    disturbed_ha, disturbed_pct = _graded(raw, 'disturbed')

    view = {
        'total_area_ha': raw['total_area_ha'],
        'remaining_forest_ha': remaining_ha,
        'remaining_forest_percentage': remaining_pct,
        'disturbed_area_ha': disturbed_ha,
        'disturbed_percentage': disturbed_pct,
        'main_pressure': raw['main_pressure'],
        'period': dict(_PERIOD),
        'drivers': raw['drivers'],
    }
    missing = [_NO_COVERAGE] if not covered else _absent(_MANGROVE_LABEL, raw['total_area_ha'])
    results = {'narrative': "", 'tables': {}, 'values': raw, 'flags': [], 'missing': missing}
    return results, view


def _peatland(aoi: AOI) -> tuple[dict, dict]:
    """3.4 shaped into the Peatland tab. Reports converted/loss and three graded indicators."""
    wrapped, covered = _guard(analyze_peatland, aoi, _PEAT_LAYERS, _ZERO_PEATLAND)
    raw = wrapped['peatland']
    remaining_ha, remaining_pct = _graded(raw, 'remaining_forest')
    disturbed_ha, disturbed_pct = _graded(raw, 'disturbed')
    converted_ha, converted_pct = _graded(raw, 'converted_loss')
    drivers = raw['drivers']

    view = {
        'total_area_ha': raw['total_area_ha'],
        'remaining_forest_ha': remaining_ha,
        'remaining_forest_percentage': remaining_pct,
        'disturbed_area_ha': disturbed_ha,
        'disturbed_percentage': disturbed_pct,
        'converted_loss_ha': converted_ha,
        'converted_loss_percentage': converted_pct,
        'canal_proximity': drivers['canal_proximity'],
        'canal_distance_m': drivers['canal_distance_m'],
        'drainage_pressure': drivers['drainage_pressure'],
        'fire_risk': drivers['fire_risk'],
    }
    notes = ["3.4: canal proximity and drainage pressure deliberately consider canals OUTSIDE the "
             "project boundary -- canals affect the water table up to 500 m away and biomass "
             "growth up to 1 km, which is where the High and Medium thresholds come from."]
    missing = [_NO_COVERAGE] if not covered else _absent(_PEATLAND_LABEL, raw['total_area_ha'])
    results = {'narrative': "", 'tables': {}, 'values': raw, 'flags': [],
               'missing': missing, 'notes': notes}
    return results, view


# name -> the adapter that produces (results, view_results). No dependency column, unlike
# run_site_characterisation: none of these four needs another's output.
SECTIONS = {
    'all ecosystems': _overview,
    'mangrove': _mangrove,
    'dryland forest': _dryland,
    'peatland': _peatland,
}


def resolve(wanted: list[str] | None) -> list[str]:
    """The sections to run, in emission order. `None` means all of them."""
    if wanted is None:
        return list(SECTIONS)
    asked = set(wanted)
    return [name for name in SECTIONS if name in asked]


def plan(wanted: list[str] | None) -> list[dict]:
    """The `processes` list for one run: the bookends plus the sections being emitted."""
    if wanted is None:
        return processes
    asked = set(wanted)
    return [processes[0]] + [p for p in processes[1:-1] if p['name'] in asked] + [processes[-1]]


def _run_sections(aoi: AOI, wanted: list[str] | None = None):
    """Run the requested sections concurrently, yielding `(view_results, error_status)` in order.

    The pool is sized to what is actually running, so a one-section retry spawns one thread rather
    than four. No section waits on another, so there is no deadlock constraint here -- unlike site
    characterisation, where four components park a worker on a dependency's future.
    """
    running = resolve(wanted)
    if not running:
        return

    with ThreadPoolExecutor(max_workers=len(running), thread_name_prefix='threat') as pool:
        futures = {name: pool.submit(safe, name, SECTIONS[name], aoi) for name in running}
        for name in running:
            results, view = futures[name].result()
            yield view, error_status(results)


def stream_threat(aoi: AOI, wanted: list[str] | None = None, retry_url=None):
    """NDJSON lines for one AOI: the plan, then one line per section, then `end`."""
    return stream(plan(wanted), _run_sections(aoi, wanted), retry_url)


if __name__ == "__main__":
    # Run on a file and print the stream the endpoint sends, no Flask app:
    #     python run_threat.py [aoi path]
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ..common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    _aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {_aoi.area_ha:,.0f} ha\n", file=sys.stderr)
    _t0 = time.perf_counter()
    for _line in stream_threat(_aoi):
        sys.stdout.write(_line)
        sys.stdout.flush()
    print(f"\n[{time.perf_counter() - _t0:.1f}s]", file=sys.stderr)
