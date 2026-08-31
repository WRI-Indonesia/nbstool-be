"""
Component 1.5 Historical Deforestation (2014 to 2024).

Reports how much forest the AOI lost between 2014 and 2024 and the annual rate using Puyravaud.

Data. `fc_2014_v3.tif` (binary forest cover 2014, Tier 1-2) and `forest_2024_v3.tif` (binary
forest cover 2024). Both dates use the same Tier 1-2 forest definition, applied upstream, through
`forest_mask_2014` and `forest_mask_2024` in common.py. The notebook derived the 2024 mask from
LC2024 at one point and the team switched it to the binary layer; the v3 pair matches that
decision, so this component is the notebook's unchanged.

Decisions locked.
- A2 = A1 - loss_ha, gross loss of 2014 forest. Forest gain on non forest 2014 land is excluded,
  so the rate stays consistent with the reported loss.
- No comparison against a national rate. The component reports the site on its own terms. The
  national lookup, its CSV and the similar band threshold have been removed from the tool.
- The rate is shown to one decimal. Screening precision, not a monitoring figure.

Puyravaud (2003). rate = (1 / (t2 - t1)) * ln(A2 / A1) * 100

Two edge cases decided here, not in the spec.
1. No loss at all. The Puyravaud formula returns 0, which is correct but reads oddly as "an
   average of 0.0% per year", so the component uses a dedicated sentence instead.
2. Total loss, A2 == 0. All 2014 forest is gone and ln(0) is undefined. The component reports
   `rate_pct = None` and says the area lost all of its forest, rather than emitting an infinite
   rate.

Downstream use. Observed loss is the empirical basis of the trajectory (Axis 2) and a direct
threat intensity signal. The AUD pathway signal now comes from the modelled risk in 1.6 rather
than from a national rate comparison here.
"""

from __future__ import annotations

import math

try:
    from ...common import AOI, fmt_ha, forest_mask_2014, forest_mask_2024, safe_pct
    from ...config import DEFOR_PERIOD_YEARS, DEFOR_YEAR_END, DEFOR_YEAR_START
except ImportError:  # `python historical_deforestation.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, fmt_ha, forest_mask_2014, forest_mask_2024, safe_pct
    from config import DEFOR_PERIOD_YEARS, DEFOR_YEAR_END, DEFOR_YEAR_START


def analyze_historical_deforestation(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.5. Forest loss 2014 to 2024 and its annual rate."""
    f2014 = forest_mask_2014(aoi)
    f2024 = forest_mask_2024(aoi)

    if f2014.is_empty:
        results = {
            'narrative': (
                f"No forest was present in this project area in {DEFOR_YEAR_START}, so a "
                "deforestation rate cannot be calculated."
            ),
            'tables': {},
            'values': {'forest_2014_ha': 0.0, 'forest_2024_ha': f2024.area_ha,
                       'loss_ha': 0.0, 'rate_pct': None},
            'flags': [],
        }
        view_results = {
            'historical_deforestation_year_start': DEFOR_YEAR_START,
            'historical_deforestation_year_end': DEFOR_YEAR_END,
            'historical_deforestation_percentage': None,
            'historical_deforestation_total_area': 0.0,
        }
        return results, view_results

    a1 = f2014.area_ha
    # Gross loss of 2014 forest. Gain elsewhere is deliberately not netted off.
    loss_ha = int((f2014.mask & ~f2024.mask).sum()) * f2014.pixel_area_ha
    a2 = a1 - loss_ha

    if loss_ha <= 0:
        rate_pct = 0.0
        narrative = (
            f"No forest loss was detected in this project area between {DEFOR_YEAR_START} and "
            f"{DEFOR_YEAR_END}."
        )
    elif a2 <= 0:
        rate_pct = None  # ln(0) guard, see the note above
        narrative = (
            f"Between {DEFOR_YEAR_START} and {DEFOR_YEAR_END}, the project area lost all of its "
            f"{fmt_ha(a1)} of forest."
        )
    else:
        rate_pct = abs((1.0 / DEFOR_PERIOD_YEARS) * math.log(a2 / a1) * 100.0)
        narrative = (
            f"Between {DEFOR_YEAR_START} and {DEFOR_YEAR_END}, the project area lost "
            f"{fmt_ha(loss_ha)} of forest, an average of {rate_pct:.1f}% per year."
        )

    results = {
        'narrative': narrative,
        'tables': {},
        'values': {
            'forest_2014_ha': a1,
            'forest_2024_ha': a2,
            'loss_ha': loss_ha,
            'rate_pct': rate_pct,                        # annual rate, Puyravaud
            'loss_pct_of_2014_forest': safe_pct(loss_ha, a1),   # total share lost over the period
            'forest_2014_pct_of_aoi': safe_pct(a1, aoi.area_ha),
        },
        'flags': [],
    }

    view_results = {
        'historical_deforestation_year_start': DEFOR_YEAR_START,
        'historical_deforestation_year_end': DEFOR_YEAR_END,
        # The ANNUAL rate, which is what the narrative quotes. `loss_pct_of_2014_forest` in
        # `values` is the other reading of "percentage" - the total share lost across the ten
        # years - if the card means that instead.
        'historical_deforestation_percentage': rate_pct,
        # Total forest loss over the period, in hectares -- the figure the narrative quotes.
        'historical_deforestation_total_area': loss_ha,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python historical_deforestation.py [aoi path]
    # The AOI is any file geopandas reads: a zipped shapefile, .shp, .geojson, .gpkg.
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\thailand.zip"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_historical_deforestation(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
