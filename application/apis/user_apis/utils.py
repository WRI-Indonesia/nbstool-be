from flask import jsonify, g as g_var
from ... import db
from ...models.user_models.models import User, UserSessions
from ...models.geos_models.models import DataAnalyzer

class UserLogic():

    @staticmethod
    def get_auth_success_response(known_user, status_code=200, message='successfully logged in'):

        access_token, token_expire_time = known_user.encode_access_token()
        refresh_token = known_user.encode_refresh_token()

        response = jsonify(
            status="success",
            user_id=known_user.id,
            message=message,
            access_token=access_token,
            refresh_token=refresh_token,
            token_type="bearer",
            expires_in=token_expire_time,
        )
        response.status_code = status_code # HTTPStatus.OK
        response.headers["Cache-Control"] = "no-store"
        response.headers["Pragma"] = "no-cache"

        return response
    

    @staticmethod
    def add_data_analyzer(session_id=None):
        known_data_analyzer = DataAnalyzer()
        known_data_analyzer.session_id = session_id

        db.session.add(known_data_analyzer)

    
    @staticmethod
    def add_user_sessions_for_log(user_id=None, session_id=None):
        # add for log
        known_user_session = UserSessions()
        known_user_session.user_id = user_id
        known_user_session.session_id = session_id if session_id else g_var.__session_id__
        known_user_session.is_active = 1
        known_user_session.is_project = 0

        db.session.add(known_user_session)

        UserLogic.add_data_analyzer(known_user_session.session_id)
        # end add for log