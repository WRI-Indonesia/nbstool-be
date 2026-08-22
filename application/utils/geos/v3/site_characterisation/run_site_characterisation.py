"""
run_site_characterisation.py - the whole of F02-P2 as ONE stream.

General, Nature, Climate and People in a single NDJSON response: one `preparation` line carrying
the full plan, then one line per component across all four modules, then done. The per-module
runners (run_general, run_nature, run_climate, run_people) are kept for running a module on its
own, but the endpoint uses this.

ONE POOL, not four. Every component of every module is submitted to a single ThreadPoolExecutor
sized for all of them. Separate pools would each pay their own baseline and, worse, would make the
cross-module dependencies below impossible to express safely.

Four components wait on another's future, and the pool being sized for ALL components is what
makes that safe. If a waiter could take the last free thread, the component it waits on might
never get one and the two would deadlock:

    1.6 deforestation risk  <- 1.2 administrative boundaries   (needs the dominant country)
    3.2 soil organic carbon <- 1.1 ecosystem type              (needs the Axis 3 set, for peat)
    carbon shares           <- 3.1 and 3.2                     (needs both carbon totals)
    6.3 social statistics   <- 1.2 administrative boundaries   (needs the dominant admin names)

THE POINT OF MERGING, beyond one request instead of four, is those dependencies. Two of them only
exist because the modules run together:

  - 3.2's peat caveat. Run separately, the Climate endpoint has no way to know whether the AOI
    contains peatland, so 3.2 reports "unknown" and flags it. Here 1.1 is running anyway, so its
    result is handed over and the caveat is real. Peat can extend metres below the 30 cm the
    raster measures, so on a peat site the difference between "unknown" and "present" is the
    difference between a headline figure and a lower bound.
  - 6.3 in its entirety. Every social table is keyed by administrative name, and those names come
    from the GIS boundary layer that 1.2 reads. Without General there is nothing to look up.

Ordering. Components are emitted CHEAPEST FIRST, in the fixed order of `processes`, not in
completion order and NOT grouped by module. The order is derived from the profiled `w` of each
component rather than declared: see `_ORDER` below.

The reason is that a component which finishes early still cannot be written until every card
before it has been, so one slow component early in the list holds up everything behind it. The
total time is unaffected either way -- it is set by the slowest component -- but WHEN EACH CARD
APPEARS is not. Measured on one AOI: unordered, 292 card-seconds of finished cards waiting their
turn; ordered, 92. The `next` field stays meaningful and `preparation` states the order this run
will use, so a client that reads the plan rather than assuming module blocks needs no change.

TWO PAYLOAD SHAPES IN ONE STREAM. General, Nature and Climate emit flat objects that a client
merges into one card set. People emits `{section: {...}}`, because its contract groups fields into
named cards and three of its components all write into `social_demography`. Both are just `data`
on their own NDJSON line, so the envelope is unchanged; a client merges People's lines section by
section and the others field by field.

Failure stays per component, via `pipeline.safe`. One component failing costs its own card, not
the response. That matters more here than in a single module: 24 components in one stream is 24
chances to take down a request that has already returned 200. Each line carries its own
`error_status`, so a client can tell a card that crashed from one that ran and had nothing to say
-- both are `data: {}` otherwise.

PARTIAL RUNS. `stream_site_characterisation(aoi, wanted)` runs only the named components, which is
what the retry button on a failed card asks for. The dependency graph is the reason this needs the
`COMPONENTS` table below rather than a filter: retrying 6.3 has to re-run 1.2 first, or it fails
again for exactly the reason it failed the first time. Dependencies run but are not emitted, so a
retry never overwrites a card the client already holds.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from ..common import AOI
    from ..pipeline import after, error_status, safe, stream
    from .climate.annual_precipitation import analyze_annual_precipitation
    from .climate.annual_temperature import analyze_annual_temperature
    from .climate.current_carbon_storage import analyze_current_carbon_storage
    from .climate.fire_susceptibility import analyze_fire_susceptibility
    from .climate.historical_burned_area import analyze_historical_burned_area
    from .climate.run_climate import _carbon_shares
    from .climate.run_climate import processes as climate_processes
    from .climate.soil_classification import analyze_soil_classification
    from .climate.soil_organic_carbon import analyze_soil_organic_carbon
    from .general.administrative_boundaries import analyze_admin_boundaries
    from .general.ecosystem_type import analyze_ecosystem_type
    from .general.historical_deforestation import analyze_historical_deforestation
    from .general.indigenous_territory import analyze_indigenous_territory
    from .general.land_cover import analyze_land_cover
    from .general.natural_disaster_risk import analyze_natural_risk
    from .general.protected_areas_wdpa import analyze_protected_areas
    from .general.run_general import _risk_after_admin
    from .general.run_general import processes as general_processes
    from .general.terrain_slope_elevation import analyze_terrain
    from .nature.conservation_significance import analyze_conservation_significance
    from .nature.endangered_trees import analyze_endangered_trees
    from .nature.forest_landscape_integrity import analyze_flii
    from .nature.habitat_area import analyze_habitat_area
    from .nature.key_biodiversity_areas import analyze_kba
    from .nature.key_species_presence import analyze_key_species
    from .nature.run_nature import processes as nature_processes
    from .people.people_demography import analyze_people_demography
    from .people.run_people import processes as people_processes
    from .people.social_statistics import analyze_social_statistics
    from .people.vulnerability_assessment import analyze_vulnerability_assessment
except ImportError:  # `python run_site_characterisation.py`: no package around it
    import pathlib
    import sys

    _here = pathlib.Path(__file__).resolve()
    sys.path.insert(0, str(_here.parents[1]))
    for _sub in ("general", "nature", "climate", "people"):
        sys.path.insert(0, str(_here.parent / _sub))
    from administrative_boundaries import analyze_admin_boundaries
    from annual_precipitation import analyze_annual_precipitation
    from annual_temperature import analyze_annual_temperature
    from common import AOI
    from conservation_significance import analyze_conservation_significance
    from current_carbon_storage import analyze_current_carbon_storage
    from ecosystem_type import analyze_ecosystem_type
    from endangered_trees import analyze_endangered_trees
    from fire_susceptibility import analyze_fire_susceptibility
    from forest_landscape_integrity import analyze_flii
    from habitat_area import analyze_habitat_area
    from historical_burned_area import analyze_historical_burned_area
    from historical_deforestation import analyze_historical_deforestation
    from indigenous_territory import analyze_indigenous_territory
    from key_biodiversity_areas import analyze_kba
    from key_species_presence import analyze_key_species
    from land_cover import analyze_land_cover
    from natural_disaster_risk import analyze_natural_risk
    from people_demography import analyze_people_demography
    from pipeline import after, error_status, safe, stream
    from protected_areas_wdpa import analyze_protected_areas
    from run_climate import _carbon_shares
    from run_climate import processes as climate_processes
    from run_general import _risk_after_admin
    from run_general import processes as general_processes
    from run_nature import processes as nature_processes
    from run_people import processes as people_processes
    from social_statistics import analyze_social_statistics
    from soil_classification import analyze_soil_classification
    from soil_organic_carbon import analyze_soil_organic_carbon
    from terrain_slope_elevation import analyze_terrain
    from vulnerability_assessment import analyze_vulnerability_assessment


def _steps(module_processes: list[dict]) -> list[dict]:
    """A module's real components: everything between its own `preparation` and `end`."""
    return module_processes[1:-1]


