# libraries -begin-
from flask import jsonify, request, make_response, current_app
from ... import db
from ...models.geos_models.models import Polygons, MapExplorer

import os, shutil, pathlib, json, pickle, datetime, rasterio, math, geopandas as gpd, numpy as np, forestatrisk as far
import time

from rasterio.mask import mask
from rasterio.warp import calculate_default_transform, reproject, Resampling
from sqlalchemy import create_engine, text
from shapely.geometry import Polygon, MultiPolygon, shape
from pyproj import CRS
# libraries -end-

# list of data sources, paths and static variables -begin-
properties = dict()

lc_path = "https://storage.googleapis.com/assets-geo/baseline/lc_ipcc_mosaicked.tif"
fcc_path = "https://storage.googleapis.com/assets-geo/baseline/fcc123_v3.tif"
defrisk_path = "https://storage.googleapis.com/assets-geo/benefit/prob_defrisk_4326_v4_1.tif"
agb_path = "https://storage.googleapis.com/assets-geo/baseline/ESA_CCI_BIOMASS_2020.tif"
c_seq_rate_path = "https://storage.googleapis.com/assets-geo/benefit/agb_forest_regrowth_c_seq_rate.tif"
wyield_path = "https://storage.googleapis.com/assets-geo/benefit/water_yield_stacked.tif" # Predefined water yields
av_ero_path = "https://storage.googleapis.com/assets-geo/benefit/avoided_erosion_stacked.tif" # Predefined avoided erosion

available_intervention_types = ["Avoided deforestation", "Ecosystem restoration"]

temp_file_path = "processed/"
# list of data sources, paths and static variables -end-

# general functions -begin-
def set_path(session_id: str):
    global clipped_fcc_path, fc_start_path, fc_end_path, clipped_fc_start_path, clipped_fc_end_path, repro_clipped_fc_start_path, repro_clipped_fc_end_path, clipped_lc_path, arr_footprint_path, repro_arr_footprint_path, clipped_defrisk_path, repro_clipped_defrisk_path, clipped_agb_path, repro_clipped_agb_path, clipped_c_seq_path, repro_clipped_c_seq_path, projected_remaining_forest_path, masked_agb_path, AD_result_path, adjusted_proj_fc_path, masked_arr_repro_clipped_agb, masked_c_seq_path, c_wyield_path, c_av_ero_path, wyield_baseline_path, wyield_bau_path, av_ero_baseline_path, av_ero_bau_path

    clipped_fcc_path = pathlib.Path(temp_file_path, session_id, "clipped_fcc.tif").resolve()
    fc_start_path = pathlib.Path(temp_file_path, session_id, "fc_start.tif").resolve()
    fc_end_path = pathlib.Path(temp_file_path, session_id, "fc_end.tif").resolve()
    clipped_fc_start_path = pathlib.Path(temp_file_path, session_id, "clipped_fc_start.tif").resolve()
    clipped_fc_end_path = pathlib.Path(temp_file_path, session_id, "clipped_fc_end.tif").resolve()
    repro_clipped_fc_start_path = pathlib.Path(temp_file_path, session_id, "repro_clipped_fc_start.tif").resolve()
    repro_clipped_fc_end_path = pathlib.Path(temp_file_path, session_id, "repro_clipped_fc_end.tif").resolve()
    clipped_lc_path = pathlib.Path(temp_file_path, session_id, "clipped_lc.tif").resolve()
    arr_footprint_path = pathlib.Path(temp_file_path, session_id, "arr_footprint.tif").resolve()
    repro_arr_footprint_path = pathlib.Path(temp_file_path, session_id, "repro_arr_footprint.tif").resolve()
    clipped_defrisk_path = pathlib.Path(temp_file_path, session_id, "clipped_defrisk.tif").resolve()
    repro_clipped_defrisk_path = pathlib.Path(temp_file_path, session_id, "repro_clipped_defrisk.tif").resolve()
    clipped_agb_path = pathlib.Path(temp_file_path, session_id, "clipped_agb.tif").resolve()
    repro_clipped_agb_path = pathlib.Path(temp_file_path, session_id, "repro_clipped_agb.tif").resolve()
    clipped_c_seq_path = pathlib.Path(temp_file_path, session_id, "clipped_c_seq_rate.tif").resolve()
    repro_clipped_c_seq_path = pathlib.Path(temp_file_path, session_id, "repro_clipped_c_seq_rate.tif").resolve()
    projected_remaining_forest_path = pathlib.Path(temp_file_path, session_id, "projected_remaining_forest.tif").resolve()
    masked_agb_path = pathlib.Path(temp_file_path, session_id, "projected_remaining_forest.tif").resolve()
    AD_result_path = pathlib.Path(temp_file_path, session_id, "avdef_footprint.tif").resolve()
    adjusted_proj_fc_path = pathlib.Path(temp_file_path, session_id, f"adjusted_fcc_{2020 + project_duration}.tif").resolve()
    masked_arr_repro_clipped_agb = pathlib.Path(temp_file_path, session_id, "masked_arr_repro_clipped_agb.tif").resolve()
    masked_c_seq_path = pathlib.Path(temp_file_path, session_id, "masked_arr_repro_clipped_c_seq.tif").resolve()
    c_wyield_path = pathlib.Path(temp_file_path, session_id, "clipped_water_yield_stacked.tif").resolve()
    c_av_ero_path = pathlib.Path(temp_file_path, session_id, "clipped_avoided_erosion_stacked.tif").resolve()

    wyield_baseline_path = pathlib.Path(temp_file_path, session_id, "wyield_baseline.tif").resolve()
    wyield_bau_path = pathlib.Path(temp_file_path, session_id, "wyield_bau.tif").resolve()
    av_ero_baseline_path = pathlib.Path(temp_file_path, session_id, "av_ero_baseline.tif").resolve()
    av_ero_bau_path = pathlib.Path(temp_file_path, session_id, "av_ero_bau.tif").resolve()

