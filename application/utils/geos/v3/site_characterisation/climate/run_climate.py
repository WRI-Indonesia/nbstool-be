"""
run_climate.py - run every F02-P2 Climate component and stream what the endpoint renders.

Same contract as run_general.py: each component returns `(results, view_results)`, the NDJSON
envelope and the per component failure rule come from v3/pipeline.py, and what lives here is the
`processes` list and the wiring. All six of the notebook's Climate components are real
`analyze_*(aoi)` functions, so all six are ported.

An eighth step, `burned area`, has no notebook section at all. It fills the contract's two
burned-area fields from the V2 backend's logic and V2's own layers under `assets-geo/baseline/`,
on the team's instruction to use the v2 version. See burned_area.py, and note that its total and
its annual series deliberately do not add up.

The one cross-module dependency. 3.2 takes `ecosystem_present`, the Axis 3 set produced by 1.1 in
the GENERAL stage, and uses it to decide whether to add the peat caveat. Climate can be run
without General, so `stream_climate` takes it as an optional argument and passes it straight
through. Three states, not two: peat present, no peat, and NOT KNOWN, which 3.2 flags rather than
silently treating as "no peat". Call it as

    stream_climate(aoi, ecosystem_present=general_1_1_results['values']['ecosystem_present'])

when the General stage has already run for this AOI, and with nothing when it has not.

Two notes on the endpoint payload. `total_carbon_storage` and the three `*_percentage` fields all
divide by SOIL + AGB + BGB and are all emitted by the `carbon shares` step, not by 3.1: it has no
soil, and its own biomass-only total stays in `results` where the notebook put it -- see
`_carbon_shares` below. And 3.5 Fire
Susceptibility has no field in the Climate contract at all -- the contract's fire fields are
burned-area history, a different measurement -- so its payload is currently unconsumed.

3.5 also reads a four-class stand-in for the notebook's five-class fire layer, see
fire_susceptibility.py. It fails cleanly through `pipeline.safe` rather than taking the response
down, as every component does.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

try:
    from ...common import AOI
    from ...pipeline import after, component_count, error_status, safe, stream
    from .annual_precipitation import analyze_annual_precipitation
    from .annual_temperature import analyze_annual_temperature
    from .burned_area import analyze_burned_area
    from .current_carbon_storage import analyze_current_carbon_storage
    from .fire_susceptibility import analyze_fire_susceptibility
    from .soil_classification import analyze_soil_classification
    from .soil_organic_carbon import analyze_soil_organic_carbon
except ImportError:  # `python run_climate.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from annual_precipitation import analyze_annual_precipitation
    from annual_temperature import analyze_annual_temperature
    from burned_area import analyze_burned_area
    from common import AOI
    from current_carbon_storage import analyze_current_carbon_storage
    from fire_susceptibility import analyze_fire_susceptibility
    from pipeline import after, component_count, error_status, safe, stream
    from soil_classification import analyze_soil_classification
    from soil_organic_carbon import analyze_soil_organic_carbon

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
    {'name': 'current carbon storage', 'w': 1.6},
    {'name': 'soil organic carbon', 'w': 2.1},
    {'name': 'carbon shares', 'w': 0.1},
    {'name': 'annual temperature', 'w': 3.6},
    {'name': 'annual precipitation', 'w': 3.4},
    {'name': 'fire susceptibility', 'w': 1.1},
    {'name': 'burned area', 'w': 6.7},
    {'name': 'soil classification', 'w': 1.6},
    {'name': 'end', 'w': 0.1},
]


CARBON_SHARE_FIELDS = (
    ('above_ground_biomass', 'above_ground_biomass_number'),
    ('below_ground_biomass', 'below_ground_biomass_number'),
    ('soil_organic_carbon', 'soil_organic_carbon_number'),
)


def _carbon_shares(aoi: AOI, carbon: tuple[dict, dict],
                   soc: tuple[dict, dict]) -> tuple[dict, dict]:
    """The endpoint's carbon TOTAL and three percentages, given 3.1's and 3.2's
    `(results, view_results)`.

    ONE BASE FOR ALL FOUR FIELDS: SOIL + AGB + BGB. The three shares sum to 100, soil has one at
    all, and `total_carbon_storage` x `above_ground_biomass_percentage` gives back
    `above_ground_biomass_number`. On AOI1 that total is 81.2M tCO2e.

    It used to be two bases. `total_carbon_storage` was 3.1's biomass-only 27.3M while the
    percentages already divided by 81.2M, so multiplying one by the other gave a number that
    appeared nowhere. Changed on your instruction 2026-08-10; nothing about 3.1's own figures
    moved, only which of them the endpoint publishes.

    Neither component may compute this alone: 3.1 owns the biomass pools, 3.2 owns soil, and
    widening either one's denominator would change the notebook figures they each report -- 3.1's
    narrative still quotes its biomass-only total and still says soil is excluded, because that
    sentence is about `results`, not about the card. The waiting is the same pattern 1.6 uses on
    1.2 in run_general -- parking a worker on `.result()` is safe only because the pool is sized
    for every component of one request.

    A failed 3.1 or 3.2 arrives as `{}` from `pipeline.safe`, so a missing pool drops out of the
    denominator rather than counting as zero, and its own share is reported as None. THAT MAKES A
    PARTIAL TOTAL LOOK LIKE A WHOLE ONE -- lose soil and `total_carbon_storage` silently becomes
    the old biomass-only number under the new name -- so the flag below names the field.

    `aoi` is unused. The signature is `(aoi, *dependencies)` for every dependent component so that
    run_site_characterisation can hold them all in one table; `pipeline.after` does the waiting.
    """
    _, carbon_view = carbon
    _, soc_view = soc
    merged = {**carbon_view, **soc_view}

    values = {pool: merged.get(field) for pool, field in CARBON_SHARE_FIELDS}
    present = [v for v in values.values() if v is not None]
    total = sum(present)

    view_results = {
        f'{pool}_percentage': (
            (value / total * 100.0) if (value is not None and total > 0) else None
        )
        for pool, value in values.items()
    }
    # None rather than 0 when no pool reported at all: 0 tCO2e is a finding about the site, and
    # "we could not read any of the three layers" is not one.
    view_results['total_carbon_storage'] = total if present else None

    flags: list[str] = []
    missing = [pool for pool, value in values.items() if value is None]
    if missing:
        flags.append(
            f"carbon shares: {', '.join(missing)} did not report, so `total_carbon_storage` and "
            "the percentages divide by a partial total and do not describe the whole carbon stock."
        )

    # No `retryable` here on purpose. A pool can be missing because 3.1 or 3.2 CRASHED, which a
    # retry may fix, or because the AOI genuinely has no soil carbon, which it never will --
    # and only `pipeline.after` can tell those apart, because only it sees how the dependency
    # ended. It sets `retryable` for the first case.
    results = {'narrative': "", 'tables': {},
               'values': dict(values, share_denominator=total), 'flags': flags}
    return results, view_results


def _run_components(aoi: AOI, ecosystem_present: set[int] | None = None):
    """Run 3.1 to 3.6 concurrently, yielding `(view_results, error_status)` in emission order.

    The six analysis components are independent; `carbon shares` is not, and parks a worker on
    3.1's and 3.2's futures. That is why the pool is sized for every step at once: if it could
    take the last free thread, 3.1 or 3.2 might never get one and the three would wait on each
    other. Same constraint as 1.6 on 1.2 in run_general.
    """
    workers = component_count(processes)
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix='climate') as pool:
        # Submitted before the tuple below, because `carbon shares` is handed both futures.
        carbon = pool.submit(safe, 'current carbon storage', analyze_current_carbon_storage, aoi)
        soc = pool.submit(safe, 'soil organic carbon', analyze_soil_organic_carbon, aoi,
                          ecosystem_present)

        futures = (
            carbon,
            soc,
            pool.submit(after, 'carbon shares', _carbon_shares, aoi, carbon, soc),
            pool.submit(safe, 'annual temperature', analyze_annual_temperature, aoi),
            pool.submit(safe, 'annual precipitation', analyze_annual_precipitation, aoi),
            pool.submit(safe, 'fire susceptibility', analyze_fire_susceptibility, aoi),
            pool.submit(safe, 'burned area', analyze_burned_area, aoi),
            pool.submit(safe, 'soil classification', analyze_soil_classification, aoi),
        )

        for future in futures:
            results, view = future.result()
            yield view, error_status(results)


def stream_climate(aoi: AOI, ecosystem_present: set[int] | None = None):
    """NDJSON lines for one AOI: the plan, then one line per component.

    `ecosystem_present` is 1.1's Axis 3 set, used only by 3.2's peat caveat. See the module
    docstring; passing nothing is a supported state, not a fallback.
    """
    return stream(processes, _run_components(aoi, ecosystem_present))


if __name__ == "__main__":
    # Run all six components on a file and print the stream the endpoint sends, no Flask app:
    #     python run_climate.py [aoi path]
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
    for line in stream_climate(aoi):
        sys.stdout.write(line)
        sys.stdout.flush()
