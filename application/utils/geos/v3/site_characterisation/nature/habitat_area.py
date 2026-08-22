"""
Component 2.3 Habitat Area (Area of Habitat, AoH).

Which species have suitable habitat inside the project area, how much of it, and what share of
the site each one covers.

Data. ONE RASTER PER SPECIES under `habitat_area/<Class>/<Species_name>.tif`: uint8, EPSG:4326,
nodata 255, DN 1 = suitable habitat. A GeoParquet inventory, `habitat_area/species_iucn_v3.
geoparquet`, carries one row per species with its class, raster path, IUCN status and a bounding
box footprint. 3,129 species: 1,112 reptiles, 946 amphibians, 752 mammals, 319 birds.

THIS IS A REWRITE, following notebook commits e2f4f49 and b4dc460 (2026-08-08) which replaced the
whole component. What changed, and why it matters:
  - PER-SPECIES RASTERS, not four multiband stacks. The old `AOH_BAND_CHUNK` reader is gone.
  - REPTILES ARE REAL. There was no reptile stack, so `reptile_number_of_species` was null and the
    card could not show the count its mock asks for. The inventory carries 1,112 of them.
  - AREA AND SHARE, not just presence. Each species now reports pixel count, habitat area in
    hectares and percentage of the AOI, so "255 species" can be read as "which ones, and how much
    of this site does each actually cover".
  - IUCN STATUS COMES WITH THE DATA. It used to be unanswerable -- the old inventory workbook's
    own `data_source` column read DUMMY_PLACEHOLDER_NOT_REAL. The GeoParquet carries a real
    `iucn_status` / `redlistCategory` pair, so the threatened-species breakdown is now derivable.

AoH is not a range map. It refines IUCN range polygons down to habitat that is actually suitable,
following Brooks et al. (2019): extant and native range only, masked to suitable habitat classes
from the Red List assessment, then cut by species-specific elevation limits. So this answers
"could this species live here", not "has it been recorded here" -- which is 2.5's question, from a
completely different source. The two will not agree and are not meant to.

Presence, not abundance. A single suitable pixel puts a species on this list.

THE FOOTPRINT IS A PREFILTER, NOT THE ANSWER. The inventory geometry is each raster's BOUNDING
BOX, so a footprint can intersect the AOI while the species' actual habitat does not: 383
footprints intersect the Indonesian test AOI and only 193 of those have a DN-1 pixel inside it.
The notebook's `if pixel_count == 0: continue` is what drops the rest, and it is load-bearing.

GEODESIC AREA. The rasters are EPSG:4326, where a pixel's ground area shrinks as latitude rises,
so a single nominal cell size would be wrong across a tall AOI. `pixel_area_by_row` measures one
pixel per raster row on the WGS84 ellipsoid and the habitat area is the row areas weighted by the
count of habitat pixels in each row. The notebook's own function, unchanged.

What this port changes at the seam, and nowhere else:
  - it takes the prepared AOI rather than reading a shapefile path;
  - the inventory and the rasters follow V3_BUCKET like every other layer, so the GeoParquet is
    fetched over HTTP into memory (227 KB) because `gpd.read_parquet` will not take an https URL;
  - the per-species loop runs on a thread pool and GDAL's sidecar probing is turned off, see
    AOH_MAX_WORKERS and AOH_GDAL_OPTIONS. Each raster is still opened, masked and tested exactly
    as the notebook does it; only the waiting overlaps and the wasted lookups stop.
  - the notebook's per-species `print` progress and its printed summary tables are dropped.

NOTEBOOK BUG carried around, not reproduced: the notebook's cell uses a module-level `GEOD` that
nothing in the repo ever defines -- it imports `Geod` and reads `GEOD_ELLPS` from config but never
instantiates it, so the cell raises NameError as published. `GEOD` here is
`Geod(ellps=AOH_GEOD_ELLPS)`, which is plainly what was meant.
"""

from __future__ import annotations

import io
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import geopandas as gpd
import numpy as np
import pandas as pd
import rasterio
from pyproj import Geod
from rasterio.mask import raster_geometry_mask

try:
    from ...common import AOI
    from ...config import (
        AOH_GDAL_OPTIONS,
        AOH_GEOD_ELLPS,
        AOH_INVENTORY,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        AOH_TARGET_DN,
        AOH_TAXON_FIELDS,
    )
    from ...settings import layer_path
