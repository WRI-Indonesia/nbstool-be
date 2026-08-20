# application/apis/master_apis/dictionary/routes.py
from flask import jsonify, request, make_response, g as g_var
from flask_login import current_user
from .. import master_apis_blueprint
from .... import db
from ....models.master_models.models import Dictionary

from datetime import datetime
from datetime import timedelta

from flask_cors import cross_origin

import gc

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler

@master_apis_blueprint.route('/dictionary/list', methods=['GET'])
def listdata_dictionary():
    g_var.__api_name__ = 'master_dictionary_list'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        
        attr = set_attr(request.args.get('attr'))
        param = get_default_list_param(request.args)

        filter_by = []
        if param.get('filter_by_col') and param.get('filter_by_text'):
            for col, text in zip(param.get('filter_by_col').split(','), param.get('filter_by_text').split(',')):
                if col.lower().strip() == 'name' and text:
                    filter_by.append(Dictionary.name == text)
                    continue
                if col.lower().strip() == 'code' and text:
                    filter_by.append(Dictionary.code == text)
                    continue
                if col.lower().strip() == 'type' and text:
                    filter_by.append(Dictionary.type == text)
                    continue

        order_by = []
        if param.get('order_by_col') and param.get('order_by_type'):
            for order_col, order_type in zip(param.get('order_by_col').split(','), param.get('order_by_type').split(',')):
                if order_col.lower().strip() == 'name':
                    order_by.append('master_dictionary.name ' + ' ' + order_type)
                    continue
                if order_col.lower().strip() == 'code':
                    order_by.append('master_dictionary.code ' + ' ' + order_type)
                    continue
                if order_col.lower().strip() == 'type':
                    order_by.append('master_dictionary.type ' + ' ' + order_type)
                    continue
                if order_col.lower().strip() == 'description':
                    order_by.append('master_dictionary.description ' + ' ' + order_type)
                    continue

        order_by = ','.join(order_by)

        items = []
        filters = (
            db.or_(
                db.func.coalesce(Dictionary.name, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'name' else '\x00'),
                db.func.coalesce(Dictionary.code, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'code' else '\x00'),
                db.func.coalesce(Dictionary.type, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'type' else '\x00'),
                db.func.coalesce(Dictionary.description, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'description' else '\x00'),
            ),
            Dictionary.rowstatus == 1,
            db.and_(*filter_by)
        )

        data = Dictionary.query.filter(*filters).order_by(db.desc(Dictionary.id) if not order_by else db.text(order_by))
        total_records = data.count()

        for row in data.paginate(page=param.get('page_index'), per_page=param.get('page_size'), error_out=False).items:
            items.append(row.to_json(attr=['code', 'name', 'type'] if not attr else attr))
        
        results = {
            'data': items,
            'total_records': total_records
        }
        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error