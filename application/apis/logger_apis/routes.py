# application/apis/logger_apis/faq/routes.py
from flask import jsonify, request, make_response, g as g_var, current_app
from flask_login import current_user
from . import logger_apis_blueprint
from ... import db
from ...models.logger_models.models import Logs, ProblemReport
from ...models.master_models.models import Dictionary

from datetime import datetime
from datetime import timedelta

from flask_cors import cross_origin

import gc

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler
from ...utils.cloud_recaptcha import CloudRecaptcha
from ...utils import telegram
from ...utils.cloud_storage import CloudStorage

import os
import re
import html
import base64
import binascii

recaptcha = CloudRecaptcha()

MAX_REPORT_DESCRIPTION = 4000
# the FE's automatic screenshot: a base64 (data URL or bare) PNG/JPEG/WebP, decoded size cap
MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024
SCREENSHOT_FOLDER = os.path.join('generated-file', 'report-problem')
_IMAGE_MAGIC = (
    (b'\x89PNG\r\n\x1a\n', 'png'),
    (b'\xff\xd8\xff', 'jpg'),
    (b'RIFF', 'webp'),
)


def _decode_screenshot(value):
    '''base64 (optionally a data URL) -> (bytes, ext). Raises AppMessageException on bad input.'''
    if not isinstance(value, str):
        raise AppMessageException('invalid input: screenshot must be a base64 string')
    value = re.sub(r'^data:image/[a-zA-Z0-9.+-]+;base64,', '', value.strip())
    if len(value) > MAX_SCREENSHOT_BYTES * 4 // 3 + 4:
        raise AppMessageException('invalid input: screenshot exceeds {} MB'.format(MAX_SCREENSHOT_BYTES // (1024 * 1024)))
    try:
        raw = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError):
        raise AppMessageException('invalid input: screenshot is not valid base64')
    if len(raw) > MAX_SCREENSHOT_BYTES:
        raise AppMessageException('invalid input: screenshot exceeds {} MB'.format(MAX_SCREENSHOT_BYTES // (1024 * 1024)))
    for magic, ext in _IMAGE_MAGIC:
        if raw.startswith(magic):
            return raw, ext
    raise AppMessageException('invalid input: screenshot must be a PNG, JPEG or WebP image')


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

@logger_apis_blueprint.route('/report-problem', methods=['POST'])
@cross_origin()
def logger_report_problem():
    g_var.__api_name__ = 'logger_report_problem'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = {}
    try:
        g_var.__request_data__ = request.get_json()
        # the screenshot is MBs of base64; keep it out of the activity log row
        if isinstance(g_var.__request_data__, dict) and 'screenshot' in g_var.__request_data__:
            g_var.__request_data__ = dict(g_var.__request_data__)
            g_var.__request_data__['screenshot'] = '<stripped>'
    except:
        pass

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')

        data = request.get_json()

        description = (data.get('description') or '').strip()
        if len(description) > MAX_REPORT_DESCRIPTION:
            raise AppMessageException('invalid input: description exceeds {} characters'.format(MAX_REPORT_DESCRIPTION))

        screenshot = None
        if data.get('screenshot'):
            screenshot = _decode_screenshot(data.get('screenshot'))

        recaptcha.verify(data.get('recaptcha_token'), 'report_problem')

        session_id = data.get('session_id')
        g_var.__session_id__ = session_id

        reporter = current_user.email if current_user.is_authenticated else 'anonymous'
        env = '{} @ {}'.format(current_app.config.get('ENV'), request.host)
        user_agent = data.get('user_agent') or request.headers.get('User-Agent')

        # 1. persist: the row is the record, Telegram is the notification
        report = ProblemReport()
        report.session_id = session_id
        report.user_id = current_user.id if current_user.is_authenticated else None
        report.reporter = reporter
        report.description = description or None
        report.page_url = str(data.get('page_url'))[:2048] if data.get('page_url') else None
        report.locale = str(data.get('locale'))[:16] if data.get('locale') else None
        report.user_agent = str(user_agent)[:1024] if user_agent else None
        report.env = env[:128]

        db.session.add(report)
        db.session.flush()

        if screenshot:
            raw, ext = screenshot
            os.makedirs(SCREENSHOT_FOLDER, exist_ok=True)
            path = os.path.join(SCREENSHOT_FOLDER, 'report_{}.{}'.format(report.id, ext))
            with open(path, 'wb') as f:
                f.write(raw)
            CloudStorage().upload(path)
            report.screenshot_path = path.replace(os.sep, '/')

        db.session.commit()

        # 2. notify; a Telegram outage must not lose (or 500) a report already saved
        fields = [
            ('Report', '#{}'.format(report.id)),
            ('Env', env),
            ('Reporter', reporter),
            ('Session', session_id),
            ('Page', data.get('page_url')),
            ('Locale', data.get('locale')),
            ('User agent', user_agent),
            ('Time (UTC)', get_date().strftime('%Y-%m-%d %H:%M:%S')),
        ]
        lines = ['\U0001F41E <b>Problem report</b>']
        lines += ['<b>{}:</b> {}'.format(k, html.escape(str(v))) for k, v in fields if v]
        if description:
            lines += ['', html.escape(description)]

        try:
            sent = telegram.send_message('\n'.join(lines))
            if screenshot:
                telegram.send_photo(
                    screenshot[0],
                    filename='report_{}.{}'.format(report.id, screenshot[1]),
                    caption='Screenshot for report #{}'.format(report.id),
                    reply_to_message_id=(sent.get('result') or {}).get('message_id'),
                )
        except Exception as e:
            current_app.logger.warning('report-problem #{} saved but telegram failed | {}'.format(report.id, str(e)))

        return make_response(jsonify(success_handler({ 'report_id': report.id }, status_code=201, message='Thank you, your report has been received')), 201)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error