except ImportError:  # `python habitat_area.py`: no package around it
    import pathlib
    import sys

    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))
    from common import AOI
    from config import (
        AOH_GDAL_OPTIONS,
        AOH_GEOD_ELLPS,
        AOH_INVENTORY,
        AOH_MAX_WORKERS,
        AOH_RASTER_ROOT,
        AOH_TARGET_DN,
        AOH_TAXON_FIELDS,
    )
    from settings import layer_path

GEOD = Geod(ellps=AOH_GEOD_ELLPS)

INVENTORY_COLUMNS = {
    "species", "class", "raster_path", "iucn_status", "redlistCategory", "geometry",
}


def geometry_area_ha(geometry):
    """Calculate geodesic polygon area in hectares."""
    area_m2, _ = GEOD.geometry_area_perimeter(geometry)
    return abs(area_m2) / 10000


def pixel_area_by_row(transform, height):
    """
    Calculate geodesic pixel area by raster row.

    Required for EPSG:4326 because pixel area changes with latitude.
    """
    pixel_width = abs(transform.a)
    row_areas_ha = np.zeros(height)

    for row_index in range(height):
        north = transform.f + row_index * transform.e
        south = north + transform.e
        west = transform.c
        east = west + pixel_width

        area_m2, _ = GEOD.polygon_area_perimeter(
            [west, east, east, west],
            [north, north, south, south],
        )
        row_areas_ha[row_index] = abs(area_m2) / 10000

    return row_areas_ha


def _load_inventory():
    """The species inventory, read from wherever V3_BUCKET points.

    `gpd.read_parquet` rejects an https path outright (`Unrecognized filesystem type in URI`), so
    a bucket-hosted inventory is fetched whole and parsed from memory. It is 227 KB.
    """
    path = layer_path(AOH_INVENTORY)
    if path.startswith("http"):
        with urllib.request.urlopen(path, timeout=60) as response:
            return gpd.read_parquet(io.BytesIO(response.read()))
    return gpd.read_parquet(path)


def _species_habitat(row, aoi_geometry, aoi_crs, aoi_area_ha, target_dn):
    """One species: open its raster, count DN-1 pixels inside the AOI, measure their area.

    The notebook's per-species loop body, unchanged apart from returning its row instead of
    appending to a shared list. Returns None where the notebook says `continue`, and a plain
    message where it prints a failure.
    """
    raster_path = f"{layer_path(AOH_RASTER_ROOT)}/{row['raster_path']}"

    # The Env is entered per call, not once around the pool: GDAL's configuration is THREAD-LOCAL,
    # so options set on the calling thread would not reach the workers. Scoping it here also keeps
    # it off every other component. See AOH_GDAL_OPTIONS.
    try:
        with rasterio.Env(**AOH_GDAL_OPTIONS), rasterio.open(raster_path) as src:

            # AOI geometry in raster CRS.
            if src.crs != aoi_crs:
                raster_aoi = (
                    gpd.GeoSeries([aoi_geometry], crs=aoi_crs).to_crs(src.crs).iloc[0]
                )
            else:
                raster_aoi = aoi_geometry

            # Get only the AOI raster window.
            try:
                outside_mask, transform, window = raster_geometry_mask(
                    src,
                    [raster_aoi.__geo_interface__],
                    crop=True,
                    all_touched=False,
                )
            except ValueError:
                return None, None      # footprint intersects, raster does not

            # Read only the AOI portion.
            data = src.read(1, window=window, masked=True)
            valid_pixels = ~outside_mask & ~np.ma.getmaskarray(data)

            # Detect DN == 1 habitat.
            presence_mask = valid_pixels & (data.data == target_dn)
            pixel_count = int(presence_mask.sum())

            # GeoParquet footprint overlaps, but actual DN 1 habitat does not.
            if pixel_count == 0:
                return None, None

            # Calculate habitat area.
            if src.crs.to_epsg() == 4326:
                row_area_ha = pixel_area_by_row(transform, data.shape[0])
                pixels_per_row = presence_mask.sum(axis=1)
                habitat_area_ha = float(np.sum(pixels_per_row * row_area_ha))
            else:
                # Valid when projected CRS units are metres.
                pixel_area_ha = abs(transform.a * transform.e) / 10000
                habitat_area_ha = pixel_count * pixel_area_ha

            # Calculate percentage of AOI.
            percentage_aoi = habitat_area_ha / aoi_area_ha * 100 if aoi_area_ha > 0 else 0

    except rasterio.errors.RasterioIOError as error:
        # The exception TYPE only: this string reaches the browser through `error_status`, and
        # rasterio puts the full /vsicurl URL in its message.
        return None, f"{row['raster_path']} ({type(error).__name__})"

    return {
        "species": row["species"],
        "class": row["class"],
        "iucn_status": row["iucn_status"],
        "redlistCategory": row["redlistCategory"],
        "pixel_count": pixel_count,
        "habitat_area_ha": round(habitat_area_ha, 2),
        "percentage_aoi": round(percentage_aoi, 2),
    }, None