# Costs, taken from the module lists rather than retyped, so a component added to a module appears
# here too and the two can never drift. `w` is median seconds of that component's own work; see any
# module's `processes` for how it was profiled. Step names are unique across the four modules,
# which the NDJSON contract needs: `process` identifies the card.
COST = {p['name']: p['w']
        for p in (_steps(general_processes) + _steps(nature_processes)
                  + _steps(climate_processes) + _steps(people_processes))}


def _soc_after_ecosystem(aoi: AOI, ecosystem: tuple[dict, dict]) -> tuple[dict, dict]:
    """3.2, given 1.1's `(results, view_results)`, for the peat caveat.

    `.get` chains rather than indexing because a failed 1.1 returns {}. A missing set is already a
    case 3.2 handles: it reports the peat state as unknown and flags it, which is exactly what the
    standalone Climate endpoint does.
    """
    ecosystem_results, _ = ecosystem
    return analyze_soil_organic_carbon(aoi, ecosystem_results.get('values', {}).get('present_set'))


def _social_after_admin(aoi: AOI, admin: tuple[dict, dict]) -> tuple[dict, dict]:
    """6.3, given 1.2's `(results, view_results)` -- the names every social table is keyed by.

    The second dependent on 1.2, alongside 1.6, and the reason People cannot be a standalone stage
    in practice: without a country and a province there is nothing to look up. A failed 1.2 returns
    {}, which 6.3 handles by reporting that no area was resolved rather than by guessing one.
    """
    admin_results, _ = admin
    return analyze_social_statistics(aoi, admin_results.get('values', {}))


