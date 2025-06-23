# application/utils/handler.py

from flask import current_app
import sys
import logging
from sqlalchemy import exc
import gc

logging.basicConfig(
    stream=sys.stdout, 
    level=logging.DEBUG,
    format='%(asctime)s - %(process)d - %(levelname)s - %(message)s',
)
env = current_app.config.get('ENV')

def eprint(*args, **kwargs):
    logging.error(*args)

class AppMessageException(Exception):
    pass

def app_exception_handler(e, status_code=None, default_data={}, message='something went wrong', services='defaultservices'):
    # print('err: ', sys.exc_info())
    # print('err: ', type(e).__name__)
    eprint('{}: {}'.format(services, str(e)))

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