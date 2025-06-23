# application/apis/document_apis/cleanup/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db
from ....models.master_models.models import DocumentList
from ....models.user_models.models import SessionsAuth, UserSessions

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename
from pathlib import Path

import os
import gc
import uuid
import json
import base64
import shutil

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler

from ....utils.geos import GeoUtils

from ....utils.logger import log_func

from .. import gcs


# legacy: /nbsapi/project-management/data-cleanup [POST]
@document_apis_blueprint.route('/cleanup', methods=['GET'])
@cross_origin()
def documents_cleanup_data():
    g_var.__api_name__ = 'documents_cleanup_data'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        request_token = request.args.get('token')
        token = 'lets_try_this_token'
        if request_token != token:
            raise AppMessageException('you dont have permission to do this!')

        query = '''
        select 
            *
        from "vwExpiredSession"
        '''

        dt = GeoUtils.get_db(db.text(query), gis_db=False)
        total_records = 0

        for s in dt:
            session_id = s['session_id']
            session = SessionsAuth.find_by_session_id(session_id)

            if session:
                total_records += 1

                # deactivate the session
                session.is_active = 0

                db.session.add(session)

                project = UserSessions.find_by_session_id(session_id)

                if project:
                    project.is_active = 0

                    db.session.add(project)

                documents = DocumentList.find_by_project_id(session_id)

                if documents:
                    for d in documents:
                        d.is_active = 0

                    db.session.add_all(documents)

                # delete current condition calculation temporary files and folder
                cc_temp_folder = Path("temp_file", session_id).resolve()

                if os.path.isdir(cc_temp_folder):
                    shutil.rmtree(cc_temp_folder)

                # delete csv data source files
                csv_temp_folder = Path("generated-file", "csv").resolve()
                for p in csv_temp_folder.glob(session_id + "*.csv"):
                    gcs.delete(os.path.join('generated-file', 'csv', p.name))
                    p.unlink()

                # delete general template documentation files
                general_template_temp_folder = Path("generated-file", "docx").resolve()
                for p in general_template_temp_folder.glob(session_id + "*.docx"):
                    gcs.delete(os.path.join('generated-file', 'docx', p.name))
                    p.unlink()

                # delete ccb documentation files
                ccb_temp_folder = Path("generated-file", "docx-ccb").resolve()
                for p in ccb_temp_folder.glob(session_id + "*.docx"):
                    gcs.delete(os.path.join('generated-file', 'docx-ccb', p.name))
                    p.unlink()

                # delete graph files
                graph_temp_folder = Path("generated-file", "graph").resolve()
                for p in graph_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'graph', p.name))
                    p.unlink()

                # delete logo files
                logo_temp_folder = Path("generated-file", "logo").resolve()
                for p in logo_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'logo', p.name))
                    p.unlink()

                # delete project area files
                project_area_temp_folder = Path("generated-file", "project-area").resolve()
                for p in project_area_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'project-area', p.name))
                    p.unlink()

                # delete excel files
                xlsx_temp_folder = Path("generated-file", "xlsx").resolve()
                for p in xlsx_temp_folder.glob(session_id + "*.xlsx"):
                    gcs.delete(os.path.join('generated-file', 'xlsx', p.name))
                    p.unlink()
        
                db.session.commit()

                g_var.__session_id__ = session_id
                g_var.log_type_code = 'LOG_SUCCESS'
                resp_data = {
                    'json_response': False,
                    'data': {
                        'message': 'manual logging on documents_cleanup_data api',
                        'text': ''
                    }
                }

                log_func(db.session, resp_data)

        
        g_var.__log_it__ = False
        status_code = 200
        message = 'Expired data has succesfully clean'
        return make_response(jsonify(success_handler({ 'result': {'total_records': total_records} }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error