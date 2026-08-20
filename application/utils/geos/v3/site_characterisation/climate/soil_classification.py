"""
Component 3.6 Soil Classification (WRB 2006).

Ranked World Reference Base 2006 reference soil groups over the AOI, with a one line gloss of the
dominant group.

Two input paths, and they report DIFFERENT QUANTITIES.

| WRB_MODE      | input                          | percentage means      |
|---------------|--------------------------------|-----------------------|
| categorical   | one code raster + a lookup     | share of AOI area     |
| probability   | one raster per group           | mean probability      |

The component records which one it produced in `values["measure"]`. Never present a categorical
result as a probability: "70% Acrisols" as a share means most of the site is Acrisols, whereas as
a probability it means the model is 70% confident about the whole site. Those are not the same
claim.

Data.
- categorical (the configured mode, and the one that runs): `soil_groups_v3.tif`, codes 0-29,
  plus the provider's lookup, supplied as `Soil Classification.xlsx` and converted to
  `soil_class_lookup.csv` beside the rasters. The codes turn out to run alphabetically, matching
  WRB_CLASSES -- now CONFIRMED by the provider rather than assumed. Waiting for the file was the
  right call: SoilGrids was free to order its legend any other way, and a wrong guess would have
  silently renamed every soil in the country with nothing to signal it.
- probability: still unavailable, no per-group probability rasters exist in the bucket.

Decisions locked.
- Groups below WRB_MIN_PROBABILITY_PCT are dropped from the list; the floor keeps a long tail of
  fractions out of the chart.
- A code in the raster that is missing from the lookup is listed as "Unmapped code N" and flagged,
  never dropped. A lookup that falls behind the raster has to be visible.
- WRB_DISPLAY_TOP_N is a hint to the frontend. The table always holds every group above the floor.
- The narrative gloss describes soil PROPERTIES only, never suitability for an intervention (see
  wrb_descriptions.py for why that boundary is deliberate). The endpoint's `description_dict`
  carries the PROVIDER's description instead, which does discuss management. Two different texts
  on purpose; see the note where view_results is built.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

try:
    from ...common import (
        AOI,
        load_raster_clipped,
        load_soil_class_presentation,
        load_soil_class_table,
        not_applicable,
        safe_pct,
    )
    from ...config import (
        SOIL_CLASS_RASTER,
        SOIL_CLASS_TABLE,
        WRB_DISPLAY_TOP_N,
        WRB_MIN_PROBABILITY_PCT,
        WRB_MODE,
        WRB_PROBABILITY_RASTERS,
        WRB_SUM_TOLERANCE_PCT,
    )
    from ...wrb_descriptions import describe_soil
except ImportError:  # `python soil_classification.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import (
        AOI,
        load_raster_clipped,
        load_soil_class_presentation,
        load_soil_class_table,
        not_applicable,
        safe_pct,
    )
    from config import (
        SOIL_CLASS_RASTER,
        SOIL_CLASS_TABLE,
        WRB_DISPLAY_TOP_N,
        WRB_MIN_PROBABILITY_PCT,
        WRB_MODE,
        WRB_PROBABILITY_RASTERS,
        WRB_SUM_TOLERANCE_PCT,
    )
    from wrb_descriptions import describe_soil


@dataclass(frozen=True)
class SoilGroup:
    """One WRB reference soil group and its percentage over the AOI.

    `percent` is a share of area in categorical mode and a mean probability in probability
    mode. The component records which one in `values["measure"]`; never present it as a
    probability without checking that field.
    """

    rank: int              # 1 = most probable or most extensive
    name: str
    percent: float
    description: str


def _empty(reason: str) -> tuple[dict, dict]:
    """The not-applicable payload, in this component's shape."""
    empty = not_applicable("3.6 Soil Classification", reason)
    results = {'narrative': empty.narrative, 'tables': {'soil_groups': []},
               'values': {}, 'flags': empty.flags, 'missing': empty.missing}
    view_results = {'soil_classifications': []}
    return results, view_results


def _soil_from_class_raster(aoi: AOI) -> tuple[dict[str, float], int, list[str]]:
    """Interim path. Share of AOI area per soil class, from a code raster plus a lookup table."""
    raster = load_raster_clipped(SOIL_CLASS_RASTER, aoi, resampling="nearest")
    if raster.valid_area_ha <= 0:
        return {}, 0, []

    lookup = load_soil_class_table(SOIL_CLASS_TABLE)
    codes, counts = np.unique(raster.values.compressed(), return_counts=True)

    percent: dict[str, float] = {}
    unmapped: list[str] = []
    for code, count in zip(codes.tolist(), counts.tolist()):
        name = lookup.get(int(code))
        if name is None:
            # Reported, not dropped: a lookup that falls behind the raster must be visible.
            name = f"Unmapped code {int(code)}"
            unmapped.append(name)
        area_ha = count * raster.pixel_area_ha
        percent[name] = percent.get(name, 0.0) + safe_pct(area_ha, raster.valid_area_ha)

    return percent, int(raster.valid_count), unmapped


