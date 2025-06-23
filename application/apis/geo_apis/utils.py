# application/apis/geo_apis/logic.py
from flask import jsonify, request, make_response, current_app
from ... import db
from ...models.geos_models.models import Polygons, DataAnalyzer, MapExplorer
from ...models.user_models.models import User, UserSessions, SessionsAuth
from ...models.master_models.models import DocumentList, Settings

import os
import gc
import zipfile
import base64
import json
import string

import fiona
import pandas as pd

import geopandas as gpd
import calendar as cal

from datetime import datetime, timedelta, timezone
from pyproj import CRS
from shapely import Polygon, MultiPolygon
from shapely import wkb, box
from shapely.ops import unary_union
from shapely.geometry import shape
from fiona.drvsupport import supported_drivers

from ...utils.common import AppMessageException
from ...utils.common import get_date
from ...utils.geos import GeoUtils

class GeoLogic():

    @staticmethod
    def form_project_area_result(area_size, user_id):
        project_area = dict()
        area_size = area_size if area_size else 0

        MAX_DRAW_AREA = Settings.find_by_name(name='MAX_DRAW_AREA')

        try:
            size_limit = int(MAX_DRAW_AREA.value)
        except Exception as e:
            current_app.logger.info('geo logic: {}'.format(str(e)))
            raise Exception('max draw area settings invalid or not found')

        known_user = User.query.filter_by(id=user_id).first()
        if known_user:
            size_limit = known_user.size_limit if known_user.size_limit else size_limit
        
        project_area["size"] = area_size
        project_area["limit"] = size_limit
        project_area["exceed"] = True if float(area_size) > float(size_limit) else False

        return project_area
    
    @staticmethod
    def calculate_project_area_db(session_id, user_id):
        area_size = 0
        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if known_polygons:
            geom = known_polygons.geom.desc

            query = '''
            select 
                project_area, district, province, country 
            from sea.v1_current_condition_highlight_site_information_adm('{aoi_geom}')
            '''.format(aoi_geom=geom)

            dt = GeoUtils.get_db(db.text(query))

            for row in dt:
                area_size = round(row.get('project_area'), 2)

        return GeoLogic.form_project_area_result(area_size, user_id)
    

    @staticmethod
    def calculate_project_area_geom(geom, user_id):
        area_size = 0
        geom = str(geom["features"][0]["geometry"]).replace("'", "\"")

        query = '''
        select
            area_size
        from sea.v1_get_area_size('{aoi}')
        '''.format(aoi=geom)

        dt = GeoUtils.get_db(db.text(query))

        for row in dt:
            area_size = row.get('area_size')

        return GeoLogic.form_project_area_result(area_size, user_id)


    @staticmethod
    def process_zip_and_get_polygon(filepath, session_id, upload_folder):
        temp_dir = 'temp_zip_extraction'
        extracted_filepath = os.path.join(upload_folder, session_id, temp_dir) # "Uploaded-File/"+session_id+"/"+temp_dir
        os.makedirs(extracted_filepath, exist_ok=True)
        
        try:
            with zipfile.ZipFile(filepath, 'r') as zip_file:
                zip_file.extractall(extracted_filepath)

            # check if ada file shp didalem zip, return error if not
            filename = None
            for root, dirs, files in os.walk(extracted_filepath):
                for file in files:
                    if not file.startswith('.') and file.endswith('.shp'):
                        filename = os.path.join(root, file)
            if not filename:
                raise AppMessageException('No .shp file found in the ZIP file.')

            # check if crs epsg != 4326 return error
            aoi = gpd.read_file(filename)
            aoi = aoi.to_crs(4326)
            """ 
            gdf = gpd.read_file(filename)
            gdf = gdf.dissolve()
            gdf = gdf.explode(index_parts=True).iloc[[0]]
            gdf4326 = gdf.to_crs(4326)
            _drop_z = lambda geom: wkb.loads(wkb.dumps(geom, output_dimension=2))
            gdf4326.geometry = gdf4326.geometry.transform(_drop_z) 
            """
            aoi_union = aoi.unary_union
            aoi_union_proj = gpd.GeoDataFrame(geometry=[aoi_union])
            # print(aoi_union_proj.geometry.transform)
            _drop_z = lambda geom: wkb.loads(wkb.dumps(geom, output_dimension=2))
            # aoi_union_proj.geometry = aoi_union_proj.geometry.transform(_drop_z)
            aoi_union_proj["geometry"] = aoi_union_proj["geometry"].apply(_drop_z)

            return aoi_union_proj.to_json()
        except Exception as e:
            raise e
        finally:
            GeoUtils.user_remove_upload_tree_file(upload_folder, session_id)

    @staticmethod
    def isvalid(geom)-> int:
        try:
            shape(geom)
            return 1
        except:
            return 0
    
    @staticmethod
    def process_kml_and_get_polygon(filepath, session_id, upload_folder):
        try:
            supported_drivers['kml'] = 'rw' # enable KML support which is disabled by default
            supported_drivers['KML'] = 'rw' # enable KML support which is disabled by default
            supported_drivers['libkml'] = 'rw' # enable KML support which is disabled by default
            supported_drivers['LIBKML'] = 'rw' # enable KML support which is disabled by default

            collection = list(fiona.open(filepath, 'r'))
            df = pd.DataFrame(collection)

            df["is_valid"] = df['geometry'].apply(lambda x: GeoLogic.isvalid(x))
            df_valid = df[df['is_valid'] == 1]
            collection = json.loads(df_valid.to_json(orient='records'))

            gdf = gpd.GeoDataFrame.from_features(collection,crs=CRS('EPSG:4326'))

            #gdf = gpd.read_file(filepath)
            gdf = gdf.dissolve()
            gdf = gdf.explode(index_parts=False)

            gdf4326 = gdf.to_crs(4326)
            # _drop_z = lambda geom: wkb.loads(wkb.dumps(geom, output_dimension=2))
            # gdf4326.geometry = gdf4326.geometry.transform(_drop_z)

            _drop_z = lambda geom: wkb.loads(wkb.dumps(geom, output_dimension=2))
            # aoi_union_proj.geometry = aoi_union_proj.geometry.transform(_drop_z)
            gdf4326["geometry"] = gdf4326["geometry"].apply(_drop_z)

            return gdf4326.to_json()
        except Exception as e:
            raise e
        finally:
            GeoUtils.user_remove_upload_tree_file(upload_folder, session_id)
    

    @staticmethod
    def process_kmz_and_get_polygon(filepath, session_id, upload_folder):
        temp_dir = 'temp_zip_extraction'
        extracted_filepath = os.path.join(upload_folder, session_id, temp_dir) # "Uploaded-File/"+session_id+"/"+temp_dir
        os.makedirs(extracted_filepath, exist_ok=True)

        try:
            with zipfile.ZipFile(filepath, 'r') as zip_file:
                zip_file.extractall(extracted_filepath)

            # check if ada file kml didalem zip, return error if not
            filename = None
            for root, dirs, files in os.walk(extracted_filepath):
                for file in files:
                    if not file.startswith('.') and file.endswith('.kml'):
                        filename = os.path.join(root, file)
            if not filename:
                raise AppMessageException('No .kml file found in the ZIP file.')

            gdf4326 = GeoLogic.process_kml_and_get_polygon(filename, session_id, upload_folder)

            return gdf4326
        except Exception as e:
            raise e
        finally:
            GeoUtils.user_remove_upload_tree_file(upload_folder, session_id)
    

    @staticmethod
    def process_input_explore_map(session_id, project_duration, estimated_unplanned_deforestation, intervention, rest_target: list):
        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            return 404, "session id not found, please input polygon with session id first"
        
        status_code = 201 # HTTPStatus.CREATED
        message = 'successfully'
        
        known_map_explorer = MapExplorer.query.filter_by(session_id=session_id).first()
        if known_map_explorer:
            status_code = 200
            message = 'updated, details is updated succesfully'
        else:
            known_map_explorer = MapExplorer()
            known_map_explorer.session_id = session_id
        
        known_map_explorer.project_duration = project_duration,
        known_map_explorer.estimated_unplanned_deforestation = estimated_unplanned_deforestation
        known_map_explorer.rest_target = rest_target
        known_map_explorer.intervention = intervention

        db.session.add(known_map_explorer)
        db.session.commit()

        return status_code, message


    @staticmethod
    def construct_polygon(session_id):
        geom_str = Polygons.find_by_session_id(session_id)
        geom = Polygons.get_geometry(session_id).first()

        res = json.loads(geom[0])
        polygon_types = res['type']
        geom_coords = res['coordinates']
        geom = False
        
        if polygon_types.lower() == "multipolygon":
            geom = shape(res)
        elif polygon_types.lower() == "polygon":
            geom = Polygon(geom_coords[0])

        gdf4326 = gpd.GeoDataFrame(index=[0], crs=CRS('EPSG:4326'), geometry=[geom])
        
        return geom_str, gdf4326


    @staticmethod
    def coords_to_string(longitude, latitude):
        lat_degrees = int(longitude)
        lat_minutes = int((longitude - lat_degrees) * 60)
        lat_seconds = (longitude - lat_degrees - lat_minutes / 60) * 3600
        lat_direction = 'N' if longitude >= 0 else 'S'
        
        lon_degrees = int(latitude)
        lon_minutes = int((latitude - lon_degrees) * 60)
        lon_seconds = (latitude - lon_degrees - lon_minutes / 60) * 3600
        lon_direction = 'E' if latitude >= 0 else 'W'

        lat_sec = "{:.1f}".format(lat_seconds).split('.')
        lat_sec = '{}.{}'.format(lat_sec[0].rjust(2, '0'), lat_sec[1])
        lon_sec = "{:.1f}".format(lon_seconds).split('.')
        lon_sec = '{}.{}'.format(lon_sec[0].rjust(2, '0'), lon_sec[1])

        dms_string = '''{}°{}'{}"{} {}°{}'{}"{}'''.format(
            str(abs(lat_degrees)).rjust(2, '0'), str(abs(lat_minutes)).rjust(2, '0'), lat_sec, lat_direction,
            str(abs(lon_degrees)).rjust(2, '0'), str(abs(lon_minutes)).rjust(2, '0'), lon_sec, lon_direction
        )
        dms_split = dms_string.split(' ')
        
        return {
            'longitude': dms_split[0],
            'latitude': dms_split[1],
        }
        # return f"{abs(lat_degrees)}°{abs(lat_minutes)}'{"{:.2f}".format(lat_seconds)}\"{lat_direction} {abs(lon_degrees)}°{abs(lon_minutes)}'{"{:.2f}".format(lon_seconds)}\"{lon_direction}"


    @staticmethod
    def handle_section_data(known_document):
        if known_document.section_2.get('emission_reduction') and type(known_document.section_2.get('emission_reduction')) == list:
            known_document.section_2['emission_reduction'] = [n for n in known_document.section_2['emission_reduction'] if n]
            
    
    @staticmethod
    def get_template_data(session_id, sections):
        data = dict()

        known_user_session = UserSessions.find_by_session_id(session_id)
        if not known_user_session:
            return data

        for i in range(1, 5):
            if len(sections) > 0:
                data["tpl_section_" + str(i)] = True if i in sections else False
            else:
                data["tpl_section_" + str(i)] = True

        document_list = DocumentList.find_by_project_id_and_document_type(session_id, "General", "all")        
        doc_version = 1
        
        if document_list:
            if len(document_list) > 0:
                doc_version = len(document_list) + 1

        data["doc_version"] = doc_version
        
        now = datetime.now(timezone.utc).replace(tzinfo=None)
        data['current_year'] = now.year
        
        geom_str, geom_gdf = GeoLogic.construct_polygon(session_id)

        # data["geom_min_x"] = dd2dms(geom_gdf.bounds.minx[0], 'x')
        # data["geom_max_x"] = dd2dms(geom_gdf.bounds.maxx[0], 'x')
        # data["geom_min_y"] = dd2dms(geom_gdf.bounds.miny[0], 'y')
        # data["geom_max_y"] = dd2dms(geom_gdf.bounds.maxy[0], 'y')

        # project_map = construct_project_area_map(session_id, geom_gdf)

        data["issued_date"] = datetime.today().strftime('%d-%B-%Y')
        # data["user_id"] = known_user_session.user_id
        data["project_name"] = known_user_session.project_name

        user = User.query.filter_by(id=known_user_session.user_id).first()

        if user:
            print(user.extended_data)
            data["prepared_by"] = user.organization_name if user.organization_name else user.name

            contact_info = []
            if user.extended_data.get('address'):
                contact_info.append(str(user.extended_data.get('address')))
            if user.extended_data.get('city'):
                contact_info.append(str(user.extended_data.get('city')))
            if user.extended_data.get('country'):
                contact_info.append(str(user.extended_data.get('country')))
            
            data["contact_info"] = '. '.join(contact_info)

            logo = ''
            if user.avatar:
                logo = base64.b64encode(user.avatar).decode('ascii')

            data["logo"] = logo

            analyzer = DataAnalyzer.find_by_session_id(session_id)

            if analyzer:
                geom_center = box(*geom_gdf.total_bounds).centroid

                # Site Information - Administrative Boundaries Data
                district = string.capwords(analyzer.site_information["administrative_boundaries"]["district"])
                province = string.capwords(analyzer.site_information["administrative_boundaries"]["province"])
                country = string.capwords(analyzer.site_information["administrative_boundaries"]["country"])
                data['administrative_boundaries'] = analyzer.site_information["administrative_boundaries"]

                data['geom_center'] = { 'longitude': geom_center.x, 'latitude': geom_center.y }
                data['geom_center_dms'] = GeoLogic.coords_to_string(geom_center.x, geom_center.y)
                data["geom_min_dms"] = GeoLogic.coords_to_string(geom_gdf.bounds.minx[0], geom_gdf.bounds.miny[0])
                data["geom_max_dms"] = GeoLogic.coords_to_string(geom_gdf.bounds.maxx[0], geom_gdf.bounds.maxy[0])

                location = district + ", " + province + ", " + country
                
                data["project_location"] = location
                data["district"] = district
                data["province"] = province
                data["country"] = country

                # data["forest_pct"] = analyzer.site_information["administrative_boundaries"]["forest_pct"]
                data["area_size"] = analyzer.site_information["administrative_boundaries"]["project_area"]

                # protect_pct = analyzer.site_information["administrative_boundaries"]["protect_pct"]
                # data["protect_pct"] = f"{protect_pct:,.2f}"

                # Site Information - Elevation Data
                data["top_elevation"] = [i for i in analyzer.site_information["elevation"]["elevation_detail"][:1]]

                # Site Information - Land Cover Data
                # land_cover = [i['lc_class'] for i in analyzer.site_information["land_cover"][:3]]
                # data["top_land_cover"] = GeoUtils.join_and(land_cover)
                # data["land_covers"] = analyzer.site_information["land_cover"]
                land_cover = [i['area_ha'] + " hectares (" + i["area_pct"] + "%) of " + i["lc_class"].lower() for i in analyzer.site_information["land_cover"]]
                data["land_covers"] = GeoUtils.join_and(land_cover, " and ")

                data["top_land_cover"] = [i for i in analyzer.site_information["land_cover"][:2]]

                # Site Information - Historical Deforestation Data
                data["deforestation_rate"] = analyzer.site_information["annual_deforestation_rate"]

                deforestation_graph_data = analyzer.site_information["annual_deforestation_rate"]["graph_data"]
                deforestation_graph_categories = list(range(2011, 2011 + 11, 1))
                GeoUtils.create_graph(session_id, "deforestation", deforestation_graph_data, deforestation_graph_categories, "#FC8D59")

                # Site Information - Driver of Deforestation Data
                driver = analyzer.site_information["driver_of_deforestation"]["driver_list"]
                data["more_than_1_driver"] = len(driver)
                data["driver_of_deforestation"] = driver

                driver_non_top = [i['driver'].lower() + " (" + i["area_pct"] + "%)" for i in analyzer.site_information["driver_of_deforestation"]["driver_list"]]
                del driver_non_top[0]
                data["other_driver_of_deforestation"] = GeoUtils.join_and(driver_non_top, " and ")

                # Site Information - Deforestation Risk Data
                data["deforestation_risk"] = analyzer.site_information["deforestation_risk"]["risk_type"].replace(" Risk Level", "")

                # Site Information - Peatland and Mangrove Data
                # data["peatland"] = analyzer.site_information["peatland_mangrove"]["peatland"]
                # data["peatland_pct"] = analyzer.site_information["peatland_mangrove"]["peatland_pct"]
                # data["mangrove"] = analyzer.site_information["peatland_mangrove"]["mangrove"]
                # data["mangrove_pct"] = analyzer.site_information["peatland_mangrove"]["mangrove_pct"]
                data["has_peatland"] = True if (analyzer.site_information["peatland_mangrove"]["peatland"] > 0) | (analyzer.site_information["peatland_mangrove"]["mangrove"] > 0) else False
                data["peatland_mangrove"] = analyzer.site_information["peatland_mangrove"]

                month_names = [cal.month_abbr[i] for i in range(1, 13)]
                data['month_name'] = month_names

                # Climate - Precipitation Data
                data["precipitation"] = analyzer.climate["precipitation"]
                precipitation_graph_data = analyzer.climate["precipitation"]["graph_data"]
                precipitation_graph_categories = month_names
                GeoUtils.create_graph(session_id, "precipitation", precipitation_graph_data, precipitation_graph_categories, "#004BC2")
                if data["precipitation"] and precipitation_graph_data: # extend for ccb
                    data['precipitation']['graph'] = {
                        'max': max(precipitation_graph_data),
                        'min': min(precipitation_graph_data),
                        'mean': sum(precipitation_graph_data) / len(precipitation_graph_data),
                    }
                    data['precipitation']['graph']['max_month'] = month_names[precipitation_graph_data.index(data['precipitation']['graph']['max'])]
                    data['precipitation']['graph']['min_month'] = month_names[precipitation_graph_data.index(data['precipitation']['graph']['min'])]
                    
                    # https://typeset.io/questions/what-is-a-precipitation-threshold-for-a-rainy-day-g0xprobfx4
                    # find dry season
                    peak_dry_season_index = precipitation_graph_data.index(data['precipitation']['graph']['min'])
                    start_dry_season_index = peak_dry_season_index
                    end_dry_season_index = peak_dry_season_index
                    total_dry_season_month = 0
                    list_month_from_peak = [(n+peak_dry_season_index)%len(month_names) for n in range(len(month_names))]
                    for month_index in list_month_from_peak[::-1]:
                        if abs(precipitation_graph_data[month_index]-precipitation_graph_data[(month_index+len(month_names)-1)%12]) > 30:
                            start_dry_season_index = month_index
                            total_dry_season_month += list_month_from_peak.index(month_index)+1
                            break
                    for month_index in list_month_from_peak:
                        if abs(precipitation_graph_data[month_index]-precipitation_graph_data[(month_index+1)%12]) > 30: # adjustable param
                            end_dry_season_index = month_index
                            total_dry_season_month += list_month_from_peak.index(month_index)
                            break
                    # end find dry season

                    data['precipitation']['graph']['dry_season'] = {
                        'peak': peak_dry_season_index,
                        'start': start_dry_season_index,
                        'end': end_dry_season_index,
                        'peak_month': month_names[peak_dry_season_index],
                        'start_month': month_names[start_dry_season_index],
                        'end_month': month_names[end_dry_season_index],
                        'category': 'short' if total_dry_season_month <= 3 else 'long' # adjustable param
                    }
                
                
                # Climate - Temperature Data
                data["temperature"] = analyzer.climate["temperature"]
                temperature_graph_data = analyzer.climate["temperature"]["graph_data"]
                temperature_graph_categories = month_names
                GeoUtils.create_graph(session_id, "temperature", temperature_graph_data, temperature_graph_categories, "#DA4738")
                if data['temperature'] and temperature_graph_data: # extend for ccb
                    data['temperature']['graph'] = {
                        'max': max(temperature_graph_data),
                        'min': min(temperature_graph_data),
                        'mean': sum(temperature_graph_data) / len(temperature_graph_data)
                    }
                    data['temperature']['graph']['max_month'] = month_names[temperature_graph_data.index(data['temperature']['graph']['max'])]
                    data['temperature']['graph']['min_month'] = month_names[temperature_graph_data.index(data['temperature']['graph']['min'])]

                # Climate - Carbon Storage
                data["carbon_storage"] = analyzer.climate["carbon_storage"]

                # Climate - Burned Area
                data["burned_area"] = analyzer.climate["burned_area"]

                # Nature - FLII Data
                # data["flii_index"] = analyzer.nature["flii"]["index"]
                data["flii"] = dict()
                data["flii"]["index"] = analyzer.nature["flii"]["index"]
                data["flii"]["integrity"] = analyzer.nature["flii"]["integrity"].replace(" Integrity", "")
                data["flii"]["meaning"] = analyzer.nature["flii"]["meaning"].replace("Meaning, it may be more suitable for ", "")

                # Nature - KBA Data
                data["kba"] = analyzer.nature["kba"]
                data["kba"]["within"] = True if analyzer.nature["kba"]["area_ha_plain"] > 0 else False
                data["kba"]["within_str"] = "within" if analyzer.nature["kba"]["area_ha_plain"] > 0 else "not within"


                # Nature - Richness Data
                endangered_species = analyzer.nature["richness"]["endangered"]
                data["endangered_species"] = f"{endangered_species:,.0f}"

                # flora = analyzer.nature["richness"]["species"]
                # data["flora"] = f"{flora:,.0f}"

                # Nature - Wildlife Data
                data["wildlife"] = analyzer.nature["wildlife"]

                # Nature - TCL Data
                tcl = analyzer.nature["tcl"]
                data["tcl"] = tcl
                data["tcl"]["has_tcl"] = True if len(tcl["items"]) > 0 else False
                
                # People - Demography Data
                # population = analyzer.people["demography"]["population"]
                # men_count = analyzer.people["demography"]["pria"]
                # women_count = analyzer.people["demography"]["wanita"]

                # data["population"] = population
                # data["men_count"] = f"{men_count:,.0f}"
                # data["women_count"] = f"{women_count:,.0f}"
                data["demography"] = analyzer.people["demography"]

                # People - Ethnicity Data
                data["has_ethnic"] = True if len(analyzer.people["ethnicity"]) > 0 else False
                data["ethnics"] = GeoUtils.join_and(analyzer.people["ethnicity"])

                # People - Education
                data["education"] = analyzer.people["education"]

                # People - Employment
                data["employment"] = analyzer.people["employment"]
                data["employment"]["top_3_sektor"] = GeoUtils.join_and(analyzer.people["employment"]["top_3_sektor"])

                # People - Health
                data["health"] = analyzer.people["health"]

                # Benefit - Site Information - land features
                data["benefit"] = dict()
                data["benefit"]["eligibility"] = dict()
                data["benefit"]["eligibility"]["avdef"] = analyzer.benefit["site_information"]["land_features"]["eligible_avdef_ha"]
                data["benefit"]["eligibility"]["avdef_pct"] = analyzer.benefit["site_information"]["land_features"]["eligible_avdef_pct"]
                data["benefit"]["eligibility"]["ecores"] = analyzer.benefit["site_information"]["land_features"]["eligible_ecosystem_restoration_ha"]
                data["benefit"]["eligibility"]["ecores_pct"] = analyzer.benefit["site_information"]["land_features"]["eligible_ecosystem_restoration_pct"]

                # Benefit - Nature - Area of Habitat
                data["benefit"]["aoh"] = analyzer.benefit["nature"]["area_of_habitat"]
                
                # Benefit - Climate - Carbon Storage Data
                if analyzer.benefit["climate"]["potential_avoided"] != 0:
                    data["carbon_avoided"] = analyzer.benefit["climate"]["potential_avoided"]["total_co2eq"]
                else:
                    data["carbon_avoided"] = 0
                data["carbon_sequestered"] = analyzer.benefit["climate"]["potential_sequestered"]["total_co2eq"]

                # Benefit - People - Erotion and Water Yield Data
                data["erotion_ratio"] = analyzer.benefit["people"]["ecosystem_services"]["reduce_erosion"]
                data["water_yield_ratio"] = analyzer.benefit["people"]["ecosystem_services"]["improve_water_yield"]

                benefit_params = MapExplorer.find_by_session_id(session_id)

                if benefit_params:
                    data["project_duration"] = benefit_params.project_duration

                    data["avdef"] = True if "Avoided Deforestation".lower() in map(str.lower, benefit_params.intervention) else False
                    data["ecores"] = True if "Ecosystem Restoration".lower() in map(str.lower, benefit_params.intervention) else False

                    data["interventions"] = GeoUtils.join_and(benefit_params.intervention, " and/or ")

                    data["doc_title"] = district + " " + GeoUtils.join_and(benefit_params.intervention)

        return data
    

    @staticmethod
    def transform_ghg_data(data):

        source_label = ['Burning of woody biomass', 'Combustion', 'Use of Fertilizer']
        keys = ['redd_gas_site1', 'arr_gas_site1', 'wrc_gas_site1']

        for process_key in keys:
            if data.section_3 and data.section_3.get(process_key):
                data.section_3['{}_ghg_data'.format(process_key.split('_')[0])] = [{
                    'source': [
                        {
                            'name': n,
                            'gas': [
                                {
                                    'name': str(d).upper(), 
                                    'included-excluded': data.section_3.get(process_key)[i].get(d).get('included-excluded') or '',
                                    'justification': data.section_3.get(process_key)[i].get(d).get('justification') or '',
                                } for d in data.section_3.get(process_key)[i].keys()
                            ]
                        } for i, n in enumerate(source_label)
                    ],
                    'site_id': 1
                }]
    

    @staticmethod
    def summarize_fauna(data):

        category_name_label = ['Birds', 'Mammals', 'Amphibians', 'Reptiles']
        
        data.section_5['project_fauna'] = []
        data.section_5['summary_fauna'] = []
        for category_name in category_name_label:
            if data.section_5 and data.section_5.get('proj_{}'.format(category_name.lower())):
                o = {
                    'species': [],
                    'iucn': [],
                    'category_name': category_name
                }

                for d in data.section_5.get('proj_{}'.format(category_name.lower())):
                    o['species'].append(str(d.get('species')))
                    o['iucn'].append(str(d.get('iucn')))
                    
                    data.section_5['summary_fauna'].append(d)
                
                o['species'] = ', '.join(o['species'])
                o['iucn'] = ', '.join(o['iucn'])

                data.section_5['summary_fauna'].append(o)