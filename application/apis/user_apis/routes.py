# application/apis/user_apis/routes.py
from . import user_apis_blueprint
from ... import db, login_manager
from ...models.user_models.models import User, UserSessions
from ...models.master_models.models import Organization
from flask import make_response, request, jsonify, current_app, g as g_var
from flask_login import current_user, login_user, logout_user, login_required

from passlib.hash import sha256_crypt

import uuid
import base64

from datetime import datetime, timedelta
from werkzeug.utils import secure_filename

from flask_cors import cross_origin

import gc
import os
import re
import json
import jwt

from flask_jwt_extended import create_access_token, decode_token

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler
from ...utils.common import allowed_image_file
from ...utils.common.mail import BaseMail, EMailUserRegister, EMailUserForgotPassword
from ...utils.cloud_storage import CloudStorage

from .utils import UserLogic

gcs = CloudStorage()

@login_manager.user_loader
def load_user(user_id):
    return User.query.filter_by(id=user_id).first()

@login_manager.request_loader
def load_user_from_request(request):
    api_key = request.headers.get('Authorization')
    if api_key:
        if api_key.startswith("Bearer "):
            api_key = api_key.replace('Bearer ', '', 1)

        try:
            key = current_app.config.get("SECRET_KEY")
            payload = jwt.decode(api_key, key, algorithms=["HS256"])
        except jwt.ExpiredSignatureError:
            # raise AppMessageException("Access token expired. Please log in again.")
            error = "Access token expired. Please log in again."
            # return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
            return None
        except jwt.InvalidTokenError:
            # raise AppMessageException("Invalid token. Please log in again.")
            error = "Invalid token. Please log in again."
            # return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
            return None

        user = User.query.filter_by(id=payload.get('user_id')).first()

        return user

    return None