def get_db(query_text:text) -> dict:
    output = []

    db_host = DevelopmentConfig.DB_HOST
    db_port = DevelopmentConfig.DB_PORT
    db_name = DevelopmentConfig.DB_NAME
    db_user = DevelopmentConfig.DB_USER
    db_pass = DevelopmentConfig.DB_PWD
    # connection configuration to database -end-
    print("postgresql://" + db_user + ":" + db_pass + "@" + db_host + ":" + db_port + "/" + db_name)

    try:
        engine = create_engine("postgresql+psycopg2://" + db_user + ":" + db_pass + "@" + db_host + ":" + db_port + "/" + db_name, isolation_level="AUTOCOMMIT")
        print('==================================================')
        connection = engine.connect()

        result = connection.execute(query_text)

        for row in result:
            output.append(row._asdict())
    except Exception as e:
        error = e
        print('ERRORRRRRRRRRRRRRR: {}'.format(str(e)))
    finally:
        pass
        connection.close()
        engine.dispose()

    return output

def remove_process_folder(session_id: str):
    user_folder = pathlib.Path(temp_file_path, session_id).resolve()

    if os.path.isdir(user_folder):
        shutil.rmtree(user_folder)

def get_polygon(session_id: str) -> gpd.GeoDataFrame:
    known_polygons_geometry = Polygons.get_geometry(session_id).first()
    
    res = json.loads(known_polygons_geometry[0])
    polygon_types = res['type']
    geom_coords = res['coordinates']
    geom = False
    
    if polygon_types.lower() == "multipolygon":
        geom = shape(res)
    elif polygon_types.lower() == "polygon":
        geom = Polygon(geom_coords[0])

    gdf = gpd.GeoDataFrame(index=[0], crs=CRS('EPSG:4326'), geometry=[geom])

    return gdf

def get_properties(session_id: str) -> dict:
    known_map_explorer = MapExplorer.find_by_session_id(session_id)

    props = {
        'project_duration': known_map_explorer.project_duration,
        'unavoided_def_rate': known_map_explorer.estimated_unplanned_deforestation,
        'rest_target': known_map_explorer.rest_target,
        'intervention': known_map_explorer.intervention,
    }

    return props

