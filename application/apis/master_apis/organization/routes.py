# application/apis/master_apis/organization/routes.py
from flask import jsonify, request, make_response, g as g_var
from flask_login import current_user
from .. import master_apis_blueprint
from .... import db
from ....models.master_models.models import Organization

from datetime import datetime
from datetime import timedelta

from flask_cors import cross_origin

import gc

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler


# legacy: /nbsapi/cms/organization-list [GET]
@master_apis_blueprint.route('/organization/list', methods=['GET'])
@cross_origin()
def organization_list():
    __api_name__ = 'master_organization_list'

    try:
        # if not (current_user.is_authenticated):
        #     return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        attr = []

        if current_user.is_authenticated:
            attr = ['id', 'name', 'is_active', 'created_at']

        data = request.args

        is_active = data.get('is_active')
        if is_active in (True, 1, 'true'):
            is_active = 1
        else:
            is_active = 0

        data = Organization.query.filter_by(is_active=is_active)

        items = []
        for row in data:
            items.append(row.to_json(attr=['id', 'name', 'is_active'] if not attr else attr))
        
        results = {
            'organizations': items
        }

        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 500) # send internal error


# legacy: /nbsapi/cms/organization-list [PUT]
@master_apis_blueprint.route('/organization', methods=['POST', 'PUT'])
@cross_origin()
def organization_saveupdate():
    __api_name__ = 'master_organization_saveupdate'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        id = data.get('id')
        name = data.get('name')
        is_active = data.get('is_active')

        if not name:
            raise AppMessageException('please input: name (text mandatory)')
        if is_active is None:
            raise AppMessageException('please input: is_active (boolean mandatory)')
        if is_active not in (True, False, 0, 1):
            raise AppMessageException('invalid input format: is_active not in [true, false, 0, 1]')
        
        if is_active:
            is_active = 1
        else:
            is_active = 0
        
        if id:
            known_organization = Organization.query.filter_by(id=id).first()
            if not known_organization:
                raise AppMessageException('Data is failed to change, not found')
        else:
            raise AppMessageException('not yet implemented') # prevent create
            known_organization = Organization()
            known_organization.created_by = 1
        
        known_organization.name = name
        known_organization.is_active = is_active
        known_organization.updated_at = get_date()

        db.session.add(known_organization)
        db.session.commit()

        gc.collect()

        return make_response(jsonify(success_handler({}, message="Data succesfully changed")), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=__api_name__)), 500) # send internal error

