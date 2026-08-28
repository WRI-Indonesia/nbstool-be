# application/apis/document_apis/ccb/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db
from ....models.user_models.models import SessionsAuth, UserSessions, User
from ....models.master_models.models import DocumentData, DocumentList
from ....models.geos_models.models import DataAnalyzer, Polygons

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename
from pathlib import Path
from shapely.geometry import box

import os
import gc
import uuid
import json
import base64
import string
import calendar as cal

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler, sanitize_for_jsonb

from ...geo_apis.utils import GeoLogic

from ....utils.document_generator import generate_document_form

_MONTH_ABBR = [cal.month_abbr[i] for i in range(1, 13)]


def _monthly_series(rows, value_key):
    """12 monthly values from the v3 `[{month, <value>}]` shape, or None when incomplete."""
    by_month = {r.get('month'): r.get(value_key) for r in rows or [] if isinstance(r, dict)}
    series = [by_month.get(m) for m in range(1, 13)]
    if any(not isinstance(v, (int, float)) for v in series):
        return None
    return [round(v, 1) for v in series]


def _series_stats(series):
    stats = {'max': max(series), 'min': min(series), 'mean': sum(series) / len(series)}
    stats['max_month'] = _MONTH_ABBR[series.index(stats['max'])]
    stats['min_month'] = _MONTH_ABBR[series.index(stats['min'])]
    return stats


def _dry_season(series):
    """The v2 builder's dry-season walk, unchanged: from the driest month, walk outward in
    both directions until a month-to-month jump above 30mm marks the season boundary."""
    peak = series.index(min(series))
    start = end = peak
    total = 0
    order = [(n + peak) % 12 for n in range(12)]
    for mi in order[::-1]:
        if abs(series[mi] - series[(mi + 11) % 12]) > 30:
            start = mi
            total += order.index(mi) + 1
            break
    for mi in order:
        if abs(series[mi] - series[(mi + 1) % 12]) > 30:
            end = mi
            total += order.index(mi)
            break
    return {
        'peak': peak, 'start': start, 'end': end,
        'peak_month': _MONTH_ABBR[peak],
        'start_month': _MONTH_ABBR[start],
        'end_month': _MONTH_ABBR[end],
        'category': 'short' if total <= 3 else 'long',
    }


def _tpl_data_v3(session_id, analyzer):
    """The tpl_data keys `assets/ccb_template.docx` actually references, from the v3 analyser
    JSONB. (The v2 builder, GeoLogic.get_template_data, read the dead pickle columns and built
    ~40 keys; the template uses 13 -- location, area, geometry corners, elevation/slope,
    peatland flag and the two monthly climate tables.)"""
    site = (analyzer.site_information_json if analyzer else None) or {}
    climate = (analyzer.climate_json if analyzer else None) or {}

    data = {'current_year': datetime.now().year}

    district = string.capwords(site.get('district') or '')
    province = string.capwords(site.get('province') or '')
    country = string.capwords(site.get('country') or '')
    data['district'] = district
    data['province'] = province
    data['project_location'] = ', '.join(p for p in (district, province, country) if p)

    _, geom_gdf = GeoLogic.construct_polygon(session_id)
    center = box(*geom_gdf.total_bounds).centroid
    data['geom_center'] = {'longitude': center.x, 'latitude': center.y}
    data['geom_center_dms'] = GeoLogic.coords_to_string(center.x, center.y)
    data['geom_min_dms'] = GeoLogic.coords_to_string(geom_gdf.bounds.minx[0], geom_gdf.bounds.miny[0])
    data['geom_max_dms'] = GeoLogic.coords_to_string(geom_gdf.bounds.maxx[0], geom_gdf.bounds.maxy[0])

    # Pathway run's figure, else the polygon's own size (legacy rows store a LIST in the
    # pathway column, which reads as absent).
    pathway = (analyzer.intervention_eligibility_json if analyzer else None) or {}
    area = pathway.get('project_area_ha') if isinstance(pathway, dict) else None
    if area is None:
        polygon = Polygons.query.filter_by(session_id=session_id).first()
        area = polygon.project_area_size if polygon else None
    data['area_size'] = f"{area:,.2f}" if isinstance(area, (int, float)) else ''

    data['has_peatland'] = any(
        eco.get('name') in ('Peatland', 'Mangrove') and (eco.get('area') or 0) > 0
        for eco in site.get('ecosystems') or [] if isinstance(eco, dict))

    # The template prints slope % alongside the predominant elevation class.
    slope = site.get('average_slope_percentage')
    data['top_elevation'] = [{
        'elevation_class': (site.get('predominant_elevation_dict') or {}).get('fallback') or '',
        'elevation_pct': f"{slope:.2f}" if isinstance(slope, (int, float)) else '',
    }]

    for key, rows_key, value_key in (
            ('precipitation', 'historical_precipitations', 'precipitation'),
            ('temperature', 'historical_temperatures', 'temperature')):
        series = _monthly_series(climate.get(rows_key), value_key)
        if series:
            block = {'graph_data': series, 'graph': _series_stats(series)}
            if key == 'precipitation':
                block['graph']['dry_season'] = _dry_season(series)
        else:
            # The monthly table indexes graph_data[0..11] unconditionally.
            block = {'graph_data': [''] * 12, 'graph': None}
        data[key] = block

    return data