def clip_raster_to_aoi(aoi, raster_path, output_path):
    start_time = time.time()
    os.makedirs(output_path.parent, exist_ok=True)

    print(raster_path)
    # Read raster
    with rasterio.open(raster_path) as src:
        print("--- %s seconds --- open clear" % (time.time() - start_time))
        start_section_time = time.time()
        print(src)
        out_image, out_transform = mask(src, aoi.geometry, crop = True, nodata = 255)
        print(out_transform)
        print("--- %s seconds --- mask" % (time.time() - start_section_time))
        print("--- %s seconds --- mask clear" % (time.time() - start_time))
        start_section_time = time.time()
        out_meta = src.meta.copy()
        out_meta.update({
            "driver": "GTiff",
            "height": out_image.shape[1],
            "width": out_image.shape[2],
            "transform": out_transform,
            "nodata": 255
        })
        print("--- %s seconds --- copy update" % (time.time() - start_section_time))
    
    # Save clipped raster
    start_section_time = time.time()
    with rasterio.open(output_path, "w", **out_meta) as dest:
        dest.write(out_image)
        print("--- %s seconds --- write" % (time.time() - start_section_time))

    print("--- %s seconds --- clear" % (time.time() - start_time))
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

def calculate_stats_pixel_value(raster_path):
    with rasterio.open(raster_path) as src:
        array = src.read(1, masked=True)

        if not array.mask.all():
            pixel_count = array[array > 0].size

            if pixel_count > 0 and not np.isnan(array.min()) and not np.isnan(array.max()) and not array[array > 0].mask.all():
                average_pixel_value = array[array > 0].mean()
                min_pixel_value = array[array > 0].min()
                max_pixel_value = array[array > 0].max()
                sum_pixel_value = array[array > 0].sum()
                count_pixel_value = array[array > 0].count()
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

def calculate_forest_area(raster_path):
    with rasterio.open(raster_path) as src:
        band = src.read(1, masked = True)
        # Calculate area per pixel in hectares (10,000 square meters)
        area_per_pixel = src.res[0] * src.res[1] / 10000
        # Calculate total forest area
        forest_area = np.sum(band == 1) * area_per_pixel

    return forest_area

def reclassify_forest(raster_path, output_raster_path, x, y):
    with rasterio.open(raster_path) as src:
        band = src.read(1, masked = True)
        band_rec = np.where((band == x) | (band == y), 1, 0)
        out_meta = src.meta.copy()
        out_meta.update({
            "nodata": 255
        })

    with rasterio.open(output_raster_path, 'w', **out_meta) as dest:
        dest.write(band_rec, 1)

def reclassify_arr(fc_start_raster_path, lc_raster_path, output_raster_path):
    with rasterio.open(fc_start_raster_path) as src, rasterio.open(lc_raster_path) as src1:
        fc_start = src.read(1, masked = True)
        lc = src1.read(1, masked = True)

        # reclassify land cover >> agriculture, grassland, other land
        arr_lc_eli = np.where(((lc == 2) | (lc == 3) | (lc == 6)), 1, 0)
        # reclassify land cover >> grassland and other land (for natural regeneration)
        # arr_lc_eli = np.where(((lc == 2) | (lc == 6)), 1, 0)
        arr_fc_eli = np.where(fc_start == 0, 1, 0)
        arr_footprint = np.where((arr_lc_eli == 1) & (arr_fc_eli == 1), 1, 0)
        out_meta = src.meta.copy()

    with rasterio.open(output_raster_path, 'w', **out_meta) as dest:
        dest.write(arr_footprint, 1)

def calculate_deforestation_rate(area_1, area_2, t1, t2):
    # Function to calculate annual deforestation rate; formula by Puyravaud (2002)
    if area_1 > 0 and area_2 > 0:
        rate = (1 / (t2 - t1)) * math.log(area_2 / area_1)
    elif area_1 > 0 and area_2 == 0:
        rate = (1 / (t2 - t1)) * math.log(0.1 / area_1)
    else:
        rate = 999

    return rate

