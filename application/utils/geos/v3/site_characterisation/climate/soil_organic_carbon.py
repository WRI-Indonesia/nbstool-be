"""
Component 3.2 Soil Organic Carbon.

Soil organic carbon held in the top 30 cm of the project area, in tCO2e.

Data. SoilGrids depth-interval rasters, `soil_carbon_stock1..5_t_ha_v3.tif` = 0-5, 5-15, 15-30,
30-60, 60-100 cm. Values are CARBON, tC/ha -- not biomass and not CO2e, so no carbon fraction is
applied here, only the 44/12 conversion. 3.2 reports 0-30 cm, so it SUMS the top three per pixel
on one shared grid (`like=`), which is what makes a per-pixel sum meaningful.

Decisions locked.
- Nodata counts as ZERO carbon, matching 3.1. Coverage is measured from the first layer and
  flagged below CARBON_COVERAGE_WARN_PCT so an incomplete raster stays visible.
- The depth is reported with every figure. A soil carbon number without its depth is unreadable.

The peat caveat, and why it has three states. Peat can extend metres below 30 cm, so on peatland
this raster is a lower bound rather than an estimate. Whether the AOI has peat comes from 1.1 in
the General stage, so the component distinguishes peat present / no peat / NOT KNOWN, and flags
the third rather than silently assuming no peat. `ecosystem_present` is that Axis 3 set; pass
None when General has not run.

Downstream use. Soil is the pool most often excluded from carbon accounting and the one that
peatland projects live or die on, so the flag matters more than the headline.
"""

from __future__ import annotations

try:
    from ...common import AOI, load_raster_clipped, not_applicable, safe_pct, sentences
    from ...config import (
        CARBON_COVERAGE_WARN_PCT,
        CO2_PER_C,
        SOIL_CARBON_0_30_RASTERS,
        SOIL_CARBON_DEPTH_CM,
    )
except ImportError:  # `python soil_organic_carbon.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, load_raster_clipped, not_applicable, safe_pct, sentences
    from config import (
        CARBON_COVERAGE_WARN_PCT,
        CO2_PER_C,
        SOIL_CARBON_0_30_RASTERS,
        SOIL_CARBON_DEPTH_CM,
    )

PEATLAND_CLASS = 3  # ecosystem code from 1.1


def analyze_soil_organic_carbon(
    aoi: AOI,
    ecosystem_present: set[int] | None = None,
) -> tuple[dict, dict]:
    """Component 3.2. Soil organic carbon in the project area, in tCO2e.

    `ecosystem_present` is the Axis 3 set from 1.1. Pass None when the General stage has not
    been run; the peat caveat is then skipped and flagged rather than silently omitted.
    """
    # 0-30 cm SOC = sum of the top three SoilGrids depth intervals (0-5, 5-15, 15-30 cm),
    # added per pixel on one shared grid (like=). Nodata counts as zero carbon, matching 3.1;
    # coverage is measured from the first layer so an incomplete raster stays visible.
    layers = [load_raster_clipped(SOIL_CARBON_0_30_RASTERS[0], aoi, resampling="average")]
    for p in SOIL_CARBON_0_30_RASTERS[1:]:
        layers.append(load_raster_clipped(p, aoi, resampling="average", like=layers[0]))
    raster = layers[0]  # grid, pixel area and coverage reference

    stock = sum(lyr.values.filled(0.0).astype(float) for lyr in layers)  # tC/ha, 0-30 cm

    # tC/ha * ha = tC. No carbon fraction: the raster is already carbon, not biomass.
    soc_tc = float(stock.sum()) * raster.pixel_area_ha
    total_tco2e = soc_tc * CO2_PER_C

    if total_tco2e <= 0:
        empty = not_applicable(
            "3.2 Soil Organic Carbon",
            "No soil carbon data is available for this project area, so soil organic carbon "
            "cannot be estimated.",
        )
        results = {'narrative': empty.narrative, 'tables': {},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        view_results = {'soil_organic_carbon_number': None}
        return results, view_results

    density_tco2e_ha = total_tco2e / aoi.area_ha if aoi.area_ha > 0 else 0.0
    coverage_pct = safe_pct(raster.valid_area_ha, aoi.area_ha)

    flags: list[str] = []
    if coverage_pct < CARBON_COVERAGE_WARN_PCT:
        flags.append(
            f"3.2: the soil carbon raster covers only {coverage_pct:.0f}% of the AOI. Nodata is "
            "counted as zero carbon, so the headline is an under-estimate by an unknown amount."
        )

    # Peat caveat. Depends on 1.1, so the three states are: peat present, no peat, unknown.
    has_peat = None if ecosystem_present is None else PEATLAND_CLASS in ecosystem_present
    peat_clause = ""
    if has_peat is None:
        flags.append(
            "3.2: the General stage has not been run, so the AOI could not be checked for "
            "peatland. If peat is present, this figure understates the soil stock heavily."
        )
    elif has_peat:
        peat_clause = (
            f"Part of this project area is peatland, where peat can extend well below "
            f"{SOIL_CARBON_DEPTH_CM} cm, so the real soil carbon stock is likely to be "
            "considerably higher."
        )
        flags.append(
            f"3.2: peatland present. The raster represents the top {SOIL_CARBON_DEPTH_CM} cm "
            "only, so this is a lower bound on soil carbon, not an estimate of it."
        )

    narrative = sentences(
        f"The soil in this project area holds approximately {total_tco2e:,.0f} tCO2e of organic "
        f"carbon in the top {SOIL_CARBON_DEPTH_CM} cm, an average of {density_tco2e_ha:,.0f} "
        "tCO2e per hectare.",
        peat_clause,
    )

    results = {
        'narrative': narrative,
        'tables': {},
        'values': {
            'total_tco2e': total_tco2e,          # headline big number
            'density_tco2e_ha': density_tco2e_ha,
            'soc_tc': soc_tc,
            'depth_cm': SOIL_CARBON_DEPTH_CM,
            'coverage_pct': coverage_pct,
            'peatland_present': has_peat,        # True, False, or None when 1.1 is unavailable
        },
        'flags': flags,
    }

    # The contract asks for the soil number only. Depth, density and the peat state stay in
    # `results`: a soil carbon figure without its depth is unreadable, so whoever renders this
    # needs SOIL_CARBON_DEPTH_CM from somewhere even though the payload does not carry it.
    # `soil_organic_carbon_percentage` is not emitted here: its denominator is soil + AGB + BGB,
    # which needs 3.1 as well. See `_carbon_shares` in run_climate.py.
    view_results = {'soil_organic_carbon_number': total_tco2e}

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python soil_organic_carbon.py [aoi path]
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
    results, view_results = analyze_soil_organic_carbon(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
