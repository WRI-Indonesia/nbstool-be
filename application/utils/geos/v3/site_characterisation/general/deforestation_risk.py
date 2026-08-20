"""
Component 1.6 Deforestation Risk.

Reports how the deforestation risk of the AOI forest compares to the national forest, as a
comparative narrative. No chart, text only.

Data. `def_risk_v3.tif`, forestatrisk model output, where 0 to PROB_SCALE_MAX encodes a 0 to 100
relative risk score. **The raster is already masked to forest upstream**, so its valid pixels
inside the AOI are the forest to assess. This component does not build its own forest mask and
never reads the land cover layer.

Interpretation warning. The forestatrisk value is a relative spatial ranking, not an absolute
probability. Two properties of the model make this so: it is fitted with case control sampling,
which ties the intercept to the chosen sampling ratio rather than the true base rate, and any
predicted probability is conditional on the length of the calibration period. What survives as
valid information is the ordering of pixels, not the level. The tool therefore never states an
absolute chance of deforestation. It only places the AOI forest inside the national distribution,
as a percentile.

Decisions locked.
- Forest extent comes from the risk raster itself. Valid pixels are forest, masked pixels are not.
- AOI summary = median risk, robust to the skewed risk distribution: most forest is low risk and a
  small frontier is very high risk, so a mean would be pulled by the right tail.
- Comparison from the national percentile position: above p60 is "higher than", p40 to p60 is
  "similar to", below p40 is "lower than". No arbitrary band.
- Baseline is national, built from the same raster, per country.
- Resampling is nearest. Bilinear would blend risk values across the forest boundary and
  contaminate the median.

Consistency check to run once. The forest that the risk raster was masked to must be the same
Tier 1-2 forest 2024 used by 1.5. If the upstream mask used a different date or a different forest
definition, this component silently reports on a different area than the rest of the module.

Open items, carried forward.
1. `_national_percentile` clamps instead of extrapolating. The reference table holds p10 to p90
   only, so an AOI above p90 always reads as "top 10%" even when it is top 2%. This understates
   exactly the sites that matter most for AUD. Fix by adding p95 and p99 to the reference.
2. The median hides the frontier. An AOI that is 80% safe interior and 20% active logging edge
   reads as low risk, although that 20% is what gives an AUD project its baseline.
3. An AOI median is compared against a table of pixel percentiles. Medians of areas cluster toward
   the centre more than individual pixels do, so "similar to the national average" fires more
   often than one in five sites.
4. If the risk raster is a mosaic of separately fitted regional models, the 0 to 100 scale is not
   guaranteed to be comparable across regions, and a national percentile table pools models with
   different calibrations.

Downstream use. Standing forest at high risk is the core AUD signal.
"""

from __future__ import annotations

import numpy as np

try:
    from ...common import (
        AOI,
        fmt_pct,
        load_national_forest_risk_percentiles,
        load_raster_clipped,
    )
    from ...config import PROB_RASTER, PROB_SCALE_MAX, RISK_HIGHER_PCTL, RISK_LOWER_PCTL
except ImportError:  # `python deforestation_risk.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        fmt_pct,
        load_national_forest_risk_percentiles,
        load_raster_clipped,
    )
    from config import PROB_RASTER, PROB_SCALE_MAX, RISK_HIGHER_PCTL, RISK_LOWER_PCTL


def _national_percentile(value: float, breakpoints: dict[int, float]) -> float:
    """Position `value` inside a country's percentile breakpoints, by linear interpolation.

    Values outside p10 to p90 are clamped, not extrapolated, because the tail shape is unknown.
    Worked example. With p60 = 38 and p70 = 47, a value of 42 sits (42 - 38) / (47 - 38) = 0.44 of
    the way between them, so the percentile is 60 + 0.44 * 10 = 64.4.
    """
    pcts = sorted(breakpoints)
    xs = [breakpoints[p] for p in pcts]
    return float(np.interp(value, xs, pcts))


def _view_results(top_share: float | None) -> dict:
    return {'deforestation_risk_share_percentage': top_share}


