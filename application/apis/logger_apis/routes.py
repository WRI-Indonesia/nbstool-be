# application/apis/logger_apis/faq/routes.py
from flask import jsonify, request, make_response, g as g_var, current_app
from flask_login import current_user
from . import logger_apis_blueprint
from ... import db
from ...models.logger_models.models import Logs
from ...models.master_models.models import Dictionary

from datetime import datetime
from datetime import timedelta

from flask_cors import cross_origin

import gc

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler


@logger_apis_blueprint.route('/list', methods=['GET'])
@cross_origin()
def logger_list():
    g_var.__api_name__ = 'logger_list'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        _id = request.args.get('id')
        request_token = request.args.get('token')
        if request_token != current_app.config['SECRET_KEY']:
            raise AppMessageException('you dont have permission to do this!')
        
        items = []
        if _id:
            data = Logs.query.filter_by(id=_id).first()
            if data:
                items = data.to_json()
        else:
            data = Logs.query.all()
        
            for row in data:
                items.append(row.to_json())

        results = {
            'result': items
        }

        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


@logger_apis_blueprint.route('/user/activity', methods=['GET'])
@cross_origin()
def logger_user_activity():
    g_var.__api_name__ = 'logger_user_activity'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        
        param = get_default_list_param(request.args)

        filter_by = []
        if param.get('filter_by_col') and param.get('filter_by_text'):
            for col, text in zip(param.get('filter_by_col').split(','), param.get('filter_by_text').split(',')):
                if col.lower().strip() == 'activity_type_code' and text:
                    filter_by.append(Dictionary.code == text)
                    continue
                if col.lower().strip() == 'type' and text:
                    if text == 'project':
                        filter_by.append(db.or_(
                            Dictionary.code.like('projects_%'),
                            Dictionary.code.like('post_%')
                        ))
                    elif text == 'document':
                        filter_by.append(Dictionary.code.like('documents_%'))
                    continue

        order_by = []
        order_by = ','.join(order_by)
        
        items = []
        filters = (
            db.or_(
                db.func.coalesce(Logs.description, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'description' else '\x00'),
                db.func.coalesce(Dictionary.name, '').like('%{}%'.format(param.get('keywords')) if param.get('search_by') == '' or param.get('search_by') == None or param.get('search_by') == 'activity_type' else '\x00'),
            ),
            Logs.created_by == current_user.id,
            db.and_(*filter_by)
        )

        select_field = {
            'activity_type': Dictionary.name,
            'description': Logs.description,
            'created_at': Logs.created_at,

            'id': Logs.id,
        }

        data =  Logs.query.filter(*filters) \
                .join(Dictionary, Logs.activity_type_id==Dictionary.id, isouter=True) \
                .with_entities(
                    *[select_field[n] for n in select_field.keys()]
                ) \
                .order_by(db.desc(Logs.id) if not order_by else db.text(order_by)) \
                .distinct()
        
        total_records = data.count()

        for row in data.paginate(page=param.get('page_index'), per_page=param.get('page_size'), error_out=False).items:
            obj = dict(zip(select_field.keys(), row))
            del obj['id']
            items.append(obj)
        
        results = {
            'data': items,
            'total_records': total_records
        }

        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error