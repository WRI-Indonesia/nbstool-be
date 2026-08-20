"""
Component 3.1 Current Carbon Storage.

Biomass carbon currently stored in the project area, in tCO2e, split into aboveground and
belowground pools.

Data. `agbd_v3.tif`, continuous DRY BIOMASS DENSITY in Mg/ha, not carbon: GEDI AGBD calibrated
with Alpha Earth. The carbon fraction (0.47) and the CO2 conversion (44/12) are applied here
rather than upstream, so both stay visible.

Belowground biomass is DERIVED, not read: BGB = AGB * ROOT_TO_SHOOT_RATIO (0.28). Two
consequences. BGB shares AGB's grid and coverage exactly, and the pool split is constant by
construction (about 78 / 22 on every site), so the percentages are NOT a site-specific finding.
The component flags that itself rather than letting the chart imply otherwise.

NB: the v3 bucket does carry `bgbd_v3.tif`, a mapped belowground layer. Reading it would change
3.1's numbers, so it is deliberately not used until the notebook switches to it.

Decisions locked.
- Density times area, so the pixel area cancels the per hectare unit: Mg/ha * ha = Mg.
- Nodata inside the AOI counts as ZERO biomass (team decision). Coverage is measured separately
  and flagged below CARBON_COVERAGE_WARN_PCT, so an incomplete raster stays visible rather than
  quietly deflating the headline.
- Pool shares are of the biomass total reported here, not of total site carbon. Soil is excluded
  (that is 3.2), so these percentages sum to 100 of a partial accounting.

Downstream use. The headline tCO2e is the baseline stock every Benefit-module projection is
measured against, and the density per hectare is what makes sites comparable.
"""

from __future__ import annotations

from dataclasses import dataclass, replace

try:
    from ...common import AOI, fmt_pct, load_raster_clipped, not_applicable, oxford_join, safe_pct
    from ...config import (
        AGB_RASTER,
        CARBON_COVERAGE_WARN_PCT,
        CARBON_FRACTION,
        CO2_PER_C,
        ROOT_TO_SHOOT_RATIO,
    )
except ImportError:  # `python current_carbon_storage.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, fmt_pct, load_raster_clipped, not_applicable, oxford_join, safe_pct
    from config import (
        AGB_RASTER,
        CARBON_COVERAGE_WARN_PCT,
        CARBON_FRACTION,
        CO2_PER_C,
        ROOT_TO_SHOOT_RATIO,
    )

CARBON_POOLS = ("Aboveground biomass", "Belowground biomass")


@dataclass(frozen=True)
class CarbonPool:
    """One biomass pool integrated over the AOI."""

    name: str
    biomass_mg: float      # total dry biomass, tonnes
    storage_tco2e: float   # after carbon fraction and 44/12
    coverage_pct: float    # share of the AOI with a valid pixel
    pct: float = 0.0       # share of the biomass carbon total, filled in once both pools exist


def _integrate_pool(name: str, path: str, aoi: AOI) -> CarbonPool:
    """Integrate one biomass raster over the AOI and convert to tCO2e.

    Density times area, so the pixel area cancels the per hectare unit:
        Mg/ha * ha = Mg
    Each pool is integrated on its own grid, so AGB and BGB need not share a resolution.
    """
    raster = load_raster_clipped(path, aoi, resampling="average")

    # Team decision: nodata inside the AOI counts as zero biomass. Coverage is measured
    # separately so an incomplete raster is still visible.
    values = raster.values.filled(0.0).astype(float)

    biomass_mg = float(values.sum()) * raster.pixel_area_ha
    storage_tco2e = biomass_mg * CARBON_FRACTION * CO2_PER_C

    return CarbonPool(
        name=name,
        biomass_mg=biomass_mg,
        storage_tco2e=storage_tco2e,
        coverage_pct=safe_pct(raster.valid_area_ha, aoi.area_ha),
    )