# name -> (function, the components whose output it needs)
#
# THE ONE PLACE THE DEPENDENCY GRAPH IS WRITTEN DOWN, and what makes a partial run possible: a
# retry of one card has to bring its dependencies with it or it reproduces the failure it was
# meant to fix -- 6.3 without 1.2 reports "no area resolved" every time.
#
# Every function takes `(aoi, *dependency_results)`, each dependency being the whole
# `(results, view_results)` tuple its component returned. `pipeline.after` waits on the futures and
# wraps the call in `safe`; the four adapters above and in run_general/run_climate exist only to
# pull the one value each dependent needs out of its dependency's `results`.
#
# Written in module order for reading. THE ORDER BELOW IS NOT THE ORDER ANYTHING RUNS OR IS EMITTED
# IN -- both come from `_ORDER`, sorted by cost. Nothing here needs to be kept in any particular
# sequence.
_UNORDERED: dict[str, tuple] = {
    # General, 1.1 to 1.8
    'ecosystem type':             (analyze_ecosystem_type, ()),
    'administrative boundaries':  (analyze_admin_boundaries, ()),
    'protected areas':            (analyze_protected_areas, ()),
    'terrain':                    (analyze_terrain, ()),
    'historical deforestation':   (analyze_historical_deforestation, ()),
    'deforestation risk':         (_risk_after_admin, ('administrative boundaries',)),
    'natural disaster risks':     (analyze_natural_risk, ()),
    'land cover':                 (analyze_land_cover, ()),
    'indigenous territory':       (analyze_indigenous_territory, ()),
    # Nature, 2.1 / 2.2 / 2.3 / 2.5 / 2.6 plus endangered trees
    'forest landscape integrity': (analyze_flii, ()),
    'key biodiversity areas':     (analyze_kba, ()),
    'habitat area':               (analyze_habitat_area, ()),
    'key species presence':       (analyze_key_species, ()),
    'conservation significance':  (analyze_conservation_significance, ()),
    'endangered trees':           (analyze_endangered_trees, ()),
    # Climate, 3.1 to 3.6 plus the carbon shares step
    'current carbon storage':     (analyze_current_carbon_storage, ()),
    'soil organic carbon':        (_soc_after_ecosystem, ('ecosystem type',)),
    'carbon shares':              (_carbon_shares, ('current carbon storage',
                                                    'soil organic carbon')),
    'annual temperature':         (analyze_annual_temperature, ()),
    'annual precipitation':       (analyze_annual_precipitation, ()),
    'fire susceptibility':        (analyze_fire_susceptibility, ()),
    'burned area':                (analyze_historical_burned_area, ()),
    'soil classification':        (analyze_soil_classification, ()),
    # People, 6.1 to 6.3
    'people demography':          (analyze_people_demography, ()),
    'vulnerability assessment':   (analyze_vulnerability_assessment, ()),
    'social statistics':          (_social_after_admin, ('administrative boundaries',)),
}


def _finishes_at(name: str) -> float:
    """Estimated seconds until this component's result exists: its own cost plus the longest
    chain of dependencies it has to wait through.

    Not the same as `COST[name]`. `carbon shares` is arithmetic and costs 0.1 s, but it cannot
    produce anything until 3.1 and 3.2 have, so it finishes around the 5 s mark and belongs there.
    """
    _, deps = _UNORDERED[name]
    return COST[name] + max((_finishes_at(d) for d in deps), default=0.0)


# CHEAPEST FIRST. Emission follows this order, not completion order, so a card that finished early
# still cannot be written until every card before it has been -- one slow component early in the
# list holds up every finished card behind it. Sorting by when each component is expected to finish
# is what makes each line land as close as possible to the moment its data exists. Measured: 2.3 in
# its natural Nature slot cost 292 card-seconds of avoidable waiting on one AOI; ordered, 92.
#
# THIS ALSO SATISFIES THE SUBMISSION CONSTRAINT for free. A component must be submitted after the
# ones it depends on, so their futures exist when `after` is handed them -- and since every cost is
# positive, a dependent's `_finishes_at` is always greater than its dependency's, so sorting can
# never put one first. The loop below asserts that rather than trusting it.
#
# Ties keep module order, because `sorted` is stable and `_UNORDERED` is written in module order.
_ORDER = sorted(_UNORDERED, key=_finishes_at)

_available: set[str] = set()
for _name in _ORDER:
    _missing = [d for d in _UNORDERED[_name][1] if d not in _available]
    if _missing:
        raise RuntimeError(f"{_name} is ordered before {_missing}, which it depends on; its future "
                           "would not exist yet at submission.")
    _available.add(_name)

# name -> (function, dependencies), in the order everything is submitted AND emitted.
COMPONENTS: dict[str, tuple] = {name: _UNORDERED[name] for name in _ORDER}

# `preparation` first with the plan, `end` last with empty data and a null `next`, so a client can
# tell a finished run from a dropped connection.
processes = (
    [{'name': 'preparation', 'w': 0.1}]
    + [{'name': name, 'w': COST[name]} for name in _ORDER]
    + [{'name': 'end', 'w': 0.1}]
)


