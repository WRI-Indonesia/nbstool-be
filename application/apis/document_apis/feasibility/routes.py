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
from ....models.master_models.models import DocumentData, DocumentList

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


# v3: F03 feasibility document. Fills the doc team's template from the session's persisted v3
# analysis (DataAnalyzer JSONB) plus the socio-economic form and free user-input overrides.
# Tags with no value stay as their literal [bracket] text for manual fill -- generating from a
# partial form is allowed by design.
#
# Body: { "session_id": str, "form": {se* keys}, "user_input": {"<tag text>": "value"} }
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

        analyzer = DataAnalyzer.find_by_session_id(session_id)
        if not analyzer:
            raise AppMessageException('fail, session id Not found')

        # The request may carry the freshest form state; anything it does not carry falls back to
        # the stored draft, which in turn sits on the analyser prefill.
        stored = DocumentData.find_by_session_id_and_type(session_id, 'FeasibilityV3')
        form = merge_form(stored.form if stored else None, feasibility_prefill(analyzer))
        form.update(data.get('form') or {})
        user_input = dict((stored.user_input if stored else None) or {})
        user_input.update(data.get('user_input') or {})

        output_path = generate_feasibility_v3(
            session_id,
            analyzer,
            form=form,
            user_input=user_input,
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
            'document_id': document.document_id,
            'document_name': document.document_name,
        }})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# v3: the F03 feasibility form draft. GET returns the stored draft merged over the analyser
# prefill (stored answers win); POST upserts -- save-draft and next both hit it, sending only
# the keys they touched, which merge into what is already stored.
@document_apis_blueprint.route('/feasibility/v3/form', methods=['GET'])
@cross_origin()
def documents_feasibility_v3_form_get():
    g_var.__api_name__ = 'documents_feasibility_v3_form_get'

    try:
        session_id = request.args.get('session_id')
        if not session_id:
            raise AppMessageException('please provide session_id')

        analyzer = DataAnalyzer.find_by_session_id(session_id)
        stored = DocumentData.find_by_session_id_and_type(session_id, 'FeasibilityV3')

        results = {
            'form': merge_form(stored.form if stored else None, feasibility_prefill(analyzer)),
            'user_input': (stored.user_input if stored else None) or {},
        }
        return make_response(jsonify(success_handler({'result': results})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


@document_apis_blueprint.route('/feasibility/v3/form', methods=['POST'])
@cross_origin()
def documents_feasibility_v3_form_save():
    g_var.__api_name__ = 'documents_feasibility_v3_form_save'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
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

        stored = DocumentData.find_by_session_id_and_type(session_id, 'FeasibilityV3')
        if not stored:
            stored = DocumentData(session_id=session_id, certification_type='FeasibilityV3')
            db.session.add(stored)

        # Key-level merge: a partial save (one step's fields) must not wipe the other steps.
        # An explicit null clears a single answer.
        stored.form = {**(stored.form or {}), **(data.get('form') or {})}
        stored.user_input = {**(stored.user_input or {}), **(data.get('user_input') or {})}
        db.session.commit()

        return make_response(jsonify(success_handler({'result': {'message': 'saved'}})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error