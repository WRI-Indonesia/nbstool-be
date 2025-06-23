# application/apis/geo_apis/current_condition.py
from flask import jsonify, request, make_response, current_app, g as g_var
from ... import db
from ...models.geos_models.models import Polygons, MapExplorer, DataAnalyzer
from ...models.user_models.models import User, SessionsAuth

import gc
import time

from ...utils.common import AppMessageException
from ...utils.common import get_date
from . import GeoUtils

from . import gcs

from .country_specific.indonesia import Social as indonesia_social
from .country_specific.vietnam import Social as vietnam_social
from .country_specific.philippines import Social as philippines_social
from .country_specific.thailand import Social as thailand_social
from .country_specific.malaysia import Social as malaysia_social

# libraries -begin-
import os, shutil, pathlib, json, math, rasterio, geopandas as gpd, numpy as np, string, matplotlib, matplotlib.pyplot as plt, contextily as cx, pyproj
matplotlib.use('agg')

from http import HTTPStatus
from flask import current_app, jsonify
from sqlalchemy import create_engine, text, CursorResult
from rasterio.mask import mask
from rasterio.warp import calculate_default_transform, reproject, Resampling
from shapely.geometry import Polygon, MultiPolygon, shape
from shapely.geometry.point import Point
from pyproj import CRS
from math import cos, pi
from matplotlib_scalebar.scalebar import ScaleBar
from geo_northarrow import add_north_arrow
from mpl_toolkits.axes_grid1.inset_locator import inset_axes
from matplotlib.patches import Polygon as poly_patches
from mpl_toolkits.basemap import Basemap

from application.apis.geo_apis.utils import GeoLogic
# libraries -end-

# list of data sources, paths and static variables -begin-
elevation_path = "https://storage.googleapis.com/assets-geo/baseline/srtm_elevation.tif"
land_cover_raster_path = "https://storage.googleapis.com/assets-geo/baseline/lc_ipcc_mosaicked.tif"
fcc_path = "https://storage.googleapis.com/assets-geo/baseline/fcc123_v3.tif"
lc_path = "https://storage.googleapis.com/assets-geo/baseline/lc_ipcc_mosaicked.tif"
peat_path = "https://storage.googleapis.com/assets-geo/baseline/peatland_xu.tif"
hist_def_path = "https://storage.googleapis.com/assets-geo/baseline/historical_deforestation.tif"
def_risk_path = "https://storage.googleapis.com/assets-geo/baseline/prob_defrisk_v3.tif"
def_driver_path = "https://storage.googleapis.com/assets-geo/baseline/driver_of_deforestation_v2.tif"
drought_path = "https://storage.googleapis.com/assets-geo/baseline/sea_drought_risk.tif"
flood_path = "https://storage.googleapis.com/assets-geo/baseline/sea_flood_risk.tif"
landslide_path = "https://storage.googleapis.com/assets-geo/baseline/sea_landslide_risk.tif"
cyclonic_path = "https://storage.googleapis.com/assets-geo/baseline/sea_cyclonic_risk.tif"

flii_path = "https://storage.googleapis.com/assets-geo/baseline/flii.tif"
end_trees_path = "https://storage.googleapis.com/assets-geo/baseline/tree_species_richness.tif"

tcl_path = "https://storage.googleapis.com/assets-geo/baseline/scl_panthera_tigris.tif"

temp_path = "https://storage.googleapis.com/assets-geo/baseline/annual_mean_temperature.tif"
prec_path = "https://storage.googleapis.com/assets-geo/baseline/annual_precipitation.tif"
cur_c_path = "https://storage.googleapis.com/assets-geo/baseline/Base_Cur_AGB_BGB_SOC_MgCha_500m_4326.tif"
aboveground_path = 'https://storage.googleapis.com/assets-geo/baseline/ESA_CCI_BIOMASS_2020.tif'
burned_path = "https://storage.googleapis.com/assets-geo/baseline/2024-05-14_MCD64A1_BurnArea_2011_2020_Frequency_EPSG4326.tif"

wyield_path = "https://storage.googleapis.com/assets-geo/baseline/water_yield_stacked.tif"

soil_carbon_paths = [
    "https://storage.googleapis.com/assets-geo/baseline/soil_carbon_stock1_t_ha.tif",
    "https://storage.googleapis.com/assets-geo/baseline/soil_carbon_stock2_t_ha.tif",
    "https://storage.googleapis.com/assets-geo/baseline/soil_carbon_stock3_t_ha.tif",
    "https://storage.googleapis.com/assets-geo/baseline/soil_carbon_stock4_t_ha.tif",
    "https://storage.googleapis.com/assets-geo/baseline/soil_carbon_stock5_t_ha.tif"
]

elevation_classes = {
    1: 'Lowland',
    2: 'Hill',
    3: 'Sub-montane',
    4: 'Lower montane',
    5: 'Middle montane',
    6: 'Upper montane',
    7: 'Subalpine'
}

land_cover_classes = {
    1: 'Forest Land',
    2: 'Grassland',
    3: 'Cropland',
    4: 'Wetlands',
    5: 'Built-up',
    6: 'Other Land'
}

land_cover_classes_icon = {
    1: 'built-up.svg',
    2: 'cropland.svg',
    3: 'natural-forest.svg',
    4: 'tree-cover.svg',
    5: 'plantation.svg',
    6: 'short-vegetation.svg',
    7: 'sparse-vegetation.svg',
    8: 'surface-water.svg'
}

def_driver_classes = {
    0: 'No data',
    1: 'Wildfire',
    2: 'Illegal logging',
    3: 'Mining',
    4: 'Agriculture',
    5: 'Settlement & infrastructure',
    6: 'Forestry (Logging)',
    7: 'Plantation',
    8: 'ULC Plantation',
    9: 'Other'
}

units = {
    0: '', # less than 1,000 no unit prefix
    1: 'k', # kilo
    2: 'M', # Mega
    3: 'G', # Giga
    4: 'T', # Terra
    4: 'P' # Petta
}

tcl_classes = {
    1: 'scl_restoration',
    2: 'scl_restoration_fragment',
    3: 'scl_species',
    4: 'scl_species_fragment',
    5: 'scl_survey',
    6: 'scl_survey_fragment'
}

tcl_classes_plain = {
    1: 'Restoration',
    2: 'Restoration Fragment',
    3: 'Species',
    4: 'Species Fragment',
    5: 'Survey',
    6: 'Survey Fragment'
}

available_intervention_types = ["Avoided deforestation", "Ecosystem restoration"]

temp_file_path = "temp_file/"
# list of data sources, paths and static variables -end-

# pre-fetch
def rasterio_fetch(path):
    with rasterio.open(path) as src:
        src

def prefetch():
    for path in (
        elevation_path, land_cover_raster_path, fcc_path, lc_path, peat_path, 
        hist_def_path, def_risk_path, def_driver_path, drought_path, flood_path, 
        landslide_path, cyclonic_path, flii_path, end_trees_path, tcl_path, 
        temp_path, prec_path, cur_c_path, aboveground_path, burned_path, wyield_path
        ):
        rasterio_fetch(path)

    for path in soil_carbon_paths:
        rasterio_fetch(path)

    for month in range(1, 13):
        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/temperature_{month}.tif"
        rasterio_fetch(raster_path)

        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/precipitation_{month}.tif"
        rasterio_fetch(raster_path)

    for year in range(2011, 2021):
        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/burned_area_{year}.tif"
        rasterio_fetch(raster_path)

prefetch()

# general functions -begin-
def remove_process_folder(session_id: str, section: str):
    user_folder = pathlib.Path(temp_file_path, session_id, section).resolve()

    if os.path.isdir(user_folder):
        shutil.rmtree(user_folder)

def string_number_to_int(str_number):
    number = 0
    try:
        number = int(str_number)
    except:
        pass
    return number

def string_number_to_float(str_number):
    number = 0
    try:
        number = float(str_number)
    except:
        pass
    return number

def number_to_human_readable(number, postfix):
    pow = math.floor(math.log(number) / math.log(1000))
    pow = min(pow, len(units) - 1)
    number /= (10 ** (3 * pow))

    number_str = f"{number:,.3f} {units[pow]}{postfix}"

    return number_str

def construct_polygon(session_id: str) -> gpd.GeoDataFrame:
    geom = Polygons.get_geometry(session_id).first()
    res = json.loads(geom[0])
    polygon_types = res['type']
    geom_coords = res['coordinates']
    geom = False
    
    if polygon_types.lower() == "multipolygon":
        geom = shape(res)
    elif polygon_types.lower() == "polygon":
        geom = Polygon(geom_coords[0])

    gdf4326 = gpd.GeoDataFrame(index=[0], crs='epsg:4326', geometry=[geom])
    
    return gdf4326

def clip_raster_to_aoi(aoi, raster_path, output_path, print_log=False):
    os.makedirs(output_path.parent, exist_ok=True)

    try:
        # Read raster
        start_section_time = time.time()
        with rasterio.open(raster_path) as src:
            out_image, out_transform = mask(src, aoi.geometry, crop=True)
            out_meta = src.meta.copy()
            out_meta.update({
                "driver": "GTiff",
                "height": out_image.shape[1],
                "width": out_image.shape[2],
                "transform": out_transform
            })
        if print_log:
            g_var.__print_list__.append("--- %s seconds --- clip_raster_to_aoi read n mask results" % (time.time() - start_section_time))
            pass
        
        # Save clipped raster
        start_section_time = time.time()
        with rasterio.open(output_path, "w", **out_meta) as dest:
            dest.write(out_image)
        if print_log:
            g_var.__print_list__.append("--- %s seconds --- clip_raster_to_aoi save clipped raster results" % (time.time() - start_section_time))
            pass
    except Exception as e:
        out_image = ""
        out_meta = ""

    return out_image, out_meta

def reproject_raster(input_raster, output_raster, dst_crs):
    with rasterio.open(input_raster) as src:
        transform, width, height = calculate_default_transform(
            src.crs, dst_crs, src.width, src.height, *src.bounds)
        kwargs = src.meta.copy()
        kwargs.update({
            'crs': dst_crs,
            'transform': transform,
            'width': width,
            'height': height
        })

        with rasterio.open(output_raster, 'w', **kwargs) as dst:
            for i in range(1, src.count + 1):
                reproject(
                    source=rasterio.band(src, i),
                    destination=rasterio.band(dst, i),
                    src_transform=src.transform,
                    src_crs=src.crs,
                    dst_transform=transform,
                    dst_crs=dst_crs,
                    resampling=Resampling.nearest)

def reclassify_forest(raster_path, output_raster_path, x):
    with rasterio.open(raster_path) as src:
        band = src.read(1)
        band_rec = np.where(band >= x, 1, 0)
        out_meta = src.meta.copy()
    with rasterio.open(output_raster_path, 'w', **out_meta) as dest:
        dest.write(band_rec, 1)

