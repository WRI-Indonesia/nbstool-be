# application/utils/logger/__init__.py
from flask import g as g_var, current_app, render_template_string
from flask_login import current_user

from sqlalchemy.orm import scoped_session, sessionmaker

from ... import db
from ...models.logger_models.models import Logs
from ...models.master_models.models import Dictionary

def log_func(session, resp_data):
    try:
        known_log_type = session.query(Dictionary).filter_by(code=g_var.get('log_type_code'), type='log type').first()
        if not known_log_type:
            known_log_type = {}
        else:
            known_log_type = known_log_type.to_json()
    except Exception as e:
        current_app.logger.warning('{} - after_request - logger (get log type): {}'.format(str(g_var.get('__api_name__')), str(e)))
        known_log_type = {}
    
    try:
        known_activity_type = session.query(Dictionary).filter_by(code=g_var.get('__api_name__'), type='activity type').first()
        if not known_activity_type:
            known_activity_type = {}
        else:
            known_activity_type = known_activity_type.to_json()
    except Exception as e:
        current_app.logger.warning('{} - after_request - logger (get activity type): {}'.format(str(g_var.get('__api_name__')), str(e)))
        known_activity_type = {}

    log = Logs()
    log.log_type_code = g_var.get('log_type_code')
    log.log_type_id = known_log_type.get('id')
    log.log_type_name = known_log_type.get('name')

    log.activity_type_code = g_var.get('__api_name__')
    log.activity_type_id = known_activity_type.get('id')
    log.activity_type_name = known_activity_type.get('name')

    log.session_id = g_var.get('__session_id__')

    log.request_data = g_var.get('__request_data__')
    log.response_data = resp_data

    if known_activity_type.get('description') and '{{' in known_activity_type.get('description') and g_var.get('__description_data__'):
        log.description = render_template_string(known_activity_type.get('description'), data=g_var.get('__description_data__'))
    else:
        log.description = known_activity_type.get('description')
    
    if current_user.is_authenticated:
        log.created_by = current_user.id
    
    try:
        session.add(log)
        session.commit()
    except Exception as e:
        current_app.logger.warning('{} - after_request - logger (db commit): {}'.format(str(g_var.get('__api_name__')), str(e)))


def logging(response):
    '''
    logging api activity to the database
    
    please provide from api context:
    g_var.__log_it__           --> log it if True
    g_var.__api_name__         --> activity name, get the name and description from master dictionary by code
    g_var.__session_id__       --> mandatory session_id
    g_var.__request_data__     --> request details
    g_var.__description_data__ --> description details context
    '''

    session = scoped_session(sessionmaker(autocommit=False, bind=db.engine))

    if response.status_code == 200:
        g_var.log_type_code = 'LOG_SUCCESS'
    else:
        g_var.log_type_code = 'LOG_FAILED'

    resp_data = {
        'status_code': response.status_code,
        'json_response': True
    }

    try:
        resp_data['data'] = response.get_json()
    except Exception as e:
        current_app.logger.warning('{} - after_request - logger (get_json): {}'.format(str(g_var.get('__api_name__')), str(e)))
        resp_data['json_response'] = False
        resp_data['data'] = {
            'message': 'error get response json: {}'.format(str(e)),
            'text': str(response.data)
        }
    
    log_func(session, resp_data)
    
    session.remove()