"""
Component 1.2 Administrative Boundaries.

Reports where the project area sits administratively, leading with the district and province that
hold most of it, and lists every other district it touches.

Data. GADM v4.1. L0 = country, L1 = province, L2 = district. `sea.adm_boundaries` in the GIS
database, the same table v2 reads. The notebook read one district-level shapefile; the v3 bucket
has no vector object for this layer (administrative_boundaries_v3.tif is a 0/1 land mask with no
names), so it loads through `db.load_admin_intersecting`, which returns a GeoDataFrame carrying
the shapefile's column names. Only the load line differs from the notebook; the dissolve, the
intersection and the narrative below are unchanged.

Decisions locked.
- Sliver threshold: a unit is reported only if its intersection is at least 1% of the AOI. This
  removes false slivers from the generalised GADM boundary lines. Stricter than the pure presence
  rule in 1.1 on purpose, because boundary geometry is coarser than the raster.
- The narrative names the dominant district and its province, and quotes the total AOI area, not
  the area inside that district.
- The province in the sentence is read from the dominant district's own `NAME_1`, not from the
  largest province. Those two can differ: an AOI can be 60% in province A split across three small
  districts and 40% in province B as one large district. Taking the parent of the named district
  keeps the pair internally consistent.
- The country is not narrated, but L0 is still processed, because `dominant_country` drives the
  national risk comparison in 1.6.
- Term for GADM L2 is "district" throughout, narrative and table header. L3 (kecamatan, huyen,
  amphoe) is not used by the tool, so the endpoint's `subdistrict` stays null.

Known limitation, accepted by the team. The second sentence always says "Within this province".
For an AOI that spans more than one province that phrasing is factually wrong, and for a
transboundary AOI it also hides the second country. The component raises a `flags` entry in both
cases so the mismatch is recorded in the output rather than passing silently.

Downstream use. The country result gates which national datasets and policies apply, for example
the APD land status overlay, which is Indonesia first. The province result links to
`gadm41_L1_with_region.csv` for the deforestation risk regions. L2 availability varies by country,
so the component falls back to province level when no district is returned.
"""

from __future__ import annotations

from dataclasses import dataclass

try:
    from ...common import AOI, M2_PER_HA, safe_pct, sentences, sort_by_area
    from ...config import ADMIN_LEVELS, ADMIN_SLIVER_PCT
    from ...db import load_admin_intersecting
except ImportError:  # `python administrative_boundaries.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI, M2_PER_HA, safe_pct, sentences, sort_by_area
    from config import ADMIN_LEVELS, ADMIN_SLIVER_PCT
    from db import load_admin_intersecting


@dataclass(frozen=True)
class AdminUnit:
    name: str
    area_ha: float
    pct: float
    parent: str | None = None  # province for a district, country for a province


def _admin_units(gdf, aoi: AOI, group_cols: list[str], name_field: str,
                 parent_field: str | None = None) -> list[AdminUnit]:
    """One admin level from the combined boundary layer, dissolved by name.

    The single shapefile is district-level, so a country or province appears as many rows. We
    dissolve by the level's NAME columns (COUNTRY, NAME_1, NAME_2), not the GID columns, because
    GID_1/GID_2 are blank for Indonesia in this file and grouping on a blank key drops the unit.
    Ancestor names are included so same-named units in different parents stay separate. Rows with
    a blank key are dropped first, for the same reason.
    """
    if gdf.empty:
        return []
    sub = gdf
    for col in group_cols:
        sub = sub[sub[col].notna() & (sub[col].astype(str).str.strip() != "")]
    if sub.empty:
        return []
    aoi_geom = aoi.geometry.iloc[0]
    diss = sub.dissolve(by=group_cols, as_index=False)
    units = []
    for _, row in diss.iterrows():
        area_ha = row.geometry.intersection(aoi_geom).area / M2_PER_HA
        units.append(AdminUnit(
            name=str(row[name_field]),
            area_ha=area_ha,
            pct=safe_pct(area_ha, aoi.area_ha),
            parent=str(row[parent_field]) if parent_field else None,
        ))
    kept = [u for u in units if u.pct >= ADMIN_SLIVER_PCT]
    return sort_by_area(kept)


def _ancestors(gdf) -> dict[tuple[str, str], str]:
    """(district, sub-district) -> province, for the overlap table.

    An AdminUnit carries one parent, so a sub-district knows its district but not its province.
    Keying on the pair keeps same-named sub-districts in different districts apart.
    """
    if "NAME_3" not in gdf.columns:
        return {}
    return {
        (str(row["NAME_2"]), str(row["NAME_3"])): str(row["NAME_1"])
        for _, row in gdf.iterrows()
    }