# legacy: /nbsapi/auth/get-token [POST]
@user_apis_blueprint.route('/account/get-token', methods=['GET'])
@cross_origin()
def user_account_get_token():
    g_var.__api_name__ = 'user_account_get_token'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        return UserLogic.get_auth_success_response(current_user, message='successfully generated user token')
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/user [GET]
@user_apis_blueprint.route('/account', methods=['GET'])
@cross_origin()
def user_get_account():
    g_var.__api_name__ = 'user_get_account'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        
        avatar = None
        if current_user.avatar:
            avatar = base64.b64encode(current_user.avatar).decode('ascii')

        results = current_user.to_json(attr=['name', 'email', 'organization_type_id', 'organization_name', 'permission_policy', 'avatar', 'id'])
        results['user_id'] = results.get('id')
        del results['id']
        results['avatar'] = avatar

        if current_user.extended_data:
            results.update(current_user.extended_data)
        
        return make_response(jsonify(success_handler(results)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/user [POST]
@user_apis_blueprint.route('/account', methods=['POST'])
@cross_origin()
def user_save_profile():
    g_var.__api_name__ = 'user_save_profile'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        
        form = request.form
        data = form.get('data')
        section = form.get('section')
        avatar = request.files.get('logo')

        try:
            data = json.loads(data)
        except:
            raise AppMessageException('bad request, invalid data')

        if avatar:
            if not avatar.filename == '' and allowed_image_file(avatar.filename):
                upload_folder = 'Uploaded-File'
                filepath = os.path.join(upload_folder, "avatar")
                if not os.path.exists(filepath):
                    os.makedirs(filepath)
                
                user_id = str(current_user.id)
                file_ext = "." + avatar.filename.rsplit('.', 1)[1].lower()

                base_filename = "avatar_" + user_id + file_ext
                file_path = os.path.join(filepath, secure_filename(base_filename))

                avatar.save(file_path)

                gcs.upload(file_path)

                data["avatar"] = file_path

        if section == "profile":
            extended_data = data

            if data.get("name"):
                current_user.name = data["name"]
                extended_data.pop("name")

            if data.get("email"):
                current_user.email = data["email"]
                extended_data.pop("email")

            if data.get("organization_type_id"):
                current_user.organization_type_id = data["organization_type_id"]
                extended_data.pop("organization_type_id")

            if data.get("organization_name"):
                current_user.organization_name = data["organization_name"]
                extended_data.pop("organization_name")

            if data.get("avatar"):
                avatar = open(data["avatar"], 'rb').read() 

                current_user.avatar = avatar

                extended_data.pop("avatar")

            current_user.extended_data = extended_data
        else:
            if data.get("permissionPolicy"):
                current_user.permission_policy = data["permissionPolicy"]

        db.session.commit()

        return make_response(jsonify(success_handler(status_code=200, message='Updated. Data is updated successfully')), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/register [POST]
@user_apis_blueprint.route('/account/register', methods=['POST'])
@cross_origin()
def user_account_register():
    g_var.__api_name__ = 'user_account_register'
    
    g_var.__log_it__ = True
    g_var.__session_id__ = str(uuid.uuid4())
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()
        
        email = data.get("email")
        password = data.get("password")
        name = data.get("name")
        organization_type_id = data.get("organizationTypeId")
        organization_name = data.get("organizationName")
        permission_policy = data.get("permissionPolicy")

        if not email:
            raise AppMessageException('please input: email (text mandatory)')
        if not password:
            raise AppMessageException('please input: password (text mandatory)')
        if not name:
            raise AppMessageException('please input: name (text mandatory)')
        if not organization_type_id:
            raise AppMessageException('please input: organizationTypeId (int mandatory)')
        if not organization_name:
            raise AppMessageException('please input: organizationName (text mandatory)')
        if not permission_policy:
            raise AppMessageException('please input: permissionPolicy (int mandatory)')
        
        if not re.match(r'[^@]+@[^@]+\.[^@]+', email):
            raise AppMessageException('invalid input format: email')
        try:
            organization_type_id = int(organization_type_id)
        except:
            raise AppMessageException('invalid input format: organizationTypeId')
        try:
            int(permission_policy)
        except:
            raise AppMessageException('invalid input format: permissionPolicy')
        
        known_organization_type = Organization.query.filter_by(id=organization_type_id).first()
        if not known_organization_type:
            raise AppMessageException('invalid input: organization type not found')
        
        known_user = User.query.filter_by(email=email).first()
        if known_user:
            if known_user.is_active:
                raise AppMessageException(f"{email} already registered and your account is active") # 400
            else:
                raise AppMessageException(f"{email} is already registered and not active, please activate your account by checking your email") # 400

        known_user = User()
        known_user.email = email
        known_user.name = name
        known_user.organization_type_id = organization_type_id
        known_user.organization_name = organization_name
        known_user.permission_policy = permission_policy
        known_user.password = password
        known_user.extended_data = {}
        
        known_user.encode_password()

        db.session.add(known_user)
        db.session.flush()
        db.session.refresh(known_user)

        # add for log
        UserLogic.add_user_sessions_for_log(known_user.id)
        # end add for log

        db.session.commit()
        
        # prepare mail
        expires = timedelta(days=1)
        access_token = create_access_token(identity=known_user.id, expires_delta=expires)
        
        mail_ = BaseMail(
            to=known_user.email,
            subject=EMailUserRegister.SUBJECT,
            template=EMailUserRegister.TEMPLATE,
            data={
                'name': known_user.email,
                'access_token': access_token
            }
        )
        mail_.send_brevo_mail()
        # end prepare mail

        return make_response(
            jsonify(
                success_handler(
                    {},
                    status_code=201, # HTTPStatus.CREATED.value,
                    message='Register is success, Please check your email to activate account')
                ),
                200
            )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/resend-verification [POST]
@user_apis_blueprint.route('/account/verification/resend', methods=['POST'])
@cross_origin()
def user_account_verification_resend():
    g_var.__api_name__ = 'user_account_verification_resend'

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        email = data.get('email')

        if not email:
            raise AppMessageException('please input: email (text mandatory)')
        if not re.match(r'[^@]+@[^@]+\.[^@]+', email):
            raise AppMessageException('invalid input format: email')
        
        known_user = User.query.filter_by(email=email).first()
        if not known_user:
            raise AppMessageException('user email is not registered')
        if known_user.is_active:
            raise AppMessageException(f'{email} already registered and your account is active')

        # prepare mail
        expires = timedelta(days=1)
        access_token = create_access_token(identity=known_user.id, expires_delta=expires)
        
        mail_ = BaseMail(
            to=known_user.email,
            subject=EMailUserRegister.SUBJECT,
            template=EMailUserRegister.TEMPLATE,
            data={
                'name': known_user.name,
                'access_token': access_token
            }
        )
        mail_.send_brevo_mail()
        # end prepare mail

        return make_response(
            jsonify(
                success_handler(
                    {},
                    status_code=201, # HTTPStatus.CREATED.value,
                    message='Your activation email has been sent. Please check your email to activate account')
                ),
                200
            )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/activate/<token> [GET]
@user_apis_blueprint.route('/account/verification/<token>', methods=['GET'])
@cross_origin()
def user_account_verification_token(token:str):
    g_var.__api_name__ = 'user_account_verification_token'
    
    g_var.__log_it__ = True
    g_var.__session_id__ = str(uuid.uuid4())
    g_var.__request_data__ = { 'token': token }

    try:
        user_token = decode_token(encoded_token=token, allow_expired=True)
        user_id = user_token['sub']
        
        known_user = User.query.filter_by(id=user_id).first()
        if not known_user:
            raise AppMessageException('Invalid activation code')
        if known_user.is_active:
            raise AppMessageException('Your account already registered and your account is active')
        
        try:
            decode_token(token)
        except Exception as e:
            if e.args[0] == 'Signature has expired':
                data = { 'user_email': known_user.email, 'status': 'fail' }
                message='Your verification code is not valid anymore'
            else:
                data = { 'status': 'fail' }
                message = 'Invalid activation code'
            return make_response(
                jsonify(
                    success_handler(
                        data,
                        status_code=401, # HTTPStatus.UNAUTHORIZED,
                        message=message)
                    ),
                    401
                )
        
        known_user.is_active = True
        
        db.session.add(known_user)

        # add for log
        UserLogic.add_user_sessions_for_log(known_user.id)
        # end add for log

        db.session.commit()

        return make_response(
                jsonify(
                    success_handler(
                        {},
                        message='Account activated successfully!')
                    ),
                    200
                )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/login [POST]
@user_apis_blueprint.route('/account/login', methods=['POST'])
@cross_origin()
def user_account_login():
    g_var.__api_name__ = 'user_account_login'

    g_var.__log_it__ = True
    g_var.__session_id__ = str(uuid.uuid4())
    try:
        g_var.__request_data__ = request.get_json()
        
        if type(g_var.__request_data__) == dict and g_var.__request_data__.get('password'):
            tmp_user = User()
            tmp_user.password = str(g_var.__request_data__['password'])
            tmp_user.encode_password()
            g_var.__request_data__['password'] = tmp_user.password
    except:
        pass

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        email = data.get('email')
        password = data.get('password')

        if not email:
            raise AppMessageException('please input: email (text mandatory)')
        if not re.match(r'[^@]+@[^@]+\.[^@]+', email):
            raise AppMessageException('invalid input format: email')
        if not password:
            raise AppMessageException('please input: password (text mandatory)')
        
        known_user = User.query.filter_by(email=email).first()
        if not known_user or not known_user.check_password(password):
            raise AppMessageException('email or password does not match')
        if not known_user.is_active:
            raise AppMessageException('user email has not been verified')
        
        login_user(known_user)
        
        # add for log
        UserLogic.add_user_sessions_for_log(known_user.id)
        db.session.commit()
        # end add for log
        
        return UserLogic.get_auth_success_response(known_user)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/forget-password [POST]
@user_apis_blueprint.route('/account/forgot-password', methods=['POST'])
@cross_origin()
def user_account_forgot_password():
    g_var.__api_name__ = 'user_account_forgot_password'

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        email = data.get('email')

        if not email:
            raise AppMessageException('please input: email (text mandatory)')
        if not re.match(r'[^@]+@[^@]+\.[^@]+', email):
            raise AppMessageException('invalid input format: email')
        
        known_user = User.query.filter_by(email=email).first()
        if not known_user:
            raise AppMessageException('email not found')
        
        access_token, token_expire_time = known_user.encode_access_token()

        # prepare mail
        mail_ = BaseMail(
            to=known_user.email,
            subject=EMailUserForgotPassword.SUBJECT,
            template=EMailUserForgotPassword.TEMPLATE,
            data={
                'name': known_user.name,
                'token': access_token,
                'expired': token_expire_time//60
            }
        )
        mail_.send_brevo_mail()
        # end prepare mail

        return make_response(
            jsonify(
                success_handler(
                    {},
                    status_code=200, # HTTPStatus.CREATED.value,
                    message='Reset Password Link is sent to your email')
                ),
                200
            )
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/auth/reset-password [POST]
@user_apis_blueprint.route('/account/reset-password', methods=['POST'])
@cross_origin()
def user_account_reset_password():
    g_var.__api_name__ = 'user_account_forgot_password'

    try:
        if not current_user.is_authenticated:
            raise AppMessageException('Access token expired or token invalid.')
            # return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)
        
        data = request.get_json()

        password = data.get('password')

        if not password:
            raise AppMessageException('please input: password (text mandatory)')

        current_user.password = password
        current_user.encode_password()

        db.session.add(current_user)
        db.session.commit()

        return make_response(jsonify(success_handler({}, message='Success')), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


