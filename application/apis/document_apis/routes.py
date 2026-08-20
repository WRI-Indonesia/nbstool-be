# application/apis/master_apis/documents/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from . import document_apis_blueprint
from ... import db
from ...models.master_models.models import DocumentList
from ...models.user_models.models import UserSessions
from ...models.geos_models.models import MapExplorer, DataAnalyzer, Polygons

from ..geo_apis.layers.routes import func_get_layers_v2

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename
from pathlib import Path
from io import StringIO, BytesIO
from osgeo import ogr, osr

import os
import gc
import uuid
import json
import base64
import csv

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler
from ...utils.geopdf import generate_geopdf

from . import gcs

# legacy: /nbsapi/front-service/read/docx [POST]
@document_apis_blueprint.route('/docx', methods=['GET'])
@cross_origin()
def documents_get_document_docx():
    g_var.__api_name__ = 'documents_get_document_docx'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    g_var.__request_data__ = request.args.to_dict()

    try:
        data = request.args

        document_id = data.get('document_id')

        known_document = DocumentList.find_by_document_id(document_id)
        if not known_document:
            raise AppMessageException('File not found')
        
        g_var.__session_id__ = known_document.project_id
        
        g_var.__description_data__['type'] = 'Feasibility Report'
        if known_document.document_type.lower() == 'ccb':
            g_var.__description_data__['type'] = 'CCB Documentation'
            folder_path = 'generated-file/docx-ccb/'
        else:
            folder_path = 'generated-file/docx/'
        
        gcs.download(folder_path + known_document.document_name)
        docx_dir = Path(folder_path + known_document.document_name).resolve()
        abs_path = os.path.join(Path(os.path.abspath(os.getcwd())), docx_dir)
        if not os.path.exists(abs_path):
            raise AppMessageException('File not found')
        
        with open(abs_path, 'rb') as f:
            docx_data = f.read()
            b64_string = base64.b64encode(docx_data).decode('utf-8')

        return make_response(jsonify(success_handler({ 'data': b64_string }, status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error



@document_apis_blueprint.route('/docx', methods=['DELETE'])
@cross_origin()
def documents_delete_document_docx():
    g_var.__api_name__ = 'documents_delete_document_docx'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    g_var.__request_data__ = request.args.to_dict()

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        document_id = data.get('document_id')

        known_document = DocumentList.find_by_document_id(document_id)
        if not known_document:
            raise AppMessageException('File not found')
        
        g_var.__session_id__ = known_document.project_id

        known_document.is_active = 0

        g_var.__description_data__['type'] = 'Feasibility Report'
        if known_document.document_type.lower() == 'ccb':
            g_var.__description_data__['type'] = 'CCB Documentation'
            folder_path = 'generated-file/docx-ccb/'
        else:
            folder_path = 'generated-file/docx/'
        
        gcs.delete(folder_path + known_document.document_name)
        docx_dir = Path(folder_path + known_document.document_name).resolve()
        abs_path = os.path.join(Path(os.path.abspath(os.getcwd())), docx_dir)
        if os.path.exists(abs_path):
            docx_dir.unlink()
        else:
            current_app.logger.info('document delete: file not found, cant unlink')

        db.session.add(known_document)
        db.session.commit()
        
        status_code = 200
        message = 'Document has succesfully deleted'
        return make_response(jsonify(success_handler({ 'result': {} }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error



@document_apis_blueprint.route('/csv', methods=['GET'])
@cross_origin()
def documents_get_document_csv():
    g_var.__api_name__ = 'documents_get_document_csv'

    g_var.__log_it__ = False
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    g_var.__request_data__ = request.args.to_dict()

    try:
        data = request.args

        session_id = data.get('session_id')

        user_sessions = UserSessions.find_by_session_id(session_id)
        if not user_sessions:
            raise AppMessageException('invalid project')
        
        g_var.__session_id__ = user_sessions.session_id

        data_analyzer = DataAnalyzer.find_by_session_id(session_id)
        if not data_analyzer:
            raise AppMessageException('data not found [da]')

        map_explorer = MapExplorer.find_by_session_id(session_id)
        if not map_explorer:
            raise AppMessageException('data not found [me]')
        
        # d = {
        #     'site_information': eval('data_analyzer.site_information'),
        #     'nature': eval('data_analyzer.nature'),
        #     'climate': eval('data_analyzer.climate'),
        #     'people': eval('data_analyzer.people'),
        #     'intervention_eligibility': eval('data_analyzer.intervention_eligibility'),
        # }
        # open('/home/del/Downloads/current_condition_8959597c-255e-4af1-bd4e-ac3eda8c8e2f.txt', 'w').write(str(d))
        # open('/home/del/Downloads/benefit_8959597c-255e-4af1-bd4e-ac3eda8c8e2f.txt', 'w').write(str(data_analyzer.benefit))
        # open('/home/del/Downloads/map_explorer_8959597c-255e-4af1-bd4e-ac3eda8c8e2f.txt', 'w').write(str(map_explorer.to_json()))
        # raise Exception(str(d))
        
        mapping_data = {
            'General': {
                'General': [
                    {'name': 'Project Name', 'source': 'user_sessions.project_name', },
                    {'name': 'Area Size', 'source': 'data_analyzer.site_information..administrative_boundaries..project_area'},
                    {'name': 'Project Duration', 'source': 'map_explorer.project_duration'},
                    {'name': 'Intervention Type', 'source': 'map_explorer.intervention'}, # convert list to string
                ]
            },
            'Current Condition': {
                'Site Information': [
                    {'name': 'Country', 'source': 'data_analyzer.site_information..administrative_boundaries..country'},
                    {'name': 'Province', 'source': 'data_analyzer.site_information..administrative_boundaries..province'},
                    {'name': 'District', 'source': 'data_analyzer.site_information..administrative_boundaries..district'},
                    {'name': 'Forest Coverage (%)', 'source': 'data_analyzer.site_information..administrative_boundaries..forest_pct'},
                    {'name': 'Protected Area (%)', 'source': 'data_analyzer.site_information..administrative_boundaries..protect_pct'},
                    {'name': 'Area Topography', 'source': 'data_analyzer.site_information..elevation..elevation_class'},
                    {'name': 'Land Covers', 'source': 'data_analyzer.site_information..land_cover'}, # convert list to string
                    {'name': 'Peatland', 'source': 'data_analyzer.site_information..peatland_mangrove..peatland'},
                    {'name': 'Mangrove', 'source': 'data_analyzer.site_information..peatland_mangrove..mangrove'},
                    {'name': 'Driver of Deforestation', 'source': 'data_analyzer.site_information..driver_of_deforestation..driver_text'},
                    {'name': 'Annual Deforestation (%)', 'source': 'data_analyzer.site_information..annual_deforestation_rate..pct', 'end': '%'},
                    {'name': 'Deforestation Risk Level', 'source': 'data_analyzer.site_information..deforestation_risk..risk_type'},
                    {'name': 'Floods Risk', 'source': 'data_analyzer.site_information..disaster_risk..floods'},
                    {'name': 'Landslide Risk', 'source': 'data_analyzer.site_information..disaster_risk..landslides'},
                    {'name': 'Drought Risk', 'source': 'data_analyzer.site_information..disaster_risk..drought'},
                    {'name': 'Cyclonic Risk', 'source': 'data_analyzer.site_information..disaster_risk..cyclone'},
                ],
                'Nature': [
                    {'name': 'Overlapped KBA', 'source': 'data_analyzer.nature..kba..kba_name'},
                    {'name': 'Overlapped KBA Area', 'source': 'data_analyzer.nature..kba..area_ha'},
                    {'name': 'FLII', 'source': 'data_analyzer.nature..flii..index'},
                    {'name': 'FLII Level', 'source': 'data_analyzer.nature..flii..integrity'},
                    {'name': 'FLII Level Meaning', 'source': 'data_analyzer.nature..flii..meaning'},
                    {'name': 'Total Amphibi', 'source': 'data_analyzer.nature..wildlife..amphibi'},
                    {'name': 'Total Bird', 'source': 'data_analyzer.nature..wildlife..bird'},
                    {'name': 'Total Mammal', 'source': 'data_analyzer.nature..wildlife..mammal'},
                    {'name': 'Total Reptile', 'source': 'data_analyzer.nature..wildlife..reptile'},
                    {'name': 'Endangered Trees', 'source': 'data_analyzer.nature..richness..endangered'},
                    {'name': 'Tiger Conservation Landscape ', 'source': 'data_analyzer.nature..tcl..text'},
                ],
                'Climate': [
                    {'name': 'Minimum Annual Temperature', 'source': 'data_analyzer.climate..temperature..min'},
                    {'name': 'Maximum Annual Temperature', 'source': 'data_analyzer.climate..temperature..max'},
                    {'name': 'Average Annual Temperature', 'source': 'data_analyzer.climate..temperature..mean'},
                    {'name': 'Minimum Precipitation', 'source': 'data_analyzer.climate..precipitation..min'},
                    {'name': 'Maximum Precipitation', 'source': 'data_analyzer.climate..precipitation..max'},
                    {'name': 'Average Precipitation', 'source': 'data_analyzer.climate..precipitation..mean'},
                    {'name': 'Current Carbon Storage Total ', 'source': 'data_analyzer.climate..carbon_storage..carbon_storage_plain'},
                    {'name': 'Current Carbon Storage Total (Walker)', 'source': 'data_analyzer.climate..carbon_storage..other_source_plain'},
                    {'name': 'Above Ground Biomass', 'source': 'data_analyzer.climate..carbon_storage..aboveground_percent', 'end': '%'},
                    {'name': 'Soil Organic Carbon', 'source': 'data_analyzer.climate..carbon_storage..soil_percent', 'end': '%'},
                    {'name': 'Below Ground Biomass', 'source': 'data_analyzer.climate..carbon_storage..belowground_percent', 'end': '%'},
                    {'name': 'Burned Area (10 years)', 'source': 'data_analyzer.climate..burned_area..burn_area'},
                    {'name': 'Average Burned Area Occurrence (10 years)', 'source': 'data_analyzer.climate..burned_area..burn_frequency', 'end': '%'},
                ]
            },
            'Benefit': {
                'Site Information': [
                    {'name': 'Allocated for Avoided Deforestation (ha)', 'source': 'data_analyzer.benefit..site_information..land_features..eligible_avdef_ha'},
                    {'name': 'Allocated for Ecosystem Restoration (ha)', 'source': 'data_analyzer.benefit..site_information..land_features..eligible_ecosystem_restoration_ha'},
                    {'name': 'Non-eligible (ha)', 'source': 'data_analyzer.benefit..site_information..land_features..non_eligible_project_area_ha'},
                ],
                'Nature': [
                    {'name': 'Habitat will be restored under Ecosystem Restoration intervention (ha)', 'source': 'data_analyzer.benefit..nature..area_of_habitat..ecosystem_restoration'},
                    {'name': 'Habitat will be conserved under Avoided Deforestation intervention (ha)', 'source': 'data_analyzer.benefit..nature..area_of_habitat..avoided_deforestation'},
                ],
                'Climate': [
                    {'name': 'Potential Avoided Carbon Emission (tonnes)', 'source': 'data_analyzer.benefit..climate..potential_avoided'},
                    {'name': 'CO2eq pottentially sequestered (tonnes)', 'source': 'data_analyzer.benefit..climate..potential_sequestered..total_co2eq'},
                ],
                'People': [
                    {'name': 'Reduce potential erosion', 'source': 'data_analyzer.benefit..people..ecosystem_services..reduce_erosion', 'end': '%'},
                    {'name': 'Improving water yield', 'source': 'data_analyzer.benefit..people..ecosystem_services..improve_water_yield', 'end': '%'},
                ]
            },
        }

        csvheader = ['category', 'section', 'indicator', 'value']
        ddot_to_dict = lambda s: ''.join([n if i == 0 else "['{}']".format(n) for i, n in enumerate(s.split('..'))])
        csvrow = []

        for category_name in mapping_data.keys():
            category = mapping_data[category_name]
            category_first_index = True

            for section_name in category.keys():
                section = category[section_name]
                section_first_index = True

                for indicator in section:

                    d = {
                        'category': '',
                        'section': '',
                        'indicator': indicator['name'],
                    }

                    if category_first_index:
                        category_first_index = False
                        d['category'] = category_name
                    
                    if section_first_index:
                        section_first_index = False
                        d['section'] = section_name
                    
                    d['value'] = eval(ddot_to_dict(indicator['source']))

                    # custom value
                    if indicator['source'] == 'data_analyzer.site_information..land_cover':
                        d['value'] = '; '.join(['{} ({} ha, {}%)'.format(n['lc_class'], n['area_ha'], n['area_pct']) for n in d['value']])
                    
                    if indicator['source'] == 'map_explorer.intervention':
                        d['value'] = '; '.join(d['value'])
                    # end custom value

                    if indicator.get('end'):
                        d['value'] = '{}{}'.format(d['value'], indicator['end'])

                    csvrow.append(d)
        
        strbuf = StringIO()
        dict_writer = csv.DictWriter(strbuf, csvheader)
        dict_writer.writeheader()
        dict_writer.writerows(csvrow)

        buf = BytesIO(strbuf.getvalue().encode('utf-8'))
        buf.seek(0)

        return send_file(
            buf, 
            as_attachment=True,
            download_name='project_data.csv',
            mimetype='text/csv'
        )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# new
@document_apis_blueprint.route('/geopdf', methods=['GET'])
@cross_origin()
def documents_get_geopdf():
    g_var.__api_name__ = 'documents_get_geopdf'

    g_var.__log_it__ = False
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    g_var.__request_data__ = request.args.to_dict()

    try:
        data = request.args

        session_id = data.get('session_id')
        layers = data.get('layers')
        layers = layers.split(',')
        geopdf_title = data.get('title') if data.get('title') else ''

        known_session = UserSessions.find_by_session_id(session_id)
        if not known_session:
            raise AppMessageException('invalid session')
        
        g_var.__session_id__ = session_id
        
        known_polygons = Polygons.find_by_session_id(known_session.session_id)
        if not known_polygons:
            raise AppMessageException('invalid data')
        
        srs = osr.SpatialReference()
        srs.ImportFromEPSG(known_polygons.geom.srid)
        
        return send_file(
            generate_geopdf(ogr.CreateGeometryFromWkb(known_polygons.geom.as_wkb().data, srs), layers=layers, base_layer_list=func_get_layers_v2(), geopdf_title=geopdf_title),
            as_attachment=True,
            download_name='geo.pdf',
            mimetype='application/pdf'
        )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error