def analyze_current_carbon_storage(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.1. Biomass carbon currently stored in the project area, in tCO2e."""
    # AGB is read; BGB is derived from it by a fixed root-to-shoot ratio (config), because there
    # is no mapped BGB layer yet. Deriving means BGB shares AGB's grid and coverage exactly, and
    # the pool split is constant by construction (see the flag below).
    agb_pool = _integrate_pool(CARBON_POOLS[0], AGB_RASTER, aoi)
    bgb_pool = CarbonPool(
        name=CARBON_POOLS[1],
        biomass_mg=agb_pool.biomass_mg * ROOT_TO_SHOOT_RATIO,
        storage_tco2e=agb_pool.storage_tco2e * ROOT_TO_SHOOT_RATIO,
        coverage_pct=agb_pool.coverage_pct,
    )
    pools = [agb_pool, bgb_pool]

    total_tco2e = sum(p.storage_tco2e for p in pools)

    if total_tco2e <= 0:
        empty = not_applicable(
            "3.1 Current Carbon Storage",
            "No biomass data is available for this project area, so current carbon storage "
            "cannot be estimated.",
        )
        results = {'narrative': empty.narrative, 'tables': {'pools': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'above_ground_biomass_number': None,
                        'below_ground_biomass_number': None}
        return results, view_results

    # Shares are of the biomass total reported here, not of total site carbon. Soil is excluded,
    # so these percentages sum to 100 of a partial accounting.
    pools = [replace(p, pct=safe_pct(p.storage_tco2e, total_tco2e)) for p in pools]

    density_tco2e_ha = total_tco2e / aoi.area_ha if aoi.area_ha > 0 else 0.0
    coverage_pct = max(p.coverage_pct for p in pools)

    # The notebook also builds a derived carbon-density raster here (`3.1_carbon_density...`) for
    # map display. This port drops raster outputs, as every General component does: the endpoint
    # streams numbers, and it saves a second full read of the AGB raster.

    flags: list[str] = []
    if coverage_pct < CARBON_COVERAGE_WARN_PCT:
        flags.append(
            f"3.1: the biomass rasters cover only {coverage_pct:.0f}% of the AOI. Nodata is "
            "counted as zero biomass, so the headline is an under-estimate by an unknown "
            "amount."
        )

    # BGB is a fixed multiple of AGB, so the pool split is the same on every site. Say so, so the
    # percentages are not read as a site finding.
    #
    # A NOTE, NOT A FLAG. Nothing is degraded and there is nothing to retry -- this fires on every
    # run, and routing it through `error_status` would mark 3.1 `partial` on every request until
    # the field stopped being read. `flags` is for what went wrong; `notes` is for what the number
    # means. See pipeline.error_status.
    notes = [
        f"3.1: belowground biomass is derived as {ROOT_TO_SHOOT_RATIO:g} x aboveground, not "
        "mapped, so the aboveground / belowground split is constant by construction and is not a "
        "site-specific result."
    ]

    breakdown = oxford_join(
        f"{p.name.lower()} holds {p.storage_tco2e:,.0f} tCO2e ({fmt_pct(p.pct)})"
        for p in pools
    )
    narrative = (
        f"This project area currently stores approximately {total_tco2e:,.0f} tCO2e in "
        f"aboveground and belowground biomass, an average of {density_tco2e_ha:,.0f} tCO2e per "
        f"hectare. Of this, {breakdown}. Soil organic carbon is not included."
    )

    results = {
        'narrative': narrative,
        'tables': {'pools': pools},
        'values': {
            'total_tco2e': total_tco2e,          # headline big number
            'density_tco2e_ha': density_tco2e_ha,
            'coverage_pct': coverage_pct,
            'pool_tco2e': {p.name: p.storage_tco2e for p in pools},
            'pool_pct': {p.name: p.pct for p in pools},
            'pools_included': list(CARBON_POOLS),
            'pools_excluded': ["deadwood", "litter", "soil organic carbon"],
            'bgb_derived_root_to_shoot': ROOT_TO_SHOOT_RATIO,
        },
        'flags': flags,
        'notes': notes,
    }

    # NEITHER `total_carbon_storage` NOR THE PERCENTAGES ARE EMITTED HERE, even though 3.1 computes
    # a total and a split of its own. Both belong to SOIL + AGB + BGB, and 3.1 has no soil: widening
    # its denominator would change the notebook figures it reports. So it publishes the two biomass
    # pools as numbers and nothing that divides by a total -- see `_carbon_shares` in run_climate.py,
    # which owns both once soil exists.
    #
    # `results` is untouched by that. `values['total_tco2e']` and `values['pool_pct']` keep the
    # notebook's biomass-only total and its 78.1 / 21.9 shares, which is what the narrative quotes
    # and what "Soil organic carbon is not included" refers to.
    view_results = {
        'above_ground_biomass_number': agb_pool.storage_tco2e,
        'below_ground_biomass_number': bgb_pool.storage_tco2e,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python current_carbon_storage.py [aoi path]
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
    results, view_results = analyze_current_carbon_storage(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