def project_deforestation(forest_cover_area_ha, deforestation_rate, project_duration):
    for _ in range(project_duration):
        forest_cover_area_ha += forest_cover_area_ha * (deforestation_rate)

    return forest_cover_area_ha

def match_raster_dimensions(base_raster_path, target_raster_path, output_path):
    # Open the base raster to match its dimensions and transform
    with rasterio.open(base_raster_path) as base_raster:
        base_data = base_raster.read(1)
        base_transform = base_raster.transform
        base_crs = base_raster.crs
        base_profile = base_raster.profile

    # Open the target raster to reproject and match the base raster
    with rasterio.open(target_raster_path) as target_raster:
        target_data = target_raster.read(1)

        # Create an empty numpy array with the shape of the base raster
        matched_data = np.empty_like(base_data)

        # Reproject and resample the target raster to match the base raster
        reproject(
            source=target_data,
            destination=matched_data,
            src_transform=target_raster.transform,
            src_crs=target_raster.crs,
            dst_transform=base_transform,
            dst_crs=base_crs,
            resampling=Resampling.nearest
        )

        # Save the adjusted raster to a new file
        with rasterio.open(output_path, 'w', **base_profile) as dst_raster:
            dst_raster.write(matched_data, 1)

def clip_stacked_raster(raster_path, output_path, aoi):

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

def clip_all_raster(aoi: gpd.GeoDataFrame):
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, fcc_path, clipped_fcc_path)
    print("--- %s seconds --- clip_raster_to_aoi 1 results" % (time.time() - start_section_time))
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, lc_path, clipped_lc_path)
    print("--- %s seconds --- clip_raster_to_aoi 2 results" % (time.time() - start_section_time))
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, defrisk_path, clipped_defrisk_path)
    print("--- %s seconds --- clip_raster_to_aoi 3 results" % (time.time() - start_section_time))
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, agb_path, clipped_agb_path)
    print("--- %s seconds --- clip_raster_to_aoi 4 results" % (time.time() - start_section_time))
    start_section_time = time.time()
    clip_raster_to_aoi(aoi, c_seq_rate_path, clipped_c_seq_path)
    print("--- %s seconds --- clip_raster_to_aoi 5 results" % (time.time() - start_section_time))

    start_section_time = time.time()
    clip_stacked_raster(wyield_path, c_wyield_path, aoi)
    print("--- %s seconds --- clip_stacked_raster 1 results" % (time.time() - start_section_time))
    start_section_time = time.time()
    clip_stacked_raster(av_ero_path, c_av_ero_path, aoi)
    print("--- %s seconds --- clip_stacked_raster 2 results" % (time.time() - start_section_time))

def reproject_all():
    reproject_raster(clipped_fc_start_path, repro_clipped_fc_start_path, "ESRI:54034")
    reproject_raster(clipped_fc_end_path, repro_clipped_fc_end_path, "ESRI:54034")
    reproject_raster(clipped_defrisk_path, repro_clipped_defrisk_path, "ESRI:54034")
    reproject_raster(clipped_agb_path, repro_clipped_agb_path, "ESRI:54034")
    reproject_raster(clipped_c_seq_path, repro_clipped_c_seq_path, "ESRI:54034")
# general functions -end-

