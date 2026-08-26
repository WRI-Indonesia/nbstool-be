"""
common.py - shared building blocks for the v3 site characterisation modules.

Port of the notebook `common.py` (nbstool_v3) into the backend. Everything more than one
component needs lives here: the AOI contract, raster access, zonal tabulation, the shared forest
masks and the small text helpers that build narratives.

Differences from the notebook version, all forced by where the data lives in the backend:

1. Layer names come from `.config` and `.settings` joins them to the root configured for the
   environment, usually the assets-geo bucket read over /vsicurl by rasterio, exactly as the v2
   layers are read in utils/geos/current_condition.py.
2. `load_vector_intersecting` is gone. The two vector layers the notebook read from shapefiles
   (admin boundaries, WDPA) live in the GIS database here, so they are loaded by `.db` instead.
3. `slope_percent_from_dem` and `classify_continuous` are gone. The v3 terrain rasters are
   already binned, so 1.4 tallies class codes and reclassifies nothing.
4. The notebook's JSON stage handoff (`save_results` / `load_results`) is gone. In the backend the
   caller owns persistence, the same way `process_input_data_analyzer_result` does for v2.

Design contract, unchanged from the notebook
--------------------------------------------
1. The AOI is accepted in any CRS. `prepare_aoi` reprojects it once to REFERENCE_CRS
   (ESRI:54034, equal area) and every component works on that object. No component reprojects
   the AOI again.
2. `AOI.area_ha` is the authoritative denominator for any share that is "share of the site".
   Components that use a different denominator (valid pixels, forest only) say so explicitly.
3. Rasters are read through `load_raster_clipped`, which returns a masked array already aligned
   to the AOI plus the pixel area. In an equal area CRS the pixel area is a constant, so area
   is always pixel_count * pixel_area_ha.
4. Components never return raw arrays to the frontend. Arrays that a later component needs
   (the forest masks) are produced by a shared function here, not passed between results.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field, is_dataclass
from functools import lru_cache
from typing import Iterable, Sequence

import geopandas as gpd
import numpy as np
import pandas as pd
import rasterio
from affine import Affine
from rasterio.features import geometry_mask
from rasterio.warp import Resampling, calculate_default_transform, reproject
from shapely.geometry import shape

try:
    from .config import (
        FC2014_FOREST_CODES,
        FC2014_RASTER,
        FC2024_FOREST_CODES,
        FC2024_RASTER,
        NATIONAL_FOREST_RISK_CSV,
        REFERENCE_CRS,
    )
    from .settings import layer_path
except ImportError:  # imported as a top-level module, e.g. by a component run as a script
    from config import (
        FC2014_FOREST_CODES,
        FC2014_RASTER,
        FC2024_FOREST_CODES,
        FC2024_RASTER,
        NATIONAL_FOREST_RISK_CSV,
        REFERENCE_CRS,
    )
    from settings import layer_path

# Resampling names the components pass, mapped to rasterio. nearest for categorical layers and
# any layer whose min or max is reported; average for stock and probability; bilinear for a
# continuous layer read only for its mean; sum for a COUNT, where the value of a cell is a
# quantity of things rather than a density, so merging cells must add rather than average.
# 6.1 is the only caller of "sum": population per cell is people, and averaging it would lose
# people wherever the destination grid is coarser than WorldPop's.
_RESAMPLING = {
    "nearest": Resampling.nearest,
    "bilinear": Resampling.bilinear,
    "average": Resampling.average,
    "sum": Resampling.sum,
}

M2_PER_HA = 10_000.0


# ============================ AOI CONTRACT ============================


@dataclass(frozen=True)
class AOI:
    """The project area, already in REFERENCE_CRS.

    Attributes
    ----------
    geometry : single polygon or multipolygon in REFERENCE_CRS.
    area_ha  : total site area. Authoritative denominator for "share of the site".
    source_crs : the CRS the user supplied, kept for reporting only.
    """

    geometry: gpd.GeoSeries
    area_ha: float
    source_crs: str


def prepare_aoi(aoi_any_crs: gpd.GeoDataFrame | gpd.GeoSeries) -> AOI:
    """Normalise the AOI once, at the entry point of the tool.

    Accepts any CRS, reprojects to REFERENCE_CRS, dissolves multi part input into one geometry
    so that area is counted once, and measures the total area.
    """
    geoms = aoi_any_crs.geometry if isinstance(aoi_any_crs, gpd.GeoDataFrame) else aoi_any_crs
    source_crs = str(geoms.crs)

    if geoms.crs is None:
        raise ValueError("AOI has no CRS. The tool cannot measure area without one.")

    geoms = geoms.to_crs(REFERENCE_CRS)
    dissolved = gpd.GeoSeries([geoms.union_all()], crs=REFERENCE_CRS)
    area_ha = float(dissolved.area.sum()) / M2_PER_HA

    if area_ha <= 0:
        raise ValueError("AOI has zero area after reprojection.")

    return AOI(geometry=dissolved, area_ha=area_ha, source_crs=source_crs)


def prepare_aoi_from_session(session_id: str) -> AOI:
    """Build the AOI from the polygon a session already stored.

    Mirrors `construct_polygon` in utils/geos/current_condition.py, with one fix: a Polygon with
    holes keeps its holes, because `shape()` is used for both geometry types instead of taking
    the outer ring only.
    """
    from ....models.geos_models.models import Polygons

    known_polygons = Polygons.find_by_session_id(session_id)
    if not known_polygons:
        raise ValueError(f"session id {session_id} has no polygon")

    geojson = Polygons.get_geometry(session_id).first()
    geom = shape(json.loads(geojson[0]))
    gdf = gpd.GeoDataFrame(index=[0], crs="epsg:4326", geometry=[geom])
    return prepare_aoi(gdf)


# ============================ RASTER ACCESS ============================
# Resampling values passed by the components, and why each one is used:
#   "nearest"   categorical rasters, and any layer whose minimum or maximum is reported, so the
#               reported values exist in the source instead of being interpolation artefacts
#   "average"   stock and probability layers, where reprojection must preserve the area
#               weighted mean
#   "bilinear"  continuous layers read only for their mean


@dataclass(frozen=True)
class RasterSlice:
    """A raster clipped and reprojected to the AOI in REFERENCE_CRS.

    values : masked float array. Masked cells are nodata or outside the AOI polygon.
    pixel_area_ha : constant, because REFERENCE_CRS is equal area.
    transform, crs : the destination grid, so another load can be forced onto it with `like`.
    """

    values: np.ma.MaskedArray
    pixel_area_ha: float
    transform: object = None   # affine.Affine of the destination grid
    crs: object = None         # REFERENCE_CRS

    @property
    def valid_count(self) -> int:
        return int(self.values.count())

    @property
    def valid_area_ha(self) -> float:
        return self.valid_count * self.pixel_area_ha


def load_raster_clipped(
    layer: str,
    aoi: AOI,
    resampling: str = "nearest",
    band: int = 1,
    like: "RasterSlice | None" = None,
) -> RasterSlice:
    """Clip `layer` to the AOI, reproject to REFERENCE_CRS, mask to the polygon.

    `layer` is a layer file name from config, not a full path. `settings.layer_path` joins it to
    the root held in tbl_master_settings, so the environment decides whether the read goes to the
    bucket over /vsicurl or to a local copy of the layers.

    `band` selects one band of a multi band raster. Only the pathway raster uses it; every other
    layer is single band and takes the default.

    `resampling` is "nearest" for every categorical layer, which is all of them in this module.
    Categorical values must never be interpolated.

    `like` forces the output onto exactly the grid of an already loaded slice: same shape, same
    origin, same pixel size, so that `a.values[i]` and `b.values[i]` describe the same ground.
    Components that only sum or tabulate one raster at a time leave it None. 1.5 needs it: the
    2014 and 2024 forest masks are compared pixel by pixel, which is only meaningful on one grid.

    Returns an all masked array when the raster does not cover the AOI at all. Components
    handle that as "not applicable", not as an error.
    """
    return load_raster_bands_clipped(layer, aoi, resampling, (band,), like)[0]


def load_raster_bands_clipped(
    layer: str,
    aoi: AOI,
    resampling: str = "nearest",
    bands: "Sequence[int]" = (1,),
    like: "RasterSlice | None" = None,
) -> list[RasterSlice]:
    """Several bands of one raster, on one shared grid, from a SINGLE open of the file.

    Same result as calling `load_raster_clipped` once per band -- identical destination grid,
    identical reprojection, identical masking -- but the file is opened once, the destination grid
    is worked out once and the polygon mask is rasterised once, instead of all three per band.

    That is the whole reason this exists. 6.1 reads twenty age bands from each of two WorldPop
    stacks; opening a remote multi-band GeoTIFF forty times means forty header fetches over
    /vsicurl before a single pixel is read, and it measured between 2 s and 114 s depending on how
    the AOI window happened to fall across the source tiling. The band loop below is the only part
    that repeats per band.

    `load_raster_clipped` is a one-band call into this, so there is one implementation of the clip
    and no way for the two paths to drift.
    """
    rs = _RESAMPLING[resampling]
    geom = aoi.geometry.iloc[0]  # single dissolved polygon, already in REFERENCE_CRS

    with rasterio.open(layer_path(layer)) as src:
        if like is not None:
            # Force the exact grid of an earlier slice, so pixels line up one to one.
            dst_transform = like.transform
            dst_h, dst_w = like.values.shape
        else:
            # Destination grid = source resolution carried into REFERENCE_CRS, cropped to the
            # AOI bounding box so only the AOI window is warped, not the whole SEA raster.
            base_transform, _, _ = calculate_default_transform(
                src.crs, REFERENCE_CRS, src.width, src.height, *src.bounds
            )
            minx, miny, maxx, maxy = geom.bounds
            inv = ~base_transform
            c0, r0 = inv * (minx, maxy)
            c1, r1 = inv * (maxx, miny)
            col_off, row_off = int(np.floor(c0)), int(np.floor(r0))
            dst_w = max(1, int(np.ceil(c1)) - col_off)
            dst_h = max(1, int(np.ceil(r1)) - row_off)
            dst_transform = base_transform * Affine.translation(col_off, row_off)

        # NaN fill separates "no source coverage" from a genuine 0 in a categorical raster.
        fill = float(src.nodata) if src.nodata is not None else np.nan
        grids = []
        for band in bands:
            dst = np.full((dst_h, dst_w), fill, dtype="float64")
            reproject(
                source=rasterio.band(src, band),
                destination=dst,
                src_transform=src.transform,
                src_crs=src.crs,
                dst_transform=dst_transform,
                dst_crs=REFERENCE_CRS,
                src_nodata=src.nodata,
                dst_nodata=fill,
                resampling=rs,
            )
            grids.append(dst)
        src_nodata = src.nodata

    # Mask everything outside the AOI polygon, then everything with no source value. `outside`
    # depends only on the destination grid, so it is rasterised once and shared by every band.
    outside = geometry_mask(
        [geom.__geo_interface__], out_shape=(dst_h, dst_w),
        transform=dst_transform, invert=False,  # True where a pixel is OUTSIDE the polygon
    )
    pixel_area_ha = abs(dst_transform.a * dst_transform.e) / M2_PER_HA

    slices = []
    for dst in grids:
        nodata_mask = np.isnan(dst) if src_nodata is None else (dst == src_nodata)
        slices.append(RasterSlice(
            values=np.ma.masked_array(dst, mask=(outside | nodata_mask)),
            pixel_area_ha=pixel_area_ha, transform=dst_transform, crs=REFERENCE_CRS,
        ))
    return slices


def load_vector_intersecting(path: str, aoi: AOI) -> gpd.GeoDataFrame:
    """Load only the features of `path` that intersect the AOI, reprojected to REFERENCE_CRS.

    Two stages, so a SEA-wide layer is never read in full. `bbox=aoi.geometry` pushes the AOI
    bounding box down to the file reader (geopandas reprojects it to the file CRS), which uses
    the file's spatial index to return only nearby features. Then an exact `intersects` test
    drops the corners the bounding box let through. Returns an empty GeoDataFrame, never None.
    """
    gdf = gpd.read_file(path, bbox=aoi.geometry)
    if gdf.empty:
        return gdf.to_crs(REFERENCE_CRS) if gdf.crs is not None else gdf
    gdf = gdf.to_crs(REFERENCE_CRS)
    aoi_geom = aoi.geometry.iloc[0]
    return gdf[gdf.intersects(aoi_geom)].reset_index(drop=True)


# Column headers as they come out of the "NBS Pathway Logic" Sheet, mapped to the names the tool
# uses. Underscore variants are accepted too, so a hand-made CSV also loads.
_ACTIVITY_COL_ALIASES = {
    "cat_id": "cat_code", "cat_code": "cat_code",
    "ecosystem": "ecosystem",
    "activity id": "activity_id", "activity_id": "activity_id",
    "activity": "activity",
    "benefit nature": "benefit_nature", "benefit_nature": "benefit_nature",
    "benefit people": "benefit_people", "benefit_people": "benefit_people",
    "benefit climate": "benefit_climate", "benefit_climate": "benefit_climate",
    "qb avoided emissions": "qb_avoided", "qb_avoided": "qb_avoided",
    "qb carbon sequestration": "qb_sequestration", "qb_sequestration": "qb_sequestration",
}

# Ecosystem is text in the Sheet; raster band 2 is an integer. This is the bridge.
_ECOSYSTEM_NAME_TO_CODE = {
    "dryland forest": 1, "mangrove": 2, "peatland": 3, "savanna": 4,
}


@lru_cache(maxsize=1)
def load_activity_table(layer: str) -> dict[tuple[int, int], tuple[dict, ...]]:
    """The canonical_v3 activity catalog, keyed on the pair (cat_code, ecosystem).

    Read once per process, like the soil lookup: it is a 42 row CSV that never changes within a
    deployment, and 4.2 would otherwise fetch it from the bucket on every request.

    `layer` is a layer NAME, joined to the configured root by `settings.layer_path` -- the CSV
    sits beside the rasters under V3_BUCKET, not in the repo. That is the one difference from the
    notebook's loader, which takes a local path.

    Rows with no ecosystem are the six ineligible categories: they carry no join key and are
    skipped here. 4.2 handles them through PATHWAY_CATCODE_TO_PATHWAY instead, so they still get
    an area and a row, just no activity.

    Returns tuples rather than lists because the result is cached and shared across threads; a
    caller that mutated a list would corrupt every later request in the process.
    """
    df = pd.read_csv(layer_path(layer))
    df.columns = [_ACTIVITY_COL_ALIASES.get(c.strip().lower(), c.strip().lower())
                  for c in df.columns]

    required = {"cat_code", "ecosystem", "activity_id", "activity",
                "benefit_nature", "benefit_people", "benefit_climate",
                "qb_avoided", "qb_sequestration"}
    missing = required - set(df.columns)
    if missing:
        raise ValueError(f"{layer} is missing columns {sorted(missing)}.")

    def as_bool(v: object) -> bool:
        return str(v).strip().lower() in {"yes", "y", "true", "1"}

    def eco_code(v: object) -> int | None:
        s = str(v).strip()
        return int(s) if s.isdigit() else _ECOSYSTEM_NAME_TO_CODE.get(s.lower())

    def clean_id(v: object) -> str:
        if pd.isna(v):
            return ""
        try:
            return str(int(float(v)))   # "11.0" from a float column -> "11"
        except (ValueError, TypeError):
            return str(v).strip()

    def clean_text(v: object) -> str:
        return "" if pd.isna(v) else " ".join(str(v).split())  # collapse stray newlines

    table: dict[tuple[int, int], list[dict]] = {}
    for _, r in df.iterrows():
        if pd.isna(r["ecosystem"]) or str(r["ecosystem"]).strip() == "":
            continue  # ineligible category, no (cat_code, ecosystem) join key
        ec = eco_code(r["ecosystem"])
        if ec is None:
            raise ValueError(f"Unknown ecosystem {r['ecosystem']!r} in {layer}.")
        table.setdefault((int(r["cat_code"]), ec), []).append({
            "activity_id": clean_id(r["activity_id"]),
            "activity": clean_text(r["activity"]),
            "benefit_nature": clean_text(r["benefit_nature"]),
            "benefit_people": clean_text(r["benefit_people"]),
            "benefit_climate": clean_text(r["benefit_climate"]),
            "qb_avoided": as_bool(r["qb_avoided"]),
            "qb_sequestration": as_bool(r["qb_sequestration"]),
        })
    return {key: tuple(rows) for key, rows in table.items()}


@lru_cache(maxsize=1)
def _soil_class_rows(layer: str) -> tuple[tuple[int, str, str, str], ...]:
    """The soil lookup CSV as (code, name, hex, description) rows. Read once per process."""
    df = pd.read_csv(layer_path(layer)).fillna("")
    return tuple(
        (int(c), str(n), str(h), str(d))
        for c, n, h, d in zip(df["code"], df["name"], df["hex"], df["description"])
    )


def load_soil_class_table(path: str | None) -> dict[int, str]:
    """Map soil class raster codes to WRB group names, from a two column lookup table.

    Returns {code: name}. Codes present in the raster but missing here are reported by 3.6 as
    unmapped rather than dropped, so a lookup that falls behind the raster stays visible.

    Supplied by the data provider as `Soil Classification.xlsx` and converted to
    `soil_class_lookup.csv`, which sits beside the rasters under V3_BUCKET like
    national_forest_risk_reference.csv. Converting rather than reading the workbook keeps Excel
    out of the backend's runtime dependencies.

    The provider's codes turn out to run 0..29 in alphabetical order, matching config.WRB_CLASSES.
    That is now CONFIRMED rather than assumed -- the earlier refusal to assume it stands, because
    SoilGrids was free to order its legend any other way and a wrong guess would have silently
    renamed every soil in the country.
    """
    if not path:
        raise NotImplementedError(
            "3.6 soil classification needs the code -> WRB name lookup for soil_groups_v3.tif. "
            "Set config.SOIL_CLASS_TABLE."
        )
    return {code: name for code, name, _hex, _desc in _soil_class_rows(path)}


def load_soil_class_presentation(path: str | None) -> dict[str, dict[str, str]]:
    """{name: {"hex": ..., "description": ...}} from the same lookup CSV.

    Separate from load_soil_class_table because that one mirrors the notebook's signature exactly
    and returns only what 3.6's analysis body asks for. The colour and the provider's own
    description are presentation, so they are read here and used only by the endpoint payload.
    """
    if not path:
        return {}
    return {
        name: {"hex": hex_code, "description": desc}
        for _code, name, hex_code, desc in _soil_class_rows(path)
    }


def load_national_forest_risk_percentiles(country: str) -> dict[int, float] | None:
    """Percentile breakpoints of national forest deforestation risk, on the 0 to 100 scale.

    Returns {10: value, 20: value, ... 90: value}, or None when the reference is not configured
    or the country is missing from it. The notebook checks Path(...).exists(); here the reference
    sits beside the rasters under V3_BUCKET, which may be a URL, so a missing file surfaces as a
    read error from the loader rather than a path test. 1.6 already handles None by reporting that
    no national comparison could be made.
    """
    if not NATIONAL_FOREST_RISK_CSV:
        return None

    df = pd.read_csv(layer_path(NATIONAL_FOREST_RISK_CSV))
    df.columns = [str(c).strip().lower() for c in df.columns]
    row = df[df["country"].astype(str).str.strip().str.lower() == str(country).strip().lower()]
    if row.empty:
        return None

    breakpoints = {}
    for pct in range(10, 100, 10):
        col = f"p{pct}"
        if col in row.columns:
            breakpoints[pct] = float(row.iloc[0][col])
    return breakpoints or None


# ============================ ZONAL TABULATION ============================


@dataclass(frozen=True)
class ClassShare:
    """One row of a class distribution."""

    code: int | str
    label: str
    area_ha: float
    pct: float


def tabulate_classes(
    raster: RasterSlice,
    class_labels: dict[int, str],
    denominator_ha: float,
) -> list[ClassShare]:
    """Count pixels per class code and turn them into area and share.

    `denominator_ha` is passed in rather than derived, because the choice of denominator is a
    locked decision that differs per component:
      - share of the whole site  -> aoi.area_ha          (1.1, 1.7)
      - share of valid pixels    -> raster.valid_area_ha (1.4)
      - share of forest          -> forest area
    """
    rows: list[ClassShare] = []
    for code, label in class_labels.items():
        # `filled(False)` because a comparison against a fully masked slice returns the masked
        # constant, which cannot be summed. A layer that does not reach the AOI counts as zero.
        count = int(np.ma.filled(raster.values == code, False).sum())
        area_ha = count * raster.pixel_area_ha
        rows.append(
            ClassShare(
                code=code,
                label=label,
                area_ha=area_ha,
                pct=safe_pct(area_ha, denominator_ha),
            )
        )
    return rows


def slope_percent_from_dem(elev: RasterSlice, aoi: AOI) -> RasterSlice:
    """Slope in percent computed from a clipped elevation raster (metres), for 1.4.

    Slope is the horizontal gradient of elevation, so it needs true ground distances. But the
    reference CRS (ESRI:54034) is equal area: it preserves area and distorts distance with
    latitude. This corrects the per-pixel run using the AOI centroid latitude, first-order exact
    for a cylindrical equal-area sphere with the standard parallel at the equator: the east-west
    run shrinks by cos(lat), the north-south run grows by 1 / cos(lat). Near the equator the
    correction is tiny; it grows toward the tropics' edge.

    Returns a RasterSlice of slope percent on the same grid, so it can be binned like any layer.
    """
    z = elev.values.astype("float64").filled(np.nan)
    if min(z.shape) < 2:
        # Too few pixels for a gradient; report flat rather than error.
        flat = np.ma.masked_array(np.zeros_like(z), mask=np.ma.getmaskarray(elev.values))
        return RasterSlice(flat, elev.pixel_area_ha, elev.transform, elev.crs)

    lat = np.radians(float(aoi.geometry.centroid.to_crs(4326).y.iloc[0]))
    cos_lat = max(float(np.cos(lat)), 1e-6)
    run_x = abs(elev.transform.a) * cos_lat        # east-west ground metres per pixel
    run_y = abs(elev.transform.e) / cos_lat        # north-south ground metres per pixel

    gy, gx = np.gradient(z, run_y, run_x)
    slope_pct = np.sqrt(gx**2 + gy**2) * 100.0
    masked = np.ma.masked_array(
        slope_pct, mask=np.isnan(slope_pct) | np.ma.getmaskarray(elev.values)
    )
    return RasterSlice(masked, elev.pixel_area_ha, elev.transform, elev.crs)


def classify_continuous(raster: RasterSlice, breaks: Sequence[float]) -> RasterSlice:
    """Bin a continuous raster into integer class codes 1..len(breaks)+1.

    Upper-exclusive edges via np.digitize: a value below breaks[0] is class 1, and a value at or
    above breaks[-1] is the top class. Used by 1.4 to classify continuous elevation and slope
    itself, since the inputs are not pre-binned. The mask is preserved, so nodata stays nodata.
    """
    codes = np.digitize(raster.values.filled(np.nan), list(breaks), right=False) + 1
    masked = np.ma.masked_array(codes, mask=np.ma.getmaskarray(raster.values))
    return RasterSlice(
        values=masked,
        pixel_area_ha=raster.pixel_area_ha,
        transform=raster.transform,
        crs=raster.crs,
    )


def safe_pct(part: float, whole: float) -> float:
    """Share in percent, 0.0 when the denominator is zero. Keeps narratives free of NaN."""
    return 0.0 if whole <= 0 else part / whole * 100.0


def dominant(rows: Sequence[ClassShare]) -> ClassShare | None:
    """The class with the largest area, or None when every class is empty.

    Ties are broken by the order of `rows`, which follows the class code order in config. This
    is deterministic but arbitrary. Exact ties are unlikely on real rasters.
    """
    present = [r for r in rows if r.area_ha > 0]
    return max(present, key=lambda r: r.area_ha) if present else None


def sort_by_area(rows: list) -> list:
    """Sort any objects that carry `area_ha` descending. Dominant first, used in narratives."""
    return sorted(rows, key=lambda r: r.area_ha, reverse=True)


# ============================ SHARED MASKS ============================


@dataclass(frozen=True)
class ForestMask:
    """The AOI forest at a single date, as a boolean array plus its area.

    Produced here rather than passed between component results, so that 1.5 and any later module
    all use one definition of forest and one alignment.
    """

    mask: np.ndarray
    pixel_area_ha: float
    area_ha: float
    slice: RasterSlice | None = None   # the grid the mask sits on, so a second date can match it

    @property
    def is_empty(self) -> bool:
        return self.area_ha <= 0


def forest_mask_2014(aoi: AOI) -> ForestMask:
    """Forest inside the AOI in 2014, from the binary FC2014 layer.

    FC2014 is already a binary product (1 = forest), so FC2014_FOREST_CODES = [1] is a raster
    value, not a land cover legend code. The Tier 1 to 2 rule was applied upstream when FC2014
    was built, so both dates carry the same forest definition.
    """
    fc = load_raster_clipped(FC2014_RASTER, aoi, resampling="nearest")
    mask = np.isin(fc.values.filled(-1), FC2014_FOREST_CODES)
    return ForestMask(
        mask=mask,
        pixel_area_ha=fc.pixel_area_ha,
        area_ha=int(mask.sum()) * fc.pixel_area_ha,
        slice=fc,
    )


def forest_mask_2024(aoi: AOI, like: RasterSlice | None = None) -> ForestMask:
    """Forest inside the AOI in 2024, from the binary FC2024 layer.

    Reads forest_2024_v3.tif (value 1 = forest), the same Tier 1-2 definition applied upstream to
    FC2014, so both dates are symmetric. `like` forces the 2014 grid, which 1.5 needs because it
    compares the two masks pixel by pixel.
    """
    fc = load_raster_clipped(FC2024_RASTER, aoi, resampling="nearest", like=like)
    mask = np.isin(fc.values.filled(-1), FC2024_FOREST_CODES)
    return ForestMask(
        mask=mask,
        pixel_area_ha=fc.pixel_area_ha,
        area_ha=int(mask.sum()) * fc.pixel_area_ha,
        slice=fc,
    )


# ============================ VECTOR OVERLAY ============================


def union_overlap_ha(aoi: AOI, features: gpd.GeoDataFrame) -> float:
    """Area of the AOI covered by the union of `features`.

    Union first, then intersect. Both WDPA (1.3) and KBA (2.2) contain sites that overlap or
    nest inside each other, so adding per site areas can exceed the AOI. The union is the only
    figure that can be reported as a share of the site.
    """
    if features.empty:
        return 0.0
    merged = features.geometry.union_all()
    return float(aoi.geometry.intersection(merged).area.sum()) / M2_PER_HA


def per_feature_overlap_ha(aoi: AOI, features: gpd.GeoDataFrame) -> np.ndarray:
    """Per feature intersection area with the AOI, in the row order of `features`.

    Used for the breakdown table only. These values may sum to more than the union area when
    sites overlap, which is expected and is why the headline uses `union_overlap_ha`.
    """
    if features.empty:
        return np.array([], dtype=float)
    aoi_geom = aoi.geometry.iloc[0]
    return np.array(
        [geom.intersection(aoi_geom).area / M2_PER_HA for geom in features.geometry],
        dtype=float,
    )


# ============================ NARRATIVE HELPERS ============================


def fmt_ha(value: float) -> str:
    """Hectares for prose. Thousands separator, no decimals. Screening precision, not cadastral."""
    return f"{value:,.0f} ha"


def fmt_pct(value: float, decimals: int = 0) -> str:
    return f"{value:.{decimals}f}%"


def oxford_join(items: Iterable[str]) -> str:
    """Join a list into readable prose: "a", "a and b", "a, b, and c"."""
    items = list(items)
    if not items:
        return ""
    if len(items) == 1:
        return items[0]
    if len(items) == 2:
        return f"{items[0]} and {items[1]}"
    return f"{', '.join(items[:-1])}, and {items[-1]}"


def sentences(*parts: str) -> str:
    """Join non empty sentence fragments with a single space. Skips None and empty strings."""
    return " ".join(p.strip() for p in parts if p)


# ============================ RESULT BASE ============================


@dataclass
class ComponentResult:
    """Common shape returned by every component.

    `applicable` is False when the component has nothing to measure. The frontend still renders
    the card and shows `narrative`, so the absence of a signal is visible rather than silently
    missing.
    """

    component: str
    applicable: bool
    narrative: str
    tables: dict[str, list] = field(default_factory=dict)
    values: dict[str, object] = field(default_factory=dict)
    flags: list[str] = field(default_factory=list)
    # Things that do not exist for this AOI, as opposed to `flags`, which is things that went
    # wrong. Both reach the wire; `pipeline.error_status` reports absence as `failed`, meaning
    # "this is the answer", because asking again cannot make the data exist.
    missing: list[str] = field(default_factory=list)


def not_applicable(component: str, reason: str) -> ComponentResult:
    """A component with nothing to measure. The reason goes to `missing`, not `flags`.

    Without it the card was indistinguishable on the wire from one that ran and found something:
    empty `flags` means `error_status: null`, so "no soil carbon data is available for this
    project area" reached the client as a healthy line carrying nulls.
    """
    return ComponentResult(component=component, applicable=False, narrative=reason,
                           missing=[reason])


def to_jsonable(obj: object) -> object:
    """Convert a result tree into plain JSON types.

    Handles the four things that appear in results and that json cannot take directly:
    dataclasses (ClassShare, AdminUnit, ProtectedSite, HazardCard), frozensets (present_set from
    1.1), numpy scalars, and numpy arrays.

    Note the lossy step: `present_set` becomes a sorted list, so a consumer must not rely on set
    semantics after a round trip. Downstream code should read it back with `set(...)`.
    """
    if is_dataclass(obj) and not isinstance(obj, type):
        return {k: to_jsonable(v) for k, v in asdict(obj).items()}
    if isinstance(obj, (set, frozenset)):
        return sorted(obj, key=str)
    if isinstance(obj, dict):
        return {str(k): to_jsonable(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [to_jsonable(v) for v in obj]
    if isinstance(obj, np.generic):
        return obj.item()
    if isinstance(obj, np.ndarray):
        return obj.tolist()
    return obj

def component_values(payload: dict, component_key: str) -> dict:
    """Shortcut for the common read: `component_values(general, "1.2")["dominant_country"]`."""
    return payload["components"][component_key]["values"]

