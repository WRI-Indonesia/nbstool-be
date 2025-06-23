# application/apis/geo_apis/features/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var
from flask_login import current_user
from .. import geo_apis_blueprint
from .... import db
from ....models.geos_models.models import Polygons, MapExplorer, DataAnalyzer
from ....models.user_models.models import SessionsAuth, UserSessions

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename

import os
import gc
import uuid
import json
import requests

from shapely.geometry import Polygon

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler
from ....utils.geos import GeoUtils
from ....utils.geos.current_condition import get_eligible_intervention, get_current_condition, set_intervention
from ....utils.geos.current_condition import process_input_data_analyzer_result
from ....utils.geos.benefit import run_benefit

from ..utils import GeoLogic


# legacy: /nbsapi/feature/benefits [POST]
@geo_apis_blueprint.route('/feature/benefits', methods=['GET'])
@cross_origin()
def geo_feature_benefits():
    g_var.__api_name__ = 'geo_feature_benefits'

    try:
        data = request.args

        session_id = data.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')
        
        set_intervention(session_id)

        known_map_explorer = MapExplorer.query.filter_by(session_id=session_id).first()
        project_duration = known_map_explorer.project_duration
        estimated_unplanned_deforestation = known_map_explorer.estimated_unplanned_deforestation
        rest_target = known_map_explorer.rest_target

        geom = db.session.query(db.func.ST_AsGeoJSON(known_polygons.geom)).first()

        new_geometry = json.loads(geom[0])
        geometry_type = new_geometry['type']
        geometry_coordinate = new_geometry['coordinates']
        
        if geometry_type.lower() == "polygon":
            new_geometry = {
                "type": geometry_type,
                "coordinates": geometry_coordinate
            }
        
        new_ses_data = {
            "type": "Feature",
            "session_id": session_id,
            "geometry": new_geometry,
            "properties": {
                "project_duration": project_duration,
                "unavoided_def_rate": int(estimated_unplanned_deforestation),
                "rest_target": rest_target
            }
        }

        # resp_from_geo_service = requests.post(current_app.config.get("BENEFIT_API_URL"), json=new_ses_data, timeout=300)
        # results = resp_from_geo_service.json()
        
        results = run_benefit(new_ses_data)
        
        process_input_data_analyzer_result(session_id=session_id, section="benefit", data=results)

        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/feature/current-condition-new [POST]
@geo_apis_blueprint.route('/feature/current-condition', methods=['GET'])
@cross_origin()
def geo_feature_current_condition():
    g_var.__api_name__ = 'geo_feature_current_condition'

    g_var.__log_it__ = False
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    try:
        g_var.__request_data__ = request.args.to_dict()
    except:
        pass

    try:
        data = request.args

        session_id = data.get('session_id')
        section_type = data.get('section_type')

        g_var.__session_id__ = session_id

        current_condition_data = get_current_condition(session_id, section_type)

        # current_condition_data = get_current_condition(session_id, 'site_information')
        # current_condition_data = get_current_condition(session_id, 'nature')
        # current_condition_data = get_current_condition(session_id, 'climate')
        # current_condition_data = get_current_condition(session_id, 'people')

        return make_response(jsonify(success_handler({ 'result': current_condition_data })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/feature/intervention-eligibility [POST]
@geo_apis_blueprint.route('/feature/intervention-eligibility', methods=['GET'])
@cross_origin()
def geo_feature_intervention_eligibility():
    g_var.__api_name__ = 'geo_feature_intervention_eligibility'

    try:
        data = request.args

        session_id = data.get('session_id')

        interventions = get_eligible_intervention(session_id)

        return make_response(jsonify(success_handler({ 'result': interventions })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/feature/data-analyzer-result [POST]
@geo_apis_blueprint.route('/feature/data-analyzer', methods=['GET'])
@cross_origin()
def geo_feature_data_analyzer_result():
    g_var.__api_name__ = 'geo_feature_data_analyzer_result'

    try:
        data = request.args

        session_id = data.get('session_id')

        known_data_analyzer = DataAnalyzer.query.filter_by(session_id=session_id).first()
        if not known_data_analyzer:
            raise AppMessageException('fail, session id Not found')
        
        geom = Polygons.get_geometry(session_id).first()
        geom = json.loads(geom[0])['coordinates'][0]

        results = {
            'session_id': session_id,
            'polygon': geom,
            'site_information': known_data_analyzer.site_information,
            'nature': known_data_analyzer.nature,
            'climate': known_data_analyzer.climate,
            'people': known_data_analyzer.people,
            'benefit': known_data_analyzer.benefit,
            'eligibility': known_data_analyzer.intervention_eligibility,
        }

        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error



