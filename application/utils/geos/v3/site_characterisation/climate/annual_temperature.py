"""
Component 3.3 Annual Temperature.

Monthly temperature profile as a twelve bar chart, plus the annual mean across the AOI.

Data. `temperature_v3.tif`, ONE 12-band raster, band m = month m, monthly mean temperature in
degrees Celsius. Read by climate_stack.read_monthly_stack, which is shared with 3.4.

VERIFY THE SOURCE. WORLDCLIM_VERSION / PERIOD / RESOLUTION in config are the notebook's own
placeholders ("verify"), and they are written into every result this component produces. A wrong
label mislabels the output, so they need confirming before this ships.

Decisions locked.
- Annual mean is taken PER PIXEL across the twelve months first, then summarised spatially. The
  other order (spatial mean per month, then average the months) gives the same mean but no
  meaningful min and max.
- A pixel counts only when all twelve months are present. See climate_stack.
- Below CLIMATE_MIN_PIXELS cells, or when every cell is identical, no spatial range is reported
  and a flag says why. At 30 arc-seconds a screening AOI gets only a handful of cells, and a
  "range" over three of them is noise.
"""

from __future__ import annotations

try:
    from ...common import AOI, not_applicable
    from ...config import (
        WORLDCLIM_PERIOD,
        WORLDCLIM_RESOLUTION,
        WORLDCLIM_TAVG_RASTER,
        WORLDCLIM_VERSION,
    )
    from .climate_stack import monthly_view, read_monthly_stack
except ImportError:  # `python annual_temperature.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from climate_stack import monthly_view, read_monthly_stack
    from common import AOI, not_applicable
    from config import (
        WORLDCLIM_PERIOD,
        WORLDCLIM_RESOLUTION,
        WORLDCLIM_TAVG_RASTER,
        WORLDCLIM_VERSION,
    )


def analyze_annual_temperature(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.3. Monthly temperature profile and the annual mean across the AOI."""
    stack = read_monthly_stack(WORLDCLIM_TAVG_RASTER, aoi, annual="mean")

    if stack is None:
        empty = not_applicable(
            "3.3 Annual Temperature",
            "No temperature data is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'monthly_temperature': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'historical_temperatures': [], 'temperature_avg_number': None,
                        'temperature_min_number': None, 'temperature_max_number': None}
        return results, view_results

    mean_c = float(stack.annual.mean())
    min_c = float(stack.annual.min())
    max_c = float(stack.annual.max())

    flags: list[str] = []
    if stack.has_range:
        narrative = (
            f"Annual mean temperature in the selected area ranges from {min_c:.1f} to "
            f"{max_c:.1f} degree Celsius with an average of {mean_c:.1f} degree Celsius."
        )
    else:
        # Too few cells, or every cell identical. Reporting a range here would be noise.
        narrative = (
            f"Annual mean temperature in the selected area is {mean_c:.1f} degree Celsius."
        )
        flags.append(
            f"3.3: only {stack.n_pixels} climate grid cells fall inside the AOI, so no spatial "
            "range is reported."
        )

    results = {
        'narrative': narrative,
        'tables': {'monthly_temperature': stack.monthly},  # twelve bars, one per month
        'values': {
            # Chart metadata travels with the series so the frontend does not hardcode units.
            'chart_series': "monthly_temperature",
            'chart_unit': "C",
            'chart_axis_label': "Temperature (C)",
            'annual_mean_c': mean_c,
            'annual_min_c': min_c,
            'annual_max_c': max_c,
            'n_pixels': stack.n_pixels,
            'source': f"{WORLDCLIM_VERSION} {WORLDCLIM_RESOLUTION}",
            'period': WORLDCLIM_PERIOD,
        },
        'flags': flags,
    }

    # min and max are SPATIAL, across the AOI's annual-mean pixels -- not the coldest and warmest
    # month. `historical_temperatures` carries the twelve monthly means separately. They are
    # emitted even when has_range is false (a one-cell AOI gives min == max == avg): the contract
    # types them as required numbers, and the flag above records that the range is not meaningful.
    view_results = {
        'temperature_min_number': min_c,
        'temperature_max_number': max_c,
        'temperature_avg_number': mean_c,
        'historical_temperatures': [
            {'temperature': m['value'], 'month': int(m['id'])} for m in monthly_view(stack)
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python annual_temperature.py [aoi path]
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_annual_temperature(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
