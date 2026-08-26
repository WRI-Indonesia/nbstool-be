"""
run_benefit.py - the F02-P5 Benefit components (5.1-5.6, 5.9-5.13) behind one runner.

Each component returns the house `(results, view_results)` pair -- plain dicts, like site
characterisation, threat and pathway. `view_results` is the flat card contract: `applicable`,
`narrative`, and exactly the fields the interactive-map Step 5 cards interpolate -- headline
metric, formula breakdown, species/hazard row lists. Diagnostics and provenance stay in
`results`; permanent methodology caveats travel as `notes` (never `flags` -- they must not
drive `error_status`).

The seams replacing the notebook's saved stage files:

    rate_pct        the caller reads component 1.5's rate from the persisted DataAnalyzer row
                    (`site_information_json.historical_deforestation_percentage`, the same
                    value the notebook reads as `component_values(general, "1.5")["rate_pct"]`)
    activity_stage  a fresh 4.2 run wrapped in the notebook's stage shape, for 5.1/5.2's QB
                    gate -- run once, never emitted

5.4/5.5/5.6 exist only for an NbS carbon project (the notebook's 4.4 toggle) and only when a
gross component is applicable; otherwise they emit as not-applicable cards -- the notebook's
"section remains deactivated". The deduction percentages come from the user (GUI), defaulting
to CARBON_RISK_DEFAULTS, and their sum may not exceed 100 -- the notebook's own validation.
"""

from __future__ import annotations

try:
    from ..common import AOI, to_jsonable
    from ..config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_CLASS,
        INTERVENTION_DURATION_DEFAULT_YEARS,
    )
    from ..pathway.activity_list import analyze_activity_list
    from ..pipeline import after, error_status, safe, stream
    from .avoided_deforestation import analyze_avoided_deforestation_emissions
    from .arr_sequestration import analyze_arr_sequestration
    from .biodiversity_uplift import analyze_biodiversity_uplift
    from .climate_resilience import analyze_climate_resilience
    from .general_benefit import analyze_general_benefit
    from .habitat_loss_avoided import analyze_habitat_loss_avoided
    from .microclimate import analyze_microclimate
    from .net_carbon import net_carbon_removal, net_emission_reduction
    from .net_errs import net_errs
    from .threatened_species import analyze_threatened_species_habitat
except ImportError:  # `python run_benefit.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "pathway"))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from activity_list import analyze_activity_list
    from arr_sequestration import analyze_arr_sequestration
    from avoided_deforestation import analyze_avoided_deforestation_emissions
    from biodiversity_uplift import analyze_biodiversity_uplift
    from climate_resilience import analyze_climate_resilience
    from common import AOI, to_jsonable
    from microclimate import analyze_microclimate
    from config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_CLASS,
        INTERVENTION_DURATION_DEFAULT_YEARS,
    )
    from general_benefit import analyze_general_benefit
    from habitat_loss_avoided import analyze_habitat_loss_avoided
    from net_carbon import net_carbon_removal, net_emission_reduction
    from net_errs import net_errs
    from pipeline import after, error_status, safe, stream
    from threatened_species import analyze_threatened_species_habitat


def run_benefit(aoi: AOI, duration_years: int = INTERVENTION_DURATION_DEFAULT_YEARS,
                rate_pct: float | None = None, carbon_project: bool = True,
                leakage: float | None = None, uncertainty: float | None = None,
                buffer: float | None = None,
                ecosystem_class: int = ECOSYSTEM_CLASS) -> dict:
    """Every emitted component's card view as ONE dict, `{process: data}` -- exactly the
    stream's lines minus the envelope, so the persisted `benefit_json` and this response are
    the same shape. Runs the components sequentially; scripts and offline checks only, the
    endpoint serves `stream_benefit`."""
    if leakage is None:
        leakage = CARBON_RISK_DEFAULTS["leakage_percentage"]
    if uncertainty is None:
        uncertainty = CARBON_RISK_DEFAULTS["uncertainty_percentage"]
    if buffer is None:
        buffer = CARBON_RISK_DEFAULTS["buffer_percentage"]
    validate(duration_years, leakage, uncertainty, buffer)

    components = _components(duration_years, rate_pct, carbon_project,
                             leakage, uncertainty, buffer, ecosystem_class, None)
    done: dict[str, tuple[dict, dict]] = {}
    for name in _order(components):
        fn, deps = components[name]
        done[name] = fn(aoi, *(done[d] for d in deps))
    return to_jsonable({name: view for name, (_, view) in done.items() if name not in _HIDDEN})


