# application/utils/handler.py

from flask import current_app, g, has_request_context
import sys
import logging
from sqlalchemy import exc
import gc


class RequestContextFilter(logging.Filter):
    """Stamp every log line with the request id and analysis session id from flask.g, so a
    stdout traceback can be joined to tbl_logger_logs rows without timestamp guessing.

    '-' outside a request context -- which includes the v3 ThreadPoolExecutor workers, since
    flask.g does not cross threads; their failures are correlated through the stream run rows
    (log_stream_event) instead."""

    def filter(self, record):
        record.request_id = '-'
        record.session_id = '-'
        try:
            if has_request_context():
                record.request_id = g.get('__request_id__') or '-'
                record.session_id = g.get('__session_id__') or '-'
        except Exception:
            pass
        return True


logging.basicConfig(
    stream=sys.stdout,
    level=logging.DEBUG,
    format='%(asctime)s - %(process)d - %(levelname)s - [req:%(request_id)s ses:%(session_id)s] - %(message)s',
)
# basicConfig's handler formats every record that reaches the root logger, so the filter must
# sit on the handler (not a logger) to guarantee the two fields exist on all of them.
for _handler in logging.getLogger().handlers:
    _handler.addFilter(RequestContextFilter())

env = current_app.config.get('ENV')

def eprint(*args, **kwargs):
    logging.error(*args)

class AppMessageException(Exception):
    pass

def app_exception_handler(e, status_code=None, default_data={}, message='something went wrong', services='defaultservices'):
    # print('err: ', sys.exc_info())
    # print('err: ', type(e).__name__)
    if isinstance(e, AppMessageException):
        eprint('{}: {}'.format(services, str(e)))
    else:
        # Unexpected failure: keep the traceback. In prod the response message is redacted
        # below, so this log line is the only place the real cause survives.
        logging.exception('{}: {}'.format(services, str(e)))

    context = { }

    message = str(e)

    try:
        raise e
    except AppMessageException: # handle app message
        pass
    except exc.SQLAlchemyError: # handle error db
        if env == 'development':
            pass
        else:
            message = '-- prod redacted, please contact admin --'
    except:
        if env == 'development':
            pass
        else:
            message = '-- prod redacted, please contact admin --'

    context['message'] = message
    context['success'] = False

    if status_code:
        context['status_code'] = status_code

    gc.collect()

    return context

def success_handler(results:dict=None, status_code:int=None, message:str=None):
    context = {}
    if results:
        context = results
    if message:
        context['message'] = message
    if status_code:
        context['status_code'] = status_code

    gc.collect()

    return context