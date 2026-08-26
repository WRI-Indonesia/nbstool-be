"""
run_analysis.py - the WHOLE analysis as ONE stream: F02-P2 site characterisation, F02-P3 threat
and F02-P4 pathway in a single NDJSON response.

THE FRONTEND FLOW IS: upload polygon, then call THIS. The individual endpoints
(/feature/site-characterisation, /feature/threat, /feature/pathway) remain for tooling and for
anything that wants one stage alone, but the product screens read this union stream and pick the
lines they need as they arrive.

MERGING IS COMPOSITION, NOT NEW MACHINERY. Site characterisation already merges four modules
into one plan; this module does the same one level up:

  - the 25 site-characterisation components come in with their dependency graph intact
    (`run_site_characterisation.COMPONENTS`);
  - the 4 threat sections are dependency-free components (`run_threat.SECTIONS`), guards and
    zero-shapes included;
  - pathway is ONE component named `pathway`, whose `data` is the same PathwaySelection document
    its own endpoint returns. Its internal 4.1 -> 4.2 -> 4.3 sequence is one function call here.

Component names are unique across the three stages (asserted at import), so `process` still
identifies a card, `?process=` retries work across the whole union, and the client's merge rule
is unchanged per stage: site-characterisation lines flat-merge (People one level deeper), threat
lines stay per-tab (their fields collide by design), `pathway` is one document.

Emission is CHEAPEST FIRST across all three stages, same rationale and same derivation as
run_site_characterisation: a finished card cannot be written until every card before it has been,
so ordering by expected finish minimises how long done work waits. Threat's weights are the
provisional ones from run_threat; pathway is ~3.5 s measured.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from .common import AOI
    from .pipeline import after, error_status, safe, stream
    from .pathway.run_pathway import run_pathway
    from .site_characterisation.run_site_characterisation import (
        COMPONENTS as _SITECHAR_COMPONENTS,
        COST as _SITECHAR_COST,
    )
    from .threat.run_threat import SECTIONS as _THREAT_SECTIONS
    from .threat.run_threat import processes as _threat_processes
except ImportError:  # `python run_analysis.py`: no package around it
    import pathlib
    import sys

    _here = pathlib.Path(__file__).resolve()
    sys.path.insert(0, str(_here.parent))
    for _sub in ("site_characterisation", "threat", "pathway"):
        sys.path.insert(0, str(_here.parent / _sub))
    for _sub in ("general", "nature", "climate", "people"):
        sys.path.insert(0, str(_here.parent / "site_characterisation" / _sub))
    from common import AOI
    from pipeline import after, error_status, safe, stream
    from run_pathway import run_pathway
    from run_site_characterisation import (
        COMPONENTS as _SITECHAR_COMPONENTS,
        COST as _SITECHAR_COST,
    )
    from run_threat import SECTIONS as _THREAT_SECTIONS
    from run_threat import processes as _threat_processes


def _pathway_component(aoi: AOI) -> tuple[dict, dict]:
    """F02-P4 as one union component. The view is the PathwaySelection document unchanged, so a
    client reads this line exactly as it reads the standalone endpoint's `result`. The document
    carries its own `messages`; a crash is handled by `pipeline.safe` like any other card."""
    return {'narrative': "", 'tables': {}, 'values': {}, 'flags': [], 'missing': []}, \
        run_pathway(aoi)


_THREAT_COST = {p['name']: p['w'] for p in _threat_processes[1:-1]}

# name -> (fn, dependencies), all three stages. Site characterisation keeps its dependency
# graph; threat sections and pathway have none.
_UNORDERED: dict[str, tuple] = {
    **_SITECHAR_COMPONENTS,
    **{name: (fn, ()) for name, fn in _THREAT_SECTIONS.items()},
    'pathway': (_pathway_component, ()),
}

COST = {
    **_SITECHAR_COST,
    **_THREAT_COST,
    'pathway': 3.5,
}

_collisions = (set(_SITECHAR_COMPONENTS) & set(_THREAT_SECTIONS)) \
    | ({'pathway'} & (set(_SITECHAR_COMPONENTS) | set(_THREAT_SECTIONS)))
if _collisions:
    raise RuntimeError(f"process names collide across stages: {sorted(_collisions)}")


def _finishes_at(name: str) -> float:
    """Own cost plus the longest dependency chain, same as run_site_characterisation."""
    _, deps = _UNORDERED[name]
    return COST[name] + max((_finishes_at(d) for d in deps), default=0.0)


_ORDER = sorted(_UNORDERED, key=_finishes_at)

_available: set[str] = set()
for _name in _ORDER:
    _missing = [d for d in _UNORDERED[_name][1] if d not in _available]
    if _missing:
        raise RuntimeError(f"{_name} is ordered before {_missing}, which it depends on.")
    _available.add(_name)

COMPONENTS: dict[str, tuple] = {name: _UNORDERED[name] for name in _ORDER}

processes = (
    [{'name': 'preparation', 'w': 0.1}]
    + [{'name': name, 'w': COST[name]} for name in _ORDER]
    + [{'name': 'end', 'w': 0.1}]
)


def resolve(wanted: list[str] | None) -> list[str]:
    """The components that have to RUN for `wanted`, dependencies closed transitively."""
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
    """Bookends plus the components being EMITTED; pulled-in dependencies run un-emitted."""
    if wanted is None:
        return processes
    asked = set(wanted)
    return [processes[0]] + [p for p in processes[1:-1] if p['name'] in asked] + [processes[-1]]


# Same width as site characterisation, and for the same reasons (see its measurement note): the
# wall clock is set by the slowest components either way, and per-run memory decides how many
# runs an instance holds. The union adds threat's twelve rasters and pathway's four band reads
# to the same run, so if anything the pressure argument for a modest pool is stronger here.
_MAX_WORKERS = 6


def _run_components(aoi: AOI, wanted: list[str] | None = None):
    """Run the requested components and their dependencies, yielding `(view, error_status)` for
    each REQUESTED one in emission order. Submission is topologically ordered (asserted above),
    so a dependency always holds a worker before its dependent blocks on it."""
    running = resolve(wanted)
    emitted = [p['name'] for p in plan(wanted)[1:-1]]

    with ThreadPoolExecutor(max_workers=min(len(running), _MAX_WORKERS),
                            thread_name_prefix='analysis') as pool:
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


def stream_analysis(aoi: AOI, wanted: list[str] | None = None, retry_url=None):
    """NDJSON lines for one AOI: the plan, then one line per component across all three stages."""
    return stream(plan(wanted), _run_components(aoi, wanted), retry_url)


if __name__ == "__main__":
    # Run the whole union on a file and print the stream, no Flask app:
    #     python run_analysis.py [aoi path] [process ...]
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from .common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    wanted = sys.argv[2:] or None
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {aoi.area_ha:,.0f} ha, {len(COMPONENTS)} components\n", file=sys.stderr)
    t0 = time.perf_counter()
    for line in stream_analysis(aoi, wanted):
        sys.stdout.write(line)
        sys.stdout.flush()
    print(f"\n[{time.perf_counter() - t0:.1f}s]", file=sys.stderr)
