"""
Component 5.2 Avoided Emissions from Unplanned Deforestation (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL, body verbatim (F02-P5 Benefit.ipynb, 2026-08-25). The seam is the
imports below and the caller: the notebook reads `rate_pct` from the saved F02-P2 general stage
and `pathway_stage` from the saved F02-P4 stage; the backend caller (run_benefit) supplies the
same values from the persisted DataAnalyzer row and a fresh 4.2 run. See the notebook's markdown
cell for the method (ranked-risk allocation of a compounding baseline loss over the Protect area).
"""

from __future__ import annotations

import math
import pathlib
from dataclasses import dataclass

import numpy as np

try:
    from ..common import (
        AOI,
        component_values,
        fmt_ha,
        load_raster_clipped,
        oxford_join,
        safe_pct,
    )
    from ..config import (
        AGB_RASTER,
        BASELINE_RATE_MAX_YEARS,
        CARBON_COVERAGE_WARN_PCT,
        CARBON_FRACTION,
        CO2_PER_C,
        DEFOR_PERIOD_YEARS,
        PATHWAY_BAND,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_PEATLAND,
        PATHWAY_RASTER,
        PROB_RASTER,
        PROTECT_CODE,
        PROTECT_ECOSYSTEM_WORDS,
        PROTECT_RISK_COVERAGE_WARN_PCT,
        ROOT_TO_SHOOT_RATIO,
    )
except ImportError:  # `python avoided_deforestation.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import (
        AOI,
        component_values,
        fmt_ha,
        load_raster_clipped,
        oxford_join,
        safe_pct,
    )
    from config import (
        AGB_RASTER,
        BASELINE_RATE_MAX_YEARS,
        CARBON_COVERAGE_WARN_PCT,
        CARBON_FRACTION,
        CO2_PER_C,
        DEFOR_PERIOD_YEARS,
        PATHWAY_BAND,
        PATHWAY_CODES,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_PEATLAND,
        PATHWAY_RASTER,
        PROB_RASTER,
        PROTECT_CODE,
        PROTECT_ECOSYSTEM_WORDS,
        PROTECT_RISK_COVERAGE_WARN_PCT,
        ROOT_TO_SHOOT_RATIO,
    )


# Repeated rather than imported: notebooks cannot import each other, and the Climate notebook
# owns the same tuple for 3.1. Keep the two in step if a pool is ever added.
BIOMASS_POOLS = ("Aboveground biomass", "Belowground biomass")


@dataclass(frozen=True)
class ProjectionYear:
    """One year of the baseline projection."""

    year: int
    cumulative_loss_ha: float
    annual_avoided_tco2e: float
    cumulative_avoided_tco2e: float


def _project_loss_series(area_ha: float, rate_frac: float, years: int) -> list[float]:
    """Cumulative area lost by the end of each year, compounding.

    Takes the rate as an argument rather than reading it from a fixed source, so that swapping
    the AOI rate for a district reference table later changes the caller, not this function.
    Compounding form: the remaining stock shrinks each year, so annual loss declines. The series
    approaches `area_ha` and never exceeds it.
    """
    return [area_ha * (1.0 - math.exp(-rate_frac * t)) for t in range(1, years + 1)]


def _cumulative_carbon_by_rank(
    density_tco2e_ha: np.ndarray, risk: np.ndarray, pixel_area_ha: float
) -> np.ndarray:
    """Carbon accumulated as pixels are taken in descending risk order.

    Returns an array where element k is the total tCO2e on the k+1 highest risk pixels. Sorting
    once here is what makes the annual series cheap: every year is a lookup into this curve
    rather than a new pass over the raster.
    """
    order = np.argsort(risk)[::-1]
    return np.cumsum(density_tco2e_ha[order]) * pixel_area_ha


