"""
Component 5.3 ARR Carbon Removal, ex-ante (F02-P5 Benefit).

PORT OF THE NOTEBOOK CELL, body verbatim (F02-P5 Benefit.ipynb, 2026-08-25). Ported as the
dependency of 5.5 Net carbon sequestration, which reads this component's gross removal. Reads the
pathway raster and AGB directly, plus elevation and monthly precipitation for the dryland zone;
no dependency on any earlier stage. Method: NBS-v3-ANX-B Section 4 reference-rate / yield-curve.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

try:
    from ..common import (
        AOI,
        fmt_ha,
        load_raster_clipped,
        safe_pct,
    )
    from ..config import (
        AGB_RASTER,
        ARR_ANR_PAIRS,
        ARR_BASELINE_CLASS_MGHA,
        ARR_BASELINE_MODE,
        ARR_CARBON_DEFERRED_ECO,
        ARR_CARBON_FRACTION,
        ARR_DRYLAND_DEFAULT_ZONE,
        ARR_DRYLAND_ZONES,
        ARR_OLD_END_YEAR,
        ARR_RATE_DM,
        ARR_RATE_DM_DRYLAND,
        ARR_RESTORE_CAT_CSTATE,
        ARR_ROOT_TO_SHOOT,
        ARR_ROOT_TO_SHOOT_DRYLAND,
        ARR_SEQ_PAIRS,
        ARR_STOCKING_ANR,
        ARR_STOCKING_PLANTING,
        ARR_UNCERTAINTY_HIGH,
        ARR_UNCERTAINTY_LOW,
        ARR_YOUNG_END_YEAR,
        ARR_ZONE_DRY_MONTH_MM,
        ARR_ZONE_DRY_SEASON_MONTHS,
        ARR_ZONE_ELEV_MONTANE_M,
        ARR_ZONE_WET_ANNUAL_MM,
        CARBON_COVERAGE_WARN_PCT,
        CO2_PER_C,
        ELEVATION_RASTER,
        PATHWAY_BAND,
        PATHWAY_CATCODE_BAND,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_ECOSYSTEM_PEATLAND,
        PATHWAY_RASTER,
        RESTORE_CODE,
        WORLDCLIM_MONTHS,
        WORLDCLIM_PREC_RASTER,
    )
except ImportError:  # `python arr_sequestration.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
    from common import (
        AOI,
        fmt_ha,
        load_raster_clipped,
        safe_pct,
    )
    from config import (
        AGB_RASTER,
        ARR_ANR_PAIRS,
        ARR_BASELINE_CLASS_MGHA,
        ARR_BASELINE_MODE,
        ARR_CARBON_DEFERRED_ECO,
        ARR_CARBON_FRACTION,
        ARR_DRYLAND_DEFAULT_ZONE,
        ARR_DRYLAND_ZONES,
        ARR_OLD_END_YEAR,
        ARR_RATE_DM,
        ARR_RATE_DM_DRYLAND,
        ARR_RESTORE_CAT_CSTATE,
        ARR_ROOT_TO_SHOOT,
        ARR_ROOT_TO_SHOOT_DRYLAND,
        ARR_SEQ_PAIRS,
        ARR_STOCKING_ANR,
        ARR_STOCKING_PLANTING,
        ARR_UNCERTAINTY_HIGH,
        ARR_UNCERTAINTY_LOW,
        ARR_YOUNG_END_YEAR,
        ARR_ZONE_DRY_MONTH_MM,
        ARR_ZONE_DRY_SEASON_MONTHS,
        ARR_ZONE_ELEV_MONTANE_M,
        ARR_ZONE_WET_ANNUAL_MM,
        CARBON_COVERAGE_WARN_PCT,
        CO2_PER_C,
        ELEVATION_RASTER,
        PATHWAY_BAND,
        PATHWAY_CATCODE_BAND,
        PATHWAY_ECOSYSTEM_BAND,
        PATHWAY_ECOSYSTEM_CODES,
        PATHWAY_ECOSYSTEM_PEATLAND,
        PATHWAY_RASTER,
        RESTORE_CODE,
        WORLDCLIM_MONTHS,
        WORLDCLIM_PREC_RASTER,
    )


# 5.3 ARR Carbon Removal (ex-ante) ------------------------------------------------------
# Reference-rate / yield-curve sequestration for Restore areas, per NBS-v3-ANX-B Section 4.
# Reads the pathway raster and AGB directly, plus elevation and monthly precipitation for the
# dryland zone. No dependency on the 4.2 JSON.


@dataclass(frozen=True)
class ArrGroup:
    """One (cat_code, ecosystem, zone) group of restoring pixels."""

    cat_code: int
    ecosystem: int
    ecosystem_label: str
    zone_label: str           # dryland zone name, or "" for mangrove and peat
    activity_mode: str        # "planting" or "ANR/EMR"
    stocking_factor: float
    area_ha: float
    net_tco2e: float          # central, after baseline and stocking
    net_tco2e_per_ha: float


@dataclass(frozen=True)
class ArrYear:
    """One year of the cumulative removal curve, summed over the AOI."""

    year: int
    cumulative_tco2e: float


def _arr_params(ecosystem: int, zone: int | None):
    """Return (rate dict, R, CF) for one ecosystem. Dryland reads by zone; others by ecosystem."""
    if ecosystem == 1:
        return (ARR_RATE_DM_DRYLAND[zone], ARR_ROOT_TO_SHOOT_DRYLAND[zone],
                ARR_CARBON_FRACTION[1])
    return ARR_RATE_DM[ecosystem], ARR_ROOT_TO_SHOOT[ecosystem], ARR_CARBON_FRACTION[ecosystem]


def _arr_accum_co2e_per_ha(rate: dict, r: float, cf: float, years: int) -> float:
    """Cumulative removal per ha before baseline and stocking, tCO2e/ha, at `years`.

    Two growth phases: Young to year 20, Old to year 40. Nothing accrues beyond year 40.
    """
    young = min(years, ARR_YOUNG_END_YEAR)
    old = max(0, min(years, ARR_OLD_END_YEAR) - ARR_YOUNG_END_YEAR)
    agb_dm = rate["young"] * young + rate["old"] * old       # Mg d.m./ha
    tb_dm = agb_dm * (1.0 + r)                                # add belowground
    return tb_dm * cf * CO2_PER_C


def _arr_baseline_co2e_per_ha(agb_mgha: np.ndarray, r: float, cf: float) -> np.ndarray:
    """Biomass carbon already on site, tCO2e/ha, from the AGB raster. Vector in, vector out."""
    return agb_mgha * (1.0 + r) * cf * CO2_PER_C


def _arr_dryland_zone(aoi: AOI, like_slice):
    """Per-pixel dryland zone code, aligned to `like_slice`, plus a validity mask.

    ANX-B Section 4.5: humid montane above 1000 m; else humid lowland if annual rainfall above
    2000 mm and fewer than 3 dry months (a dry month is below 100 mm); else seasonal lowland.
    Pixels with no elevation or precip data get the conservative default zone.
    """
    elev = load_raster_clipped(ELEVATION_RASTER, aoi, resampling="bilinear", like=like_slice)
    prec = [
        load_raster_clipped(WORLDCLIM_PREC_RASTER, aoi, resampling="average", band=b,
                            like=like_slice)
        for b in range(1, WORLDCLIM_MONTHS + 1)
    ]
    prec_stack = np.ma.stack([p.values for p in prec])           # (12, H, W)
    annual = prec_stack.sum(axis=0)                               # mm/yr
    dry_months = (prec_stack < ARR_ZONE_DRY_MONTH_MM).sum(axis=0)  # count of dry months

    inputs_valid = ~np.ma.getmaskarray(elev.values) & ~np.ma.getmaskarray(annual)
    elev_f = elev.values.filled(0.0)
    annual_f = np.ma.filled(annual, 0.0)                          # missing -> 0 -> not humid
    dry_f = np.ma.filled(dry_months, ARR_ZONE_DRY_SEASON_MONTHS)  # missing -> not humid

    zone = np.full(elev_f.shape, ARR_DRYLAND_DEFAULT_ZONE, dtype=int)
    humid = (annual_f > ARR_ZONE_WET_ANNUAL_MM) & (dry_f < ARR_ZONE_DRY_SEASON_MONTHS)
    zone[humid] = 1
    zone[elev_f > ARR_ZONE_ELEV_MONTANE_M] = 3   # elevation criterion wins over rainfall
    zone[~inputs_valid] = ARR_DRYLAND_DEFAULT_ZONE
    return zone, inputs_valid


def _na(reason: str) -> tuple[dict, dict]:
    """Nothing to quantify -- `missing` drives error_status `failed`, the answer not a fault."""
    return ({'narrative': reason, 'tables': {}, 'values': {}, 'flags': [], 'missing': [reason]},
            {'applicable': False, 'narrative': reason})


def analyze_arr_sequestration(aoi: AOI, duration_years: int) -> tuple[dict, dict]:
    """Component 5.3. Ex-ante carbon removal from ARR restoration on the Restore area, in tCO2e."""
    if duration_years < 1:
        raise ValueError("PROJECT_DURATION_YEARS must be a whole number of years, at least 1.")

    pathway = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest", band=PATHWAY_BAND)
    eco = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                              band=PATHWAY_ECOSYSTEM_BAND, like=pathway)
    cat = load_raster_clipped(PATHWAY_RASTER, aoi, resampling="nearest",
                              band=PATHWAY_CATCODE_BAND, like=pathway)
    agb = load_raster_clipped(AGB_RASTER, aoi, resampling="average", like=pathway)
    pix = pathway.pixel_area_ha

    restore = (pathway.values == RESTORE_CODE).filled(False)
    if not restore.any():
        return _na(
            "No area of this project falls under the Restore pathway, so ARR carbon removal "
            "cannot be estimated."
        )

    catv = cat.values.filled(0).astype(int)
    ecov = eco.values.filled(0).astype(int)
    agbv = agb.values.filled(0.0).astype(float)
    agb_valid = ~np.ma.getmaskarray(agb.values)

    # Dryland zone, derived once if any dryland Restore pixel exists.
    dryland_restore = restore & (ecov == 1)
    if dryland_restore.any():
        zone_arr, zone_valid = _arr_dryland_zone(aoi, pathway)
    else:
        zone_arr = np.full(restore.shape, ARR_DRYLAND_DEFAULT_ZONE, dtype=int)
        zone_valid = np.ones(restore.shape, dtype=bool)

    groups: list[ArrGroup] = []
    quantified = np.zeros_like(restore, dtype=bool)
    annual = np.zeros(duration_years, dtype=float)
    agb_valid_quant_ha = 0.0
    gross_tco2e = 0.0        # before baseline deduction, for diagnosis
    clamped_ha = 0.0         # area where baseline >= accumulation, net forced to zero
    net_classbaseline_tco2e = 0.0   # diagnostic: net under a small class-based baseline
    net_gedi_tco2e = 0.0            # diagnostic: net under the per-pixel GEDI baseline

    for cc, ec in sorted(ARR_SEQ_PAIRS):
        if ec in ARR_CARBON_DEFERRED_ECO:
            continue  # ecosystem held out of quantification (savanna deferred, peat excluded)
        base_mask = restore & (catv == cc) & (ecov == ec)
        if not base_mask.any():
            continue
        # Dryland splits into its three zones; other ecosystems are a single group.
        subgroups = ([(z, base_mask & (zone_arr == z)) for z in ARR_DRYLAND_ZONES]
                     if ec == 1 else [(None, base_mask)])

        for zone_code, gmask in subgroups:
            n = int(gmask.sum())
            if n == 0:
                continue
            quantified |= gmask
            area = n * pix
            rate, r, cf = _arr_params(ec, zone_code)
            conv = (1.0 + r) * cf * CO2_PER_C
            stocking = ARR_STOCKING_ANR if (cc, ec) in ARR_ANR_PAIRS else ARR_STOCKING_PLANTING
            mode = "ANR/EMR" if (cc, ec) in ARR_ANR_PAIRS else "planting"
            accum = _arr_accum_co2e_per_ha(rate, r, cf, duration_years)

            # Three baselines. The primary one is chosen by ARR_BASELINE_MODE; the other two
            # travel in values for comparison. base_class and base_none are constant per pixel.
            cstate = ARR_RESTORE_CAT_CSTATE.get(cc)
            base_gedi = agbv[gmask] * conv                                     # per-pixel vector
            base_class = np.full(n, ARR_BASELINE_CLASS_MGHA.get(cstate, 0.0) * conv)
            base_primary = {"class": base_class, "per_pixel_agb": base_gedi,
                            "none": np.zeros(n)}[ARR_BASELINE_MODE]

            def _net(b, a=accum, st=stocking):
                return float(np.maximum(0.0, a - b).sum()) * st * pix

            net = _net(base_primary)
            gross_tco2e += accum * stocking * area                            # baseline = 0
            net_gedi_tco2e += _net(base_gedi)
            net_classbaseline_tco2e += _net(base_class)
            clamped_ha += int((base_gedi >= accum).sum()) * pix               # GEDI diagnostic
            groups.append(ArrGroup(
                cat_code=cc, ecosystem=ec,
                ecosystem_label=PATHWAY_ECOSYSTEM_CODES.get(ec, f"eco {ec}"),
                zone_label=(ARR_DRYLAND_ZONES[zone_code] if ec == 1 else ""),
                activity_mode=mode, stocking_factor=stocking,
                area_ha=area, net_tco2e=net,
                net_tco2e_per_ha=(net / area if area else 0.0),
            ))

            for i, t in enumerate(range(1, duration_years + 1)):
                accum_t = _arr_accum_co2e_per_ha(rate, r, cf, t)
                annual[i] += float(np.maximum(0.0, accum_t - base_primary).sum()) * stocking * pix

            agb_valid_quant_ha += int((gmask & agb_valid).sum()) * pix

    if not groups:
        return _na(
            "No Restore area carries an ARR activity eligible for carbon sequestration (planting "
            "or ANR on dryland or mangrove), so ex-ante carbon removal cannot be estimated."
        )

    total = sum(g.net_tco2e for g in groups)
    quantified_ha = sum(g.area_ha for g in groups)

    # Restore area eligible for an activity but not carbon quantified.
    deferred = restore & ~quantified
    savanna_ha = int((deferred & (ecov == 4)).sum()) * pix
    peat_ha = int((deferred & (ecov == PATHWAY_ECOSYSTEM_PEATLAND)).sum()) * pix
    other_deferred_ha = int(deferred.sum()) * pix - savanna_ha - peat_ha

    # Dryland pixels that could not be zoned and fell to the default.
    zone_defaulted_ha = int((quantified & (ecov == 1) & ~zone_valid).sum()) * pix

    # Permanent methodology caveats -- `notes`, never `flags`: they must not drive error_status.
    notes: list[str] = []
    cov = safe_pct(agb_valid_quant_ha, quantified_ha)
    if ARR_BASELINE_MODE == "per_pixel_agb" and cov < CARBON_COVERAGE_WARN_PCT:
        notes.append(
            f"5.3: the AGB raster covers only {cov:.0f}% of the quantified area. Missing AGB is "
            "read as zero baseline, so no standing biomass is deducted there and the removal is "
            "over-estimated by an unknown amount."
        )
    notes.append(
        f"5.3: baseline mode is '{ARR_BASELINE_MODE}'. The class-based values are PLACEHOLDERS "
        f"(C4 {ARR_BASELINE_CLASS_MGHA['C4']:g}, C5 {ARR_BASELINE_CLASS_MGHA['C5']:g}, "
        f"C6 {ARR_BASELINE_CLASS_MGHA['C6']:g} Mg/ha) pending references, so the total is "
        f"indicative. For comparison, the per-pixel GEDI baseline gives {net_gedi_tco2e:,.0f} "
        f"tCO2e and gross (no baseline) {gross_tco2e:,.0f}."
    )
    if savanna_ha > 0:
        notes.append(
            f"5.3: {fmt_ha(savanna_ha)} of Restore area is savanna, whose carbon is deferred "
            "(recovery is mainly soil and roots). Activity and benefits still apply."
        )
    if peat_ha > 0:
        notes.append(
            f"5.3: {fmt_ha(peat_ha)} of Restore area is peatland, temporarily excluded from "
            "carbon quantification by team decision (the biomass method and rates exist; remove "
            "peatland from ARR_CARBON_DEFERRED_ECO to re-enable). Activity and benefits still apply."
        )
    if zone_defaulted_ha > 0:
        notes.append(
            f"5.3: {fmt_ha(zone_defaulted_ha)} of dryland could not be zoned (missing elevation "
            "or precipitation) and defaulted to the seasonal-lowland rate."
        )
    if duration_years > ARR_OLD_END_YEAR:
        notes.append(
            f"5.3: the accumulation curve is defined only to year {ARR_OLD_END_YEAR}; the "
            f"{duration_years - ARR_OLD_END_YEAR} years beyond it add no further removal."
        )
    notes.append(
        "5.3: the ANR/EMR stocking factor (0.8) and the planting-vs-ANR split are uncalibrated. "
        "Dryland zones use elevation and 12-band monthly precipitation (units to verify), with a "
        "dry month defined as below 100 mm (Walsh 1996)."
    )

    total_low = total * ARR_UNCERTAINTY_LOW
    total_high = total * ARR_UNCERTAINTY_HIGH

    net_by_ecosystem: dict[str, float] = {}
    area_by_ecosystem: dict[str, float] = {}
    area_by_dryland_zone: dict[str, float] = {}
    for g in groups:
        net_by_ecosystem[g.ecosystem_label] = net_by_ecosystem.get(g.ecosystem_label, 0.0) + g.net_tco2e
        area_by_ecosystem[g.ecosystem_label] = area_by_ecosystem.get(g.ecosystem_label, 0.0) + g.area_ha
        if g.zone_label:
            area_by_dryland_zone[g.zone_label] = area_by_dryland_zone.get(g.zone_label, 0.0) + g.area_ha

    # Placeholder wording, see the markdown cell.
    narrative = (
        f"Restoring the eligible areas of this project could remove an estimated {total:,.0f} "
        f"tCO2e over {duration_years} years, with an indicative range of {total_low:,.0f} to "
        f"{total_high:,.0f} tCO2e."
    )

    groups_sorted = sorted(groups, key=lambda g: -g.net_tco2e)
    annual_rows = [ArrYear(t + 1, annual[t]) for t in range(duration_years)]
    values = {
        "total_tco2e": total,                # headline central estimate
        "total_low_tco2e": total_low,
        "total_high_tco2e": total_high,
        "duration_years": duration_years,
        "quantified_ha": quantified_ha,
        "baseline_mode": ARR_BASELINE_MODE,
        "total_gross_tco2e": gross_tco2e,
        "net_perpixel_gedi_tco2e": net_gedi_tco2e,
        "net_classbaseline_tco2e": net_classbaseline_tco2e,
        "gedi_baseline_zeroed_ha": clamped_ha,   # diagnostic on the GEDI baseline
        "net_by_ecosystem_tco2e": net_by_ecosystem,
        "area_by_ecosystem_ha": area_by_ecosystem,
        "area_by_dryland_zone_ha": area_by_dryland_zone,
        "agb_coverage_pct": cov,
        "zone_defaulted_ha": zone_defaulted_ha,
        "deferred_savanna_ha": savanna_ha,
        "deferred_peat_ha": peat_ha,
        "peat_excluded": True,
        "deferred_other_ha": other_deferred_ha,
        "method": "reference-rate / yield-curve, NBS-v3-ANX-B v2",
        "pools_included": ["aboveground biomass", "belowground biomass"],
        "pools_excluded": ["soil organic carbon", "peat soil", "dead wood", "litter",
                           "avoided emissions"],
        "uncertainty": {"low": ARR_UNCERTAINTY_LOW, "high": ARR_UNCERTAINTY_HIGH},
    }
    results = {
        'narrative': narrative,
        'tables': {"annual_projection": annual_rows, "groups": groups_sorted},
        'values': values,
        'flags': [],
        'notes': notes,
    }
    # The card contract: central estimate with its indicative range and the removal curve.
    view_results = {
        'applicable': True,
        'narrative': narrative,
        **{k: values[k] for k in (
            'total_tco2e', 'total_low_tco2e', 'total_high_tco2e',
            'duration_years', 'quantified_ha')},
        'annual_projection': annual_rows,
        'notes': notes,
    }
    return results, view_results

if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python arr_sequestration.py [aoi path] [duration]
    import json
    import os
    import sys
    import time

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ..common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    duration = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    results, view_results = analyze_arr_sequestration(aoi, duration)
    elapsed = time.perf_counter() - t0

    print(f"AOI: {aoi.area_ha:,.0f} ha, duration {duration} y  [{elapsed:.1f}s]\n")
    payload = to_jsonable(view_results)
    payload["annual_projection"] = payload.get("annual_projection", [])[:3]
    print(json.dumps(payload, indent=2, ensure_ascii=False))