def analyze_biodiversity(aoi: AOI, target_dn: int = AOH_TARGET_DN) -> dict:
    """The notebook's 2.3, on the prepared AOI rather than a shapefile path."""
    # The AOI arrives in REFERENCE_CRS; the inventory and the rasters are EPSG:4326.
    aoi_geometry = aoi.geometry.to_crs(4326).union_all()
    aoi_crs = "EPSG:4326"
    aoi_area_ha = geometry_area_ha(aoi_geometry)

    inventory = _load_inventory()
    missing_columns = INVENTORY_COLUMNS - set(inventory.columns)
    if missing_columns:
        raise KeyError(f"Missing GeoParquet columns: {sorted(missing_columns)}")

    if inventory.crs != aoi_crs:
        inventory = inventory.to_crs(aoi_crs)

    # Spatial filter of candidate species. Bounding boxes -- see the module docstring.
    candidates = inventory[inventory.intersects(aoi_geometry)].copy()

    failures: list[str] = []
    results = []

    if len(candidates):
        # The only departure from the notebook's loop: the opens overlap. Nothing about how a
        # single raster is read changes, and the table is sorted below, so order is not carried.
        with ThreadPoolExecutor(max_workers=AOH_MAX_WORKERS) as pool:
            for record, failure in pool.map(
                lambda r: _species_habitat(r, aoi_geometry, aoi_crs, aoi_area_ha, target_dn),
                (r for _, r in candidates.iterrows()),
            ):
                if record is not None:
                    results.append(record)
                if failure is not None:
                    failures.append(failure)

    result_table = pd.DataFrame(results)
    if not result_table.empty:
        result_table = result_table.sort_values(["class", "species"]).reset_index(drop=True)

    class_summary = {} if result_table.empty else result_table["class"].value_counts().to_dict()
    iucn_summary = (
        {} if result_table.empty else result_table["iucn_status"].value_counts().to_dict()
    )

    return {
        "aoi_area_ha": round(aoi_area_ha, 2),
        "inventory_species_count": len(inventory),
        "inventory_classes": set(inventory["class"]),
        "candidate_species_count": len(candidates),
        "species_count": len(result_table),
        "class_summary": class_summary,
        "iucn_summary": iucn_summary,
        "species": result_table,
        "failures": failures,
    }


