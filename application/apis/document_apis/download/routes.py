# application/apis/master_apis/documents/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db
from ....models.master_models.models import DocumentList

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

from .. import gcs


# legacy: /nbsapi/front-service/download/docx-ccb [POST]
@document_apis_blueprint.route('/download/docx-ccb', methods=['GET'])
@cross_origin()
def documents_get_document_download_docx_ccb():
    g_var.__api_name__ = 'documents_get_document_download_docx_ccb'

    try:
        data = request.args

        document_id = data.get('document_id')

        gcs.download("generated-file/docx-ccb/"+document_id+"-ccb-templates.docx")
        document_path = Path("generated-file/docx-ccb/"+document_id+"-ccb-templates.docx").resolve()

        if not os.path.exists(document_path):
            raise AppMessageException('we are still generating your file, please wait a minute')
        
        return send_file(document_path, mimetype='application/msword', as_attachment=True, download_name="Preliminary Assessment Document.docx")
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/front-service/download/xlsx [POST]
@document_apis_blueprint.route('/download/xlsx', methods=['GET'])
@cross_origin()
def documents_get_document_download_xlsx():
    g_var.__api_name__ = 'documents_get_document_download_xlsx'

    try:
        data = request.args

        session_id = data.get('session_id')

        gcs.download("generated-file/xlsx/"+session_id+"-excel-format.xlsx")
        document_path = Path("generated-file/xlsx/"+session_id+"-excel-format.xlsx").resolve()

        if not os.path.exists(document_path):
            raise AppMessageException('we are still generating your file, please wait a minute')
        
        return send_file(document_path, as_attachment=True)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/front-service/download/csv [POST]
@document_apis_blueprint.route('/download/csv', methods=['GET'])
@cross_origin()
def documents_get_document_download_csv():
    g_var.__api_name__ = 'documents_get_document_download_csv'

    try:
        data = request.args

        session_id = data.get('session_id')

        gcs.download("generated-file/csv/"+session_id+"-data.csv")
        document_path = Path("generated-file/csv/"+session_id+"-data.csv").resolve()

        if not os.path.exists(document_path):
            raise AppMessageException('we are still generating your file, please wait a minute')
        
        return send_file(document_path, as_attachment=True)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/front-service/download/docx [POST]
@document_apis_blueprint.route('/download/docx', methods=['GET'])
@cross_origin()
def documents_get_document_download_docx():
    g_var.__api_name__ = 'documents_get_document_download_docx'

    try:
        data = request.args

        document_id = data.get('document_id')

        known_document = DocumentList.find_by_document_id(document_id)
        if not known_document:
            raise AppMessageException('we are still generating your file, please wait a minute')
        
        if known_document.document_type.lower() == 'ccb':
            folder_path = 'generated-file/docx-ccb/'
            client_name = "CCB Project Documentation.docx"
        elif known_document.document_type == 'FeasibilityV3':
            folder_path = 'generated-file/docx-v3/'
            client_name = "Feasibility Study.docx"
        else:
            folder_path = 'generated-file/docx/'
            client_name = "Preliminary Assessment Document.docx"
        
        gcs.download(folder_path + known_document.document_name)
        document_path = Path(folder_path + known_document.document_name).resolve()
        if not os.path.exists(document_path):
            raise AppMessageException('we are still generating your file, please wait a minute')
        
        return send_file(document_path, mimetype='application/msword', as_attachment=True, download_name=client_name)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error