# main functions -start-
def preparing_data(data):
    global project_duration, interventions, fc_start_area, fc_end_area, arr_elig_area, total_aoi_area_ha, session_id

    session_id = data.get('session_id')

    start_section_time = time.time()
    properties = get_properties(session_id)
    print("--- %s seconds --- get_properties results" % (time.time() - start_section_time))

    project_duration = properties["project_duration"] # years
    interventions = properties["intervention"] # interventions

    start_section_time = time.time()
    set_path(session_id)
    print("--- %s seconds --- set_path results" % (time.time() - start_section_time))

    start_section_time = time.time()
    aoi = get_polygon(session_id)
    print("--- %s seconds --- get_polygon results" % (time.time() - start_section_time))
    
    aoi_proj = gpd.GeoDataFrame(geometry=[aoi.unary_union], crs="EPSG:4326").to_crs("ESRI:54034")
    total_aoi_area_ha = aoi_proj.geometry.area.sum() / 10000

    start_section_time = time.time()
    clip_all_raster(aoi)
    print("--- %s seconds --- clip_all_raster results" % (time.time() - start_section_time))

    ### forest at start
    start_section_time = time.time()
    reclassify_forest(clipped_fcc_path, fc_start_path, 2, 3)
    clip_raster_to_aoi(aoi, fc_start_path, clipped_fc_start_path)
    print("--- %s seconds --- forest_at_start results" % (time.time() - start_section_time))

    ### forest at end
    start_section_time = time.time()
    reclassify_forest(clipped_fcc_path, fc_end_path, 3, 3)
    clip_raster_to_aoi(aoi, fc_end_path, clipped_fc_end_path)
    print("--- %s seconds --- forest_at_end results" % (time.time() - start_section_time))

    start_section_time = time.time()
    reproject_all()
    print("--- %s seconds --- reproject_all results" % (time.time() - start_section_time))

    ### Calculate forest area    
    start_section_time = time.time()
    fc_start_area = calculate_forest_area(repro_clipped_fc_start_path)
    fc_end_area = calculate_forest_area(repro_clipped_fc_end_path)
    print("--- %s seconds --- calculate_forest_area results" % (time.time() - start_section_time))

    # Define eligible area for ecosystem restoration
    start_section_time = time.time()
    reclassify_arr(clipped_fc_start_path, clipped_lc_path, arr_footprint_path)
    reproject_raster(arr_footprint_path, repro_arr_footprint_path, "ESRI:54034")
    print("--- %s seconds --- ecosystem restoration results" % (time.time() - start_section_time))

    start_section_time = time.time()
    arr_elig_area = calculate_forest_area(repro_arr_footprint_path)
    print("--- %s seconds --- calculate_forest_area results" % (time.time() - start_section_time))

def avoided_deforestation(cr_fc_2010_path, cr_fc_2020_path, project_duration, cr_def_risk_path):
    # AVOIDED DEFORESTATION

    # Calculate projected forest cover
    ## Deforestation rate
    deforestation_rate = calculate_deforestation_rate(fc_start_area, fc_end_area, 2010, 2020)
    # print(f"Deforestation rate: {deforestation_rate * 100:,.1f}%")

    ## Projected forest cover
    # projected_forest_cover = fc_end_area * (1 + deforestation_rate) ** project_duration
    projected_forest_cover = project_deforestation(fc_end_area, deforestation_rate, project_duration)
    # print(f"Projected remaining forest cover in {2020 + project_duration}: {projected_forest_cover:,.1f} ha")

    proj_defor = round(fc_end_area - projected_forest_cover)
    # print(f"Projected deforestation between 2020 to {2020 + project_duration}: {proj_defor:,.1f} ha")

    ## Compute future forest cover
    # proj_fc_path = pathlib.Path(temp_file_path, f"fcc_{2020 + project_duration}.tif").resolve()
    input_raster_path = str(repro_clipped_defrisk_path.absolute())
    output_file_path = str(projected_remaining_forest_path.absolute())

    stats = far.deforest(
        input_raster = input_raster_path,
        hectares = proj_defor,
        output_file = output_file_path,
        blk_rows = 128
    )

    # Avoided deforestation footprint    
    match_raster_dimensions(repro_clipped_fc_end_path, projected_remaining_forest_path, adjusted_proj_fc_path)

    ## Subtract forest cover to get the avoided deforestation footprint
    init_raster_path = repro_clipped_fc_end_path
    proj_raster_path = adjusted_proj_fc_path
    with rasterio.open(init_raster_path) as src1, rasterio.open(proj_raster_path) as src2:
        fc_init = src1.read(1)
        fc_init_out_meta = src1.meta.copy()
        fc_proj = src2.read(1)
    av_def_footprint = np.where((fc_init == 1) & (fc_proj == 0), 1, 0)

    with rasterio.open(AD_result_path, "w", **fc_init_out_meta) as dest:
        dest.write(av_def_footprint, 1)

    res_avoided_defor = {
        "forest_cover_2010_ha": float(round(fc_start_area, 2)),
        "forest_cover_2020_ha": float(round(fc_end_area, 2)),
        "annual_deforestation_rate": float(round(deforestation_rate, 2)),
        "projected deforestation_from_2020_to_": 2020 + project_duration,
        "proj_deforestation_ha": float(round(proj_defor, 1)),
        "proj_forest_cover_ha": float(round(projected_forest_cover, 1))
    }

    return AD_result_path, proj_defor, projected_forest_cover, res_avoided_defor

