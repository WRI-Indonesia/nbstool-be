# application/apis/document_apis/ccb/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db
from ....models.user_models.models import SessionsAuth, UserSessions
from ....models.master_models.models import DocumentData, DocumentList
from ....models.geos_models.models import MapExplorer
from ....models.user_models.models import User

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename
from pathlib import Path

import os
import gc
import uuid
import json
import base64

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler

from ...geo_apis.utils import GeoLogic

from ....utils.document_generator import generate_document_form


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
        
        if section == '1':
            known_document_data.section_1 = data
        elif section == '2':
            known_document_data.section_2 = data
        elif section == '3':
            known_document_data.section_3 = data
            
            GeoLogic.transform_ghg_data(known_document_data)
        elif section == '4':
            known_document_data.section_4 = data
        elif section == '5':
            known_document_data.section_5 = data

            GeoLogic.summarize_fauna(known_document_data)

        db.session.add(known_document_data)
        db.session.commit()

        known_user_session = UserSessions.find_by_session_id(known_document_list.project_id)
        if not known_user_session:
            known_user_session = UserSessions()
        known_user = User.query.filter_by(id=known_user_session.user_id).first()
        if not known_user:
            known_user = User()
        
        intervention = MapExplorer.find_by_session_id(known_document_list.project_id)
        if not intervention:
            intervention = MapExplorer()
        
        GeoLogic.handle_section_data(known_document_data)
        
        data = {
            'project_id': known_document_list.project_id,
            'user': {
                'fullname': known_user.name,
                'email': known_user.email
            },
            'param': {
                'project_duration': intervention.project_duration,
                'estimated_unplanned_deforestation': intervention.estimated_unplanned_deforestation,
                'rest_target': intervention.rest_target,
            },
            'section_1': known_document_data.section_1,
            'section_2': known_document_data.section_2,
            'section_3': known_document_data.section_3,
            'section_4': known_document_data.section_4,
            'section_5': known_document_data.section_5,

            'tpl_data': GeoLogic.get_template_data(session_id, [n for n in range(1, 6) if eval('known_document_data.section_{}'.format(n))]),
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