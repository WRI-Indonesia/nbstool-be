"""
Components 5.4 Net carbon emission reduction and 5.5 Net carbon sequestration (F02-P5 Benefit).

The notebook writes these as inline cells applying the 4.4 Carbon Risk Assumption deductions to
5.2's and 5.3's gross totals; the FORMULAS AND NARRATIVES below are the cells' own, verbatim.
Both sections exist only for an NbS carbon project (the user's 4.4 toggle) and only when their
gross component is applicable -- the caller gates on both, exactly as the notebook's `if` does.

The deductions are percentages of the gross figure: leakage, uncertainty, and buffer-pool
contribution. Per the notebook's 4.4 disclaimer (Verra NPRT): the 15% leakage default is an
optional conservative default, not a universal minimum, and the official buffer pool
contribution must be determined with the Verra AFOLU Non-Permanence Risk Tool at validation.
"""

from __future__ import annotations


def _deductions(gross: float, leakage: float, uncertainty: float, buffer: float):
    """The notebook's three deduction lines, shared by 5.4 and 5.5."""
    leakage_t = gross * leakage / 100
    uncertainty_t = gross * uncertainty / 100
    buffer_t = gross * buffer / 100
    net = gross - leakage_t - uncertainty_t - buffer_t
    return leakage_t, uncertainty_t, buffer_t, net


def net_emission_reduction(gross_er: float, leakage: float, uncertainty: float,
                           buffer: float) -> tuple[dict, dict]:
    """Component 5.4. Deductions applied to 5.2's gross emission reduction (`total_tco2e`)."""
    leakage_er, uncertainty_er, buffer_er, net_er = _deductions(
        gross_er, leakage, uncertainty, buffer)
    narrative = (
        f"Net carbon emissions reduction is estimated at {net_er:,.2f} tCO2e "
        f"after applying deductions to the total estimated carbon reduction "
        f"of {gross_er:,.2f} tCO2e."
    )
    # The whole breakdown IS the card's formula line, so results and view carry the same fields.
    values = {
        "gross_tco2e": gross_er,
        "leakage_pct": leakage,
        "leakage_tco2e": leakage_er,
        "uncertainty_pct": uncertainty,
        "uncertainty_tco2e": uncertainty_er,
        "buffer_pct": buffer,
        "buffer_tco2e": buffer_er,
        "net_tco2e": net_er,               # headline
    }
    results = {'narrative': narrative, 'tables': {}, 'values': values, 'flags': []}
    view_results = {'applicable': True, 'narrative': narrative, **values}
    return results, view_results


def net_carbon_removal(gross_removal: float, leakage: float, uncertainty: float,
                       buffer: float) -> tuple[dict, dict]:
    """Component 5.5. Deductions applied to 5.3's gross carbon removal (`total_tco2e`)."""
    leakage_removal, uncertainty_removal, buffer_removal, net_removal = _deductions(
        gross_removal, leakage, uncertainty, buffer)
    narrative = (
        f"Net carbon sequestration is estimated at {net_removal:,.2f} tCO2e "
        f"after applying deductions to the total estimated carbon sequestration "
        f"of {gross_removal:,.2f} tCO2e."
    )
    values = {
        "gross_tco2e": gross_removal,
        "leakage_pct": leakage,
        "leakage_tco2e": leakage_removal,
        "uncertainty_pct": uncertainty,
        "uncertainty_tco2e": uncertainty_removal,
        "buffer_pct": buffer,
        "buffer_tco2e": buffer_removal,
        "net_tco2e": net_removal,          # headline
    }
    results = {'narrative': narrative, 'tables': {}, 'values': values, 'flags': []}
    view_results = {'applicable': True, 'narrative': narrative, **values}
    return results, view_results