def _soil_from_probability(aoi: AOI) -> tuple[dict[str, float], int, list[str]]:
    """Target path. Mean probability per WRB group across the AOI pixels."""
    names = list(WRB_PROBABILITY_RASTERS)
    slices = [
        load_raster_clipped(WRB_PROBABILITY_RASTERS[n], aoi, resampling="average") for n in names
    ]

    shapes = {s.values.shape for s in slices}
    if len(shapes) > 1:
        raise ValueError(
            f"The WRB probability rasters do not share one grid after clipping: {shapes}."
        )

    # A pixel counts only where every group is valid, so the breakdown keeps summing to 100.
    data = np.ma.stack([s.values.astype(float) for s in slices]).filled(np.nan)
    all_valid = ~np.isnan(data).any(axis=0)
    n_pixels = int(all_valid.sum())
    if n_pixels == 0:
        return {}, 0, []

    return {n: float(data[i][all_valid].mean()) for i, n in enumerate(names)}, n_pixels, []


def analyze_soil_classification(aoi: AOI) -> tuple[dict, dict]:
    """Component 3.6. Ranked WRB 2006 soil groups over the AOI.

    The percentage is a share of area or a mean probability depending on WRB_MODE. See the
    module docstring for why the two must not be presented the same way.
    """
    if WRB_MODE == "probability":
        percent, n_pixels, unmapped = _soil_from_probability(aoi)
        measure, axis_label = "mean_probability", "Probability (%)"
    else:
        percent, n_pixels, unmapped = _soil_from_class_raster(aoi)
        measure, axis_label = "share_of_area", "Share of area (%)"

    if not percent:
        return _empty("No soil classification data is available for this project area.")

    total_pct = sum(percent.values())

    flags: list[str] = []
    if measure == "mean_probability" and abs(total_pct - 100.0) > WRB_SUM_TOLERANCE_PCT:
        flags.append(
            f"3.6: the soil group probabilities sum to {total_pct:.1f}% rather than 100%. The "
            "raster set is probably incomplete, so every percentage in this component is "
            "unreliable."
        )
    if unmapped:
        flags.append(
            f"3.6: {len(unmapped)} soil class code(s) in the raster are missing from the lookup "
            f"table: {', '.join(unmapped)}. They are listed unnamed rather than dropped."
        )

    ranked = sorted(percent.items(), key=lambda kv: kv[1], reverse=True)
    groups = [
        SoilGroup(rank=i + 1, name=n, percent=p, description=describe_soil(n))
        for i, (n, p) in enumerate(ranked)
        if p >= WRB_MIN_PROBABILITY_PCT
    ]

    if not groups:
        return _empty("No soil type reaches a meaningful share of this project area.")

    dom = groups[0]
    narrative = (
        "Based on the World Reference Base for Soil Resources (WRB) 2006, the soils in this "
        f"area are predominantly {dom.name}, {dom.description}. The distribution of all "
        "identified soil types is presented below."
    )

    results = {
        'narrative': narrative,
        'tables': {'soil_groups': groups},   # ranked, every group above the floor
        'values': {
            'measure': measure,              # share_of_area or mean_probability
            'mode': WRB_MODE,
            'chart_series': "soil_groups",
            'chart_unit': "%",
            'chart_axis_label': axis_label,
            'display_top_n': WRB_DISPLAY_TOP_N,   # hint only, the table holds all of them
            'dominant_group': dom.name,
            'dominant_percent': dom.percent,
            'dominant_description': dom.description,
            'listed_group_count': len(groups),
            'percent_sum': total_pct,
            'n_pixels': n_pixels,
            'reference': "WRB 2006",
        },
        'flags': flags,
    }

    # `description_dict` carries the DATA PROVIDER's description from the lookup CSV, not the
    # `describe_soil` gloss the narrative above uses. The two differ on purpose: wrb_descriptions
    # keeps to soil properties and never says whether a soil suits an intervention, whereas the
    # provider's copy does discuss management ("often require liming..."). The provider's text is
    # what the client supplied for this contract, so it is what the endpoint returns; the
    # narrative keeps the neutral gloss. Worth knowing they will not read identically.
    presentation = load_soil_class_presentation(SOIL_CLASS_TABLE)

    def _slug(name: str) -> str:
        return "soil_" + name.lower().replace(" ", "_")

    view_results = {
        'soil_classifications': [
            {
                'id': str(g.rank),
                'dict': {'key': _slug(g.name), 'fallback': g.name},
                'description_dict': {
                    'key': f"{_slug(g.name)}_description",
                    'fallback': presentation.get(g.name, {}).get('description') or g.description,
                },
                'percentage': g.percent,
            }
            for g in groups
        ],
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python soil_classification.py [aoi path]
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
    results, view_results = analyze_soil_classification(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
