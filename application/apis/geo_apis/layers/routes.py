# application/apis/geo_apis/polygon/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var
from flask_login import current_user
from .. import geo_apis_blueprint
from .... import db

from datetime import datetime, timedelta
from flask_cors import cross_origin

import gc
import uuid
import json
import time

from shapely.geometry import Polygon

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler
from ....utils.geos import GeoUtils

from ..utils import GeoLogic


# legacy: /nbsapi/geoapi/layers [GET]
@geo_apis_blueprint.route('/layers', methods=['GET'])
@cross_origin()
def geo_get_layers():
    __api_name__ = 'geo_get_layers'

    try:
        
        start_section_time = time.time()
        query = '''
        select
            layer_id, layer_group, layer_name, source, source_link,
            citation, description, short_description, content_date,
            disclaimer, data_format, spatial_resolution, gs_name,
            gs_type, gs_url, gs_srs, gs_style, gs_format, gs_version,
            group_code, group_image, order_no, matrix_set
        from "public"."vwLayers"
        '''
        dt = GeoUtils.get_db(db.text(query))
        current_app.logger.info("--- %s seconds --- query the vwLayers results" % (time.time() - start_section_time))

        start_section_time = time.time()
        results = {}
        for row in dt:
            data = {
                'id': row.get('layer_id'),
                'name': row.get('layer_name'),
                'url': row.get('gs_url'),
                'matrix_set': row.get('matrix_set'),
                'service': row.get('gs_type'),
                'version': row.get('gs_version'),
                'layers': row.get('gs_name'),
                'srs': row.get('gs_srs'),
                'styles': row.get('gs_style'),
                'format': row.get('gs_format'),
                'source': row.get('source'),
                'source_link': row.get('source_link'),
                'citation': row.get('citation'),
                'description': row.get('description'),
                'short_description': row.get('short_description'),
                'content_date': row.get('content_date'),
                'disclaimer': row.get('disclaimer'),
                'data_format': row.get('data_format'),
                'spatial_resolution': row.get('spatial_resolution'),
            }

            if row.get('order_no') not in results.keys():
                results[row.get('order_no')] = {
                    'title': row.get('layer_name'),
                    'order_no': row.get('order_no'),
                    'group_image': row.get('group_image'),
                    'items': []
                }
            
            results[row.get('order_no')]['items'].append(data)
        current_app.logger.info("--- %s seconds --- build the dict" % (time.time() - start_section_time))

        start_section_time = time.time()
        results = [results[n] for n in results.keys()]
        current_app.logger.info("--- %s seconds --- build the results" % (time.time() - start_section_time))
        
        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 500) # send internal error

def func_get_layers_v2():
    start_section_time = time.time()
    query = '''
    select
        layer_id, layer_group, layer_name, source, source_link,
        citation, description, short_description, content_date,
        disclaimer, data_format, spatial_resolution, gs_name,
        gs_type, gs_url, gs_srs, gs_style, gs_format, gs_version,
        group_code, group_image, order_no, matrix_set,

        country, has_child, parent_id
    from "public"."vwLayers_v2"
    '''
    dt = GeoUtils.get_db(db.text(query))
    current_app.logger.info("--- %s seconds --- query the vwLayers results" % (time.time() - start_section_time))

    start_section_time = time.time()

    nodes = {}
    for row in dt:
        row['child'] = []
        row['country'] = row.get('country').title() if row.get('country') else ''
        nodes[row.get('layer_id')] = row
    
    records = []
    for row in dt:
        if row.get('parent_id') == 0:
            records.append(nodes[row.get('layer_id')])
        else:
            nodes[row['parent_id']]['child'].append(nodes[row.get('layer_id')])

    results = {}
    for row in records:

        if row.get('order_no') not in results.keys():
            results[row.get('order_no')] = {
                'title': row.get('layer_group'),
                'order_no': row.get('order_no'),
                'group_image': row.get('group_image'),
                'items': {
                    'country': {
                        'Indonesia': [],
                        'Philippine': [],
                        'Thailand': [],
                        'Malaysia': [],
                        'Cambodia': [],
                    },
                    'global': []
                }
            }

        country = row.get('country')

        if is_country(country):
            if country not in results[row.get('order_no')]['items']['country'].keys(): # check
                results[row.get('order_no')]['items']['country'][country] = []
            results[row.get('order_no')]['items']['country'][country].append(serialize_rowdata(row))
        else:
            results[row.get('order_no')]['items']['global'].append(serialize_rowdata(row))

    current_app.logger.info("--- %s seconds --- build the dict" % (time.time() - start_section_time))

    start_section_time = time.time()
    results = [results[n] for n in results.keys()]
    current_app.logger.info("--- %s seconds --- build the results" % (time.time() - start_section_time))

    return results

@geo_apis_blueprint.route('/layers/v2', methods=['GET'])
@cross_origin()
def geo_get_layers_v2():
    __api_name__ = 'geo_get_layers_v2'

    try:
        return make_response(jsonify(success_handler({ 'result': func_get_layers_v2() })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 500) # send internal error


def is_country(country):
    return False if not country or country == 'Global' else True


def serialize_rowdata(row):
    return {
        'id': row.get('layer_id'),
        'name': row.get('layer_name'),
        'url': row.get('gs_url'),
        'matrix_set': row.get('matrix_set'),
        'service': row.get('gs_type'),
        'version': row.get('gs_version'),
        'layers': row.get('gs_name'),
        'srs': row.get('gs_srs'),
        'styles': row.get('gs_style'),
        'format': row.get('gs_format'),
        'source': row.get('source'),
        'source_link': row.get('source_link'),
        'citation': row.get('citation'),
        'description': row.get('description'),
        'short_description': row.get('short_description'),
        'content_date': row.get('content_date'),
        'disclaimer': row.get('disclaimer'),
        'data_format': row.get('data_format'),
        'spatial_resolution': row.get('spatial_resolution'),
        'has_child': True if row.get('has_child') == 1 else False,
        'child': row.get('child'), # 'child': [],
    }