def get_eligibility_data(avdev_result_path):
    if interventions.count("Avoided Deforestation") > 0 :
        avdef_elig_area = calculate_forest_area(avdev_result_path)
    else:
        avdef_elig_area = 0

    res_eligibility_area = {
        "total_aoi_area_ha": float(round(total_aoi_area_ha, 1)),
        "eligible_avdef_ha": float(round(avdef_elig_area, 1)),
        "eligible_avdef_pct": float(round(avdef_elig_area / total_aoi_area_ha * 100, 1)),
        "eligible_ecosystem_restoration_pct": float(round(arr_elig_area / total_aoi_area_ha * 100, 1)),
        "eligible_ecosystem_restoration_ha": float(round(arr_elig_area, 1)),
        "non_eligible_project_area_ha": float(round(total_aoi_area_ha - avdef_elig_area - arr_elig_area, 1)),
        "non_eligible_project_area_pct": float(round((total_aoi_area_ha - avdef_elig_area - arr_elig_area) / total_aoi_area_ha * 100, 1))
    }

    return res_eligibility_area

def potential_avoided_carbon_emission(AD_result_path, repro_clipped_agb_path):
    # Open avoided deforestation project footprint and read its properties
    with rasterio.open(AD_result_path) as src1:
        AD_footprint_data = src1.read(1)
        AD_footprint_profile = src1.profile
    # Open above-ground biomass and read its properties
    with rasterio.open(repro_clipped_agb_path) as src2:
        AGB_data = src2.read(1)
        AGB_profile = src2.profile
        
    # Resample avoided deforestation project footprint to match the resolution of above-ground biomass
    resampled_AD_footprint_data = np.empty(shape=(src2.height, src2.width), dtype=AD_footprint_data.dtype)
    reproject(
        source=AD_footprint_data,
        destination=resampled_AD_footprint_data,
        src_transform=src1.transform,
        src_crs=src1.crs,
        dst_transform=src2.transform,
        dst_crs=src2.crs,
        resampling=Resampling.nearest
    )

    # Create a mask from the resampled avoided deforestation project footprint (consider non-zero values as mask)
    mask = resampled_AD_footprint_data != 0

    # Apply mask to above-ground biomass
    masked_AGB_data = np.where(mask, AGB_data, 0)

    # Save the masked Raster 2
    with rasterio.open(masked_agb_path, 'w', **src2.profile) as dst:
        dst.write(masked_AGB_data, 1)

    total_agb = calculate_stats_pixel_value(masked_agb_path)[3]
    total_biomass = total_agb + (total_agb * 0.27)
    total_co2eq = total_biomass * 0.47 * 3.667

    output = {
        "total_co2eq": float(round(total_co2eq, 2)),
        "project_duration": project_duration
    }

    return output

