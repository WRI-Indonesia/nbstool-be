"""
Component 6.1 People Demography.

How many people live inside the project area, split by sex and by five-year age group.

Data. WorldPop, TWO rasters (was three -- see history below). `female_pop_v3.tif` and
`male_pop_v3.tif` carry twenty five-year age bands each, as BANDS, named `f_00_2025` ..
`f_90_2025` and `m_00_2025` .. `m_90_2025`. The twenty collapse to the fourteen display ranges
in PEOPLE_AGE_GROUPS.

POPULATION IS A COUNT, so this reads with `sum` resampling, not `average`. Merging four cells of
a count means adding the people in them; averaging would quietly delete three quarters of them
wherever the analysis grid is coarser than WorldPop's. It also means the figures are never
multiplied by cell area -- they are already people, not people per hectare.

TOTAL POPULATION IS MALE + FEMALE (notebook commit `ab308c9`, 2026-08-22). The total raster
`gridded_population_v3.tif` is no longer read at all: the male raster is the reference grid, the
female stack is forced onto it with `like`, and the headline total is the sum of all forty sex-age
bands. With one source there is nothing to cross-check, so the old coverage machinery -- the
withheld breakdown, the shared-coverage comparison and their flags -- is gone with the raster, and
the sex percentages are now shares of the same total the narrative quotes. Values are rounded to
ten decimals (`round10`) upstream, so the port emits exactly what the notebook prints.

The port is the notebook's function unchanged apart from the two seams: `layer_path` resolves the
layer name (the notebook opens a literal path), and the twenty-band loop reads through
`load_raster_bands_clipped` (see `_load_population_bands`).
"""

from __future__ import annotations

from dataclasses import dataclass

import rasterio

try:
    from ...common import (
        AOI,
        RasterSlice,
        load_raster_bands_clipped,
        not_applicable,
        safe_pct,
    )
    from ...config import (
        PEOPLE_AGE_GROUPS,
        POP_FEMALE_RASTER,
        POP_MALE_RASTER,
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
        not_applicable,
        safe_pct,
    )
    from config import (
        PEOPLE_AGE_GROUPS,
        POP_FEMALE_RASTER,
        POP_MALE_RASTER,
    )
    from settings import layer_path

AGE_CODES = (
    "00", "01", "05", "10", "15", "20", "25", "30", "35", "40",
    "45", "50", "55", "60", "65", "70", "75", "80", "85", "90"
)

round10 = lambda x: None if x is None else round(float(x), 10)  # noqa: E731  (notebook's own)


def _check_population_band_contract(path: str, prefix: str):
    # `layer_path` is what turns the config layer name into the bucket URL or the local file; the
    # notebook opened a literal path and so had no need of it.
    expected = tuple(f"{prefix}_{age}_2025" for age in AGE_CODES)
    with rasterio.open(layer_path(path)) as src:
        if src.count != 20 or tuple(src.descriptions) != expected:
            raise ValueError(f"{path} has incorrect population bands.")


def _load_population_bands(path: str, aoi: AOI, like: RasterSlice | None = None) -> list[RasterSlice]:
    # The notebook loops `load_raster_clipped` over the twenty bands with `like` chained from the
    # first band, which opens the file twenty times. `load_raster_bands_clipped` is that loop moved
    # inside a single open: with `like=None` it derives the destination grid from the source once,
    # exactly what the notebook's first band does, and every later band shares it. Same grid, same
    # reprojection, same mask, verified to return bit-identical arrays. Over /vsicurl the twenty
    # extra header fetches per stack were most of this component's wall time.
    return load_raster_bands_clipped(path, aoi, resampling="sum", bands=range(1, 21), like=like)


def _band_total(bands, positions):
    return sum(
        float(bands[i - 1].filled(0).sum())
        for i in positions
    )


@dataclass(frozen=True)
class AgeGroup:
    age_group: str
    male_population: float
    female_population: float
    male_percent: float
    female_percent: float


def analyze_people_demography(aoi: AOI) -> tuple[dict, dict]:
    """Component 6.1. Population of the project area, by sex and age group."""

    _check_population_band_contract(POP_MALE_RASTER, "m")
    _check_population_band_contract(POP_FEMALE_RASTER, "f")

    # Male raster becomes the reference grid
    male_bands = _load_population_bands(
        POP_MALE_RASTER,
        aoi
    )

    female_bands = _load_population_bands(
        POP_FEMALE_RASTER,
        aoi,
        like=male_bands[0]
    )

    male_values = [b.values for b in male_bands]
    female_values = [b.values for b in female_bands]

    # -------------------------------------------------------------------------
    # Total population = all male + all female age bands
    # -------------------------------------------------------------------------

    male_population = sum(
        float(b.filled(0).sum())
        for b in male_values
    )

    female_population = sum(
        float(b.filled(0).sum())
        for b in female_values
    )

    total_population = (
        male_population
        + female_population
    )

    if total_population <= 0:
        empty = not_applicable(
            "6.1 People Demography",
            "No population data is available for this project area."
        )
        results = {'narrative': empty.narrative, 'tables': {'age_groups': []},
                   'values': {}, 'flags': empty.flags, 'missing': empty.missing}
        return results, _empty_view()

    # -------------------------------------------------------------------------
    # Age groups
    # -------------------------------------------------------------------------

    age_rows = []

    for label, positions in PEOPLE_AGE_GROUPS.items():

        male = _band_total(
            male_values,
            positions
        )

        female = _band_total(
            female_values,
            positions
        )

        age_rows.append(
            AgeGroup(
                age_group=label,
                male_population=round10(male),
                female_population=round10(female),
                male_percent=round10(
                    safe_pct(male, total_population)
                ),
                female_percent=round10(
                    safe_pct(female, total_population)
                ),
            )
        )

    male_pct = safe_pct(
        male_population,
        total_population
    )

    female_pct = safe_pct(
        female_population,
        total_population
    )

    narrative = (
        f"Based on gridded population data, the selected area has an estimated "
        f"total population of {total_population:,.0f}, consisting of "
        f"{male_population:,.0f} males and {female_population:,.0f} females."
    )

    results = {
        'narrative': narrative,
        'tables': {"age_groups": age_rows},
        'values': {
            "total_population": round10(total_population),
            "male_population": round10(male_population),
            "female_population": round10(female_population),
            "male_pct": round10(male_pct),
            "female_pct": round10(female_pct),
            "chart_series": "age_groups",
            "chart_unit": "people",
            "chart_axis_label": "Estimated population",
        },
        'flags': [],
    }

    return results, _view(round10(total_population), round10(male_population),
                          round10(female_population), round10(male_pct), round10(female_pct),
                          age_rows)


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
