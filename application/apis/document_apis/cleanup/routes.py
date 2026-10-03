# application/apis/document_apis/cleanup/routes.py
from flask import jsonify, request, make_response, current_app, g as g_var, send_file
from flask_login import current_user
from .. import document_apis_blueprint
from .... import db
from ....models.master_models.models import DocumentList
from ....models.user_models.models import SessionsAuth, UserSessions

# privacy levels whose data stays on the server past the session expiry
PRIVACY_LEVELS_RETAINED = (2, 3)

from datetime import datetime, timedelta
from flask_cors import cross_origin
from werkzeug.utils import secure_filename
from pathlib import Path

import os
import gc
import uuid
import json
import base64
import shutil

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler

from ....utils.geos import GeoUtils

from ....utils.logger import log_func

from ....utils.common.mail import BaseMail, EMailProjectDeletionReminder

from .. import gcs


def _check_jobs_token():
    # shared with jobs/documents_cleanup.py; unset on the server = every call refused
    token = os.environ.get('JOBS_TOKEN')
    if not token or request.args.get('token') != token:
        raise AppMessageException('you dont have permission to do this!')


@document_apis_blueprint.route('/cleanup/reminder', methods=['GET'])
@cross_origin()
def documents_cleanup_reminder():
    g_var.__api_name__ = 'documents_cleanup_reminder'

    g_var.__log_it__ = False
    g_var.__session_id__ = None

    try:
        _check_jobs_token()

        # Level 1 (or unset) projects of standard users, 12h+ old, not yet past the 24h
        # deletion and not yet reminded -- the same population vwExpiredSession deletes.
        query = '''
        select
            us.id,
            us.session_id,
            us.project_name,
            u.email,
            s.created_at + interval '24 hours' as deletion_at
        from tbl_user_sessions us
        join tbl_sessions s on s.session_id = us.session_id
        join tbl_users u on u.id = us.user_id
        where us.is_active = 1
            and s.is_active = 1
            and us.is_project
            and u.permission_policy = 1
            and coalesce(us.privacy_level, 1) = 1
            and us.deletion_reminder_sent_at is null
            and s.created_at <= current_timestamp - interval '12 hours'
            and s.created_at > current_timestamp - interval '24 hours'
        '''

        sent = failed = 0
        for row in db.session.execute(db.text(query)).mappings().all():
            project_name = row['project_name'] or 'Untitled project'
            mail_ = BaseMail(
                to=row['email'],
                subject=EMailProjectDeletionReminder.SUBJECT.format(project_name),
                template=EMailProjectDeletionReminder.TEMPLATE,
                data={
                    'user_email': row['email'],
                    'project_name': project_name,
                    'session_id': row['session_id'],
                    'deletion_date_time': row['deletion_at'].strftime('%d %B %Y, %H:%M UTC'),
                }
            )
            if not mail_.send_mail():
                failed += 1
                continue

            # stamped per mail so a crash mid-loop never re-sends; updated_at kept as-is
            # (the reminder is not a user edit of the project)
            UserSessions.query.filter_by(id=row['id']).update({
                'deletion_reminder_sent_at': get_date(),
                'updated_at': UserSessions.updated_at,
            })
            db.session.commit()
            sent += 1

        status_code = 200
        message = 'Deletion reminders sent'
        return make_response(jsonify(success_handler({ 'result': {'sent_records': sent, 'failed_records': failed} }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/project-management/data-cleanup [POST]
@document_apis_blueprint.route('/cleanup', methods=['GET'])
@cross_origin()
def documents_cleanup_data():
    g_var.__api_name__ = 'documents_cleanup_data'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        _check_jobs_token()

        query = '''
        select 
            *
        from "vwExpiredSession"
        '''

        dt = GeoUtils.get_db(db.text(query), gis_db=False)
        total_records = 0
        retained_records = 0

        for s in dt:
            session_id = s['session_id']
            session = SessionsAuth.find_by_session_id(session_id)

            if session:
                project = UserSessions.find_by_session_id(session_id)

                # Privacy level (see PROJECT_PRIVACY_LEVELS): level 1 = kept for the user's
                # 1x24h only, so it expires like before; level 2 and 3 = stored on the server
                # for the user's (and, at 3, partners') later use, so the expiry never touches
                # it. Rows without a level (legacy) follow the as-is expiry path.
                if project and project.privacy_level in PRIVACY_LEVELS_RETAINED:
                    retained_records += 1
                    continue

                total_records += 1

                # deactivate the session
                session.is_active = 0

                db.session.add(session)

                if project:
                    project.is_active = 0

                    db.session.add(project)

                documents = DocumentList.find_by_project_id(session_id)

                if documents:
                    for d in documents:
                        d.is_active = 0

                    db.session.add_all(documents)

                # delete current condition calculation temporary files and folder
                cc_temp_folder = Path("temp_file", session_id).resolve()

                if os.path.isdir(cc_temp_folder):
                    shutil.rmtree(cc_temp_folder)

                # delete csv data source files
                csv_temp_folder = Path("generated-file", "csv").resolve()
                for p in csv_temp_folder.glob(session_id + "*.csv"):
                    gcs.delete(os.path.join('generated-file', 'csv', p.name))
                    p.unlink()

                # delete general template documentation files
                general_template_temp_folder = Path("generated-file", "docx").resolve()
                for p in general_template_temp_folder.glob(session_id + "*.docx"):
                    gcs.delete(os.path.join('generated-file', 'docx', p.name))
                    p.unlink()

                # delete ccb documentation files
                ccb_temp_folder = Path("generated-file", "docx-ccb").resolve()
                for p in ccb_temp_folder.glob(session_id + "*.docx"):
                    gcs.delete(os.path.join('generated-file', 'docx-ccb', p.name))
                    p.unlink()

                # delete graph files
                graph_temp_folder = Path("generated-file", "graph").resolve()
                for p in graph_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'graph', p.name))
                    p.unlink()

                # delete logo files
                logo_temp_folder = Path("generated-file", "logo").resolve()
                for p in logo_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'logo', p.name))
                    p.unlink()

                # delete project area files
                project_area_temp_folder = Path("generated-file", "project-area").resolve()
                for p in project_area_temp_folder.glob(session_id + "*.jpg"):
                    gcs.delete(os.path.join('generated-file', 'project-area', p.name))
                    p.unlink()

                # delete excel files
                xlsx_temp_folder = Path("generated-file", "xlsx").resolve()
                for p in xlsx_temp_folder.glob(session_id + "*.xlsx"):
                    gcs.delete(os.path.join('generated-file', 'xlsx', p.name))
                    p.unlink()
        
                db.session.commit()

                g_var.__session_id__ = session_id
                g_var.log_type_code = 'LOG_SUCCESS'
                resp_data = {
                    'json_response': False,
                    'data': {
                        'message': 'manual logging on documents_cleanup_data api',
                        'text': ''
                    }
                }

                log_func(db.session, resp_data)

        
        g_var.__log_it__ = False
        status_code = 200
        message = 'Expired data has succesfully clean'
        return make_response(jsonify(success_handler({ 'result': {'total_records': total_records, 'retained_records': retained_records} }, status_code=status_code, message=message)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error