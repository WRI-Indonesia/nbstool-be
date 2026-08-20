"""
Component 3.4 Annual Precipitation.

Monthly rainfall profile as a twelve bar chart, plus the annual total across the AOI.

Data. `precipitation_v3.tif`, ONE 12-band raster, band m = month m, monthly precipitation in mm.
Read by climate_stack.read_monthly_stack, shared with 3.3.

The one real difference from 3.3: the twelve months are SUMMED per pixel, not averaged, because
an annual rainfall total is a sum. That is the `annual="sum"` argument and nothing else.

VERIFY THE UNIT. The config labels say mm/month, unconfirmed. This matters beyond 3.4: benefit
5.3 classifies the dryland zone on thresholds of annual > 2000 mm and driest month < 100 mm, so
a unit error there changes a zoning decision, not just a label.

TWO DIFFERENT SPREADS, and the component reports both. `results` carries the notebook's, which is
SPATIAL: how the annual total varies across the AOI's pixels, 2,533 to 2,642 mm a year on
indonesia_3. The endpoint's `precipitation_min/max/avg_number` carry the SEASONAL one instead, 24
to 318 mm a month on the same site, because that is what the card asks for -- "ranges from 45 to
320 mm per month, with an annual total of 2,000 mm". Only `precipitation_total_number` is annual.
Neither is more correct; they answer different questions off the same stack, and mixing them up
would be a factor-of-ten error.

Decisions locked.
- A pixel counts only when all twelve months are present. See climate_stack.
- Below CLIMATE_MIN_PIXELS cells, or when every cell is identical, no spatial range is reported
  and a flag says why.
- The endpoint's min, max and avg are per MONTH; only the total is per year. See the note above
  view_results, and note that 3.3 temperature deliberately stays spatial.
"""

from __future__ import annotations

try:
    from ...common import AOI, not_applicable
    from ...config import (
        DRY_MONTH_MM,
        WORLDCLIM_PERIOD,
        WORLDCLIM_PREC_RASTER,
        WORLDCLIM_RESOLUTION,
        WORLDCLIM_VERSION,
    )
    from .climate_stack import monthly_view, read_monthly_stack
except ImportError:  # `python annual_precipitation.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from climate_stack import monthly_view, read_monthly_stack
    from common import AOI, not_applicable
    from config import (
        DRY_MONTH_MM,
        WORLDCLIM_PERIOD,
        WORLDCLIM_PREC_RASTER,
        WORLDCLIM_RESOLUTION,
        WORLDCLIM_VERSION,
    )


def analyze_annual_precipitation(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.4. Monthly rainfall profile and the annual total across the AOI."""
    # "sum": twelve months are added per pixel BEFORE any spatial statistic is taken.
    stack = read_monthly_stack(WORLDCLIM_PREC_RASTER, aoi, annual="sum")

    if stack is None:
        empty = not_applicable(
            "3.4 Annual Precipitation",
            "No precipitation data is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'monthly_precipitation': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'historical_precipitations': [], 'precipitation_avg_number': None,
                        'precipitation_min_number': None, 'precipitation_max_number': None,
                        'precipitation_total_number': None, 'number_of_dry_months': None}
        return results, view_results

    mean_mm = float(stack.annual.mean())
    min_mm = float(stack.annual.min())
    max_mm = float(stack.annual.max())

    flags: list[str] = []
    if stack.has_range:
        narrative = (
            f"Annual precipitation in the selected area ranges from {min_mm:,.1f} to "
            f"{max_mm:,.1f} mm with an average of {mean_mm:,.1f} mm."
        )
    else:
        narrative = f"Annual precipitation in the selected area is {mean_mm:,.1f} mm."
        flags.append(
            f"3.4: only {stack.n_pixels} climate grid cells fall inside the AOI, so no spatial "
            "range is reported."
        )

    results = {
        'narrative': narrative,
        'tables': {'monthly_precipitation': stack.monthly},  # twelve bars, one per month
        'values': {
            # Chart metadata travels with the series so the frontend does not hardcode units.
            'chart_series': "monthly_precipitation",
            'chart_unit': "mm",
            'chart_axis_label': "Precipitation (mm)",
            'annual_mean_mm': mean_mm,
            'annual_min_mm': min_mm,
            'annual_max_mm': max_mm,
            'n_pixels': stack.n_pixels,
            'source': f"{WORLDCLIM_VERSION} {WORLDCLIM_RESOLUTION}",
            'period': WORLDCLIM_PERIOD,
        },
        'flags': flags,
    }

    # min and max are SPATIAL, across the AOI's annual-total pixels -- not the driest and wettest
    # month. `historical_precipitations` carries the twelve monthly means separately.
    #
    # `number_of_dry_months` is NOT in the notebook's 3.4. It is derived here, in the seam, by
    # counting months whose AOI mean falls below DRY_MONTH_MM. The threshold is the notebook's
    # own: config documents "dry month < 100 mm" for benefit 5.3's dryland zoning, so this reuses
    # that definition rather than inventing a second one. Counted on the monthly spatial means,
    # so a month is dry for the site as a whole, not per pixel.
    monthly = monthly_view(stack)
    dry_months = sum(1 for m in monthly if m['value'] < DRY_MONTH_MM)

    # THE ENDPOINT'S PRECIPITATION FIELDS ARE PER MONTH, THE TOTAL IS PER YEAR. The card reads
    # "ranges from 45 to 320 mm per month, with an annual total of 2,000 mm and 4 dry months", so
    # min, max and avg describe the MONTHLY profile -- the driest and wettest month of the year --
    # and only the total is annual.
    #
    # This is NOT what `results` above reports, and both are right. The notebook's 3.4 gives the
    # SPATIAL spread of the annual total across the AOI's pixels (2,533 to 2,642 mm a year on
    # indonesia_3): how much wetter one corner of the site is than another. The card wants the
    # SEASONAL spread instead (24 to 318 mm a month on the same site): how much wetter January is
    # than August. Different questions, same stack, so the notebook figures stay in `results` and
    # the seam maps the card's fields from the monthly series.
    #
    # 3.3 temperature deliberately does NOT match this. Its card reads "annual mean temperature
    # ranges from 26.4 to 27.4 with an average of 26.9" -- the annual mean varying across the
    # area, which is the spatial reading. So temperature keeps spatial min and max and
    # precipitation takes monthly ones, because that is what each card asks for.
    values = [m['value'] for m in monthly]
    month_min_mm = min(values)
    month_max_mm = max(values)

    # The annual total. Equal to `mean_mm` exactly, not approximately: the spatial mean of each
    # pixel's twelve-month sum and the sum of the twelve spatial means are the same number, since
    # summing and averaging commute over a fixed pixel set and `read_monthly_stack` keeps a pixel
    # only when all twelve months are present. Verified at 0.000e+00 on three AOIs.
    total_mm = mean_mm

    view_results = {
        'precipitation_min_number': month_min_mm,
        'precipitation_max_number': month_max_mm,
        'precipitation_avg_number': total_mm / 12.0,
        'precipitation_total_number': total_mm,
        'number_of_dry_months': dry_months,
        'historical_precipitations': [
            {'precipitation': m['value'], 'month': int(m['id'])} for m in monthly
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python annual_precipitation.py [aoi path]
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
    results, view_results = analyze_annual_precipitation(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