def resolve(wanted: list[str] | None) -> list[str]:
    """The components that have to RUN for `wanted` to be answerable, in submission order.

    `None` means all of them. Anything else is the requested set plus its dependencies, closed
    transitively, so asking for `carbon shares` alone still runs 3.1, 3.2 and 1.1 behind it.
    """
    if wanted is None:
        return list(COMPONENTS)

    needed: set[str] = set()
    stack = list(wanted)
    while stack:
        name = stack.pop()
        if name in needed:
            continue
        needed.add(name)
        stack.extend(COMPONENTS[name][1])
    return [name for name in COMPONENTS if name in needed]


def plan(wanted: list[str] | None) -> list[dict]:
    """The `processes` list for one run: the bookends, plus the components being EMITTED.

    A dependency pulled in by `resolve` is not in here. It runs, but its card is not re-sent: a
    retry of 6.3 should not silently overwrite the ADMINISTRATIVE BOUNDARIES card the client
    already has, and the plan a client is handed has to be exactly what it will receive.
    """
    if wanted is None:
        return processes
    asked = set(wanted)
    return [processes[0]] + [p for p in processes[1:-1] if p['name'] in asked] + [processes[-1]]


# WIDER THAN THIS BUYS NOTHING AND COSTS A LOT. Habitat area alone takes ~21.9 s; the other 24
# components sum to ~69 s, which six workers clear behind it, so the wall clock floor is the same at
# 6 as at 25. Measured over six runs each, same AOI, back to back:
#
#     workers   wall median   wall range      added RSS   GIS connections
#     1         37.4 s        (one sample)    130 MB      1
#     6         15.6 s        15.1 - 17.5     180 MB      3
#     25        23.3 s        13.0 - 31.6     323 MB      4
#
# 25 has the better BEST case and a worst case twice as bad: it sits on a contention cliff between
# the network, the GIL and GDAL's caches and falls off it about half the time. Six is 33% faster on
# the median with the spread collapsed from 18.6 s to 2.4 s, on 44% less memory -- and per-run
# memory is what decides how many runs an instance can hold.
_MAX_WORKERS = 6


def _run_components(aoi: AOI, wanted: list[str] | None = None):
    """Run the requested components and their dependencies concurrently, yielding
    `(view_results, error_status)` for each REQUESTED one in emission order.

    THE POOL DOES NOT HAVE TO FIT EVERY COMPONENT, and an earlier version of this docstring claimed
    it did. The worry was real but misdiagnosed: four components park a worker on another's future
    through `pipeline.after`, so a waiter holding the last thread could in principle deadlock the
    pair. It cannot happen HERE, because submission is topologically ordered -- asserted at import,
    see `_ORDER` -- and `ThreadPoolExecutor`'s queue is strictly FIFO. A dependency is therefore
    always dequeued before its dependent, so it already holds a worker by the time the dependent
    blocks on its result. That holds at any width down to one, and was checked: six runs at width 6
    and one at width 1, all producing identical output with no hang.

    `min` keeps a retry cheap: asking for one component still spawns one thread, not six.
    """
    running = resolve(wanted)
    emitted = [p['name'] for p in plan(wanted)[1:-1]]

    with ThreadPoolExecutor(max_workers=min(len(running), _MAX_WORKERS),
                            thread_name_prefix='sitechar') as pool:
        futures: dict[str, object] = {}
        for name in running:
            fn, deps = COMPONENTS[name]
            futures[name] = (
                pool.submit(after, name, fn, aoi, *(futures[d] for d in deps)) if deps
                else pool.submit(safe, name, fn, aoi)
            )

        for name in emitted:
            results, view = futures[name].result()
            yield view, error_status(results)


def stream_site_characterisation(aoi: AOI, wanted: list[str] | None = None, retry_url=None):
    """NDJSON lines for one AOI: the plan, then one line per component.

    `wanted` is None for a full run and a list of component names for a retry. The stream is the
    same shape either way -- `preparation` carrying the plan for THIS run, one line per component,
    then `end` -- so a client parses and merges a retry with the code it already has, and `w`/`a`
    give the retry its own progress bar.

    `retry_url` is an optional `name -> url` function; the route supplies one, a script does not.
    """
    return stream(plan(wanted), _run_components(aoi, wanted), retry_url)


if __name__ == "__main__":
    # Run everything on a file and print the stream the endpoint sends, no Flask app:
    #     python run_site_characterisation.py [aoi path]
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

    aoi = prepare_aoi(gpd.read_file(aoi_path))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    t0 = time.perf_counter()
    for line in stream_site_characterisation(aoi):
        sys.stdout.write(line)
        sys.stdout.flush()
    print(f"\n[{time.perf_counter() - t0:.1f}s]", file=sys.stderr)
