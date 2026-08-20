# application/apis/geo_apis/polygon/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var
from flask_login import current_user
from .. import geo_apis_blueprint
from .... import db
from ....models.geos_models.models import Polygons
from ....models.user_models.models import SessionsAuth

from datetime import datetime, timedelta
from flask_cors import cross_origin

import gc
import uuid
import json
# import geopandas as gpd  # off: only fed prefetch_cx_basemap

from shapely.geometry import Polygon, MultiPolygon

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler
from ....utils.geos import GeoUtils
# from ....utils.geos.current_condition import prefetch_cx_basemap  # off: v2 current-condition

from ..utils import GeoLogic
from ...user_apis.utils import UserLogic


# legacy: /nbsapi/geoapi/handler/input_polygon [GET]
# legacy: /nbsapi/geoapi/handler/get-polygon [POST]
@geo_apis_blueprint.route('/polygon', methods=['GET'])
@cross_origin()
def geo_get_polygon():
    g_var.__api_name__ = 'geo_get_polygon'

    try:
        data = request.args

        session_id = data.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if not known_polygons:
            raise AppMessageException('fail, session id not found')

        geom = db.session.query(db.func.ST_AsGeoJSON(known_polygons.geom)).first()

        results = json.loads(geom[0])
        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/geoapi/handler/input_polygon [POST]
@geo_apis_blueprint.route('/polygon', methods=['POST'])
@cross_origin()
def geo_post_polygon():
    g_var.__api_name__ = 'geo_post_polygon'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        user_id = data.get('user_id') if data.get('user_id') else 0
        session_id = data.get('session_id')
        geom_type = data.get('geometry').get('type').lower()
        is_debug = data.get('is_debug')

        if is_debug:
            raise AppMessageException('This is a general error for debugging')

        gdf = GeoUtils.construct_gdf(data)

        if geom_type == 'polygon':
            geom = Polygon(data.get('geometry').get('coordinates')[0])
        elif geom_type == 'multipolygon':
            polygon_list = [Polygon(cord) for polygon in data.get('geometry').get('coordinates') for cord in polygon]
            geom = MultiPolygon(polygon_list)
        
        query = '''
        select
            *
        from sea.gadm_bbox('{aio_geom}')
        '''.format(aio_geom=geom)
        
        dt = GeoUtils.get_db(db.text(query))

        for row in dt:
            if row.get('within') == 0:
                raise AppMessageException('Area is not in our coverage analysis')
        
        status_code = 201
        message = 'successfully'

        session_id = str(session_id).strip() if session_id else None
        if not session_id:
            session_id = str(uuid.uuid4())

            # add for log
            user_id = current_user.id if current_user.is_authenticated else None
            UserLogic.add_user_sessions_for_log(user_id, session_id)
            # end add for log

        known_session = SessionsAuth.query.filter_by(session_id=session_id).first()
        if not known_session:
            
            known_session = SessionsAuth()
            known_session.session_id = session_id
            
            db.session.add(known_session)

        g_var.__session_id__ = session_id
        
        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if known_polygons:
            status_code = 200
            message = 'updated, Geometry is updated successfully'
        else:
            known_polygons = Polygons()
            known_polygons.session_id = session_id

        known_polygons.geom = str(geom)

        db.session.add(known_polygons)
        db.session.commit()
        db.session.refresh(known_polygons)

        project_area = GeoLogic.calculate_project_area_db(known_polygons, user_id)

        # off: v2 current-condition basemap warm-up
        # gdf4326 = gpd.GeoDataFrame(index=[0], crs='epsg:4326', geometry=[geom])
        # prefetch_cx_basemap(gdf4326)

        results = {
            'session_id': known_polygons.session_id,
            'area_size': project_area.get('size'),
            'user_limit': project_area.get('limit'),
            'exceed_size': project_area.get('exceed'),
            'country': known_polygons.country,
            'iso_3': known_polygons.iso_3
        }

        g_var.__description_data__['project_area'] = results.get('area_size')
        
        return make_response(jsonify(success_handler({ 'result': results }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


