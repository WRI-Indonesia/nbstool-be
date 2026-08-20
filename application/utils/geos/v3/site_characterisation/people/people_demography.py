"""
Component 6.1 People Demography.

How many people live inside the project area, split by sex and by five-year age group.

Data. WorldPop, three rasters. `gridded_population_v3.tif` is a plain count of people per cell.
`female_pop_v3.tif` and `male_pop_v3.tif` carry twenty five-year age bands each, as BANDS, named
`f_00_2025` .. `f_90_2025` and `m_00_2025` .. `m_90_2025`. The twenty collapse to the fourteen
display ranges in PEOPLE_AGE_GROUPS.

POPULATION IS A COUNT, so this reads with `sum` resampling, not `average`. Merging four cells of
a count means adding the people in them; averaging would quietly delete three quarters of them
wherever the analysis grid is coarser than WorldPop's. It also means the figures are never
multiplied by cell area -- they are already people, not people per hectare.

THE TWO SOURCES ARE ON DIFFERENT GRIDS, and the notebook says so at length. The total raster's
origin sits under a millionth of a degree from the sex rasters' and it is one pixel smaller in
each dimension; the female and male rasters share one grid between them. `like=total` forces both
sex stacks onto the total's grid so that a cell means the same ground in all three, which is what
makes the cross-check at the end meaningful.

The sex and age breakdown is reported ONLY when the sex rasters completely cover every valid
total-population cell. Partial coverage would let the age table quietly describe a smaller area
than the headline total, so it is withheld instead, with a flag. That is why every age figure is
`None` rather than `0` in that case: no one measured zero people.

The port is the notebook's function unchanged. The three module-level asserts that document
`_band_total` and `_has_complete_sex_age_coverage` are kept, because they are the only statement
anywhere of what those two helpers promise.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
import rasterio

try:
    from ...common import (
        AOI,
        RasterSlice,
        load_raster_bands_clipped,
        load_raster_clipped,
        not_applicable,
        safe_pct,
    )
    from ...config import (
        PEOPLE_AGE_GROUPS,
        POP_FEMALE_RASTER,
        POP_MALE_RASTER,
        POP_TOTAL_RASTER,
    )
    from ...settings import layer_path
except ImportError:  # `python people_demography.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        RasterSlice,
        load_raster_bands_clipped,
        load_raster_clipped,
        not_applicable,
        safe_pct,
    )
    from config import (
        PEOPLE_AGE_GROUPS,
        POP_FEMALE_RASTER,
        POP_MALE_RASTER,
        POP_TOTAL_RASTER,
    )
    from settings import layer_path

AGE_CODES = ("00", "01", "05", "10", "15", "20", "25", "30", "35", "40",
             "45", "50", "55", "60", "65", "70", "75", "80", "85", "90")


def _check_population_band_contract(path: str, prefix: str) -> None:
    # `layer_path` is what turns the config layer name into the bucket URL or the local file; the
    # notebook opened a literal path and so had no need of it.
    expected = tuple(f"{prefix}_{age}_2025" for age in AGE_CODES)
    with rasterio.open(layer_path(path)) as src:
        if src.count != len(expected) or tuple(src.descriptions) != expected:
            raise ValueError(
                f"{path} must contain these 20 bands in order: {', '.join(expected)}."
            )


def _load_population_bands(path: str, aoi: AOI, like: RasterSlice) -> list[RasterSlice]:
    # The notebook loops `load_raster_clipped` over the twenty bands, which opens the file twenty
    # times. `load_raster_bands_clipped` is that loop moved inside a single open: same destination
    # grid, same reprojection, same mask, verified to return bit-identical arrays. Over /vsicurl
    # the twenty extra header fetches per stack were most of this component's wall time.
    return load_raster_bands_clipped(path, aoi, resampling="sum", bands=range(1, 21), like=like)


def _band_total(bands: list[np.ma.MaskedArray], positions: tuple) -> float:
    return sum(float(bands[position - 1].filled(0.0).sum()) for position in positions)


def _has_complete_sex_age_coverage(total_mask: np.ndarray, sex_age_masks: list[np.ndarray]) -> bool:
    return bool(sex_age_masks) and all(np.all(~mask[~total_mask]) for mask in sex_age_masks)


_demo_bands = [np.ma.array([[float(index)]]) for index in range(1, 21)]
assert _band_total(_demo_bands, (1, 2)) == 3.0
assert _band_total(_demo_bands, (15, 16, 17, 18, 19, 20)) == 105.0
_demo_total_mask = np.array([[False, False]])
_demo_sex_age_masks = [np.array([[False, False]]) for _ in range(40)]
assert _has_complete_sex_age_coverage(_demo_total_mask, _demo_sex_age_masks)
_demo_sex_age_masks[-1] = np.array([[False, True]])
assert not _has_complete_sex_age_coverage(_demo_total_mask, _demo_sex_age_masks)


@dataclass(frozen=True)
class AgeGroup:
    age_group: str
    male_population: float | None
    female_population: float | None
    male_percent: float | None
    female_percent: float | None


def analyze_people_demography(aoi: AOI) -> tuple[dict, dict]:
    """Component 6.1. Population of the project area, by sex and age group."""
    flags: list[str] = []

    total = load_raster_clipped(POP_TOTAL_RASTER, aoi, resampling="sum")
    if total.valid_count == 0:
        empty = not_applicable(
            "6.1 People Demography",
            "No population data is available for this project area.",
        )
        results = {'narrative': empty.narrative, 'tables': {'age_groups': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        return results, _empty_view()

    _check_population_band_contract(POP_FEMALE_RASTER, "f")
    _check_population_band_contract(POP_MALE_RASTER, "m")
    female_bands = _load_population_bands(POP_FEMALE_RASTER, aoi, total)
    male_bands = _load_population_bands(POP_MALE_RASTER, aoi, total)

    female_values = [band.values for band in female_bands]
    male_values = [band.values for band in male_bands]
    total_mask = np.ma.getmaskarray(total.values)
    female_mask = np.logical_or.reduce([np.ma.getmaskarray(band.values) for band in female_bands])
    male_mask = np.logical_or.reduce([np.ma.getmaskarray(band.values) for band in male_bands])
    shared = ~(total_mask | female_mask | male_mask)
    sex_age_complete = _has_complete_sex_age_coverage(
        total_mask,
        [np.ma.getmaskarray(band.values) for band in female_bands + male_bands],
    )
    total_population = float(total.values.filled(0.0).sum())
    age_rows = [
        AgeGroup(
            age_group=label,
            male_population=(m := _band_total(male_values, positions)) if sex_age_complete else None,
            male_percent=safe_pct(m, total_population) if sex_age_complete else None,
            female_population=(f := _band_total(female_values, positions)) if sex_age_complete else None,
            female_percent=safe_pct(f, total_population) if sex_age_complete else None,
        )
        for label, positions in PEOPLE_AGE_GROUPS.items()
    ]
    female_population = (
        sum(row.female_population for row in age_rows if row.female_population is not None)
        if sex_age_complete else None
    )
    male_population = (
        sum(row.male_population for row in age_rows if row.male_population is not None)
        if sex_age_complete else None
    )

    if female_population is not None and male_population is not None:
        sex_total = female_population + male_population
        male_pct = safe_pct(male_population, sex_total)
        female_pct = safe_pct(female_population, sex_total)
    else:
        male_pct = None
        female_pct = None
        flags.append(
            "6.1: sex-age rasters do not completely cover all valid total-population cells; "
            "sex and age breakdown is unavailable."
        )

    if shared.any():
        female_grid = sum(band.values.filled(0.0) for band in female_bands)
        male_grid = sum(band.values.filled(0.0) for band in male_bands)
        shared_sex_total = float((female_grid[shared] + male_grid[shared]).sum())
        shared_population_total = float(total.values.filled(0.0)[shared].sum())
        # Source grids differ by <1e-6 deg; sum resampling of edge cells drifts ~0.02
        # people on this AOI. 1 person guards genuine data errors, not resampling noise.
        if not np.isclose(shared_sex_total, shared_population_total, rtol=1e-6, atol=1.0):
            flags.append(
                "6.1: the sex-age population total does not match the total population "
                "raster on their shared valid coverage."
            )
    else:
        flags.append(
            "6.1: the sex-age rasters have no shared valid coverage with the total population raster."
        )

    if female_population is not None and male_population is not None:
        narrative = (
            f"Based on gridded world population data, the selected area has an estimated "
            f"total population of {total_population:,.0f}, consisting of "
            f"{male_population:,.0f} males and {female_population:,.0f} females."
        )
    else:
        narrative = (
            f"Based on gridded world population data, the selected area has an estimated "
            f"total population of {total_population:,.0f}. The sex and age breakdown is unavailable."
        )

    results = {
        'narrative': narrative,
        'tables': {"age_groups": age_rows},
        'values': {
            "total_population": total_population,
            "male_population": male_population,
            "female_population": female_population,
            "male_pct": male_pct,
            "female_pct": female_pct,
            "chart_series": "age_groups",
            "chart_unit": "people",
            "chart_axis_label": "Estimated population",
        },
        'flags': flags,
    }

    return results, _view(total_population, male_population, female_population,
                          male_pct, female_pct, age_rows)


# ============================ ENDPOINT SHAPE ============================
# The People contract NESTS by section, unlike General, Nature and Climate, which are flat. Every
# People component therefore emits `{section: {...}}`, and 6.1, 6.2 and 6.3 all contribute to
# `social_demography`. Keeping the wrapper here rather than in the runner means one component's
# payload is complete on its own, which is what the per-component streaming relies on.
#
# The age table becomes three lists: the male series, the female series, and the groups
# themselves. They are separate because the frontend renders a back-to-back population pyramid,
# where each side is its own series and the shared axis is the group list. `age_group_id` is the
# label, not a surrogate key -- the same choice 2.5 makes for species. `color` is left null: the
# palette is the frontend's, and inventing hex codes here would put the design system in the
# analysis layer.


def _empty_view() -> dict:
    """Zeros for the headline counts, empty series. Used when no population cell falls in the AOI."""
    return {'social_demography': {
        'total_population': 0,
        'male_population': 0,
        'female_population': 0,
        'male_population_percentage': 0,
        'female_population_percentage': 0,
        'age_distributions_male': [],
        'age_distributions_female': [],
        'age_groups': [],
    }}


def _view(total_population, male_population, female_population, male_pct, female_pct,
          age_rows) -> dict:
    return {'social_demography': {
        'total_population': total_population,
        'male_population': male_population,
        'female_population': female_population,
        'male_population_percentage': male_pct,
        'female_population_percentage': female_pct,
        'age_distributions_male': [
            {'id': row.age_group, 'population': row.male_population, 'age_group_id': row.age_group}
            for row in age_rows
        ],
        'age_distributions_female': [
            {'id': row.age_group, 'population': row.female_population,
             'age_group_id': row.age_group}
            for row in age_rows
        ],
        'age_groups': [{'id': row.age_group, 'color': None} for row in age_rows],
    }}


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python people_demography.py [aoi path]
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
    results, view_results = analyze_people_demography(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
