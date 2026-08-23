# application/apis/master_apis/documents/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db

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

from ....utils.document_generator import generate_document

from .. import gcs


# legacy: /nbsapi/feature/feasibility-template [POST]
@document_apis_blueprint.route('/feasibility', methods=['POST'])
@cross_origin()
def documents_feasibility_template():
    g_var.__api_name__ = 'documents_feasibility_template'

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
        sections = data.get('sections') # sample: [1, 2, 3, 4]

        g_var.__session_id__ = session_id
        try:
            g_var.__description_data__['section'] = ','.join([str(n) for n in sections])
        except Exception as e:
            g_var.__description_data__['section'] = str(sections)

        data = GeoLogic.get_template_data(session_id, sections)

        # tasks = feasibility_template_task.delay(session_id, data)
        generate_document(session_id, data)
        results = str(uuid.uuid4())

        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error