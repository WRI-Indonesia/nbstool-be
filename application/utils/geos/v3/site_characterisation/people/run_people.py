"""
run_people.py - run every F02-P2 People component and stream what the endpoint renders.

Same contract as run_general.py: each component returns `(results, view_results)`, the NDJSON
envelope and the per component failure rule come from v3/pipeline.py, and what lives here is the
`processes` list and the wiring.

Three components, two of them from the notebook:

| component                   | in the notebook                                    | ported |
|-----------------------------|----------------------------------------------------|--------|
| 6.1 People Demography       | `analyze_people_demography(aoi) -> ComponentResult` | yes    |
| 6.2 Vulnerability Assessment| `analyze_vulnerability_assessment(aoi)`             | yes    |
| 6.3 Social Statistics       | NOT IN THE NOTEBOOK                                 | new    |

6.3 has no notebook heading at all. The People notebook is explicit that it covers 6.1 and 6.2 and
that "Household estimation is excluded", so everything the contract asks for outside population
and vulnerability -- employment, education, economy, health, housing -- is filled from the `se_v3`
tables against the specification in the sample queries. See social_statistics.py.

THE PAYLOAD IS NESTED, unlike the other three modules. Every People component emits
`{section: {...}}` because the contract groups these fields into cards, and three components all
contribute to `social_demography`: 6.1 the population counts, 6.2 the four vulnerability levels,
6.3 the household count. A client merging the three lines section by section gets the contract
back.

The one dependency. 6.3 takes `admin_values`, the `results['values']` of 1.2 in the GENERAL
module, because every social table is keyed by administrative NAME and those names come from the
GIS database, not this one. People can be run without General, so `stream_people` takes it as an
optional argument and passes it straight through; with nothing, 6.3 reports that no area was
resolved rather than guessing one.

    stream_people(aoi, admin_values=general_1_2_results['values'])

Ordering. 6.1 dominates: it opens the population rasters 41 times, once for the total and once per
band of each sex stack, and over /vsicurl that measured between 2 s and 114 s depending on how the
AOI window falls across the source tiling. 6.2 reads four small rasters and 6.3 is a handful of
indexed lookups, so both are effectively free beside it.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from ...common import AOI
    from ...pipeline import component_count, error_status, safe, stream
    from .people_demography import analyze_people_demography
    from .social_statistics import analyze_social_statistics
    from .vulnerability_assessment import analyze_vulnerability_assessment
except ImportError:  # `python run_people.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from common import AOI
    from people_demography import analyze_people_demography
    from pipeline import component_count, error_status, safe, stream
    from social_statistics import analyze_social_statistics
    from vulnerability_assessment import analyze_vulnerability_assessment

# One entry per step, in emission order. `w` is a relative cost. 6.1's weight is set by its 41
# raster opens and is the only one that matters; 6.2 reads four small rasters and 6.3 is database
# lookups on indexed foreign tables, measured at about 0.15 s for a whole country.
# `end` is emitted last, with empty data and a null `next`, so a client can tell a finished
# run from a dropped connection.
processes = [
    {'name': 'preparation', 'w': 0.1},
    {'name': 'people demography', 'w': 10.5},
    {'name': 'vulnerability assessment', 'w': 5.2},
    {'name': 'social statistics', 'w': 1.5},
    {'name': 'end', 'w': 0.1},
]


def _run_components(aoi: AOI, admin_values: dict | None = None):
    """Run 6.1 to 6.3 concurrently, yielding `(view_results, error_status)` in emission order.

    Nothing here waits on anything else: 6.3's dependency is already resolved by the caller, which
    is why this module needs no future-waiter of the kind run_general and run_climate carry.
    """
    workers = component_count(processes)
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix='people') as pool:
        futures = (
            pool.submit(safe, 'people demography', analyze_people_demography, aoi),
            pool.submit(safe, 'vulnerability assessment', analyze_vulnerability_assessment, aoi),
            pool.submit(safe, 'social statistics', analyze_social_statistics, aoi, admin_values),
        )

        for future in futures:
            results, view = future.result()
            yield view, error_status(results)


def stream_people(aoi: AOI, admin_values: dict | None = None):
    """NDJSON lines for one AOI: the plan, then one line per component.

    `admin_values` is 1.2's `results['values']`, used only by 6.3. See the module docstring;
    passing nothing is a supported state, not a fallback.
    """
    return stream(processes, _run_components(aoi, admin_values))


if __name__ == "__main__":
    # Run all three components on a file and print the stream the endpoint sends, no Flask app:
    #     python run_people.py [aoi path]
    # 1.2 is run first, because 6.3 cannot look anything up without the administrative names.
    import os
    import pathlib
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ...common import prepare_aoi

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "general"))
    from administrative_boundaries import analyze_admin_boundaries

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    admin_results, _ = analyze_admin_boundaries(aoi)

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    for line in stream_people(aoi, admin_results['values']):
        sys.stdout.write(line)
        sys.stdout.flush()