# legacy: /nbsapi/feature/document-data [POST]
@document_apis_blueprint.route('/ccb', methods=['POST'])
@cross_origin()
def documents_update_document_data_ccb():
    g_var.__api_name__ = 'documents_update_document_data_ccb'

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

        session_id = data.get('session_id')
        document_type = data.get('document_type')
        section = data.get('section')
        data = data.get('data') # override request json data

        known_session_auth = SessionsAuth.find_by_session_id(session_id)
        if not known_session_auth:
            raise AppMessageException('Fail, session id not found')
        
        g_var.__session_id__ = session_id
        
        status_code = 200
        message = 'Updated. Data is updated successfully'

        g_var.__description_data__['operation'] = 'Update'
        
        known_document_data = DocumentData.find_by_session_id_and_type(session_id, document_type)
        if not known_document_data:
            g_var.__description_data__['operation'] = 'Initialize'

            known_document_data = DocumentData()
            known_document_data.session_id = session_id
            known_document_data.certification_type = document_type

            known_document_list = DocumentList()
            known_document_list.project_id = session_id
            known_document_list.document_id = str(uuid.uuid4())
            known_document_list.document_type = document_type
            known_document_list.document_status = 'Draft'
            known_document_list.document_name = '{}-ccb-templates.docx'.format(known_document_list.project_id)

            db.session.add(known_document_list)

            status_code = 201
            message = 'Created. Data is created successfully'
        else:
            known_document_list = DocumentList.find_by_project_id_and_document_type(project_id=session_id, document_type=document_type)
        
        safe_data = sanitize_for_jsonb(data)
        if section == '1':
            known_document_data.section_1 = data
            known_document_data.section_1_json = safe_data
        elif section == '2':
            known_document_data.section_2 = data
            known_document_data.section_2_json = safe_data
        elif section == '3':
            known_document_data.section_3 = data
            known_document_data.section_3_json = safe_data

            GeoLogic.transform_ghg_data(known_document_data)
        elif section == '4':
            known_document_data.section_4 = data
            known_document_data.section_4_json = safe_data
        elif section == '5':
            known_document_data.section_5 = data
            known_document_data.section_5_json = safe_data

            GeoLogic.summarize_fauna(known_document_data)

        db.session.add(known_document_data)
        db.session.commit()

        if isinstance(known_document_data.section_2, dict):
            GeoLogic.handle_section_data(known_document_data)

        known_user_session = UserSessions.find_by_session_id(session_id)
        known_user = (User.query.filter_by(id=known_user_session.user_id).first()
                      if known_user_session else None)

        analyzer = DataAnalyzer.find_by_session_id(session_id)
        # Project lifetime: the v2 MapExplorer params are dead; the v3 benefit run's
        # assumptions carry the duration.
        benefit = (getattr(analyzer, 'benefit_json', None) if analyzer else None) or {}
        duration = (benefit.get('assumptions') or {}).get('duration_years')

        # Exactly what the template reads: the five section dicts, tpl_data, the preparer's
        # user block and the project duration. The v2 tpl builder read the dead pickle
        # analyser columns -- _tpl_data_v3 builds the referenced keys from the v3 JSONB.
        data = {
            'project_id': known_document_list.project_id,
            'user': {
                'fullname': known_user.name if known_user else '',
                'email': known_user.email if known_user else '',
                'phone': ((known_user.extended_data or {}).get('phone') or ''
                          if known_user else ''),
            },
            'param': {
                'project_duration': duration,
            },
            'section_1': known_document_data.section_1,
            'section_2': known_document_data.section_2,
            'section_3': known_document_data.section_3,
            'section_4': known_document_data.section_4,
            'section_5': known_document_data.section_5,

            'tpl_data': _tpl_data_v3(session_id, analyzer),
        }

        # tasks = form_template_task.delay(session_id, data)
        generate_document_form(session_id, data)

        return make_response(jsonify(success_handler({ 'result': known_document_list.document_id }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/feature/get-document-data [POST]
@document_apis_blueprint.route('/ccb', methods=['GET'])
@cross_origin()
def documents_get_document_data_ccb():
    g_var.__api_name__ = 'documents_get_document_data_ccb'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__request_data__ = request.args.to_dict()

    try:
        data = request.args

        session_id = data.get('session_id')
        document_type = data.get('document_type')

        known_document_data = DocumentData.find_by_session_id_and_type(session_id, document_type)
        if not known_document_data:
            raise AppMessageException('fail, session id Not found')
        
        g_var.__session_id__ = session_id
        
        results = {
            'session_id': session_id,
            'section_1': known_document_data.section_1,
            'section_2': known_document_data.section_2,
            'section_3': known_document_data.section_3,
            'section_4': known_document_data.section_4,
            'section_5': known_document_data.section_5,
        }

        return make_response(jsonify(success_handler({ 'result': results }, status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error