def potential_c_seq_natural_reg(repro_arr_footprint_path, repro_clipped_c_seq_path, repro_clipped_agb_path, project_duration):
    # Open ARR footprint area layer
    with rasterio.open(repro_arr_footprint_path) as src1:
        ARR_footprint_data = src1.read(1)
        ARR_footprint_profile = src1.profile
    # Open carbon sequestration rate from natural regeneration layer
    with rasterio.open(repro_clipped_c_seq_path) as src2:
        c_seq_data = src2.read(1)
        c_seq_profile = src2.profile
    # Open aboveground biomass layer
    with rasterio.open(repro_clipped_agb_path) as src3:
        AGB_data = src3.read(1)
        AGB_profile = src3.profile

    # Resample ARR footprint area to match the resolution of carbon sequestration rate layer
    resampled_ARR_footprint_data = np.empty(shape=(src2.height, src2.width), dtype = ARR_footprint_data.dtype)
    reproject(
        source = ARR_footprint_data,
        destination = resampled_ARR_footprint_data,
        src_transform = src1.transform,
        src_crs = src1.crs,
        dst_transform = src2.transform,
        dst_crs = src2.crs,
        resampling=Resampling.nearest
    )

    # Resample aboveground biomass layer to match the resolution of carbon sequestration rate layer
    resampled_AGB_data = np.empty(shape=(src2.height, src2.width), dtype = AGB_data.dtype)
    reproject(
        source = AGB_data,
        destination = resampled_AGB_data,
        src_transform = src3.transform,
        src_crs = src3.crs,
        dst_transform = src2.transform,
        dst_crs = src2.crs,
        resampling = Resampling.nearest
    )

    # Apply mask
    masked_c_seq_data = np.where(resampled_ARR_footprint_data == 1, c_seq_data, 0)
    masked_AGB_data = np.where(resampled_ARR_footprint_data == 1, resampled_AGB_data, 0)

    with rasterio.open(masked_arr_repro_clipped_agb, 'w', **src3.profile) as dst:
        dst.write(masked_AGB_data, 1)

    with rasterio.open(masked_c_seq_path, 'w', **src2.profile) as dst:
        dst.write(masked_c_seq_data, 1)

    total_agb = calculate_stats_pixel_value(masked_arr_repro_clipped_agb)[3]
    total_biomass = total_agb + (total_agb * 0.27)
    total_c_seq_rate = calculate_stats_pixel_value(masked_c_seq_path)[3]
    total_c_seq = total_c_seq_rate * 100 * project_duration
    #total_co2eq = (total_biomass * 0.47 * 3.667) + (total_c_seq * 3.667)
    total_co2eq = total_c_seq * 3.667

    output = {
        "total_co2eq": float(round(total_co2eq, 2)),
        "proj_duration": project_duration
    }

    return output

def get_ecosystem_services(project_duration, c_wyield_path, c_av_ero_path, session_id):
    # Given year
    given_year = 2020 + project_duration

    wyield_year_list = dict([(2020, 0), (2030, 1), (2035, 2), (2040, 3),
                            (2045, 4), (2050, 5), (2055, 6), (2060, 7),
                            (2065, 8), (2070, 9), (2075, 10), (2080, 11),
                            (2085, 12), (2090, 13), (2095, 14), (2100, 15)])
    
    def find_nearest_year_band(target_year, start_year=2020, interval=5):
        # Find the nearest year  
        nearest_year = start_year + round((target_year - start_year) / interval) * interval
        
        # Calculate the band index (1-based index)
        band_index = wyield_year_list.get(given_year)

        return band_index, nearest_year
    
    # Find the band index for the nearest year
    band_index, nearest_year = find_nearest_year_band(given_year)

    # Read the baseline water yield raster layer
    with rasterio.open(c_wyield_path) as src:
        wyield_baseline = src.read(1)
        wyield_bau = src.read(band_index + 1)
        wyield_profile = src.profile

    with rasterio.open(wyield_baseline_path, 'w', **src.profile) as out_raster:
        out_raster.write(wyield_baseline, 1)
    with rasterio.open(wyield_bau_path, 'w', **src.profile) as out_raster:
        out_raster.write(wyield_bau, 1)

    wyield_baseline_value = calculate_stats_pixel_value(wyield_baseline_path)[3]
    wyield_bau_value = calculate_stats_pixel_value(wyield_bau_path)[3]
    if wyield_baseline_value > 0:
        wyield_perc_change = (wyield_bau_value - wyield_baseline_value) / wyield_baseline_value * 100
    else:
        wyield_perc_change = 0

    # Read the baseline avoided erosion raster layer
    with rasterio.open(c_av_ero_path) as src1:
        av_ero_baseline = src1.read(1)
        av_ero_bau = src1.read(band_index + 1)
        av_ero_profile = src1.profile

    with rasterio.open(av_ero_baseline_path, 'w', **src1.profile) as out_raster:
        out_raster.write(av_ero_baseline, 1)
    with rasterio.open(av_ero_bau_path, 'w', **src1.profile) as out_raster:
        out_raster.write(av_ero_bau, 1)

    av_ero_baseline_value = calculate_stats_pixel_value(wyield_baseline_path)[2]
    av_ero_bau_value = calculate_stats_pixel_value(wyield_bau_path)[2]

    if av_ero_baseline_value > 0:
        av_ero_perc_change = (av_ero_bau_value - av_ero_baseline_value) / av_ero_baseline_value * 100
    else:
        av_ero_perc_change = 0

    res_annual_water_yield = {
        "improve_water_yield": float(round(wyield_perc_change, 1)),
        "reduce_erosion": float(round(av_ero_perc_change, 1)),
        "project_duration": project_duration
    }

    return res_annual_water_yield

