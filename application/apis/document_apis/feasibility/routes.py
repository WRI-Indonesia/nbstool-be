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
from ....utils.document_generator.v3 import generate_feasibility_v3
from ....utils.document_generator.v3.prefill import feasibility_prefill, merge_form
from ....models.geos_models.models import DataAnalyzer
from ....models.master_models.models import DocumentList
from ..utils import load_draft, save_draft

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


# v3: F03 feasibility, one endpoint. GET returns the form draft (stored answers merged over the
# analyser prefill, stored wins). POST always SAVES the payload's form/user_input (key-level
# merge, so a partial save never wipes another step; explicit null clears one answer), and with
# `generate: true` also renders the docx from the saved state plus the persisted v3 analysis.
# Tags with no value render as their literal [bracket] text for manual fill -- generating from a
# partial form is allowed by design.
#
# Body: { "session_id": str, "form": {se* keys}, "user_input": {"<tag text>": "value"},
#         "generate": bool (default false) }
@document_apis_blueprint.route('/feasibility/v3', methods=['GET'])
@cross_origin()
def documents_feasibility_v3_get():
    g_var.__api_name__ = 'documents_feasibility_v3_get'

    try:
        session_id = request.args.get('session_id')
        if not session_id:
            raise AppMessageException('please provide session_id')

        _, form, user_input = load_draft(session_id, 'FeasibilityV3')

        results = {'form': form, 'user_input': user_input}
        return make_response(jsonify(success_handler({'result': results})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


@document_apis_blueprint.route('/feasibility/v3', methods=['POST'])
@cross_origin()
def documents_feasibility_v3():
    g_var.__api_name__ = 'documents_feasibility_v3'

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
        if not session_id:
            raise AppMessageException('please provide session_id')
        g_var.__session_id__ = session_id

        # Save first, always: the draft is never stale after a POST, whether or not it generates.
        # The merge happens atomically in Postgres -- see utils.save_draft.
        stored = save_draft(session_id, 'FeasibilityV3',
                            data.get('form') or {}, data.get('user_input') or {})

        if not data.get('generate'):
            db.session.commit()
            return make_response(jsonify(success_handler({'result': {'message': 'saved'}})), 200)

        analyzer = DataAnalyzer.find_by_session_id(session_id)
        if not analyzer:
            raise AppMessageException('fail, session id Not found')

        output_path = generate_feasibility_v3(
            session_id,
            analyzer,
            form=merge_form(stored.form, feasibility_prefill(analyzer)),
            user_input=stored.user_input or {},
        )

        gcs.upload(output_path)

        document = DocumentList()
        document.project_id = session_id
        document.document_id = str(uuid.uuid4())
        document.document_type = 'FeasibilityV3'
        document.document_status = 'Final'
        document.document_name = os.path.basename(output_path)
        document.client_name = 'Feasibility Study'
        db.session.add(document)
        db.session.commit()

        return make_response(jsonify(success_handler({'result': {
            'message': 'generated',
            'document_id': document.document_id,
            'document_name': document.document_name,
        }})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error