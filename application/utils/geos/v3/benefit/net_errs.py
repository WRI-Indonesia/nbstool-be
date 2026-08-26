"""
Component 5.6 Estimated net emission reduction and removals, Net ERRs (F02-P5 Benefit).

FORMULAS AND NARRATIVE VERBATIM from the notebook cell (F02-P5 Benefit.ipynb, 2026-08-25). Note
the formula DIFFERS from 5.4/5.5 on purpose: leakage and uncertainty come off the gross first,
then the buffer is taken from the ADJUSTED figure, and the narrative reports the buffer as
withheld separately for non-permanence rather than as a loss.

SEAM ASSUMPTION, surfaced: the notebook cell reads a `gross_err` that no earlier cell defines
(the notebook is mid-rework). The section title says "reduction AND removals", so the caller
supplies gross = 5.2's total + 5.3's total, whichever of the two are applicable. Re-check this
against the notebook when the Benefit rework settles.
"""

from __future__ import annotations

try:
    from ..common import ComponentResult
except ImportError:  # `python net_errs.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import ComponentResult


def net_errs(gross_err: float, leakage: float, uncertainty: float, buffer: float,
             project_duration: int) -> ComponentResult:
    """Component 5.6. The notebook cell's arithmetic, verbatim."""
    leakage_err = gross_err * leakage / 100
    uncertainty_err = gross_err * uncertainty / 100

    adjusted_err = gross_err - leakage_err - uncertainty_err
    buffer_err = adjusted_err * buffer / 100
    net_err = adjusted_err - buffer_err

    annual_err = net_err / project_duration

    narrative = (
        f"Over a {project_duration} year crediting period, the project area could "
        f"generate an estimated {net_err:,.0f} tCO2e in emission reductions and removals, "
        f"equivalent to an average of {annual_err:,.0f} tCO2e per year. "
        f"This estimate accounts for leakage and uncertainty deductions from a gross "
        f"potential of {gross_err:,.0f} tCO2e. It does not include the buffer pool "
        f"contribution, which is withheld separately to address non-permanence risk."
    )

    return ComponentResult(
        component="5.6 Estimated net emission reduction and removals (Net ERRs)",
        applicable=True,
        narrative=narrative,
        values={
            "gross_tco2e": gross_err,
            "leakage_pct": leakage,
            "leakage_tco2e": leakage_err,
            "uncertainty_pct": uncertainty,
            "uncertainty_tco2e": uncertainty_err,
            "adjusted_tco2e": adjusted_err,
            "buffer_pct": buffer,
            "buffer_tco2e": buffer_err,
            "net_tco2e": net_err,              # headline
            "annual_mean_tco2e": annual_err,
            "duration_years": project_duration,
        },
        flags=[
            "5.6: gross is taken as 5.2 avoided emissions plus 5.3 removals; the notebook cell "
            "reads an undefined `gross_err`, so this combination is a port assumption pending "
            "the Benefit rework settling."
        ],
    )
