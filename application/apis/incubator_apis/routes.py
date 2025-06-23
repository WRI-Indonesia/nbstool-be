# application/apis/incubator_apis/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from . import incubator_apis_blueprint
from ... import db
from ...models.master_models.models import DocumentList, DocumentData, Organization, Settings
from ...models.user_models.models import UserSessions

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename

import os
import gc
import uuid
import json
import base64

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler
from ...utils.common.mail import BaseMail, EMailIncubatorsReviewToIncubator, EMailIncubatorsReviewToUser

from ...utils.cloud_gdrive import CloudDrive
from ...utils.cloud_storage import CloudStorage

drive = CloudDrive()
gcs = CloudStorage()

@incubator_apis_blueprint.route('/review', methods=['POST'])
@cross_origin()
def post_incubator_review():
    g_var.__api_name__ = 'post_incubator_review'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        document_id = data.get('document_id')

        if not document_id:
            raise AppMessageException('please provide document id')
        
        known_document = DocumentList.query.filter_by(document_id=document_id).first()
        if not known_document:
            raise AppMessageException('invalid document id: document not found')
        
        user_sessions = UserSessions.query.filter_by(session_id=known_document.project_id).first()
        if not user_sessions:
            raise AppMessageException('sorry, you dont have permission to submit this document for incubators (1)')
        if user_sessions.user_id != current_user.id:
            raise AppMessageException('sorry, you dont have permission to submit this document for incubators (2)')
        
        g_var.__session_id__ = known_document.project_id
        
        organization_type = Organization.query.filter_by(id=current_user.organization_type_id).first()
        user = current_user.to_json()

        if current_user.extended_data:
            user.update(current_user.extended_data)

        # prepare incubators email
        INCUBATOR_EMAIL_LIST = Settings.find_by_name(name='INCUBATOR_EMAIL_LIST')
        try:
            INCUBATOR_EMAIL_LIST = INCUBATOR_EMAIL_LIST.value
        except Exception as e:
            current_app.logger.info('incubator review: {}'.format(str(e)))
            raise Exception('incubator review: incubator email list invalid or not found')
        # end prepare incubators email
        
        # download document from gcs
        filename = known_document.document_name
        if known_document.document_type == 'CCB':
            filepath = 'generated-file/docx-ccb/{}'.format(filename)
        else:
            filepath = 'generated-file/docx/{}'.format(filename)
        gcs.download(filepath)
        # end download document from gcs
        
        # upload document to drive
        INCUBATOR_DRIVE_ID = Settings.find_by_name(name='INCUBATOR_DRIVE_ID')
        try:
            INCUBATOR_DRIVE_ID = INCUBATOR_DRIVE_ID.value
        except Exception as e:
            current_app.logger.info('incubator review: {}'.format(str(e)))
            raise Exception('incubator review: incubator drive id invalid or not found')
        folder = drive.create_anyone_folder('[{}] {} Document\'s'.format(str(get_date().timestamp()).split('.')[0], user.get('organization_name')), INCUBATOR_DRIVE_ID)
        file_id = drive.create_document(
            path=filepath,
            name=known_document.client_name,
            parents_id=folder.get('id')
        )
        # end upload document to drive
        
        # prepare mail
        mail_ = BaseMail(
            to=INCUBATOR_EMAIL_LIST,
            subject=EMailIncubatorsReviewToIncubator.SUBJECT,
            template=EMailIncubatorsReviewToIncubator.TEMPLATE,
            data={
                'user': current_user.to_json(),
                'document_link': folder.get('webViewLink'),
                'organization_type': organization_type.to_json(),
            }
        )
        mail_.send_brevo_mail() # send to incubator

        mail_.to = current_user.email
        mail_.subject = EMailIncubatorsReviewToUser.SUBJECT
        mail_.template = EMailIncubatorsReviewToUser.TEMPLATE
        mail_.send_brevo_mail() # send to user
        # end prepare mail

        results = { }

        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error
