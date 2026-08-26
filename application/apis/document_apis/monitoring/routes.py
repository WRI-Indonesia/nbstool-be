# application/apis/document_apis/monitoring/routes.py
from flask import jsonify, request, make_response, g as g_var
from .. import document_apis_blueprint
from .... import db

from flask_cors import cross_origin

import os
import uuid

from ....utils.common import AppMessageException
from ....utils.common import app_exception_handler, success_handler

from ....utils.document_generator.v3 import generate_monitoring_v3
from ....models.geos_models.models import DataAnalyzer
from ....models.master_models.models import DocumentList
from ..utils import load_draft, save_draft

from .. import gcs

# Monitoring layers on top of feasibility: the socio-economic answers are collected once in the
# feasibility form, so the monitoring draft and render read them underneath monitoring's own.
_BASE_TYPES = ('FeasibilityV3',)


# v3: F03 monitoring plan, same single-endpoint flow as /feasibility/v3. GET returns the form
# draft (monitoring answers over feasibility answers over analyser prefill). POST always SAVES
# the payload's form/user_input into the MonitoringV3 draft (atomic key-level merge, see
# utils.save_draft), and with `generate: true` also renders the docx.
#
# Body: { "session_id": str, "form": {...}, "user_input": {"<tag text>": "value"},
#         "generate": bool (default false) }
@document_apis_blueprint.route('/monitoring/v3', methods=['GET'])
@cross_origin()
def documents_monitoring_v3_get():
    g_var.__api_name__ = 'documents_monitoring_v3_get'

    try:
        session_id = request.args.get('session_id')
        if not session_id:
            raise AppMessageException('please provide session_id')

        _, form, user_input = load_draft(session_id, 'MonitoringV3', _BASE_TYPES)

        results = {'form': form, 'user_input': user_input}
        return make_response(jsonify(success_handler({'result': results})), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


@document_apis_blueprint.route('/monitoring/v3', methods=['POST'])
@cross_origin()
def documents_monitoring_v3():
    g_var.__api_name__ = 'documents_monitoring_v3'

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
        save_draft(session_id, 'MonitoringV3',
                   data.get('form') or {}, data.get('user_input') or {})

        if not data.get('generate'):
            db.session.commit()
            return make_response(jsonify(success_handler({'result': {'message': 'saved'}})), 200)

        analyzer, form, user_input = load_draft(session_id, 'MonitoringV3', _BASE_TYPES)
        if not analyzer:
            raise AppMessageException('fail, session id Not found')

        output_path = generate_monitoring_v3(session_id, analyzer, form=form,
                                             user_input=user_input)

        gcs.upload(output_path)

        document = DocumentList()
        document.project_id = session_id
        document.document_id = str(uuid.uuid4())
        document.document_type = 'MonitoringV3'
        document.document_status = 'Final'
        document.document_name = os.path.basename(output_path)
        document.client_name = 'Monitoring Plan'
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
