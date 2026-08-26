"""
Component 5.6 Estimated net emission reduction and removals, Net ERRs (F02-P5 Benefit).

FORMULAS AND NARRATIVE VERBATIM from the notebook cell (F02-P5 Benefit.ipynb, commit `ae3c786`
+ `f64407a`, 2026-08-26): the adjusted figure is 5.4's intermediates plus 5.5's --
`(gross_er - leakage_er - uncertainty_er) + (gross_removal - leakage_removal -
uncertainty_removal)` -- and the buffer is then taken from the ADJUSTED figure, deliberately
unlike 5.4/5.5 which take it from their gross. The earlier port assumed exactly this combined
gross; the data team's rework confirmed it.

Two notebook artefacts carried verbatim: the narrative's missing space ("grosspotential" -- the
f-string joins "gross" straight onto "potential"), and its `gross_err` variable, which no cell
defines; the sum of the two grosses is the only reading consistent with the formula and is what
this function prints there.
"""

from __future__ import annotations


def net_errs(gross_er: float, gross_removal: float, leakage: float, uncertainty: float,
             buffer: float, project_duration: int) -> tuple[dict, dict]:
    """Component 5.6. The notebook cell's arithmetic, per-component deductions then the sum,
    so the floating-point path matches the cell exactly."""
    leakage_er = gross_er * leakage / 100
    uncertainty_er = gross_er * uncertainty / 100
    leakage_removal = gross_removal * leakage / 100
    uncertainty_removal = gross_removal * uncertainty / 100
    gross_err = gross_er + gross_removal

    adjusted_err = (gross_er - leakage_er - uncertainty_er) + (gross_removal - leakage_removal - uncertainty_removal)
    buffer_err = adjusted_err * buffer / 100
    net_err = adjusted_err - buffer_err

    annual_err = net_err / project_duration

    narrative = (
        f"Over a {project_duration} year crediting period, the project area could "
        f"generate an estimated {net_err:,.0f} tCO2e in emission reductions and removals, "
        f"equivalent to an average of {annual_err:,.0f} tCO2e per year. "
        f"This figure already subtracts leakage and uncertainty deductions from the gross"
        f"potential of {gross_err:,.0f} tCO2e. It does not yet subtract the buffer pool "
        f"contribution, which is held to cover non-permanence risk."
    )

    # The whole breakdown IS the card's formula line, so results and view carry the same fields.
    values = {
        "gross_tco2e": gross_err,
        "gross_emission_reduction_tco2e": gross_er,
        "gross_removal_tco2e": gross_removal,
        "leakage_pct": leakage,
        "leakage_tco2e": leakage_er + leakage_removal,
        "uncertainty_pct": uncertainty,
        "uncertainty_tco2e": uncertainty_er + uncertainty_removal,
        "adjusted_tco2e": adjusted_err,
        "buffer_pct": buffer,
        "buffer_tco2e": buffer_err,
        "net_tco2e": net_err,              # headline
        "annual_mean_tco2e": annual_err,
        "duration_years": project_duration,
    }
    results = {'narrative': narrative, 'tables': {}, 'values': values, 'flags': []}
    view_results = {'applicable': True, 'narrative': narrative, **values}
    return results, view_results