def reclassify_forest_eligibility(raster_path, output_raster_path, x, y):
    with rasterio.open(raster_path) as src:
        band = src.read(1, masked = True)
        band_rec = np.where((band == x) | (band == y), 1, 0)
        out_meta = src.meta.copy()
        out_meta.update({
            "nodata": 255
        })
    with rasterio.open(output_raster_path, 'w', **out_meta) as dest:
        dest.write(band_rec, 1)


def calculate_forest_area(raster_path):
    with rasterio.open(raster_path) as src:
        band = src.read(1)
        # Calculate area per pixel in hectares (10,000 square meters)
        area_per_pixel = src.res[0] * src.res[1] / 10000
        # Calculate total forest area
        forest_area = np.sum(band == 1) * area_per_pixel
    return forest_area

def calculate_deforestation_rate(area_1, area_2, t1, t2):
    # Function to calculate annual deforestation rate; formula by Puyravaud (2002)
    if area_1 > 0 and area_2 > 0:
        rate = (1 / (t2 - t1)) * math.log(area_2 / area_1) * 100
    elif area_1 > 0 and area_2 == 0:
        rate = (1 / (t2 - t1)) * math.log(0.1 / area_1) * 100
    else:
        rate = 999

    return rate

def calculate_stats_pixel_value(raster_path):
    if raster_path.is_file():
        with rasterio.open(raster_path) as src:
            array = src.read(1, masked=True)
            no_data_val = src.nodata

            if not array.mask.all():
                array = array[array != no_data_val]

                if array.size > 0:
                    average_pixel_value = array.mean()
                    min_pixel_value = array.min()
                    max_pixel_value = array.max()
                    sum_pixel_value = array.sum()
                    count_pixel_value = array.count()
                else:
                    average_pixel_value = 0
                    min_pixel_value = 0
                    max_pixel_value = 0
                    sum_pixel_value = 0
                    count_pixel_value = 0
            else:
                average_pixel_value = 0
                min_pixel_value = 0
                max_pixel_value = 0
                sum_pixel_value = 0
                count_pixel_value = 0
    else:
        average_pixel_value = 0
        min_pixel_value = 0
        max_pixel_value = 0
        sum_pixel_value = 0
        count_pixel_value = 0
        
    return average_pixel_value, min_pixel_value, max_pixel_value, sum_pixel_value, count_pixel_value

def clip_stacked_raster(aoi, raster_path, output_path):
    os.makedirs(output_path.parent, exist_ok=True)

    aoi_proj = gpd.GeoDataFrame(geometry=[aoi.unary_union], crs="EPSG:4326").to_crs("ESRI:54034")

    # Open the stacked raster
    with rasterio.open(raster_path) as src:
        # Clip the raster with the AOI
        out_image, out_transform = mask(src, aoi_proj.geometry, crop=True)
        out_meta = src.meta.copy()

        # Update the metadata to match the new dimensions
        out_meta.update({
            "driver": "GTiff",
            "height": out_image.shape[1],
            "width": out_image.shape[2],
            "transform": out_transform
        })

        # Write the clipped stacked raster to a new file
        with rasterio.open(output_path, 'w', **out_meta) as dest:
            dest.write(out_image)

def reclassify_arr(fc_start_raster_path, lc_raster_path, output_raster_path):
    with rasterio.open(fc_start_raster_path) as src, rasterio.open(lc_raster_path) as src1:
        fc_start = src.read(1, masked = True)
        lc = src1.read(1, masked = True)

        # reclassify land cover >> agriculture, grassland, other land
        # arr_lc_eli = np.where(((lc == 2) | (lc == 3) | (lc == 6)), 1, 0)
        # reclassify land cover >> grassland and other land (for natural regeneration)
        arr_lc_eli = np.where(((lc == 2) | (lc == 6)), 1, 0)
        arr_fc_eli = np.where(fc_start == 0, 1, 0)
        arr_footprint = np.where((arr_lc_eli == 1) & (arr_fc_eli == 1), 1, 0)
        out_meta = src.meta.copy()
    with rasterio.open(output_raster_path, 'w', **out_meta) as dest:
        dest.write(arr_footprint, 1)

# general functions -end-

# site information calculation functions -begin-
def get_site_information_administrative_boundaries_data(aoi: str) -> dict:
    g_var.__print_list__.append(aoi)
    output = dict()

    start_section_time = time.time()
    # 0.04s query
    query = text("select project_area, district, province, country from sea.v1_current_condition_highlight_site_information_adm('{aoi_geom}');".format(aoi_geom=aoi))
    adm_highlight = GeoUtils.get_db(query)
    # g_var.__print_list__.append("--- %s seconds --- adm_highlight query results" % (time.time() - start_section_time))

    for row in adm_highlight:
        project_area_size = row["project_area"]

        output['project_area'] = f"{project_area_size:,.2f}"
        output['district'] = row["district"]
        output['province'] = row["province"]
        output['country'] = row["country"]

    start_section_time = time.time()
    # karna setiap query open and close connection, maka cache di db tidak berfungsi, jadi setiap query ke gist index akan lama ngeloadnya ~5s
    # dgn ganti where ST_Intersects(elr.geom, aoi.geom) menjadi where elr.geom && aoi.geom dari ~5s jadi ~0.05s
    # before ~1-5s query, after ~0.05S query
    query = text("select adm_bound, forest_cover, forest_pct, protect_pct from sea.v1_current_condition_detail_site_information_adm('{aoi_geom}');".format(aoi_geom=aoi)) # need pre-warm
    adm_detail = GeoUtils.get_db(query)
    # g_var.__print_list__.append("--- %s seconds --- adm_detail query results" % (time.time() - start_section_time))

    for row in adm_detail:
        output['adm_bound'] = row["adm_bound"]
        output['forest_cover'] = row["forest_cover"]
        output['forest_pct'] = row["forest_pct"]
        output['protect_pct'] = row["protect_pct"]

    return output

def get_site_information_elevation_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_elevation_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_elevation.tif").resolve()
    clipped_reprojected_elevation_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_reprojected_elevation.tif").resolve()

    # Step 1: Clip the elevation raster layer based on the AOI
    clip_raster_to_aoi(aoi, elevation_path, clipped_elevation_path)

    # Step 2: Convert CRS of clipped elevation raster layer to ESRI:54034
    reproject_raster(clipped_elevation_path, clipped_reprojected_elevation_path, "ESRI:54034")

    # Step 3: Calculate the area in hectares of each elevation class and its percentage of total area
    with rasterio.open(clipped_reprojected_elevation_path) as src:
        band = src.read(1)
        unique, counts = np.unique(band, return_counts=True)
        elevation_counts = dict(zip(unique, counts))
        # Calculate area per pixel in hectares (10,000 square meters)
        area_per_pixel = src.res[0] * src.res[1] / 10000
        total_area = np.sum(band > 0) * area_per_pixel
        elevation_areas = {elevation_classes[key]: value * area_per_pixel for key, value in elevation_counts.items() if key in elevation_classes}
        elevation_percentages = {elevation_classes[key]: (value * area_per_pixel / total_area) * 100 for key, value in elevation_counts.items() if key in elevation_classes}

    # Step 4: Sort the elevation class from largest to smallest area
    sorted_elevation = sorted(elevation_areas.items(), key=lambda item: item[1], reverse=True)
    elevation_list = [i[0] for i in sorted_elevation]

    # Step 5: Print the results
    result_str = ""
    result_str += ", ".join([f"{elevation} ({elevation_percentages[elevation]:.2f}%)" for elevation, area in sorted_elevation])

    elevation_detail = []

    for elevation, area in sorted_elevation:
        output_temp = dict()

        output_temp['elevation_class'] = elevation
        output_temp['elevation_pct'] = round(elevation_percentages[elevation], 2)

        elevation_detail.append(output_temp)

    output = dict()

    output["number_of"] = len(sorted_elevation)
    output["elevation_class"] = result_str
    output["elevation_list"] = elevation_list
    output["elevation_detail"] = elevation_detail

    return output

def get_site_information_land_cover_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_lc_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_lc.tif").resolve()
    clipped_reprojected_lc = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_reprojected_lc.tif").resolve()

    # Step 1: Clip the land cover raster layer based on the AOI
    clip_raster_to_aoi(aoi, land_cover_raster_path, clipped_lc_path)

    # Step 2: Convert CRS of clipped land cover raster layer to ESRI:54034
    reproject_raster(clipped_lc_path, clipped_reprojected_lc, "ESRI:54034")

    # Step 3: Calculate the area in hectares of each land cover class and its percentage of total area
    with rasterio.open(clipped_reprojected_lc) as src:
        band = src.read(1)
        unique, counts = np.unique(band, return_counts=True)
        land_cover_counts = dict(zip(unique, counts))
        # Calculate area per pixel in hectares (10,000 square meters)
        area_per_pixel = src.res[0] * src.res[1] / 10000
        total_area = np.sum(band > 0) * area_per_pixel
        land_cover_areas = {land_cover_classes[key]: value * area_per_pixel for key, value in land_cover_counts.items() if key in land_cover_classes}
        land_cover_icon = {land_cover_classes[key]: land_cover_classes_icon[key] for key, value in land_cover_counts.items() if key in land_cover_classes}
        land_cover_percentages = {land_cover_classes[key]: (value * area_per_pixel / total_area) * 100 for key, value in land_cover_counts.items() if key in land_cover_classes}

    # Step 4: Sort the land cover class from largest to smallest area
    sorted_land_cover = sorted(land_cover_areas.items(), key=lambda item: item[1], reverse=True)

    lc_data = []
    output = dict()
    forest_land = dict()

    for land_cover, area in sorted_land_cover:
        output_temp = dict()

        output_temp['lc_class'] = land_cover
        output_temp['lc_icon'] = land_cover_icon[land_cover]
        output_temp['area_ha'] = f"{area:,.1f}"
        output_temp['area_pct'] = f"{land_cover_percentages[land_cover]:.2f}"

        if land_cover == "Forest Land":
            forest_land["area_ha"] = f"{area:,.1f}"
            forest_land["area_pct"] = f"{land_cover_percentages[land_cover]:.2f}"

        lc_data.append(output_temp)

    output["land_cover"] = lc_data
    output["forest_land"] = forest_land

    return output

def get_site_information_peatland_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_peat_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_peat.tif").resolve()

    # Calculate the total area of the AOI in hectares
    aoi_reproject = gpd.GeoDataFrame(geometry=[aoi.unary_union], crs="EPSG:4326").to_crs("ESRI:54034")
    total_aoi_area_ha = aoi_reproject.geometry.area.sum() / 10000

    # Clip peatland raster layer based on AOI
    clip_raster_to_aoi(aoi, peat_path, clipped_peat_path)

    # Calculate peatland statistics
    peat_stats = calculate_stats_pixel_value(clipped_peat_path)
    # peat_sum = peat_stats[3] * clipped_peat_path.res[0] * clipped_peat_path.res{1] / 10000
    peat_sum = peat_stats[3]
    peat_stats_pct = peat_sum / total_aoi_area_ha * 100

    output = dict()

    output['peatland'] = float(round(peat_sum, 2))
    output['peatland_pct'] = float(round(peat_stats_pct, 2))

    return output