def analyze_admin_boundaries(aoi: AOI) -> tuple[dict, dict]:
    """Component 1.2. Where the project area sits administratively."""
    gdf = load_admin_intersecting(aoi)
    countries = _admin_units(gdf, aoi, *ADMIN_LEVELS["country"])
    provinces = _admin_units(gdf, aoi, *ADMIN_LEVELS["province"])
    districts = _admin_units(gdf, aoi, *ADMIN_LEVELS["district"])
    # Levels 3 and 4 are not part of the notebook's three levels. They are filled when the source
    # carries them, which today means Indonesia only, and stay empty everywhere else.
    subdistricts = _admin_units(gdf, aoi, *ADMIN_LEVELS["subdistrict"])
    villages = _admin_units(gdf, aoi, *ADMIN_LEVELS["village"])

    # "approximate total area" is the whole AOI, not the part inside the named district.
    area_text = f"{aoi.area_ha:,.0f}"

    flags: list[str] = []
    missing: list[str] = []

    if districts:
        main = districts[0]
        # Province of the named district, not the largest province. See the note above.
        main_province = main.parent or (provinces[0].name if provinces else None)
        where = f"{main.name}, {main_province}" if main_province else main.name
    elif provinces:
        # L2 is missing for some countries. Fall back to province level rather than emit a
        # sentence with an empty slot.
        where = provinces[0].name
        flags.append(
            "1.2: no GADM L2 district returned above the 1% sliver threshold. The narrative "
            "falls back to province level."
        )
    else:
        where = None
        # ABSENCE: the AOI is outside every GADM unit. The L2 fallback above IS a degradation --
        # a province name where a district was wanted -- so that one stays in `flags`.
        missing.append(
            "1.2: AOI does not intersect any GADM unit above the 1% sliver threshold. "
            "The national comparison in 1.6 will report as not applicable."
        )

    opening = (
        f"This project area is majorly located in {where} with an approximate total area of "
        f"{area_text} hectares."
        if where
        else f"This project area has an approximate total area of {area_text} hectares."
    )

    # Second sentence only when there is more than one district to list.
    follow = (
        "Within this province, it also overlaps with the following districts:"
        if len(districts) > 1
        else ""
    )

    if len(provinces) > 1:
        flags.append(
            f"1.2: AOI spans {len(provinces)} provinces, but the narrative says \"Within this "
            "province\". Accepted by the team; recorded here so the mismatch is visible."
        )
    if len(countries) > 1:
        flags.append(
            "1.2: AOI is transboundary. The narrative does not mention it, and the national "
            "risk comparison in 1.6 uses the dominant country only."
        )

    results = {
        # Rendered under the narrative as District / Province / Area (ha), dominant first. Every
        # district is listed, including the one named in the sentence, so the areas in the table
        # add up to the AOI.
        'narrative': sentences(opening, follow),
        'tables': {
            'district_table': districts,
            'country': countries,
            'province': provinces,
            'subdistrict': subdistricts,
            'village': villages,
        },
        'values': {
            'dominant_country': countries[0].name if countries else None,
            'dominant_province': districts[0].parent if districts else (
                provinces[0].name if provinces else None
            ),
            'dominant_district': districts[0].name if districts else None,
            'dominant_subdistrict': subdistricts[0].name if subdistricts else None,
            'dominant_village': villages[0].name if villages else None,
            'transboundary': len(countries) > 1,
            'provinces': [u.name for u in provinces],
        },
        'flags': flags,
        'missing': missing,
    }

    # The overlap table is listed at the deepest level the source carries, so an Indonesian AOI
    # names its sub-districts and everything else still names districts. `_ancestors` recovers the
    # district and province of a sub-district, which AdminUnit cannot hold: it carries one parent.
    if subdistricts:
        ancestors = _ancestors(gdf)
        overlapping = [
            {
                'subdistrict': unit.name,
                'district': unit.parent,
                'province': ancestors.get((unit.parent, unit.name)),
                'area': unit.area_ha,
            }
            for unit in subdistricts
        ]
    else:
        overlapping = [
            {
                'subdistrict': None,
                'district': unit.name,
                'province': unit.parent,
                'area': unit.area_ha,
            }
            for unit in districts
        ]

    view_results = {
        # Dominant L4 (desa). Indonesia only today -- null wherever the source has no village
        # level. The overlap table below stays at sub-district depth on purpose: an AOI can touch
        # hundreds of villages, and the card's table is not the place to list them.
        'village': results['values']['dominant_village'],
        'subdistrict': results['values']['dominant_subdistrict'],
        'district': results['values']['dominant_district'],
        'province': results['values']['dominant_province'],
        # Dominant L0. Was values-only; promoted so report templates and stored results carry the
        # country without a second lookup.
        'country': results['values']['dominant_country'],
        'overlapping_administration': overlapping,
    }

    return results, view_results


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python administrative_boundaries.py [aoi path]
    # The AOI is any file geopandas reads: a zipped shapefile, .shp, .geojson, .gpkg.
    import json
    import os
    import sys

    import geopandas as gpd

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\thailand.zip"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    results, view_results = analyze_admin_boundaries(aoi)

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}\n")
    dump("results", results)
    print()
    dump("view_results", view_results)
