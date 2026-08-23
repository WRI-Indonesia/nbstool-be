# application/apis/geo_apis/map/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var
from flask_login import current_user
from .. import geo_apis_blueprint
from .... import db
from ....models.geos_models.models import Polygons, MapExplorer
from ....models.user_models.models import SessionsAuth

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename

import os
import gc
import uuid
import json

from shapely.geometry import Polygon

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler, sanitize_for_jsonb
from ....utils.geos import GeoUtils

from ..utils import GeoLogic
from ...user_apis.utils import UserLogic


# legacy: /nbsapi/geoapi/handler/map-upload [POST]
@geo_apis_blueprint.route('/map/upload', methods=['POST'])
@cross_origin()
def geo_map_upload():
    g_var.__api_name__ = 'geo_map_upload'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    try:
        g_var.__request_data__ = request.form.to_dict()
    except:
        pass

    try:
        data = request.form
        
        session_id = data.get('session_id')
        user_id = data.get('user_id') if data.get('user_id') else 0
        file = request.files.get('file')

        if not file:
            raise AppMessageException('No selected file')
        if not file.filename:
            raise appmessageexception('No selected file')
        if not GeoUtils.allowed_file(file.filename):
            raise AppMessageException('Invalid file format, only ZIP, KML or KMZ files are allowed')
        
        if not session_id:
            session_id = str(uuid.uuid4())

            # add for log
            user_id = current_user.id if current_user.is_authenticated else None
            UserLogic.add_user_sessions_for_log(user_id, session_id)
            # end add for log
        
        g_var.__session_id__ = session_id
        
        upload_folder = 'Uploaded-File'
        filepath = os.path.join(upload_folder, session_id)
        if not os.path.exists(filepath):
            os.makedirs(filepath)
        
        fullpath = os.path.join(filepath, secure_filename(file.filename))
        file.save(fullpath)

        extension = file.filename.rsplit('.', 1)[1].lower()
        g_var.__description_data__['file_type'] = extension

        if extension == 'zip':
            results = GeoLogic.process_zip_and_get_polygon(fullpath, session_id, upload_folder)
        elif extension == 'kml':
            results = GeoLogic.process_kml_and_get_polygon(fullpath, session_id, upload_folder)
        elif extension == 'kmz':
            results = GeoLogic.process_kmz_and_get_polygon(fullpath, session_id, upload_folder)
        
        results = json.loads(results)
        project_area = GeoLogic.calculate_project_area_geom(results, user_id)

        results['session_id'] = session_id
        results['area_size'] = project_area.get('size')
        results['user_limit'] = project_area.get('limit')
        results['exceed_size'] = project_area.get('exceed')
        message = 'File successfully uploaded'

        g_var.__description_data__['project_area'] = results.get('area_size')

        db.session.commit()

        return make_response(jsonify(success_handler({ 'selected_polygon': results }, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/geoapi/handler/get-map-explorer [POST]
# legacy: /nbsapi/geoapi/handler/map-explorer [GET]
@geo_apis_blueprint.route('/map/explorer', methods=['GET'])
@cross_origin()
def geo_get_map_explorer():
    g_var.__api_name__ = 'geo_get_map_explorer'

    try:
        data = request.args

        session_id = data.get('session_id')

        known_map_explorer = MapExplorer.query.filter_by(session_id=session_id).first()
        if not known_map_explorer:
            raise AppMessageException('fail, session id not found')

        return make_response(jsonify(success_handler({ 'result': known_map_explorer.to_json() })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/geoapi/handler/map-explorer [POST]
@geo_apis_blueprint.route('/map/explorer', methods=['POST'])
@cross_origin()
def geo_post_map_explorer():
    g_var.__api_name__ = 'geo_post_map_explorer'

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        session_id = data.get('session_id')
        project_duration = data.get('project_duration')
        estimated_unplanned_deforestation = data.get('estimated_unplanned_deforestation')
        rest_target = data.get('rest_target')
        intervention = data.get('intervention')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if not known_polygons:
            raise AppMessageException('session id not found, please input polygon with session id first')
        
        status_code = 200 # HTTPStatus.OK
        message = 'updated, details is updated succesfully'
        
        known_map_explorer = MapExplorer.query.filter_by(session_id=session_id).first()
        if not known_map_explorer:
            known_map_explorer = MapExplorer()
            known_map_explorer.session_id = session_id

            status_code = 201 # HTTPStatus.CREATED
            message = 'successfully'
        
        known_map_explorer.project_duration = project_duration
        known_map_explorer.estimated_unplanned_deforestation = estimated_unplanned_deforestation
        known_map_explorer.rest_target = rest_target
        known_map_explorer.intervention = intervention
        known_map_explorer.rest_target_json = sanitize_for_jsonb(rest_target)
        known_map_explorer.intervention_json = sanitize_for_jsonb(intervention)

        db.session.add(known_map_explorer)
        db.session.commit()

        return make_response(jsonify(success_handler({}, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/geoapi/search-location [POST]
@geo_apis_blueprint.route('/map/location', methods=['GET'])
@cross_origin()
def geo_get_map_location():
    g_var.__api_name__ = 'geo_get_map_location'

    try:
        data = request.args

        filter_text = data.get('filter')

        query = '''
        select
            location_value,
            location_name,
            display_name,
            location_type
        from public."mvwSearchLocation"
        where lower(location_name) ~~ lower('%{}%')
        limit 10
        '''.format(filter_text)

        dt = GeoUtils.get_db(db.text(query))

        items = []
        for row in dt:
            items.append({
                'location_value': row.get('location_value'),
                'location_name': row.get('display_name'),
                'location_type': row.get('location_type'),
            })
        
        return make_response(jsonify(success_handler(items)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/geoapi/get-location [POST]
@geo_apis_blueprint.route('/map/location', methods=['POST'])
@cross_origin()
def geo_search_map_location():
    g_var.__api_name__ = 'geo_search_map_location'

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        location_name = data.get('location_name')

        query = '''
        select
            lat, lng
        from public."mvwGetLocation"
        where location_value = '{}'
        '''.format(location_name)

        dt = GeoUtils.get_db(db.text(query))

        results = { }
        for row in dt:
            results['lat'] = row.get('lat')
            results['lng'] = row.get('lng')
        
        return make_response(jsonify(success_handler({'selected_location': results}, message='Location has been successfully identify')), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error
