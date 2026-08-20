"""
run_general.py - run every F02-P2 General component and stream what the endpoint renders.

Each component returns `(results, view_results)`. `results` is the full analytical output, kept
for narratives, tables and flags; `view_results` is the flat slice the frontend card reads. This
module runs the eight components on one AOI, concurrently, and emits each component's
`view_results` as a line of its own, so cards render as they land instead of after the last one.

The NDJSON envelope, the per component failure rule and the progress weights are v3/pipeline.py,
shared with the modules that follow this one. What lives here is what is specific to General: the
`processes` list, and the wiring between the eight components.

Ordering. The eight run together on a thread pool, but are emitted in the fixed 1.1 to 1.8 order
so the `next` field stays meaningful and no client has to change. One dependency is real: 1.6
compares the AOI against the forest of its dominant country, and that country comes from 1.2.

Weights. `w` is median seconds of a component's OWN work, profiled across three AOIs. Under
concurrency they still do not add up to elapsed time -- eight things run at once, so the sum is
the serial cost, not the wall clock -- and a bar driven by them advances unevenly. What they do
give is a true ranking and true relative magnitudes, which is what a bar and the emission order
both need.

Failure is per component, via `pipeline.safe`. It is the reason this endpoint cannot report a
partial failure through the HTTP status: by the time a component runs, 200 and the headers have
already been sent.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from ...common import AOI
    from ...pipeline import after, component_count, error_status, safe, stream
    from .administrative_boundaries import analyze_admin_boundaries
    from .deforestation_risk import analyze_deforestation_risk
    from .ecosystem_type import analyze_ecosystem_type
    from .historical_deforestation import analyze_historical_deforestation
    from .land_cover import analyze_land_cover
    from .natural_disaster_risk import analyze_natural_risk
    from .protected_areas_wdpa import analyze_protected_areas
    from .terrain_slope_elevation import analyze_terrain
except ImportError:  # `python run_general.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from administrative_boundaries import analyze_admin_boundaries
    from common import AOI
    from deforestation_risk import analyze_deforestation_risk
    from ecosystem_type import analyze_ecosystem_type
    from historical_deforestation import analyze_historical_deforestation
    from land_cover import analyze_land_cover
    from natural_disaster_risk import analyze_natural_risk
    from pipeline import after, component_count, error_status, safe, stream
    from protected_areas_wdpa import analyze_protected_areas
    from terrain_slope_elevation import analyze_terrain

# One entry per step, in emission order. `w` IS MEDIAN SECONDS, profiled per component across three
# AOIs (67,439 ha AOI1, 77,307 ha Thailand, 7,259 ha South Sumatra) with the wait on any dependency
# excluded, so it measures work rather than queueing. The first AOI of a process pays a large
# one-off GDAL and /vsicurl setup cost that belongs to no single component, which is why the median
# and not the mean is used. Absolute times move with the AOI and the network; the RANKING held on
# all three, and the ranking is what the emission order in run_site_characterisation is built from.
# `end` is emitted last, with empty data and a null `next`, so a client can tell a finished
# run from a dropped connection.
processes = [
    {'name': 'preparation', 'w': 0.1},
    {'name': 'ecosystem type', 'w': 2.9},
    {'name': 'administrative boundaries', 'w': 1.9},
    {'name': 'protected areas', 'w': 1.2},
    {'name': 'terrain', 'w': 3.7},
    {'name': 'historical deforestation', 'w': 3.5},
    {'name': 'deforestation risk', 'w': 2.2},
    {'name': 'natural disaster risks', 'w': 3.7},
    {'name': 'land cover', 'w': 3.7},
    {'name': 'end', 'w': 0.1},
]


def _risk_after_admin(aoi: AOI, admin: tuple[dict, dict]) -> tuple[dict, dict]:
    """1.6, given 1.2's `(results, view_results)`.

    The only ordering constraint in the module. `pipeline.after` does the waiting and the `safe`
    wrapping, and it waits ON A WORKER, which is why the pool is sized for every component at once:
    if 1.6 could take the last free thread, 1.2 might never get one and the two would wait on each
    other.

    Signature is `(aoi, *dependencies)`, uniform across every dependent component so that
    run_site_characterisation can hold them all in one table. `.get` chains rather than indexing
    because a failed 1.2 returns {}. A missing country is already a case 1.6 handles, reporting
    that no national comparison could be made.
    """
    admin_results, _ = admin
    country = admin_results.get('values', {}).get('dominant_country')
    return analyze_deforestation_risk(aoi, country)


def _run_components(aoi: AOI):
    """Run 1.1 to 1.8 concurrently, yielding `(view_results, error_status)` in emission order.

    The components are independent apart from 1.6, and their cost is almost entirely waiting:
    raster reads over /vsicurl and two PostGIS queries. rasterio releases the GIL inside GDAL and
    the database driver releases it on the socket, so threads genuinely overlap and total wall
    time falls to the slowest component rather than the sum of all eight.

    Threads, not processes, because the payload that would have to be pickled is the raster
    output, and because nothing here touches Flask: the components reach the databases through
    v3/db.py and v3/settings.py, which carry their own engines, and the AOI is built by the caller
    before any of this starts. A worker thread therefore needs no application context.

    Results are yielded in the fixed order of `processes`, not in completion order, so the NDJSON
    contract is unchanged and a client needs no update. The cost is that a component which
    finished early waits its turn to be written; the saving in total time is unaffected.
    """
    workers = component_count(processes)
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix='sitechar') as pool:
        # Submitted before the tuple below, because 1.6 is handed this future.
        admin = pool.submit(safe, 'administrative boundaries', analyze_admin_boundaries, aoi)

        futures = (
            pool.submit(safe, 'ecosystem type', analyze_ecosystem_type, aoi),
            admin,
            pool.submit(safe, 'protected areas', analyze_protected_areas, aoi),
            pool.submit(safe, 'terrain', analyze_terrain, aoi),
            pool.submit(safe, 'historical deforestation', analyze_historical_deforestation, aoi),
            pool.submit(after, 'deforestation risk', _risk_after_admin, aoi, admin),
            pool.submit(safe, 'natural disaster risks', analyze_natural_risk, aoi),
            pool.submit(safe, 'land cover', analyze_land_cover, aoi),
        )

        for future in futures:
            results, view = future.result()
            yield view, error_status(results)


def stream_general(aoi: AOI):
    """NDJSON lines for one AOI: the plan, then one line per component."""
    return stream(processes, _run_components(aoi))


if __name__ == "__main__":
    # Run all eight components on a file and print the stream the endpoint sends, no Flask app:
    #     python run_general.py [aoi path]
    # The session path needs the app, so it is only reachable through the endpoint.
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ...common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    for line in stream_general(aoi):
        sys.stdout.write(line)
        sys.stdout.flush()
