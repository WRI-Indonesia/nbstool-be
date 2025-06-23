# application/apis/master_apis/faq/routes.py
from flask import jsonify, request, make_response, g as g_var
from flask_login import current_user
from .. import master_apis_blueprint
from .... import db
# from ....models.master_models.models import Organization

from datetime import datetime
from datetime import timedelta

from flask_cors import cross_origin

import gc

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler


# legacy: /nbsapi/front-service/faq/list [GET]
# legacy: /nbsapi/faq/faq/list [GET]
@master_apis_blueprint.route('/faq/list', methods=['GET'])
@cross_origin()
def faq_list():
    g_var.__api_name__ = 'master_faq_list' # define di master dictionary, bisa dipakai untuk access/permission (next improvement)

    g_var.__log_it__ = True # mark for log the api activity
    g_var.__request_data__ = request.args.to_dict() # ganti ini sesuai dgn POST (request.get_json() atau request.data) atau GET (request.args)
    g_var.__session_id__ = None # bisa ganti ini saat nemu data session_id, mandatory untuk log

    try:
        query = '''
        select
            faq_id, group_id, group_name,
            question, answer
        from public."vwFAQs"
        '''

        dt = db.session.execute(db.text(query)).all()

        items = []
        for row in dt:
            items.append({
                'faq_id': row.faq_id,
                'group_id': row.group_id,
                'group_name': row.group_name,
                'question': row.question,
                'answer': row.answer,
            })

        results = {
            'result': items
        }

        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error