def _carbon_on_area(cumulative: np.ndarray, area_ha: float, pixel_area_ha: float) -> float:
    """Carbon on the highest risk `area_ha`, splitting the pixel that straddles the boundary.

    Whole pixels first, then the fraction of the next pixel needed to reach the target area.
    Splitting rather than rounding keeps the annual series smooth on small sites, where one
    pixel can be a visible share of a year of projected loss.
    """
    if area_ha <= 0 or cumulative.size == 0:
        return 0.0

    total_ha = cumulative.size * pixel_area_ha
    if area_ha >= total_ha:
        return float(cumulative[-1])

    whole = int(area_ha // pixel_area_ha)
    carbon = float(cumulative[whole - 1]) if whole > 0 else 0.0

    remainder_ha = area_ha - whole * pixel_area_ha
    if remainder_ha > 0 and whole < cumulative.size:
        prev = float(cumulative[whole - 1]) if whole > 0 else 0.0
        next_pixel_carbon = float(cumulative[whole]) - prev
        carbon += next_pixel_carbon * (remainder_ha / pixel_area_ha)

    return carbon


def _protect_qb_avoided_ha(pathway_stage: dict) -> float:
    """Area of Protect-pathway categories in the AOI that carry a QB Avoided Emissions activity.

    Reads 4.2's by_category. A category counts when its pathway is Protect and at least one of
    its catalog activities has `qb_avoided` True, i.e. the "QB Avoided Emissions" column of
    canonical_v3_activities reads Yes. Returns 0.0 when the pathway stage or 4.2 is absent, which
    makes the gate below fail closed.
    """
    try:
        by_category = component_values(pathway_stage, "4.2")["by_category"]
    except KeyError:
        return 0.0
    area_ha = 0.0
    for info in by_category.values():
        if info.get("pathway") != PATHWAY_CODES[PROTECT_CODE]:
            continue
        if any(a.get("qb_avoided") for a in info.get("activities", [])):
            area_ha += float(info.get("area_ha", 0.0))
    return area_ha


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to quantify -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def analyze_avoided_deforestation_emissions(
    aoi: AOI, duration_years: int, rate_pct: float | None, pathway_stage: dict
) -> tuple[dict, dict]:
    """Component 5.2. Avoided emissions from unplanned deforestation on the Protect area, in tCO2e.

    `rate_pct` is the annual deforestation rate from 1.5, in percent, measured over the whole
    AOI forest. See the markdown cell for why the Protect area cannot supply its own rate.
    `pathway_stage` is the F02-P4 result, read only for the QB gate below.
    """
    if duration_years < 1:
        raise ValueError("PROJECT_DURATION_YEARS must be a whole number of years, at least 1.")

    # Gate, at the start. Quantification runs only when the AOI holds at least one Protect
    # activity whose "QB Avoided Emissions" column is Yes. This is the flag that says avoided
    # deforestation is the right accounting method for the site. In the current canonical catalog
    # every Cat 1 (Protect) row is QB Avoided = Yes, so the gate passes whenever Protect exists
    # with a catalog row; it earns its place by failing closed if the catalog changes, or if the
    # Protect pixels have no matching activity row at all.
    qb_avoided_ha = _protect_qb_avoided_ha(pathway_stage)
    if qb_avoided_ha <= 0:
        return _na(
            "No Protect-pathway activity in this project area is flagged for avoided-emissions "
            "quantification (QB Avoided Emissions = Yes), so avoided deforestation emissions are "
            "not quantified for this site."
        )

    # The risk layer defines the working grid. It is already forest masked upstream, and it is
    # the layer the allocation ranks on, so everything else is aligned to it.
    risk_slice = load_raster_clipped(PROB_RASTER, aoi, resampling="nearest")
    pathway = load_raster_clipped(
        PATHWAY_RASTER, aoi, resampling="nearest", band=PATHWAY_BAND, like=risk_slice
    )

    pixel_area_ha = risk_slice.pixel_area_ha

    # Protect pixels that also carry a risk value. Protect on a non forest reference ecosystem,
    # grassland or savanna for instance, has no risk value because prob.tif is forest masked,
    # and cannot receive projected deforestation.
    protect_all = (pathway.values == PROTECT_CODE).filled(False)
    pool = protect_all & ~np.ma.getmaskarray(risk_slice.values)

    protect_all_ha = int(protect_all.sum()) * pixel_area_ha
    protect_ha = int(pool.sum()) * pixel_area_ha

    if protect_ha <= 0:
        return _na(
            "No forest in this project area falls under the Protect pathway, so avoided "
            "emissions from deforestation cannot be estimated."
        )

    if rate_pct is None:
        return _na(
            "No historical deforestation rate is available for this project area, so a "
            "baseline for avoided emissions cannot be projected."
        )

    # Permanent methodology caveats -- `notes`, never `flags`: they must not drive error_status.
    notes: list[str] = []

    risk_coverage_pct = safe_pct(protect_ha, protect_all_ha)
    if risk_coverage_pct < PROTECT_RISK_COVERAGE_WARN_PCT:
        notes.append(
            f"5.2: the risk layer covers only {risk_coverage_pct:.0f}% of the Protect area. "
            "The remainder carries no projected loss and no avoided emissions."
        )

    if duration_years > BASELINE_RATE_MAX_YEARS:
        notes.append(
            f"5.2: the deforestation rate was measured over {DEFOR_PERIOD_YEARS} years and is "
            f"projected over {duration_years}. A rate that far outside its measurement window "
            "is an assumption, not an observation."
        )

    # Pathway band 3 does two jobs here, from one pass: it supplies the ecosystem word the
    # narrative needs, and it locates peatland.
    ecosystem = load_raster_clipped(
        PATHWAY_RASTER, aoi, resampling="nearest", band=PATHWAY_ECOSYSTEM_BAND, like=risk_slice
    )
    codes, counts = np.unique(ecosystem.values.filled(0)[pool].astype(int), return_counts=True)
    ecosystem_ha = {int(c): int(n) * pixel_area_ha for c, n in zip(codes, counts)}

    # Named in descending area, so the reading order matches what the site is mostly made of.
    ecosystem_words = [
        PROTECT_ECOSYSTEM_WORDS[c]
        for c in sorted(ecosystem_ha, key=ecosystem_ha.get, reverse=True)
        if c in PROTECT_ECOSYSTEM_WORDS
    ]
    ecosystem_label = oxford_join(ecosystem_words) or "natural"

    # A pool pixel outside the mapping means prob.tif and band 3 disagree about what is forest.
    unmapped_ha = sum(ha for c, ha in ecosystem_ha.items() if c not in PROTECT_ECOSYSTEM_WORDS)
    if unmapped_ha > 0:
        notes.append(
            f"5.2: {fmt_ha(unmapped_ha)} of the Protect area carries a risk value but a "
            "reference ecosystem that is not forest, mangrove or peatland. The risk layer and "
            "pathway band 3 disagree about what is forest."
        )

    peat_ha = ecosystem_ha.get(PATHWAY_ECOSYSTEM_PEATLAND, 0.0)
    if peat_ha > 0:
        notes.append(
            f"5.2: {fmt_ha(peat_ha)} of the Protect area sits on peatland. Only aboveground and "
            "belowground biomass is counted, and on peat the avoided emission is dominated by "
            "peat oxidation, so this figure is a large under-estimate there."
        )

    # Carbon density per pixel, tCO2e per hectare. Nodata is zero biomass, as in 3.1. Belowground
    # biomass is derived from aboveground by the root-to-shoot ratio (config), not read from a
    # raster, so total biomass is AGB * (1 + ratio). 5.2 only sums the two pools, so the constant
    # ratio does not distort it the way it flattens the 3.1 pool split.
    agb = load_raster_clipped(AGB_RASTER, aoi, resampling="average", like=risk_slice)
    biomass_mgha = agb.values.filled(0.0).astype(float) * (1.0 + ROOT_TO_SHOOT_RATIO)
    density = biomass_mgha * CARBON_FRACTION * CO2_PER_C

    biomass_coverage_pct = safe_pct(
        int((pool & ~np.ma.getmaskarray(agb.values)).sum()) * pixel_area_ha, protect_ha
    )
    if biomass_coverage_pct < CARBON_COVERAGE_WARN_PCT:
        notes.append(
            f"5.2: the biomass raster covers only {biomass_coverage_pct:.0f}% of the Protect "
            "area. Nodata counts as zero carbon, so the estimate is an under-estimate by an "
            "unknown amount."
        )

    pool_density = density[pool]
    pool_risk = risk_slice.values.filled(0)[pool].astype(float)
    cumulative = _cumulative_carbon_by_rank(pool_density, pool_risk, pixel_area_ha)

    standing_tco2e = float(cumulative[-1])

    rate_frac = rate_pct / 100.0
    loss_series = _project_loss_series(protect_ha, rate_frac, duration_years)

    rows: list[ProjectionYear] = []
    previous_carbon = 0.0
    for year, cumulative_loss_ha in enumerate(loss_series, start=1):
        capped_ha = min(cumulative_loss_ha, protect_ha)
        carbon = _carbon_on_area(cumulative, capped_ha, pixel_area_ha)
        rows.append(
            ProjectionYear(
                year=year,
                cumulative_loss_ha=capped_ha,
                annual_avoided_tco2e=carbon - previous_carbon,
                cumulative_avoided_tco2e=carbon,
            )
        )
        previous_carbon = carbon

    total_tco2e = rows[-1].cumulative_avoided_tco2e
    projected_loss_ha = rows[-1].cumulative_loss_ha
    annual_mean_tco2e = total_tco2e / duration_years

    # Diagnostic. What the same projected loss would be worth if it were spread evenly over the
    # Protect area instead of placed on the highest risk pixels. Ranked allocation normally
    # gives the smaller number, because frontier forest carries less carbon than interior.
    uniform_tco2e = standing_tco2e * safe_pct(projected_loss_ha, protect_ha) / 100.0

    if rate_pct <= 0:
        narrative = (
            "No forest loss was recorded in this project area between 2014 and 2024, so the "
            "baseline projects no further loss and no avoided emissions can be claimed from "
            "protecting the standing forest."
        )
    else:
        narrative = (
            f"Protecting this {ecosystem_label} ecosystem can avoid an estimated "
            f"{total_tco2e:,.0f} tonnes of CO2eq emissions over the project's "
            f"{duration_years} year duration."
        )

    values = {
        "chart_series": "annual_projection",
        "chart_unit": "tCO2e",
        "chart_axis_label": "Cumulative avoided emissions (tCO2e)",
        "total_tco2e": total_tco2e,              # headline big number
        "annual_mean_tco2e": annual_mean_tco2e,
        "duration_years": duration_years,        # recorded so a saved result is reproducible
        "protect_ha": protect_ha,                # measured on the risk grid, not the 4.1 grid
        "qb_avoided_protect_ha": qb_avoided_ha,  # Protect area with a QB Avoided activity (4.2 grid)
        "protect_risk_coverage_pct": risk_coverage_pct,
        "projected_loss_ha": projected_loss_ha,
        "standing_tco2e": standing_tco2e,        # all carbon on the Protect area
        "baseline_rate_pct": rate_pct,
        "baseline_rate_source": "AOI forest 2014 to 2024, component 1.5",
        "allocation": "descending deforestation risk",
        "uniform_allocation_tco2e": uniform_tco2e,   # diagnostic, see the markdown cell
        "peat_ha": peat_ha,
        "biomass_coverage_pct": biomass_coverage_pct,
        "pools_included": list(BIOMASS_POOLS),
        "ecosystem_ha": ecosystem_ha,            # reference ecosystem split of the Protect pool
        "ecosystem_label": ecosystem_label,      # the word used in the narrative
    }
    results = {
        'narrative': narrative,
        'tables': {"annual_projection": rows},
        'values': values,
        'flags': [],
        'notes': notes,
    }
    # The card contract: headline metric, chart series, and the fields its narrative bolds.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'total_tco2e', 'annual_mean_tco2e', 'duration_years', 'ecosystem_label',
            'protect_ha', 'projected_loss_ha', 'baseline_rate_pct',
            'chart_unit', 'chart_axis_label')},
        'annual_projection': rows,
    }
    if notes:
        view_results['notes'] = notes
    return results, view_results

if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python avoided_deforestation.py [aoi path] [duration] [rate_pct]
    # The QB gate needs the 4.2 activity table, so 4.2 is run here the same way run_benefit does.
    import json
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "pathway"))
        from activity_list import analyze_activity_list
    except ImportError:
        from ..common import prepare_aoi, to_jsonable
        from ..pathway.activity_list import analyze_activity_list

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    rate = float(sys.argv[3]) if len(sys.argv) > 3 else 1.23
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    stage = {"components": {"4.2": to_jsonable(analyze_activity_list(aoi))}}

    t0 = time.perf_counter()
    results, view_results = analyze_avoided_deforestation_emissions(aoi, duration, rate, stage)
    elapsed = time.perf_counter() - t0

    print(f"AOI: {aoi.area_ha:,.0f} ha, duration {duration} y, rate {rate}%  [{elapsed:.1f}s]\n")
    payload = to_jsonable(view_results)
    payload["annual_projection"] = payload.get("annual_projection", [])[:3]
    print(json.dumps(payload, indent=2, ensure_ascii=False))