def analyze_habitat_area(aoi: AOI) -> tuple[dict, dict]:
    """Component 2.3. Species with suitable habitat inside the project area."""
    flags: list[str] = []
    # A class the inventory does not publish is ABSENCE -- the count is null and always will be.
    # Rasters that could not be READ are a different thing and stay in `flags`, retryable.
    missing: list[str] = []

    try:
        result = analyze_biodiversity(aoi)
    except Exception as e:
        # The inventory is a single point of failure for the whole component: without it there are
        # no candidates to read. A zero here would claim a survey found nothing, so the counts go
        # null instead.
        flags.append(f"2.3: the species inventory could not be read ({type(e).__name__}), so no "
                     "habitat assessment was made and every species count is null, not zero.")
        results = {'narrative': "Habitat data is not available for this project area.",
                   'tables': {'species': []}, 'values': {}, 'flags': flags, 'retryable': True}
        return results, dict.fromkeys(AOH_TAXON_FIELDS.values(), None) | {
            'total_wildlife_species': None, 'habitat_species_name': [],
            'species_list': [], 'iucn_summary': None}

    species_rows = result["species"].to_dict(orient="records")
    class_summary = result["class_summary"]

    # Unreadable rasters are the one thing here a retry can fix: they are /vsicurl reads that
    # failed on the network. The other flag below -- a class the inventory does not publish -- is
    # the same answer every time, so it must not light up a retry button.
    retryable = bool(result["failures"])

    if result["failures"]:
        shown = ", ".join(result["failures"][:3])
        more = f" and {len(result['failures']) - 3} more" if len(result["failures"]) > 3 else ""
        flags.append(
            f"2.3: {len(result['failures'])} of {result['candidate_species_count']} candidate "
            f"species rasters could not be read ({shown}{more}), so their species are missing "
            "from the counts."
        )

    # Every count field the HABITAT AREA card shows. A class the inventory does not publish at all
    # is NULL, never 0: the card would otherwise read "Reptile 0 Species" on a site full of
    # reptiles, which claims a survey rather than an absent layer. A class that IS published and
    # simply has no species here is a true 0.
    published = result["inventory_classes"]
    # int() because value_counts gives numpy scalars, which json.dumps will not take.
    counts: dict[str, int | None] = {
        field: (int(class_summary.get(taxon, 0)) if taxon in published else None)
        for taxon, field in AOH_TAXON_FIELDS.items()
    }

    unpublished = sorted(t for t in AOH_TAXON_FIELDS if t not in published)
    if unpublished:
        missing.append(
            f"2.3: the species inventory publishes nothing for {', '.join(unpublished)}, so "
            f"{', '.join(AOH_TAXON_FIELDS[t] for t in unpublished)} is null rather than a count."
        )

    # The card's headline. The notebook's own `species_count`, which is the height of the table and
    # so includes any class the endpoint has no field for.
    total = result["species_count"]

    # Underscored in the data (Aonyx_cinereus); the readable form is what a card shows. The
    # notebook's own form is kept in the table below.
    species_names = sorted(row["species"].replace("_", " ") for row in species_rows)

    # The per-species rows a report's species annex needs: name, class and both IUCN fields the
    # inventory carries. Already sorted by class then species. The area/pixel columns stay in
    # `results['tables']` -- they size the table without serving any card or template field.
    species_list = [
        {
            'scientific_name': row["species"].replace("_", " "),
            'taxon_class': row["class"],
            'iucn_status': row["iucn_status"],
            'redlist_category': row["redlistCategory"],
        }
        for row in species_rows
    ]

    values = {
        'aoi_area_ha': result["aoi_area_ha"],
        'inventory_species_count': result["inventory_species_count"],
        'candidate_species_count': result["candidate_species_count"],
        'species_count': result["species_count"],
        'class_summary': class_summary,
        'iucn_summary': result["iucn_summary"],
    }

    if not species_names:
        results = {
            'narrative': "No species in the Area of Habitat inventory have suitable habitat in "
                         "this project area.",
            'tables': {'species': []},
            'values': values,
            'flags': flags,
            'missing': missing,
            'retryable': retryable,
        }
        return results, dict(counts, total_wildlife_species=total, habitat_species_name=[],
                             species_list=[], iucn_summary={})

    named = ", ".join(species_names[:5])
    more = f", and {len(species_names) - 5} more" if len(species_names) > 5 else ""
    results = {
        'narrative': (
            f"The selected area contains suitable habitat for {total} species, including "
            f"{named}{more}."
        ),
        'tables': {'species': species_rows},
        'values': values,
        'flags': flags,
        'missing': missing,
        'retryable': retryable,
    }

    # The HABITAT AREA card: four counts and their total, plus the species rows and the IUCN
    # category counts for report templates. `iucn_summary` counts `iucn_status`, per the
    # notebook's own value_counts; int() there already made its values JSON-safe. 2.5's recorded
    # occurrences are a separate card and share no field with these -- see the module docstring.
    return results, dict(counts, total_wildlife_species=total, habitat_species_name=species_names,
                         species_list=species_list,
                         iucn_summary={k: int(v) for k, v in result["iucn_summary"].items()})


if __name__ == "__main__":
    # Run this component on its own, no Flask app and no database:
    #     python habitat_area.py [aoi path]
    import json
    import os
    import sys
    import time

    try:
        from common import prepare_aoi, to_jsonable
    except ImportError:
        from ...common import prepare_aoi, to_jsonable

    aoi_path = sys.argv[1] if len(sys.argv) > 1 else r"D:\Documents\ALL\_test\nbs\AOI1.shp"
    if aoi_path.lower().endswith(".zip"):
        aoi_path = "zip://" + os.path.abspath(aoi_path).replace("\\", "/")

    aoi = prepare_aoi(gpd.read_file(aoi_path))
    t0 = time.perf_counter()
    results, view_results = analyze_habitat_area(aoi)
    elapsed = time.perf_counter() - t0

    def dump(title: str, payload) -> None:
        print(f"=== {title} ===")
        print(json.dumps(to_jsonable(payload), indent=2, ensure_ascii=False))

    print(f"AOI: {aoi.area_ha:,.0f} ha, supplied in {aoi.source_crs}  [{elapsed:.1f}s]\n")
    dump("results", {**results, 'tables': {'species': results['tables']['species'][:8]}})
    print()
    dump("view_results", {**view_results,
                          'habitat_species_name': view_results['habitat_species_name'][:8]})