def run_benefit(parameters):
    AD_result_path = ""
    res_potential_avoided = 0

    now = datetime.datetime.now()
    print("Start time: " + now.strftime("%Y-%m-%d %H:%M:%S"))
    
    start_time = time.time()
    
    start_section_time = time.time()
    preparing_data(parameters)
    print("--- %s seconds --- preparing_data results" % (time.time() - start_section_time))

    if interventions.count("Avoided Deforestation") > 0:
        start_section_time = time.time()
        AD_result_path, proj_defor, projected_forest_cover, res_avoided_defor = avoided_deforestation(repro_clipped_fc_start_path, repro_clipped_fc_end_path, project_duration, repro_clipped_defrisk_path)
        print("--- %s seconds --- avoided_deforestation results" % (time.time() - start_section_time))

        start_section_time = time.time()
        res_potential_avoided = potential_avoided_carbon_emission(AD_result_path, repro_clipped_agb_path)
        print("--- %s seconds --- potential_avoided_carbon_emission results" % (time.time() - start_section_time))

    start_section_time = time.time()
    res_eligibility_area = get_eligibility_data(AD_result_path)
    print("--- %s seconds --- get_eligibility_data results" % (time.time() - start_section_time))

    start_section_time = time.time()
    res_potential_sequestration = potential_c_seq_natural_reg(repro_arr_footprint_path, repro_clipped_c_seq_path, repro_clipped_agb_path, project_duration)
    print("--- %s seconds --- potential_c_seq_natural_reg results" % (time.time() - start_section_time))

    start_section_time = time.time()
    res_ecosystem_services = get_ecosystem_services(project_duration, c_wyield_path, c_av_ero_path, session_id)
    print("--- %s seconds --- get_ecosystem_services results" % (time.time() - start_section_time))

    print("--- %s seconds --- end results" % (time.time() - start_time))

    result = {
        "site_information": {
            "land_features": res_eligibility_area
        },
        "nature": {
            "area_of_habitat": {
                "avoided_deforestation": res_eligibility_area["eligible_avdef_ha"],
                "ecosystem_restoration": res_eligibility_area["eligible_ecosystem_restoration_ha"]
            }
        },
        "climate": {
            "potential_avoided": res_potential_avoided,
            "potential_sequestered": res_potential_sequestration
        },
        "people": {
            "ecosystem_services": res_ecosystem_services
        }
    }

    remove_process_folder(parameters.get('session_id'))

    now = datetime.datetime.now()
    print("End time: " + now.strftime("%Y-%m-%d %H:%M:%S"))
    
    return result
# main functions -end-