# ---------------------------------------------------------------------------------------------
# Streaming form -- what the endpoint serves. Same NDJSON envelope as every other v3 stream.
#
# WHY STREAMED: the carbon components are ready in about six seconds while 5.9 and 5.10 take
# ~25 s each, so a single document makes the fast cards wait for the slowest. Process names are
# the SAME UNDERSCORE KEYS the JSON response used (`avoided_emissions`, `net_errs`, ...), so the
# persisted benefit_json keeps its exact shape -- persist nests each line under its process name.
#
# Each line's `data` is the component's own `view_results` -- the flat card contract
# (applicable / narrative / the card's fields and row lists). `assumptions` is the first line:
# the run's inputs and the screen's `selections`, persisted with everything else. When
# `carbon_project` is off,
# 5.4/5.5/5.6 still EMIT (as not-applicable "deactivated" cards) so a re-run with the toggle off
# overwrites a previous carbon run in the persisted row instead of leaving stale nets behind.
#
# Retry: the endpoint is POST, so `error_status.retry_url` stays null; a client re-POSTs the
# same body with `process: [names]` instead. `activity_stage` (4.2) runs as an un-emitted
# dependency, exactly like site characterisation's pulled-in dependencies.
# ---------------------------------------------------------------------------------------------

# Weights: 5.2/5.3 measured standalone; 5.9/5.10 measured with their parallel habitat loops;
# the arithmetic components are effectively free. `activity_stage` is the 4.2 run.
_W = {
    'assumptions': 0.1,
    'activity_stage': 2.0,
    'general_benefit': 0.1,
    'avoided_emissions': 4.0,
    'arr_sequestration': 9.0,
    'net_emission_reduction': 0.1,
    'net_carbon_removal': 0.1,
    'net_errs': 0.1,
    'habitat_loss_avoided': 27.0,
    'threatened_species_habitat': 25.0,
    'biodiversity_uplift': 30.0,
    'climate_resilience': 9.0,
    'microclimate': 6.5,
}

_DEACTIVATED = ("This section is deactivated: the project was not marked as an NbS carbon "
                "project.")

# The People tab's six user-answered benefit cards ("Tap to specify this benefit"), in the FE's
# display order. Their prompts, options and statement templates live in the frontend; the
# backend only stores what the user specified.
PEOPLE_BENEFIT_KEYS = ('food_water', 'livelihood', 'social', 'equity', 'tenure', 'cultural')