def get_site_information_mangrove_data(aoi: str) -> dict:
    query = text("select mangrove, mangrove_pct from sea.v1_current_condition_detail_site_information_pm('{aoi_geom}');".format(aoi_geom=aoi))
    result = GeoUtils.get_db(query)

    output = dict()

    for row in result:
        output['mangrove'] = row['mangrove']
        output['mangrove_pct'] = row['mangrove_pct']

    return output

def get_site_information_peatland_mangrove_data(session_id: str, aoi: dict) -> dict:
    geom = aoi["geom"]
    geom_gdf = aoi["geom_gdf"]

    peatland = get_site_information_peatland_data(session_id, geom_gdf)
    mangrove = get_site_information_mangrove_data(geom)

    output = dict()

    output['peatland'] = peatland['peatland']
    output['peatland_pct'] = peatland['peatland_pct']
    output['mangrove'] = mangrove['mangrove']
    output['mangrove_pct'] = mangrove['mangrove_pct']

    return output

def get_historical_deforestation_graph_data(session_id, aoi):
    c_hist_def_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_historical_deforestation.tif").resolve()

    clip_raster_to_aoi(aoi, hist_def_path, c_hist_def_path)

    # Load the Raster Data
    with rasterio.open(c_hist_def_path) as src:
        raster_array = src.read(1)  # Read the first band

    # Calculate Frequency of Each Value
    values, counts = np.unique(raster_array, return_counts=True)
    deforestation_data = dict(zip(values, counts))

    # Filter the data to include only years 2001 to 2020 (values 1 to 20) and convert to area
    deforestation_area = {year: deforestation_data.get(year, 0) * 30 * 30 / 1000000 for year in range(2, 21)}

    # Prepare the Data for Plotting
    years = range(2010, 2021)  # Years from 2011 to 2020
    areas = [deforestation_area.get(year-2000, 0) for year in years]

    return areas

def get_site_information_historical_deforestation_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_fcc_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_fcc_123.tif").resolve()
    clipped_reprojected_fcc_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_reprojected_fcc_123.tif").resolve()
    fc_before_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_fc_before.tif").resolve()
    fc_after_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_fc_after.tif").resolve()

    year_before = 2010
    year_after = 2020
    year_gap = year_after - year_before

    # Step 1: Clip the raster layers
    clip_raster_to_aoi(aoi, fcc_path, clipped_fcc_path)

    # Step 2: Reproject the clipped raster layers
    reproject_raster(clipped_fcc_path, clipped_reprojected_fcc_path, "ESRI:54034")

    # Step 3: Calculate the forest cover area in hectares
    reclassify_forest(clipped_reprojected_fcc_path, fc_before_path, 2)
    reclassify_forest(clipped_reprojected_fcc_path, fc_after_path, 3)
    forest_area_before = calculate_forest_area(fc_before_path)
    forest_area_after = calculate_forest_area(fc_after_path)

    # Step 4: Calculate the annual forest cover change in hectares per year
    annual_change = (forest_area_after - forest_area_before) / year_gap  # 2020 - 2010 = 10 years
    change = forest_area_before - forest_area_after

    # Step 5: Calculate the annual deforestation rate
    deforestation_rate = calculate_deforestation_rate(forest_area_before, forest_area_after, year_before, year_after)

    output = dict()

    if deforestation_rate == 999:
        output["text"] = f"The project area you specified had no forest cover in " + year_before + ", therefore the deforestation rate calculation cannot be applied."
        output["number"] = 0
        output["pct"] = 0
        output["graph_data"] = ""
    else:
        output["text"] = f"This project location has experienced forest loss of up to {change:,.1f} ha in the past 10 years, with the annual historical rate of deforestation of {deforestation_rate:.2f}% ({annual_change:,.2f} ha/year)"
        output["change"] = f"{change:,.1f}"
        output["pct"] = round(deforestation_rate, 2)
        output["annual"] = f"{annual_change:,.1f}"
        output["graph_data"] = get_historical_deforestation_graph_data(session_id, aoi)

    return output

