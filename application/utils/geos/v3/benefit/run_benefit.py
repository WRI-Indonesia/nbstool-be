"""
run_benefit.py - F02-P5 carbon components (5.2, 5.3, 5.4, 5.5) as ONE JSON response.

Ported scope (team request 2026-08-25): 5.2 avoided emissions, 5.4 net emission reduction, 5.5
net sequestration -- plus 5.3, which 5.5 reads its gross removal from and cannot run without.
5.1 General Benefit and the nature-benefit modules (5.7+) are NOT ported; the notebook is still
reworking them.

A SINGLE JSON DOCUMENT like pathway, not a stream: two raster analyses and two subtractions.
The seams, replacing the notebook's saved stage files:

    rate_pct        the caller reads component 1.5's rate from the persisted DataAnalyzer row
                    (`site_information_json.historical_deforestation_percentage`, the same
                    value the notebook reads as `component_values(general, "1.5")["rate_pct"]`)
    pathway_stage   a fresh 4.2 run wrapped in the notebook's stage shape, for 5.2's QB gate

5.4 and 5.5 exist only for an NbS carbon project (the notebook's 4.4 toggle) and only when their
gross component is applicable; otherwise they are None in the response, which is the notebook's
"section remains deactivated". The deduction percentages come from the user (GUI), defaulting to
CARBON_RISK_DEFAULTS, and their sum may not exceed 100 -- the notebook's own validation.
"""

from __future__ import annotations

try:
    from ..common import AOI, to_jsonable
    from ..config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_CLASS,
        INTERVENTION_DURATION_DEFAULT_YEARS,
    )
    from ..pathway.activity_list import analyze_activity_list
    from .avoided_deforestation import analyze_avoided_deforestation_emissions
    from .arr_sequestration import analyze_arr_sequestration
    from .general_benefit import analyze_general_benefit
    from .habitat_loss_avoided import analyze_habitat_loss_avoided
    from .net_carbon import net_carbon_removal, net_emission_reduction
    from .net_errs import net_errs
    from .threatened_species import analyze_threatened_species_habitat
except ImportError:  # `python run_benefit.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "pathway"))
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    from activity_list import analyze_activity_list
    from arr_sequestration import analyze_arr_sequestration
    from avoided_deforestation import analyze_avoided_deforestation_emissions
    from common import AOI, to_jsonable
    from config import (
        CARBON_RISK_DEFAULTS,
        ECOSYSTEM_CLASS,
        INTERVENTION_DURATION_DEFAULT_YEARS,
    )
    from general_benefit import analyze_general_benefit
    from habitat_loss_avoided import analyze_habitat_loss_avoided
    from net_carbon import net_carbon_removal, net_emission_reduction
    from net_errs import net_errs
    from threatened_species import analyze_threatened_species_habitat


def run_benefit(aoi: AOI, duration_years: int = INTERVENTION_DURATION_DEFAULT_YEARS,
                rate_pct: float | None = None, carbon_project: bool = True,
                leakage: float | None = None, uncertainty: float | None = None,
                buffer: float | None = None,
                ecosystem_class: int = ECOSYSTEM_CLASS) -> dict:
    """The F02-P5 carbon components for one AOI, as a jsonable dict."""
    if leakage is None:
        leakage = CARBON_RISK_DEFAULTS["leakage_percentage"]
    if uncertainty is None:
        uncertainty = CARBON_RISK_DEFAULTS["uncertainty_percentage"]
    if buffer is None:
        buffer = CARBON_RISK_DEFAULTS["buffer_percentage"]
    # The GUI's per-field range ("1-100%, default X%"), enforced server-side too so a client
    # bypassing the spinners cannot slip a zero or negative deduction through the sum check.
    for name, value in (("leakage", leakage), ("uncertainty", uncertainty), ("buffer", buffer)):
        if not 1 <= value <= 100:
            raise ValueError(f"{name} must be between 1 and 100 percent.")
    # The notebook's 4.4 validation, verbatim rule.
    if leakage + uncertainty + buffer > 100:
        raise ValueError("Total deductions cannot exceed 100%.")

    # 5.2's QB gate reads the notebook's saved pathway stage; a fresh 4.2 run wrapped in the same
    # shape is the backend equivalent (one raster pass, no persisted internals needed).
    pathway_stage = {"components": {"4.2": to_jsonable(analyze_activity_list(aoi))}}

    result_5_1 = analyze_general_benefit(pathway_stage)
    result_5_2 = analyze_avoided_deforestation_emissions(
        aoi, duration_years, rate_pct, pathway_stage)
    result_5_3 = analyze_arr_sequestration(aoi, duration_years)

    result_5_4 = None
    result_5_5 = None
    result_5_6 = None
    if carbon_project and result_5_2.applicable:
        result_5_4 = net_emission_reduction(
            result_5_2.values["total_tco2e"], leakage, uncertainty, buffer)
    if carbon_project and result_5_3.applicable:
        result_5_5 = net_carbon_removal(
            result_5_3.values["total_tco2e"], leakage, uncertainty, buffer)
    # 5.6 combines both gross figures (port assumption, see net_errs.py).
    if carbon_project and (result_5_2.applicable or result_5_3.applicable):
        gross_err = ((result_5_2.values["total_tco2e"] if result_5_2.applicable else 0.0)
                     + (result_5_3.values["total_tco2e"] if result_5_3.applicable else 0.0))
        result_5_6 = net_errs(gross_err, leakage, uncertainty, buffer, duration_years)

    result_5_9 = analyze_habitat_loss_avoided(aoi, duration_years, rate_pct, ecosystem_class)
    result_5_10 = analyze_threatened_species_habitat(aoi, duration_years, rate_pct,
                                                     ecosystem_class)

    return to_jsonable({
        "duration_years": duration_years,
        "carbon_project": carbon_project,
        "carbon_risk": {
            "leakage_percentage": leakage,
            "uncertainty_percentage": uncertainty,
            "buffer_percentage": buffer,
        },
        "general_benefit": result_5_1,            # 5.1
        "avoided_emissions": result_5_2,          # 5.2
        "arr_sequestration": result_5_3,          # 5.3, carried because 5.5 reads it
        "net_emission_reduction": result_5_4,     # 5.4, None unless carbon project + applicable
        "net_carbon_removal": result_5_5,         # 5.5, None unless carbon project + applicable
        "net_errs": result_5_6,                   # 5.6, same gating
        "habitat_loss_avoided": result_5_9,       # 5.9
        "threatened_species_habitat": result_5_10, # 5.10
    })


if __name__ == "__main__":
    # Run on a file and print the response, no Flask app:
    #     python run_benefit.py [aoi path] [duration] [rate_pct]
    import json
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi
    except ImportError:
        from ..common import prepare_aoi

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    rate = float(sys.argv[3]) if len(sys.argv) > 3 else None
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    print(f"AOI: {aoi.area_ha:,.0f} ha over {duration} years, rate={rate}\n")
    t0 = time.perf_counter()
    result = run_benefit(aoi, duration, rate)
    print(json.dumps(result, indent=2, ensure_ascii=False, default=str)[:6000])
    print(f"\n[{time.perf_counter() - t0:.1f}s]", file=sys.stderr)