def _na_pair(reason: str) -> tuple[dict, dict]:
    """A not-applicable card built in this seam (deactivated toggle, missing dependency).
    `missing` -> error_status `failed`: this IS the answer, no retry can change it. The
    components' own caveats travel as `notes` in results and the view, never `flags` --
    routed into `error_status` they once marked every carbon line `failed` while it carried
    real numbers."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def _components(duration_years: int, rate_pct, carbon_project: bool,
                leakage: float, uncertainty: float, buffer: float,
                ecosystem_class: int, selections, people_benefits=None) -> dict:
    """name -> (fn, dependencies) for one request, parameters closed over."""

    def _assumptions(aoi: AOI) -> tuple[dict, dict]:
        view = {
            'duration_years': duration_years,
            'carbon_project': carbon_project,
            'carbon_risk': {
                'leakage_percentage': leakage,
                'uncertainty_percentage': uncertainty,
                'buffer_percentage': buffer,
            },
        }
        if selections is not None:
            view['selections'] = selections
        # The People-tab cards are answered by the user, not computed. Every slot is emitted
        # (null = not yet specified) so the client knows what can be filled; a filled card is
        # {"statement": ..., "answer": ...} (answer is a list for the livelihood multi-select).
        # Saving an answer is a re-POST with process: ["assumptions"] and the filled dict --
        # only this line re-runs and the persisted merge keeps everything else.
        view['people_benefits'] = {
            key: (people_benefits or {}).get(key) for key in PEOPLE_BENEFIT_KEYS
        }
        return {'flags': [], 'missing': []}, view

    def _activity_stage(aoi: AOI) -> tuple[dict, dict]:
        # 4.2 wrapped in the notebook stage shape 5.1 and 5.2 read. Never emitted.
        return {'stage': {'components': {'4.2': to_jsonable(analyze_activity_list(aoi))}}}, {}

    def _stage_of(dep: tuple[dict, dict]) -> dict:
        results, _ = dep
        return results.get('stage', {'components': {}})

    def _general(aoi: AOI, stage) -> tuple[dict, dict]:
        return analyze_general_benefit(_stage_of(stage))

    def _avoided(aoi: AOI, stage) -> tuple[dict, dict]:
        return analyze_avoided_deforestation_emissions(
            aoi, duration_years, rate_pct, _stage_of(stage))

    def _arr(aoi: AOI) -> tuple[dict, dict]:
        return analyze_arr_sequestration(aoi, duration_years)

    def _gross_of(dep: tuple[dict, dict]) -> float | None:
        _, view = dep
        if view.get('applicable'):
            return view['total_tco2e']
        return None

    def _net_er(aoi: AOI, avoided) -> tuple[dict, dict]:
        if not carbon_project:
            return _na_pair(_DEACTIVATED)
        gross = _gross_of(avoided)
        if gross is None:
            return _na_pair("5.2 is not applicable on this site, so there is no gross "
                            "emission reduction to deduct from.")
        return net_emission_reduction(gross, leakage, uncertainty, buffer)

    def _net_removal(aoi: AOI, arr) -> tuple[dict, dict]:
        if not carbon_project:
            return _na_pair(_DEACTIVATED)
        gross = _gross_of(arr)
        if gross is None:
            return _na_pair("5.3 is not applicable on this site, so there is no gross "
                            "removal to deduct from.")
        return net_carbon_removal(gross, leakage, uncertainty, buffer)

    def _net_errs_c(aoi: AOI, avoided, arr) -> tuple[dict, dict]:
        if not carbon_project:
            return _na_pair(_DEACTIVATED)
        gross_er = _gross_of(avoided) or 0.0
        gross_removal = _gross_of(arr) or 0.0
        if gross_er + gross_removal <= 0:
            return _na_pair("Neither 5.2 nor 5.3 is applicable on this site, so there are no "
                            "gross ERRs to deduct from.")
        return net_errs(gross_er, gross_removal, leakage, uncertainty, buffer, duration_years)

    def _habitat(aoi: AOI) -> tuple[dict, dict]:
        return analyze_habitat_loss_avoided(aoi, duration_years, rate_pct, ecosystem_class)

    def _threatened(aoi: AOI) -> tuple[dict, dict]:
        return analyze_threatened_species_habitat(aoi, duration_years, rate_pct,
                                                  ecosystem_class)

    def _uplift(aoi: AOI) -> tuple[dict, dict]:
        return analyze_biodiversity_uplift(aoi, duration_years)

    def _resilience(aoi: AOI) -> tuple[dict, dict]:
        return analyze_climate_resilience(aoi, duration_years)

    def _microclimate(aoi: AOI) -> tuple[dict, dict]:
        return analyze_microclimate(aoi, duration_years, ecosystem_class)

    return {
        'assumptions': (_assumptions, ()),
        'activity_stage': (_activity_stage, ()),
        'general_benefit': (_general, ('activity_stage',)),
        'avoided_emissions': (_avoided, ('activity_stage',)),
        'arr_sequestration': (_arr, ()),
        'net_emission_reduction': (_net_er, ('avoided_emissions',)),
        'net_carbon_removal': (_net_removal, ('arr_sequestration',)),
        'net_errs': (_net_errs_c, ('avoided_emissions', 'arr_sequestration')),
        'habitat_loss_avoided': (_habitat, ()),
        'threatened_species_habitat': (_threatened, ()),
        'biodiversity_uplift': (_uplift, ()),
        'climate_resilience': (_resilience, ()),
        'microclimate': (_microclimate, ()),
    }


def _order(components: dict) -> list[str]:
    """Cheapest-expected-finish first, dependencies always ahead of dependents -- the
    run_site_characterisation derivation."""
    def finishes_at(name: str) -> float:
        _, deps = components[name]
        return _W[name] + max((finishes_at(d) for d in deps), default=0.0)
    return sorted(components, key=finishes_at)


# 5.9 and 5.10 each run their own 16-worker habitat pool; this outer pool only has to let the
# independent stages overlap, so wall time is set by the slowest component (~27 s), not the sum.
_MAX_WORKERS = 6

# The one never-emitted internal step, mirroring site characterisation's run-but-not-re-emitted
# dependencies.
_HIDDEN = {'activity_stage'}


def validate(duration_years: int, leakage: float, uncertainty: float, buffer: float) -> None:
    """The GUI's per-field ranges plus the notebook's 4.4 sum rule, shared by both forms."""
    for name, value in (("leakage", leakage), ("uncertainty", uncertainty), ("buffer", buffer)):
        if not 1 <= value <= 100:
            raise ValueError(f"{name} must be between 1 and 100 percent.")
    if leakage + uncertainty + buffer > 100:
        raise ValueError("Total deductions cannot exceed 100%.")


def plan_for(components: dict, wanted: list[str] | None) -> list[dict]:
    emitted = [n for n in _order(components) if n not in _HIDDEN
               and (wanted is None or n in wanted)]
    return ([{'name': 'preparation', 'w': 0.1}]
            + [{'name': n, 'w': _W[n]} for n in emitted]
            + [{'name': 'end', 'w': 0.1}])


def stream_benefit(aoi: AOI, duration_years: int = INTERVENTION_DURATION_DEFAULT_YEARS,
                   rate_pct: float | None = None, carbon_project: bool = True,
                   leakage: float | None = None, uncertainty: float | None = None,
                   buffer: float | None = None, ecosystem_class: int = ECOSYSTEM_CLASS,
                   selections=None, wanted: list[str] | None = None, people_benefits=None):
    """The F02-P5 components as NDJSON lines, one per component, cheapest first."""
    if leakage is None:
        leakage = CARBON_RISK_DEFAULTS["leakage_percentage"]
    if uncertainty is None:
        uncertainty = CARBON_RISK_DEFAULTS["uncertainty_percentage"]
    if buffer is None:
        buffer = CARBON_RISK_DEFAULTS["buffer_percentage"]
    validate(duration_years, leakage, uncertainty, buffer)

    components = _components(duration_years, rate_pct, carbon_project,
                             leakage, uncertainty, buffer, ecosystem_class, selections,
                             people_benefits)
    order = _order(components)
    emitted = [n for n in order if n not in _HIDDEN and (wanted is None or n in wanted)]

    # Dependencies of the emitted set, closed transitively -- run, not emitted.
    running: set[str] = set()
    stack = list(emitted)
    while stack:
        name = stack.pop()
        if name in running:
            continue
        running.add(name)
        stack.extend(components[name][1])
    run_order = [n for n in order if n in running]

    def views(pool_names=run_order):
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(max_workers=min(len(pool_names), _MAX_WORKERS),
                                thread_name_prefix='benefit') as pool:
            futures: dict[str, object] = {}
            for name in pool_names:
                fn, deps = components[name]
                futures[name] = (
                    pool.submit(after, name, fn, aoi, *(futures[d] for d in deps)) if deps
                    else pool.submit(safe, name, fn, aoi)
                )
            for name in emitted:
                results, view = futures[name].result()
                yield view, error_status(results)

    return stream(plan_for(components, wanted), views())


if __name__ == "__main__":
    # Run on a file and print the response, no Flask app:
    #     python run_benefit.py [aoi path] [duration] [rate_pct]
    import json
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ..common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    rate = float(sys.argv[3]) if len(sys.argv) > 3 else None
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {aoi.area_ha:,.0f} ha over {duration} years, rate={rate}\n")
    t0 = time.perf_counter()
    result = run_benefit(aoi, duration, rate)
    print(json.dumps(result, indent=2, ensure_ascii=False, default=str)[:6000])
    print(f"\n[{time.perf_counter() - t0:.1f}s]", file=sys.stderr)