def get_site_information_deforestation_risk(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_defrisk_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_defrisk.tif").resolve()

    # 1. Clip the raster layer based on AOI
    clipped_image, clipped_meta = clip_raster_to_aoi(aoi, def_risk_path, clipped_defrisk_path)

    # Divide pixel values by 65535
    clipped_image = clipped_image / 65535.0

    # 2. Calculate the average and median from all pixels in the clipped raster
    # Remove the no data values from the calculation
    no_data_value = clipped_meta.get('nodata')
    if no_data_value is not None:
        data = clipped_image[clipped_image != no_data_value]
    else:
        data = clipped_image

    average_value = np.mean(data)

    # 3. Determine the deforestation risk class
    deforestation_risk_class = ""
    if average_value < 0.3:
        deforestation_risk_class = "Low"
    elif 0.3 <= average_value < 0.7:
        deforestation_risk_class = "Moderate"
    elif average_value >= 0.7:
        deforestation_risk_class = "High"

    message = {
        "Low": "The selected area has a low average risk of future deforestation.",
        "Moderate": "The selected area has a moderate average risk of future deforestation.",
        "High": "The selected area has a high average risk of future deforestation."
    }

    output = dict()

    output["risk_type"] = f"{deforestation_risk_class} Risk Level"
    output["risk_type_long"] = f"{message[deforestation_risk_class]}"

    return output

def get_site_information_driver_of_deforestation(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_def_driver_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_def_drivers.tif").resolve()
    clipped_reprojected_def_driver_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_reprojected_def_drivers.tif").resolve()

    # Step 1: Clip the deforestation driver raster layer based on the AOI
    clip_raster_to_aoi(aoi, def_driver_path, clipped_def_driver_path)

    # Step 2: Convert CRS of clipped deforestation driver land cover raster layer to ESRI:54034
    reproject_raster(clipped_def_driver_path, clipped_reprojected_def_driver_path, "ESRI:54034")

    # Step 3: Calculate the area in hectares of each deforestation driver land cover class and its percentage of total area
    with rasterio.open(clipped_reprojected_def_driver_path) as src:
        band = src.read(1)
        unique, counts = np.unique(band, return_counts=True)
        driver_counts = dict(zip(unique, counts))

        # remove 'No Data' or 'No Deforestation'
        del driver_counts[0]
        
        # Calculate area per pixel in hectares (10,000 square meters)
        area_per_pixel = src.res[0] * src.res[1] / 10000
        total_area = np.sum(band > 0) * area_per_pixel
        driver_areas = {def_driver_classes[key]: value * area_per_pixel for key, value in driver_counts.items() if key in def_driver_classes}
        driver_percentages = {def_driver_classes[key]: (value * area_per_pixel / total_area) * 100 for key, value in driver_counts.items() if key in def_driver_classes}

    # Step 4: Sort the land cover class from largest to smallest area
    sorted_driver = sorted(driver_areas.items(), key=lambda item: item[1], reverse=True)

    # Step 5: Print the results
    result_str = ""
    result_str += ", ".join([f"{land_cover} ({driver_percentages[land_cover]:.2f}%)" for land_cover, area in sorted_driver])

    dod_list = []
    for land_cover, area in sorted_driver:
        temp_list = dict()

        temp_list["driver"] = land_cover
        temp_list["area"] = f"{area:,.1f} ha"
        temp_list["area_pct"] = f"{driver_percentages[land_cover]:.2f}%"

        dod_list.append(temp_list)

    output = dict()

    output["driver_text"] = result_str
    output["driver_list"] = dod_list

    return output

def get_site_information_disaster_risk(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    c_drought_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_drought.tif").resolve()
    c_flood_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_flood.tif").resolve()
    c_landslide_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_landslide.tif").resolve()    
    c_cyclone_path = pathlib.Path(temp_file_path, session_id, "site_information", "clipped_cyclone.tif").resolve()

    start_section_time = time.time()
    clip_raster_to_aoi(aoi, drought_path, c_drought_path)
    clip_raster_to_aoi(aoi, flood_path, c_flood_path)
    clip_raster_to_aoi(aoi, landslide_path, c_landslide_path)
    clip_raster_to_aoi(aoi, cyclonic_path, c_cyclone_path)
    g_var.__print_list__.append("--- %s seconds --- 4_clip_raster_to_aoi results" % (time.time() - start_section_time))

    # Calculate the Pixel Value Statistics
    start_section_time = time.time()
    drought_stats = calculate_stats_pixel_value(c_drought_path)[0]
    flood_stats = calculate_stats_pixel_value(c_flood_path)[0]
    landslide_stats = calculate_stats_pixel_value(c_landslide_path)[0]
    cyclone_stats = calculate_stats_pixel_value(c_cyclone_path)[0]
    g_var.__print_list__.append("--- %s seconds --- calculate_stats_pixel_value results" % (time.time() - start_section_time))

    # Determine the disaster risk class
    def dr_class(average_value):
        if average_value == 0:
            disaster_risk_class = "No"
        elif 0 < average_value < 0.3:
            disaster_risk_class = "Low"
        elif 0.3 <= average_value < 0.7:
            disaster_risk_class = "Moderate"
        elif average_value >= 0.7:
            disaster_risk_class = "High"
        
        return disaster_risk_class
    
    def flood_class(average_value):
        average_value = average_value / 100

        flood_risk_class = "0"

        if average_value == 0:
            flood_risk_class = "0"
        elif 0 < average_value <= 1:
            flood_risk_class = "0.1 - 1"
        elif 1 < average_value <= 3:
            flood_risk_class = "1.1 - 3"
        elif 3 < average_value <= 5:
            flood_risk_class = "3.1 - 5"
        elif 5 < average_value <= 7:
            flood_risk_class = "5.1 - 7"
        elif average_value > 7:
            flood_risk_class = "> 7"

        return flood_risk_class + ' meter'
    
    def drought_class(average_value):
        drought_risk_class = "No"

        if average_value < 1:
            drought_risk_class = "No"
        elif 1 <= average_value < 5:
            drought_risk_class = "Low"
        elif 5 <= average_value < 8:
            drought_risk_class = "Moderate"
        elif 8 <= average_value < 10:
            drought_risk_class = "High"

        return drought_risk_class
    
    def landslide_class(average_value):
        landslide_risk_class = "No"

        if average_value < 6:
            landslide_risk_class = "No"
        elif 6 <= average_value <= 8:
            landslide_risk_class = "Moderate"
        elif average_value > 8:
            landslide_risk_class = "High"

        return landslide_risk_class
    
    def cyclonic_class(average_value):
        cyclonic_risk_class = "No"
        
        if average_value < 1:
            cyclonic_risk_class = "No"
        elif 1 <= average_value < 5:
            cyclonic_risk_class = "Low"
        elif 5 <= average_value < 8:
            cyclonic_risk_class = "Moderate"
        elif average_value >= 8:
            cyclonic_risk_class = "High"
        
        return cyclonic_risk_class
    
    output = dict()

    output["floods"] = f"{flood_class(flood_stats)} risk"
    output["landslides"] = f"{landslide_class(landslide_stats)} risk"
    output["drought"] = f"{drought_class(drought_stats)} risk"
    output["cyclone"] = f"{cyclonic_class(cyclone_stats)} risk"

    return output

def get_site_information(session_id: str, section_type: str, aoi: dict) -> dict:
    geom = aoi["geom"]
    geom_gdf = aoi["geom_gdf"]

    has_lc = False

    output = dict()
    land_cover_data = dict()

    strs = []

    start_time = time.time()

    start_section_time = time.time()
    if section_type == "site_information" or section_type == "land_cover":
        land_cover_data = get_site_information_land_cover_data(session_id, geom_gdf) # 1.6340277194976807 seconds
        output['land_cover'] = land_cover_data["land_cover"]
        has_lc = True if "forest_land" in land_cover_data["land_cover"] else False
    g_var.__print_list__.append("--- %s seconds --- land_cover results" % (time.time() - start_section_time))

    start_section_time = time.time()
    if section_type == "site_information" or section_type == "administrative_boundaries":
        adm_data = get_site_information_administrative_boundaries_data(geom) # 2.008894920349121 seconds before, 0.1sec after
        g_var.__print_list__.append("--- %s seconds --- get_site_information_administrative_boundaries_data results" % (time.time() - start_section_time))

        if has_lc:
            adm_data['forest_cover'] = land_cover_data["forest_land"]["area_ha"]
            adm_data['forest_pct'] = land_cover_data["forest_land"]["area_pct"]

        start_section_time_v2 = time.time()
        construct_project_area_map(session_id, geom_gdf) # 3.8390004634857178 seconds, mostly karna loading basemap tiles
        g_var.__print_list__.append("--- %s seconds --- construct_project_area_map results" % (time.time() - start_section_time_v2))

        output['administrative_boundaries'] = adm_data
    # g_var.__print_list__.append("--- %s seconds --- administrative_boundaries results" % (time.time() - start_section_time))

    start_section_time = time.time()
    if section_type == "site_information" or section_type == "elevation":
        output['elevation'] = get_site_information_elevation_data(session_id, geom_gdf) # 1.8553152084350586 seconds
    g_var.__print_list__.append("--- %s seconds --- elevation results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    if section_type == "site_information" or section_type == "peatland_mangrove":
        output['peatland_mangrove'] = get_site_information_peatland_mangrove_data(session_id, aoi) # 1.6747007369995117 seconds
    g_var.__print_list__.append("--- %s seconds --- peatland_mangrove results" % (time.time() - start_section_time))

    start_section_time = time.time()
    if section_type == "site_information" or section_type == "annual_deforestation_rate":
        output['annual_deforestation_rate'] = get_site_information_historical_deforestation_data(session_id, geom_gdf) # 2.847438097000122 seconds
    g_var.__print_list__.append("--- %s seconds --- annual_deforestation_rate results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    if section_type == "site_information" or section_type == "deforestation_risk":
        output['deforestation_risk'] = get_site_information_deforestation_risk(session_id, geom_gdf) # 1.6045658588409424 seconds
    g_var.__print_list__.append("--- %s seconds --- deforestation_risk results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    if section_type == "site_information" or section_type == "driver_of_deforestation":
        output['driver_of_deforestation'] = get_site_information_driver_of_deforestation(session_id, geom_gdf) # 1.3783352375030518 seconds
    g_var.__print_list__.append("--- %s seconds --- driver_of_deforestation results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    if section_type == "site_information" or section_type == "disaster_risk":
        output['disaster_risk'] = get_site_information_disaster_risk(session_id, geom_gdf) # 3.4096429347991943 seconds
    g_var.__print_list__.append("--- %s seconds --- disaster_risk results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    process_input_data_analyzer_result(session_id=session_id, section="site_information", data=output) # 0.16662096977233887 seconds
    g_var.__print_list__.append("--- %s seconds --- process_input_data_analyzer_result results" % (time.time() - start_section_time))

    g_var.__print_list__.append("--- %s seconds --- site information calculation functions results" % (time.time() - start_time))

    return output # 20.41856813430786 seconds
# site information calculation functions -end-

# nature calculation functions -begin-
def get_nature_kba_data(aoi: str) -> dict:
    output = dict()

    query = text("select kba_name, area, pct from sea.v1_current_condition_detail_nature_kba('{aoi_geom}');".format(aoi_geom=aoi))
    adm_highlight = GeoUtils.get_db(query)

    output['kba_name'] = ""
    output['area_ha'] = "0"
    output['area_ha_plain'] = 0
    output['area_pct'] = 0
    output['text'] = "The selected area has not overlapped with any Key Biodiversity Area"
    
    for row in adm_highlight:
        kba_overlapped_size = row["area"]
        kba_overlapped_pct = row["pct"]

        output['kba_name'] = row["kba_name"]
        output['area_ha'] = f"{kba_overlapped_size:,.2f} ha ({kba_overlapped_pct:,.2f}%)"
        output['area_ha_plain'] = round(kba_overlapped_size, 2)
        output['area_pct'] = round(kba_overlapped_pct, 2)

        output['text'] = "The selected area is located within {kba_name} Key Biodiversity Area and has overlapped area by {kba_size} ha".format(
            kba_name = row["kba_name"],
            kba_size = kba_overlapped_size
        )

    return output

def get_nature_flii_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_flii_path = pathlib.Path(temp_file_path, session_id, "nature", "clipped_flii.tif").resolve()

    clip_raster_to_aoi(aoi, flii_path, clipped_flii_path)
    stats_flii = calculate_stats_pixel_value(clipped_flii_path)

    flii_category = ""
    flii_text = ""
    flii_index = f"{stats_flii[0]:,.1f}"

    if stats_flii[0] < 6:
        flii_category = "Low Integrity"
        flii_text = "Meaning, it may be more suitable for an ecosystem restoration project"
    elif stats_flii[0] > 6 and stats_flii[0] < 9.6:
        flii_category = "Medium Integrity"
        flii_text = "Meaning, it may be more suitable for an avoided deforestation project"
    else:
        flii_category = "High Integrity"
        flii_text = "Meaning it may be more suitable for an avoided deforestation project"

    output = dict()

    output["index"] = flii_index
    output["integrity"] = flii_category
    output["meaning"] = flii_text

    return output

def get_nature_wildlife_data(aoi: str) -> dict:
    output = dict()

    query = text("select amphibians, birds, mammals, reptiles, fish, total from sea.v1_current_condition_detail_nature_wildlife('{aoi_geom}');".format(aoi_geom=aoi))
    adm_highlight = GeoUtils.get_db(query)

    for row in adm_highlight:
        output['amphibi'] = row["amphibians"]
        output['amphibi_list'] = get_nature_wildlife_list_data(aoi, 'Amphibians')
        output['bird'] = row["birds"]
        output['bird_list'] = get_nature_wildlife_list_data(aoi, 'Birds')
        output['mammal'] = row["mammals"]
        output['mammal_list'] = get_nature_wildlife_list_data(aoi, 'Mammals')
        output['reptile'] = row["reptiles"]
        output['reptile_list'] = get_nature_wildlife_list_data(aoi, 'Reptiles')
        output['marine_fish'] = row["fish"]
        output['sum_all'] = row["total"]

    return output

def get_nature_wildlife_list_data(aoi: str, category: str) -> dict:
    output = []

    query = text("select category, binomial_name, category_iucn from sea.v1_current_condition_detail_nature_wildlife_list('{aoi_geom}', '{category}');".format(aoi_geom=aoi, category=category))
    adm_highlight = GeoUtils.get_db(query)

    for row in adm_highlight:
        output.append(row)

    return output

def get_nature_richness_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_endangered_trees = pathlib.Path(temp_file_path, session_id, "nature", "clipped_end_trees.tif").resolve()

    # Clip Endangered Trees Species Richness Layer
    clip_raster_to_aoi(aoi, end_trees_path, clipped_endangered_trees)

    # Calculate the Pixel Value Statistics
    stats_pixel_value = calculate_stats_pixel_value(clipped_endangered_trees)

    output = dict()

    output["species"] = 0 #no data
    output["endangered"] = round(stats_pixel_value[0])
    output["desc"] = f"The project area is home to {round(stats_pixel_value[0]):.0f} of endangered trees."
    
    return output

def get_nature_tiger_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_tcl_path = pathlib.Path(temp_file_path, session_id, "nature", "clipped_tcl.tif").resolve()
    clipped_reprojected_tcl_path = pathlib.Path(temp_file_path, session_id, "nature", "clipped_reprojected_tcl.tif").resolve()

    clip_raster_to_aoi(aoi, tcl_path, clipped_tcl_path)

    output = dict()

    if clipped_tcl_path.is_file():
        # Calculate the total area of the AOI in hectares
        aoi_reproject = gpd.GeoDataFrame(geometry=[aoi.unary_union], crs="EPSG:4326").to_crs("ESRI:54034")
        total_aoi_area_ha = aoi_reproject.geometry.area.sum() / 10000

        reproject_raster(clipped_tcl_path, clipped_reprojected_tcl_path, "ESRI:54034")

        # Step 3: Calculate the area in hectares of each land cover class and its percentage of total area
        with rasterio.open(clipped_reprojected_tcl_path) as src:
            band = src.read(1)
            unique, counts = np.unique(band, return_counts=True)
            tcl_counts = dict(zip(unique, counts))
            # Calculate area per pixel in hectares (10,000 square meters)
            area_per_pixel = src.res[0] * src.res[1] / 10000
            total_area = np.sum(band > 0) * area_per_pixel
            tcl_areas = {tcl_classes[key]: value * area_per_pixel for key, value in tcl_counts.items() if key in tcl_classes}
            tcl_names = {tcl_classes[key]: tcl_classes_plain[key] for key, value in tcl_counts.items() if key in tcl_classes}
            tcl_percentages = {tcl_classes[key]: (value * area_per_pixel / total_area) * 100 for key, value in tcl_counts.items() if key in tcl_classes}

        # Step 4: Sort the land cover class from largest to smallest area
        sorted_tcl = sorted(tcl_areas.items(), key=lambda item: item[1], reverse=True)

        # Step 5: Print the results
        result_str = ""
        result_str += ", ".join([f"{tcl_names[tcl_areas]} ({((area / total_aoi_area_ha) * 100):,.2f}%)" for tcl_areas, area in sorted_tcl])
        
        tcl = []
        for tcl_areas, area in sorted_tcl:
            tcl_temp = dict()

            tcl_temp['tcl_areas'] = tcl_areas
            tcl_temp['area'] = f"{area:,.2f}"

            tcl.append(tcl_temp)

        output['items'] = tcl
        output['text'] = result_str
    else:
        output['items'] = []
        output['text'] = ""

    return output

def get_nature(session_id: str, section_type: str, aoi: dict) -> dict:
    start_time = time.time()
    g_var.__print_list__.append("----------------------- nature -----------------------")
    geom = aoi["geom"]
    geom_gdf = aoi["geom_gdf"]

    output = dict()

    if section_type == "nature" or section_type == "kba":
        start_section_time = time.time()
        output['kba'] = get_nature_kba_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_nature_kba_data results" % (time.time() - start_section_time))

    if section_type == "nature" or section_type == "flii":
        start_section_time = time.time()
        output['flii'] = get_nature_flii_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_nature_flii_data results" % (time.time() - start_section_time))

    if section_type == "nature" or section_type == "wildlife":
        start_section_time = time.time()
        output['wildlife'] = get_nature_wildlife_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_nature_wildlife_data results" % (time.time() - start_section_time))
    
    if section_type == "nature" or section_type == "richness":
        start_section_time = time.time()
        output['richness'] = get_nature_richness_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_nature_richness_data results" % (time.time() - start_section_time))

    if section_type == "nature" or section_type == "tcl":
        start_section_time = time.time()
        output['tcl'] = get_nature_tiger_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_nature_tiger_data results" % (time.time() - start_section_time))

    process_input_data_analyzer_result(session_id=session_id, section="nature", data=output)

    g_var.__print_list__.append("--- %s seconds --- nature calculation functions results" % (time.time() - start_time))

    return output
# nature calculation functions -end-

# climate calculation functions -begin-
def get_climate_temperature_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    start_section_time = time.time()
    clipped_temp_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_temp.tif").resolve()

    # Clip raster layer based on AOI
    clip_raster_to_aoi(aoi, temp_path, clipped_temp_path)
    # g_var.__print_list__.append("--- %s seconds --- clip_raster_to_aoi results" % (time.time() - start_section_time))

    # Calculate the average, minimum, and maximum temperature
    start_section_time = time.time()
    stats_prec = calculate_stats_pixel_value(clipped_temp_path)
    # g_var.__print_list__.append("--- %s seconds --- calculate_stats_pixel_value results" % (time.time() - start_section_time))

    output = dict()

    output["min"] = f"{stats_prec[1]:,.1f}"
    output["max"] = f"{stats_prec[2]:,.1f}"
    output["mean"] = f"{stats_prec[0]:,.1f}"

    # Plot monthly average temperature
    # Initialize a list to hold monthly average temperature values
    monthly_averages_temp = []

    # Iterate over each monthly precipitation file
    start_section_time = time.time()
    for month in range(1, 13):
        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/temperature_{month}.tif"
        clipped_temp = pathlib.Path(temp_file_path, session_id, "climate", f"clipped_temperature_{month}.tif").resolve()

        clip_raster_to_aoi(aoi, raster_path, clipped_temp)
        
        mean_temp = round(calculate_stats_pixel_value(clipped_temp)[0], 1)
        monthly_averages_temp.append(mean_temp)
    # g_var.__print_list__.append("--- %s seconds --- monthly_averages results" % (time.time() - start_section_time))

    output["graph_data"] = monthly_averages_temp

    return output

def get_climate_precipitation_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_prec_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_prec.tif").resolve()

    # Clip raster layer based on AOI
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, prec_path, clipped_prec_path)
    # g_var.__print_list__.append("--- %s seconds --- clip_raster_to_aoi results" % (time.time() - start_section_time))

    # Calculate the average, minimum, and maximum precipitation
    start_section_time = time.time()
    stats_prec = calculate_stats_pixel_value(clipped_prec_path)
    # g_var.__print_list__.append("--- %s seconds --- calculate_stats_pixel_value results" % (time.time() - start_section_time))

    output = dict()

    output["min"] = f"{stats_prec[1]:,.1f}"
    output["max"] = f"{stats_prec[2]:,.1f}"
    output["mean"] = f"{stats_prec[0]:,.1f}"

    # Plot monthly average precipitation
    # Initialize a list to hold monthly average precipitation values
    monthly_averages = []

    # Iterate over each monthly precipitation file
    start_section_time = time.time()
    for month in range(1, 13):
        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/precipitation_{month}.tif"
        clipped_raster = pathlib.Path(temp_file_path, session_id, "climate", f"clipped_precipitation_{month}.tif").resolve()

        clip_raster_to_aoi(aoi, raster_path, clipped_raster)

        mean_precip = round(calculate_stats_pixel_value(clipped_raster)[0], 1)
        monthly_averages.append(mean_precip)
    # g_var.__print_list__.append("--- %s seconds --- monthly_averages results" % (time.time() - start_section_time))

    output["graph_data"] = monthly_averages

    return output

def get_climate_carbon_storage_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_agb_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_agb.tif").resolve()
    clipped_sc1_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_sc1.tif").resolve()
    clipped_sc2_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_sc2.tif").resolve()
    clipped_sc3_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_sc3.tif").resolve()
    clipped_sc4_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_sc4.tif").resolve()
    clipped_sc5_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_sc5.tif").resolve()

    clipped_c_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_c.tif").resolve()

    clip_raster_to_aoi(aoi, aboveground_path, clipped_agb_path)
    clip_raster_to_aoi(aoi, soil_carbon_paths[0], clipped_sc1_path)
    clip_raster_to_aoi(aoi, soil_carbon_paths[1], clipped_sc2_path)
    clip_raster_to_aoi(aoi, soil_carbon_paths[2], clipped_sc3_path)
    clip_raster_to_aoi(aoi, soil_carbon_paths[3], clipped_sc4_path)
    clip_raster_to_aoi(aoi, soil_carbon_paths[4], clipped_sc5_path)

    agb_stats = calculate_stats_pixel_value(clipped_agb_path)
    total_c_agb = agb_stats[3]
    total_c_bgb = 0.207 * total_c_agb

    sc1_stats = calculate_stats_pixel_value(clipped_sc1_path) 
    sc2_stats = calculate_stats_pixel_value(clipped_sc2_path)
    sc3_stats = calculate_stats_pixel_value(clipped_sc3_path)
    sc4_stats = calculate_stats_pixel_value(clipped_sc4_path)
    sc5_stats = calculate_stats_pixel_value(clipped_sc5_path)
    total_c_soil = sc1_stats[3] * 6.25 + sc2_stats[3] * 6.25 + sc3_stats[3] * 6.25 + sc4_stats[3] * 6.25 + sc5_stats[3] * 6.25
    total_c = total_c_agb + total_c_bgb + total_c_soil

    agb_pct = total_c_agb / total_c * 100
    bgb_pct = total_c_bgb / total_c * 100
    soil_pct = total_c_soil / total_c * 100

    clip_raster_to_aoi(aoi, cur_c_path, clipped_c_path)
    c_stats_other = calculate_stats_pixel_value(clipped_c_path)
    total_c_other = c_stats_other[3] * 25

    output = dict()

    output["carbon_storage_plain"] = f"{total_c:,.2f}"
    output["carbon_storage"] = number_to_human_readable(total_c, "t")
    output["aboveground_number"] = f"{total_c_agb:,.2f}"
    output["aboveground_percent"] = f"{agb_pct:,.2f}"
    output["belowground_number"] = f"{total_c_bgb:,.2f}"
    output["belowground_percent"] = f"{bgb_pct:,.2f}"
    output["soil_number"] = f"{total_c_soil:,.2f}"
    output["soil_percent"] = f"{soil_pct:,.2f}"
    output["other_source_plain"] = f"{total_c_other:,.2f}"
    output["other_source"] = f"Another data source (Walker et al 2022) estimates a total carbon storage of +/- {total_c_other:,.1f} tonnes."

    return output

def get_climate_burned_area_data(session_id: str, aoi: gpd.GeoDataFrame) -> dict:
    clipped_burned_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_burned.tif").resolve()
    clipped_reprojected_burned_path = pathlib.Path(temp_file_path, session_id, "climate", "clipped_reprojected_burned.tif").resolve()

    clip_raster_to_aoi(aoi, burned_path, clipped_burned_path)
    start_section_time = time.time()
    reproject_raster(clipped_burned_path, clipped_reprojected_burned_path, "ESRI:54034")
    # g_var.__print_list__.append("--- %s seconds --- clip_burned_raster results" % (time.time() - start_section_time))

    start_section_time = time.time()
    burned_stats = calculate_stats_pixel_value(clipped_reprojected_burned_path)
    # g_var.__print_list__.append("--- %s seconds --- burned_stats results" % (time.time() - start_section_time))

    burned_min_freq = burned_stats[1]
    burned_max_freq = burned_stats[2]
    burned_freq = round((burned_min_freq + burned_max_freq) / 2)

    def calculate_raster_area(raster_path):
        with rasterio.open(raster_path) as src:
            band = src.read(1)
            # Calculate area per pixel in hectares (10,000 square meters)
            area_per_pixel = src.res[0] * src.res[1] / 10000
            # Calculate total raster area
            area = np.sum(band >= 1) * area_per_pixel
        return area
    
    start_section_time = time.time()
    burned_area = calculate_raster_area(clipped_reprojected_burned_path)
    # g_var.__print_list__.append("--- %s seconds --- burned_area results" % (time.time() - start_section_time))

    if burned_area > 0:
        burn_text = f"Over the past ten years, fires have affected this project location, impacting up to {burned_area:,.1f} hectares of burned area, with an average frequency {burned_freq:,.0f} occurrences in several areas."
    else:
        burn_text = "There are no historical burned area detected within this project area over the past ten years."

    # Plot annual burned area
    # Initialize a list to hold monthly average temperature values
    annual_burned_list = []
    annual_burned_total = 0

    # Iterate over each monthly precipitation file
    start_section_time = time.time()
    for year in range(2011, 2021):
        raster_path = f"https://storage.googleapis.com/assets-geo/baseline/burned_area_{year}.tif"
        clipped_raster = pathlib.Path(temp_file_path, session_id, "climate", f"clipped_burned_area_{year}.tif").resolve()
        
        start_section_time = time.time()
        clip_raster_to_aoi(aoi, raster_path, clipped_raster) # ~0.3sec
        # g_var.__print_list__.append("--- %s seconds --- cliprasteraoi results" % (time.time() - start_section_time))

        start_section_time = time.time()
        annual_burned = round(calculate_stats_pixel_value(clipped_raster)[3] * 250 * 250 / 10000, 1)
        # g_var.__print_list__.append("--- %s seconds --- annualburned results" % (time.time() - start_section_time))

        annual_burned_list.append(annual_burned)
    # g_var.__print_list__.append("--- %s seconds --- annual_burned_list results" % (time.time() - start_section_time))

    output = dict() 

    output["burn_area"] = float(round(burned_area, 2))
    output["burn_frequency"] = float(round(burned_freq, 0))
    output["burn_text"] = burn_text
    output["graph_data"] = annual_burned_list

    return output

def get_climate(session_id: str, section_type: str, aoi: dict) -> dict:
    start_time = time.time()
    g_var.__print_list__.append("----------------------- climate -----------------------")
    geom_gdf = aoi["geom_gdf"]
    
    output = dict()

    if section_type == "climate" or section_type == "temperature":
        start_section_time = time.time()
        output['temperature'] = get_climate_temperature_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_climate_temperature_data results" % (time.time() - start_section_time))
    
    if section_type == "climate" or section_type == "precipitation":
        start_section_time = time.time()
        output['precipitation'] = get_climate_precipitation_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_climate_precipitation_data results" % (time.time() - start_section_time))
    
    if section_type == "climate" or section_type == "carbon_storage":
        start_section_time = time.time()
        output['carbon_storage'] = get_climate_carbon_storage_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_climate_carbon_storage_data results" % (time.time() - start_section_time))

    if section_type == "climate" or section_type == "burned_area":
        start_section_time = time.time()
        output['burned_area'] = get_climate_burned_area_data(session_id, geom_gdf)
        g_var.__print_list__.append("--- %s seconds --- get_climate_burned_area_data results" % (time.time() - start_section_time))

    process_input_data_analyzer_result(session_id=session_id, section="climate", data=output)

    g_var.__print_list__.append("--- %s seconds --- climate calculation functions results" % (time.time() - start_time))
    
    return output
# climate calculation functions -end-

# people calculation functions -begin-
def get_people_demograpgy_data(aoi: str) -> dict:
    query = text("select district, province, country from sea.v1_current_condition_highlight_site_information_adm('{aoi_geom}');".format(aoi_geom=aoi))
    adm = GeoUtils.get_db(query)

    district = ""
    province = ""
    country = ""
    
    for row in adm:
        district = row["district"]
        province = row["province"]
        country = row["country"]

    query = text("select population, keluarga, kepadatan, pertumbuhan, perpindahan, pria, pria_pct, wanita, wanita_pct, u_10_30, u_10_30_pct, u_31_50, u_31_50_pct, u_a50, u_a50_pct from sea.v1_current_condition_detail_people_demography('{aoi_geom}');".format(aoi_geom=aoi))
    demography = GeoUtils.get_db(query)

    output = dict()

    for row in demography:
        population = row["population"]
        household = row["keluarga"]
        density = row["kepadatan"]
        population_growth = row["pertumbuhan"]
        migrated = row["perpindahan"]

        output['social'] = f"The selected area is located in {district} District of {province} Province which has {population:,.0f} of people with {household:,.0f} total number of household that could be impacted by the project. Population density in this district/city is {density:,.2f} people/sq km with {population_growth:,.2f}%/year of population growth.The province has {migrated:,.0f} people have migrated."
        output['district'] = string.capwords(district) if district else ""
        output['province'] = string.capwords(province)
        output['population'] = f"{population:,.0f}"
        output['household'] = f"{household:,.0f}"
        output['density'] = f"{density:,.2f}"
        output['population_growth'] = f"{population_growth:,.2f}"
        output['migrated'] = f"{migrated:,.0f}"
        output['pria'] = row["pria"]
        output['pria_pct'] = float(round(row["pria_pct"], 2))
        output['wanita'] = row["wanita"]
        output['wanita_pct'] = float(round(row["wanita_pct"], 2))
        output['u_10_30'] = row["u_10_30"]
        output['u_10_30_pct'] = float(round(row["u_10_30_pct"], 2))
        output['u_31_50'] = row["u_31_50"]
        output['u_31_50_pct'] = float(round(row["u_31_50_pct"], 2))
        output['u_a50'] = row["u_a50"]
        output['u_a50_pct'] = float(round(row["u_a50_pct"], 2))

    return output

def get_people_employment_data(aoi: str) -> dict:
    query = text("select pengangguran, pengangguran_pct, sektor, sektor_3 from sea.v1_current_condition_detail_people_employment('{aoi_geom}');".format(aoi_geom=aoi))
    employment = GeoUtils.get_db(query)

    output = dict()

    for row in employment:
        output['pengangguran'] = int(round(row["pengangguran"], 0))
        output['pengangguran_pct'] = float(round(row["pengangguran_pct"], 2))
        output['sektor'] = row["sektor"]
        output['top_3_sektor'] = str(row["sektor_3"]).split(",")

    return output

def get_people_ethnicity_data(aoi: str) -> dict:
    query = text("select ethnicity from sea.v1_current_condition_detail_people_ethnicity('{aoi_geom}');".format(aoi_geom=aoi))
    ethnicity = GeoUtils.get_db(query)

    output = []

    for row in ethnicity:
        output.append(row["ethnicity"])

    return output

def get_people_health_data(aoi: str) -> dict:
    query = text("select facilities, med_work from sea.v1_current_condition_detail_people_health('{aoi_geom}');".format(aoi_geom=aoi))
    health = GeoUtils.get_db(query)

    output = dict()

    for row in health:
        output['facilities'] = row["facilities"]
        output['med_work'] = row["med_work"]

    return output

def get_people_education_data(aoi: str) -> dict:
    query = text("select junior, university from sea.v1_current_condition_detail_people_education('{aoi_geom}');".format(aoi_geom=aoi))
    education = GeoUtils.get_db(query)

    output = dict()

    for row in education:
        output['junior'] = row["junior"]
        output['university'] = row["university"]

    return output

def get_water_yield(session_id: str, aoi: gpd.GeoDataFrame) -> float:
    c_wyield_path = pathlib.Path(temp_file_path, session_id, "people", "clipped_water_yield_stacked.tif").resolve()
    wyield_baseline_path = pathlib.Path(temp_file_path, session_id, "people", "wyield_baseline.tif").resolve()

    # Clip stacked raster layer based on AOI
    clip_stacked_raster(aoi, wyield_path, c_wyield_path)

    with rasterio.open(c_wyield_path) as src:
        wyield_baseline = src.read(1)
        wyield_profile = src.profile

    with rasterio.open(wyield_baseline_path, 'w', **src.profile) as out_raster:
        out_raster.write(wyield_baseline, 1)

    wyield_baseline_value = calculate_stats_pixel_value(wyield_baseline_path)[3]

    return wyield_baseline_value

def get_people_water_data(session_id: str, aoi: dict) -> dict:
    geom = aoi["geom"]
    geom_gdf = aoi["geom_gdf"]

    query = text("select surface_water, ground_water, other from sea.v1_current_condition_detail_people_dws('{aoi_geom}');".format(aoi_geom=geom))
    water = GeoUtils.get_db(query)

    output = dict()

    for row in water:
        output['surface_water'] = float(round(row["surface_water"], 2))
        output['ground_water'] = float(round(row["ground_water"], 2))
        output['other'] = float(round(row["other"], 2))

    water_yield = get_water_yield(session_id, geom_gdf)

    output["water_yield"] = f"{water_yield:,.2f}"

    return output

def get_people_cs_location(geom: str) -> dict: # cs: country specific
    location = dict()

    query = """select country, province, district from sea.v2_get_location('{geom}');"""
    query = query.format(geom=geom)

    dt = GeoUtils.get_db(text(query))

    for row in dt:
        location['country'] = row.get('country')
        location['province'] = row.get('province')
        location['district'] = row.get('district')
    
    return location

def get_people(session_id: str, section_type: str, aoi: dict) -> dict:
    start_time = time.time()
    g_var.__print_list__.append("----------------------- people -----------------------")
    geom = aoi["geom"]

    output = dict()

    if section_type == 'people':
        country_specific = dict()

        start_section_time = time.time()
        location = get_people_cs_location(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_cs_location results" % (time.time() - start_section_time))

        start_section_time = time.time()
        ccs = None
        if str(location.get('country')).lower() == 'indonesia': # temp
            ccs = indonesia_social(location)
        elif str(location.get('country')).lower() == 'vietnam': # temp
            ccs = vietnam_social(location)
        elif str(location.get('country')).lower() == 'philippines': # temp
            ccs = philippines_social(location)
        elif str(location.get('country')).lower() == 'thailand': # temp
            ccs = thailand_social(location)
        elif str(location.get('country')).lower() == 'malaysia': # temp
            ccs = malaysia_social(location)
        g_var.__print_list__.append("--- %s seconds --- construct ccs results" % (time.time() - start_section_time))
        
        start_section_time = time.time()
        if ccs:
            country_specific = ccs.get_social_country_specific()
            country_specific['location'] = location

            output['country_specific'] = country_specific
        g_var.__print_list__.append("--- %s seconds --- get_social_country_specific results" % (time.time() - start_section_time))

    if section_type == "people" or section_type == "demography":
        start_section_time = time.time()
        output['demography'] = get_people_demograpgy_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_demograpgy_data results" % (time.time() - start_section_time))
    
    if section_type == "people" or section_type == "employment":
        start_section_time = time.time()
        output['employment'] = get_people_employment_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_employment_data results" % (time.time() - start_section_time))
    
    if section_type == "people" or section_type == "ethnicity":
        start_section_time = time.time()
        output['ethnicity'] = get_people_ethnicity_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_ethnicity_data results" % (time.time() - start_section_time))

    if section_type == "people" or section_type == "health":
        start_section_time = time.time()
        output['health'] = get_people_health_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_health_data results" % (time.time() - start_section_time))

    if section_type == "people" or section_type == "education":
        start_section_time = time.time()
        output['education'] = get_people_education_data(geom)
        g_var.__print_list__.append("--- %s seconds --- get_people_education_data results" % (time.time() - start_section_time))

    if section_type == "people" or section_type == "water":
        start_section_time = time.time()
        output['water'] = get_people_water_data(session_id, aoi)
        g_var.__print_list__.append("--- %s seconds --- get_people_water_data results" % (time.time() - start_section_time))

    process_input_data_analyzer_result(session_id=session_id, section="people", data=output)
    
    g_var.__print_list__.append("--- %s seconds --- people calculation functions results" % (time.time() - start_time))

    return output
# people calculation functions -end-

# main execution function
def get_current_condition(session_id: str, section_type: str) -> dict:
    existing_session = Polygons.find_by_session_id(session_id)

    if not existing_session:
        raise AppMessageException('fail, session id Not found')
        # return jsonify(status_code=HTTPStatus.NOT_FOUND, message="fail, session id Not found")
    
    geom = existing_session.geom.desc
    geom_gdf = construct_polygon(session_id)

    aoi = dict()

    aoi["geom"] = geom
    aoi["geom_gdf"] = geom_gdf

    output = dict()

    if section_type == "site_information" or section_type == "administrative_boundaries" or section_type == "elevation" or section_type == "land_cover" or section_type == "peatland_mangrove" or section_type == "annual_deforestation_rate" or section_type == "deforestation_risk" or section_type == "driver_of_deforestation" or section_type == "disaster_risk":
        output['site_information'] = get_site_information(session_id, section_type, aoi)

        remove_process_folder(session_id, "site_information")
    
    if section_type == "nature" or section_type == "kba" or section_type == "flii" or section_type == "wildlife" or section_type == "richness" or section_type == "tcl":
        output['nature'] = get_nature(session_id, section_type, aoi)

        remove_process_folder(session_id, "nature")
    
    if section_type == "climate" or section_type == "temperature" or section_type == "precipitation" or section_type == "carbon_storage" or section_type == "burned_area":
        output['climate'] = get_climate(session_id, section_type, aoi)

        remove_process_folder(session_id, "climate")

    if section_type == "people" or section_type == "demography" or section_type == "employment" or section_type == "ethnicity" or section_type == "health" or section_type == "education" or section_type == "water":
        output['people'] = get_people(session_id, section_type, aoi)

        remove_process_folder(session_id, "people")

    return output

# current condition getter/setter -begin-
def process_input_data_analyzer_result(session_id: str, section: str, data: dict):
    check_existing_session = SessionsAuth.find_by_session_id(session_id)

    if check_existing_session:
        check_existing_session = DataAnalyzer.find_by_session_id(session_id)

        if not check_existing_session:
            if section == "site_information":
                new_analyzer_result = DataAnalyzer(session_id=session_id, site_information=data)
            elif section == "nature":
                new_analyzer_result = DataAnalyzer(session_id=session_id, nature=data)
            elif section == "climate":
                new_analyzer_result = DataAnalyzer(session_id=session_id, climate=data)
            elif section == "people":
                new_analyzer_result = DataAnalyzer(session_id=session_id, people=data)
            elif section == "benefit":
                new_analyzer_result = DataAnalyzer(session_id=session_id, benefit=data)
            elif section == "eligibility":
                new_analyzer_result = DataAnalyzer(session_id=session_id, intervention_eligibility=data)

            db.session.add(new_analyzer_result)
            db.session.commit()

            return "saved"
        else:
            analyzer_result = DataAnalyzer.query.filter_by(session_id=session_id).first()

            if section == "site_information":
                analyzer_result.site_information = data
            elif section == "nature":
                analyzer_result.nature = data
            elif section == "climate":
                analyzer_result.climate = data
            elif section == "people":
                analyzer_result.people = data
            elif section == "benefit":
                analyzer_result.benefit = data
            elif section == "eligibility":
                analyzer_result.intervention_eligibility = data
            
            db.session.commit()

            return "saved"

def process_get_data_analyzer(session_id: str):
    check_existing_session = DataAnalyzer.find_by_session_id(session_id)

    if not check_existing_session:
        return jsonify(status_code=HTTPStatus.NOT_FOUND, message="fail, session id Not found")
    else:
        geom = Polygons.get_geometry(session_id).first()
        geom = json.loads(geom[0])['coordinates'][0]
        
        res = dict(
            session_id = session_id,
            polygon = geom,
            site_information = check_existing_session.site_information,
            nature = check_existing_session.nature,
            climate = check_existing_session.climate,
            people = check_existing_session.people,
            benefit = check_existing_session.benefit,
            eligibility = check_existing_session.intervention_eligibility
        )
        
        return jsonify(result=res)
# current condition getter/setter -end-

# calculate intervention list -begin-
def get_eligible_intervention(session_id: str) -> dict:
    aoi = construct_polygon(session_id)

    ### Calculate forest end area
    clipped_fcc_path = pathlib.Path(temp_file_path, session_id, "eligibility", "clipped_fcc.tif").resolve()
    clipped_lc_path = pathlib.Path(temp_file_path, session_id, "eligibility", "clipped_lc.tif").resolve()

    arr_footprint_path = pathlib.Path(temp_file_path, session_id, "eligibility", "arr_footprint.tif").resolve()
    repro_arr_footprint_path = pathlib.Path(temp_file_path, session_id, "eligibility", "repro_arr_footprint.tif").resolve()

    fc_start_path = pathlib.Path(temp_file_path, session_id, "eligibility", "fc_start.tif").resolve()
    fc_end_path = pathlib.Path(temp_file_path, session_id, "eligibility", "fc_end.tif").resolve()
    
    clipped_fc_start_path = pathlib.Path(temp_file_path, session_id, "eligibility", "clipped_fc_start.tif").resolve()
    clipped_fc_end_path = pathlib.Path(temp_file_path, session_id, "eligibility", "clipped_fc_end.tif").resolve()
    
    repro_clipped_fc_start_path = pathlib.Path(temp_file_path, session_id, "eligibility", "repro_clipped_fc_start.tif").resolve()
    repro_clipped_fc_end_path = pathlib.Path(temp_file_path, session_id, "eligibility", "repro_clipped_fc_end.tif").resolve()

    clip_raster_to_aoi(aoi, fcc_path, clipped_fcc_path)
    clip_raster_to_aoi(aoi, lc_path, clipped_lc_path)

    reclassify_forest_eligibility(clipped_fcc_path, fc_start_path, 2, 3)
    reclassify_forest_eligibility(clipped_fcc_path, fc_end_path, 3, 3)

    clip_raster_to_aoi(aoi, fc_start_path, clipped_fc_start_path)
    clip_raster_to_aoi(aoi, fc_end_path, clipped_fc_end_path)

    reproject_raster(clipped_fc_start_path, repro_clipped_fc_start_path, "ESRI:54034")
    reproject_raster(clipped_fc_end_path, repro_clipped_fc_end_path, "ESRI:54034")

    fc_start_area = calculate_forest_area(repro_clipped_fc_start_path)
    fc_end_area = calculate_forest_area(repro_clipped_fc_end_path)

    # Define eligible area for ecosystem restoration
    reclassify_arr(clipped_fc_start_path, clipped_lc_path, arr_footprint_path)
    reproject_raster(arr_footprint_path, repro_arr_footprint_path, "ESRI:54034")
    arr_elig_area = calculate_forest_area(repro_arr_footprint_path)

    deforestation_rate = calculate_deforestation_rate(fc_start_area, fc_end_area, 2010, 2020)

    interventions = []

    if ((fc_end_area > 0) & (deforestation_rate < 0)) & (arr_elig_area > 0):
        interventions.append(available_intervention_types[0])
        interventions.append(available_intervention_types[1])
    elif ((fc_end_area <= 0) | (deforestation_rate <= 0)) & (arr_elig_area > 0):
        interventions.append(available_intervention_types[1])
    elif ((fc_end_area > 0) & (deforestation_rate > 0)) | (arr_elig_area < 0):
        interventions.append(available_intervention_types[0])

    remove_process_folder(session_id, "eligibility")

    process_input_data_analyzer_result(session_id=session_id, section="eligibility", data=interventions)
    
    return interventions
# calculate intervention list -end-

# create project area map -begin-
def calculate_boundaries(lat, lng, zoom, width, height): # -> tuple:
    upper_left, lower_right = {}, {}
    C = 40075 # km - Equator distance around the world
    y = pi * lat / 180 # convert latitude degree to radian
    S = C * cos(y) / 2 ** (zoom + 8) # km distance of 1 px - https://wiki.openstreetmap.org/wiki/Pt:Zoom_levels
    S_deg = S * cos(y) / 100 # convert km (distance of 1 px) to degrees (coordinates)

    upper_left['lat'] = lat + height / 2 * S_deg
    upper_left['lng'] = lng - width / 2 * S_deg

    lower_right['lat'] = lat - height / 2 * S_deg
    lower_right['lng'] = lng + width / 2 * S_deg

    return upper_left, lower_right

def prefetch_cx_basemap(geom):
    cm = 1/2.54  # centimeters in inches

    start_section_time = time.time()
    loc = gpd.GeoSeries(geom.geometry.unary_union)
    loc = loc.set_crs(epsg=4326)

    min_x = loc.bounds.minx[0]
    max_x = loc.bounds.maxx[0]

    min_y = loc.bounds.miny[0]
    max_y = loc.bounds.maxy[0]

    geom_center = loc.to_crs(epsg=3035).centroid.to_crs(epsg=4326).head(1)

    center_lng = geom_center.x[0]
    center_lat = geom_center.y[0]

    upper_left, lower_right = calculate_boundaries(center_lat, center_lng, 7, 15*cm*100, 15*cm*100)
    upper_left_inmap, lower_right_inmap = calculate_boundaries(center_lat, center_lng, 10, 15*cm*100, 15*cm*100)

    fig, ax = plt.subplots(figsize=(15*cm,15*cm))

    ax.patch.set_edgecolor('black')
    ax.patch.set_linewidth(3)
    plt.gca().secondary_xaxis('top')
    secay = plt.gca().secondary_yaxis('right')

    start_section_time = time.time()
    geom_plot = loc.plot(ax=ax, facecolor='none', edgecolor="#077f68", linewidth=3)
    plt.margins(y=.5, x=.5)
    g_var.__print_list__.append("--- %s seconds --- geom_plot results" % (time.time() - start_section_time))

    start_section_time = time.time()
    cx.add_basemap(
        ax,
        crs=CRS('EPSG:4326'),
        source=cx.providers.OpenStreetMap.Mapnik,
        attribution="",
    ) # kayaknya bisa di prefetch pas polygon udah masuk
    g_var.__print_list__.append("--- %s seconds --- add_basemap results" % (time.time() - start_section_time))

def construct_project_area_map(session_id, geom):
    filename = session_id + ".jpg"
    filepath = os.path.join("generated-file", "project-area")

    if not os.path.exists(filepath):
        os.makedirs(filepath)
        
    user_folder = pathlib.Path("generated-file", "project-area",  filename).resolve()
    cm = 1/2.54  # centimeters in inches

    start_section_time = time.time()
    loc = gpd.GeoSeries(geom.geometry.unary_union)
    loc = loc.set_crs(epsg=4326)

    min_x = loc.bounds.minx[0]
    max_x = loc.bounds.maxx[0]

    min_y = loc.bounds.miny[0]
    max_y = loc.bounds.maxy[0]

    geom_center = loc.to_crs(epsg=3035).centroid.to_crs(epsg=4326).head(1)

    center_lng = geom_center.x[0]
    center_lat = geom_center.y[0]

    upper_left, lower_right = calculate_boundaries(center_lat, center_lng, 7, 15*cm*100, 15*cm*100)
    upper_left_inmap, lower_right_inmap = calculate_boundaries(center_lat, center_lng, 10, 15*cm*100, 15*cm*100)

    fig, ax = plt.subplots(figsize=(15*cm,15*cm))

    ax.patch.set_edgecolor('black')
    ax.patch.set_linewidth(3)
    plt.gca().secondary_xaxis('top')
    secay = plt.gca().secondary_yaxis('right')

    start_section_time = time.time()
    geom_plot = loc.plot(ax=ax, facecolor='none', edgecolor="#077f68", linewidth=3)
    plt.margins(y=.5, x=.5)
    g_var.__print_list__.append("--- %s seconds --- geom_plot results" % (time.time() - start_section_time))

    start_section_time = time.time()
    cx.add_basemap(
        ax,
        crs=CRS('EPSG:4326'),
        source=cx.providers.OpenStreetMap.Mapnik,
        attribution="",
    ) # kayaknya bisa di prefetch pas polygon udah masuk
    g_var.__print_list__.append("--- %s seconds --- add_basemap results" % (time.time() - start_section_time))

    start_section_time = time.time()
    points = gpd.GeoSeries([Point(min_x, min_y), Point(max_x, max_y)], crs=4326)
    points = points.to_crs(32619)
    distance_meters = points[0].distance(points[1])
    scalebar = ScaleBar(distance_meters, location="lower left", border_pad=1, pad=0.5, label="Map Scale", scale_loc="right") # 1 pixel = 0.2 meter
    ax.add_artist(scalebar)
    g_var.__print_list__.append("--- %s seconds --- scale_bar results" % (time.time() - start_section_time))

    start_section_time = time.time()
    add_north_arrow(ax, scale=.25, xlim_pos=.98, ylim_pos=.96, color='#000', text_scaler=4, text_yT=-2.5)
    g_var.__print_list__.append("--- %s seconds --- add_north_arrow results" % (time.time() - start_section_time))

    start_section_time = time.time()
    axin = inset_axes(ax, width="20%", height="20%", loc="lower right")
    g_var.__print_list__.append("--- %s seconds --- inset_axes results" % (time.time() - start_section_time))
    
    start_section_time = time.time()
    inmap = Basemap(llcrnrlon=upper_left['lng'], urcrnrlon=lower_right['lng'], llcrnrlat=lower_right['lat'], urcrnrlat=upper_left['lat'], projection='lcc', lon_0=center_lng, lat_0=center_lat, resolution='c', ax=axin)
    g_var.__print_list__.append("--- %s seconds --- inmap results" % (time.time() - start_section_time))

    start_section_time = time.time()
    inmap.shadedrelief(scale=1)
    g_var.__print_list__.append("--- %s seconds --- inmap_shadedrelief results" % (time.time() - start_section_time))

    start_section_time = time.time()
    inmap_lngs = [upper_left_inmap['lng'], upper_left_inmap['lng'], lower_right_inmap['lng'], lower_right_inmap['lng']]
    inmap_lats = [lower_right_inmap['lat'], upper_left_inmap['lat'], upper_left_inmap['lat'], lower_right_inmap['lat']]
    g_var.__print_list__.append("--- %s seconds --- inmap_lngs results" % (time.time() - start_section_time))

    start_section_time = time.time()
    x, y = inmap(inmap_lngs, inmap_lats)
    xy = zip(x,y)
    poly = poly_patches( list(xy), facecolor='red', alpha=0.4, closed=False )
    g_var.__print_list__.append("--- %s seconds --- poly_patches results" % (time.time() - start_section_time))

    start_section_time = time.time()
    inmap.ax.add_patch(poly)
    g_var.__print_list__.append("--- %s seconds --- add_patch poly results" % (time.time() - start_section_time))

    start_section_time = time.time()
    fig.tight_layout()
    fig.savefig(user_folder)
    g_var.__print_list__.append("--- %s seconds --- fig_save results" % (time.time() - start_section_time))

    start_section_time = time.time()
    gcs.upload(os.path.join("generated-file", "project-area", filename))
    g_var.__print_list__.append("--- %s seconds --- gcs_upload results" % (time.time() - start_section_time))

    return user_folder

def set_intervention(session_id):
    mapExplorer = MapExplorer.find_by_session_id(session_id)

    if mapExplorer:
        if mapExplorer.intervention == None:
            intervention = get_eligible_intervention(session_id)

            session_id = session_id
            project_duration = mapExplorer.project_duration
            estimated_unplanned_deforestation = 0
            rest_target = []

            GeoLogic.process_input_explore_map(session_id=session_id,
                              project_duration=project_duration,
                              estimated_unplanned_deforestation=estimated_unplanned_deforestation,
                              intervention = intervention,
                              rest_target=rest_target)
            
            return intervention

def get_carbon_storage():
    # query = text("select session_id from \"vwActiveSites\" where session_id = '{sid}';".format(sid="27d8f0a8-bc75-4c99-a134-5e1909d6734a"))
    query = text("select session_id from \"vwActiveSites\";")
    data = GeoUtils.get_db(query, False)

    cs = dict()

    site = 0
    
    for row in data:
        site = site + 1

        analyzer = process_get_data_analyzer(row["session_id"]).json["result"]

        country = analyzer["site_information"]["administrative_boundaries"]["country"]
        project_area = float(str(analyzer["site_information"]["administrative_boundaries"]["project_area"]).replace(",", ""))

        carbon_storage_num = float(str(analyzer["climate"]["carbon_storage"]["carbon_storage_plain"]).replace(",", ""))
        carbon_storage_other_num = float(str(analyzer["climate"]["carbon_storage"]["other_source_plain"]).replace(",", ""))

        agb_num = float(str(analyzer["climate"]["carbon_storage"]["aboveground_number"]).replace(",", ""))
        bgb_num = float(str(analyzer["climate"]["carbon_storage"]["belowground_number"]).replace(",", ""))
        soil_num = float(str(analyzer["climate"]["carbon_storage"]["soil_number"]).replace(",", ""))

        aoh_avdef = float(str(analyzer["benefit"]["nature"]["area_of_habitat"]["avoided_deforestation"]).replace(",", ""))
        aoh_ecores = float(str(analyzer["benefit"]["nature"]["area_of_habitat"]["ecosystem_restoration"]).replace(",", ""))

        water_yield = float(str(analyzer["benefit"]["people"]["ecosystem_services"]["improve_water_yield"]).replace(",", ""))
        reduce_erotion = float(str(analyzer["benefit"]["people"]["ecosystem_services"]["reduce_erosion"]).replace(",", ""))

        avoided = analyzer["benefit"]["climate"]["potential_avoided"]["total_co2eq"] if analyzer["benefit"]["climate"]["potential_avoided"] != 0 else 0
        sequestered = analyzer["benefit"]["climate"]["potential_sequestered"]["total_co2eq"] if analyzer["benefit"]["climate"]["potential_sequestered"] != 0 else 0

        if country in cs:
            prev_site_num = int(cs[country]["site"])
            prev_area_ha = float(cs[country]["area_ha"])

            prev_carbon_storage = float(cs[country]["carbon_storage"])
            prev_carbon_storage_other = float(cs[country]["carbon_storage_other"])

            prev_agb = float(cs[country]["agb"])
            prev_bgb = float(cs[country]["bgb"])
            prev_soil = float(cs[country]["soil"])

            prev_aoh_avdef = float(cs[country]["aoh_avdef"])
            prev_aoh_ecores = float(cs[country]["aoh_ecores"])

            prev_water = float(cs[country]["water"])
            prev_erotion = float(cs[country]["erotion"])

            prev_avoided = float(cs[country]["avoided"])
            prev_sequestered = float(cs[country]["sequestered"])
        else:
            cs[country] = dict()

            prev_site_num = 0
            prev_area_ha = 0

            prev_carbon_storage = 0
            prev_carbon_storage_other = 0

            prev_agb = 0
            prev_bgb = 0
            prev_soil = 0

            prev_aoh_avdef = 0
            prev_aoh_ecores = 0

            prev_water = 0
            prev_erotion = 0

            prev_avoided = 0
            prev_sequestered = 0

        total_project_area = prev_area_ha + project_area
        total_cs = prev_carbon_storage + carbon_storage_num
        total_cso = prev_carbon_storage_other + carbon_storage_other_num
        
        total_agb = prev_agb + agb_num
        total_bgb = prev_bgb + bgb_num
        total_soil = prev_soil + soil_num

        total_aoh_avdef = prev_aoh_avdef + aoh_avdef
        total_aoh_ecores = prev_aoh_ecores + aoh_ecores

        total_water = prev_water + water_yield
        total_erotion = prev_erotion + reduce_erotion

        total_avoided = prev_avoided + avoided
        total_sequestered = prev_sequestered + sequestered

        cs[country]["site"] = prev_site_num + 1
        cs[country]["area_ha"] = round(total_project_area, 2)
        cs[country]["carbon_storage"] = round(total_cs, 2)
        cs[country]["carbon_storage_other"] = round(total_cso, 2)
        cs[country]["agb"] = round(total_agb, 2)
        cs[country]["bgb"] = round(total_bgb, 2)
        cs[country]["soil"] = round(total_soil, 2)
        cs[country]["aoh_avdef"] = round(total_aoh_avdef, 2)
        cs[country]["aoh_ecores"] = round(total_aoh_ecores, 2)
        cs[country]["water"] = round(total_water, 2)
        cs[country]["erotion"] = round(total_erotion, 2)
        cs[country]["avoided"] = round(total_avoided, 2)
        cs[country]["sequestered"] = round(total_sequestered, 2)

    aoh_avdef = 0
    aoh_ecores = 0
    water = 0
    erotion = 0
    avoided = 0
    sequestered = 0

    for c, d in cs.items():
        agb_pct = (d["agb"] / d["carbon_storage"]) * 100
        bgb_pct = (d["bgb"] / d["carbon_storage"]) * 100
        soil_pct = (d["soil"] / d["carbon_storage"]) * 100

        aoh_avdef = aoh_avdef + d["aoh_avdef"]
        aoh_ecores = aoh_ecores + d["aoh_ecores"]

        water = water + d["water"]
        water_country = round((d["water"] / d["site"]), 2)

        erotion = erotion + d["erotion"]
        erotion_country = round((d["erotion"] / d["site"]), 2)

        avoided = avoided + d["avoided"]
        sequestered = sequestered + d["sequestered"]

        cs[c]["text"] = "In {country} there are {site_num} number of site with total area of {area_ha:,.2f} ha. The total carbon storage is estimated at {carbon_storage:,.2f} tonnes. This includes {agb:,.2f} tonnes ({agb_pct:,.2f}%) stored in aboveground biomass, {bgb:,.2f} tonnes ({bgb_pct:,.2f}%) in belowground biomass, and {soil:,.2f} tonnes ({soil_pct:,.2f}%) in the soil. Another data source  estimates a slightly different total carbon storage of {other:,.2f} tonnes.".format(
            country = c,
            site_num = d["site"],
            area_ha = d["area_ha"],
            carbon_storage = d["carbon_storage"],
            agb = d["agb"],
            agb_pct = agb_pct,
            bgb = d["bgb"],
            bgb_pct = bgb_pct,
            soil = d["soil"],
            soil_pct = soil_pct,
            other = d["carbon_storage_other"]
        )

        cs[c]["water"] = water_country
        cs[c]["erotion"] = erotion_country

    water = round(water / site, 2)
    erotion = round(erotion / site, 2)

    cs["aoh_avdef"] = aoh_avdef
    cs["aoh_ecores"] = aoh_ecores
    cs["water"] = water
    cs["erotion"] = erotion
    cs["avoided"] = avoided
    cs["sequestered"] = sequestered

    return cs