def analyze_deforestation_risk(aoi: AOI, dominant_country: str | None) -> tuple[dict, dict]:
    """Component 1.6. Relative deforestation risk of the AOI forest against the national forest."""
    # The risk raster is already masked to forest upstream, so its valid pixels are the forest to
    # assess. No forest mask is built here.
    prob = load_raster_clipped(PROB_RASTER, aoi, resampling="nearest")
    valid = prob.values.compressed()

    if valid.size == 0:
        results = {
            'narrative': (
                "No forest covered by the deforestation risk model is present in this project "
                "area, so deforestation risk cannot be assessed."
            ),
            'tables': {},
            'values': {'aoi_risk': None, 'assessed_ha': 0.0, 'national_percentile': None},
            # ABSENCE, and it used to be silent: an empty `flags` meant `error_status: null`, so
            # a card with nothing in it looked identical to a healthy one.
            'flags': [],
            'missing': ["1.6: no forest covered by the deforestation risk model is present in "
                        "this project area, so risk cannot be assessed."],
        }
        return results, _view_results(None)

    # 0 to PROB_SCALE_MAX encodes a 0 to 100 relative risk score.
    forest_risk = valid.astype(float) / PROB_SCALE_MAX * 100.0
    aoi_risk = float(np.median(forest_risk))
    assessed_ha = valid.size * prob.pixel_area_ha

    base_values = {
        'aoi_risk': aoi_risk,
        'assessed_ha': assessed_ha,
        'assessed_pct_of_aoi': assessed_ha / aoi.area_ha * 100.0 if aoi.area_ha else 0.0,
        'national_percentile': None,
    }

    if not dominant_country:
        results = {
            'narrative': "No country could be determined, so risk cannot be compared nationally.",
            'tables': {},
            'values': base_values,
            # ABSENCE of the input, not a fault here. `after` still marks it retryable when the
            # reason 1.2 gave no country is that 1.2 CRASHED.
            'missing': [
                "1.6: no dominant country from 1.2, so the national comparison is skipped."
            ],
        }
        return results, _view_results(None)

    breakpoints = load_national_forest_risk_percentiles(dominant_country)
    if breakpoints is None:
        results = {
            'narrative': (
                f"No national forest risk reference is available for {dominant_country}, so "
                "risk cannot be compared nationally."
            ),
            'tables': {},
            'values': base_values,
            'missing': [f"1.6: no national risk reference is published for {dominant_country}, "
                        "so the percentile is null."],
        }
        return results, _view_results(None)

    percentile = _national_percentile(aoi_risk, breakpoints)
    top_share = 100.0 - percentile

    if percentile > RISK_HIGHER_PCTL:
        comparison = "higher than"
        narrative = (
            "Forest in this area is at higher deforestation risk than the national average, "
            f"ranking in the top {fmt_pct(top_share)} of the country's forest for "
            "deforestation risk."
        )
    elif percentile < RISK_LOWER_PCTL:
        comparison = "lower than"
        narrative = (
            "Forest in this area is at lower deforestation risk than the national average, in "
            f"the bottom {fmt_pct(percentile)} of the country's forest."
        )
    else:
        comparison = "similar to"
        narrative = (
            "Forest in this area is at deforestation risk similar to the national average, "
            "around the national median."
        )

    results = {
        'narrative': narrative,
        'tables': {},
        'values': {
            **base_values,
            'national_percentile': percentile,
            'top_share_pct': top_share,
            'comparison': comparison,
        },
        'flags': [],
    }

    # "Share" here is the AOI's position in the national distribution: top N% of the country's
    # forest by risk. It stays null until the national percentile reference exists.
    return results, _view_results(top_share)


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python deforestation_risk.py [aoi path] [country]
    # `country` is what 1.2 would supply as dominant_country; omit it and the component reports
    # that no national comparison could be made.
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\thailand.zip"
    country = sys.argv[2] if len(sys.argv) > 2 else None
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_deforestation_risk(aoi, country)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
