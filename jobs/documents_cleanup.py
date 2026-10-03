#!/usr/bin/python3

'''
documents cleanup for retention policy
every 15 minutes: delete level 1 projects 24h after creation,
and email their owners a deletion reminder 12h after creation
'''

__jobs_name__ = 'documents_cleanup'

import os
import sys
import time
from datetime import datetime, UTC

def print_log(s):
    now = datetime.now(UTC)
    print('[{} UTC] {} - {}'.format(now.isoformat(), __jobs_name__, s), file=sys.stdout)
    sys.stdout.flush()

def close_script():
    exit()

try:
    import requests
    from dotenv import load_dotenv
    import urllib.parse
except Exception as e:
    print_log('failed to start. please install package: requests==2.32.3, python-dotenv==1.0.1')
    print_log('python3 -m pip install requests==2.32.3')
    print_log('python3 -m pip install python-dotenv==1.0.1')
    close_script()

dotenv_path = os.path.join(os.path.dirname(__file__), '.env')
if os.path.exists(dotenv_path):
    load_dotenv(dotenv_path)

BACKEND_API_URL = os.environ.get('BACKEND_API_URL') # https://nbstool-be.scenecoalition.org
JOBS_TOKEN = os.environ.get('JOBS_TOKEN') # the_token
TELEGRAM_BOT_API_KEY = os.environ.get('TELEGRAM_BOT_API_KEY')
TELEGRAM_BOT_CHAT_ID = os.environ.get('TELEGRAM_BOT_CHAT_ID')
TELEGRAM_BOT_CHAT_THREAD_ID = os.environ.get('TELEGRAM_BOT_CHAT_THREAD_ID')

if BACKEND_API_URL is None or BACKEND_API_URL == '':
    print_log('please provide BACKEND_API_URL either in .env files or environment variables.')
    close_script()
elif JOBS_TOKEN is None or JOBS_TOKEN == '':
    print_log('please provide JOBS_TOKEN either in .env files or environment variables.')
    close_script()
elif TELEGRAM_BOT_API_KEY is None or TELEGRAM_BOT_API_KEY == '':
    print_log('please provide TELEGRAM_BOT_API_KEY either in .env files or environment variables.')
    close_script()
elif TELEGRAM_BOT_CHAT_ID is None or TELEGRAM_BOT_CHAT_ID == '':
    print_log('please provide TELEGRAM_BOT_CHAT_ID either in .env files or environment variables.')
    close_script()

def telegram_escape_char(s):
    sym_list = ['_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!', '&']
    for sym in sym_list: #sym_list.keys()
        s = s.replace(sym, '\\{}'.format(sym))
    return s

def telegram_send_message(s):
    msg = '`{}`'.format(urllib.parse.quote_plus(telegram_escape_char(s)))

    send_text = 'https://api.telegram.org/bot'
    send_text += TELEGRAM_BOT_API_KEY
    send_text += '/sendMessage?chat_id='
    send_text += TELEGRAM_BOT_CHAT_ID
    if TELEGRAM_BOT_CHAT_THREAD_ID:
        send_text += '&message_thread_id='
        send_text += TELEGRAM_BOT_CHAT_THREAD_ID
    send_text += '&parse_mode=markdownv2&text='
    send_text += msg
    print_log('telegram: {}'.format(send_text))

    response = requests.get(send_text)
    print_log('telegram: {}'.format(str(response.json())))

    return response.json()

def call_job(label, path, count_keys):
    message = ''
    success = False
    count = 0
    try:
        r = requests.get(BACKEND_API_URL + path + '?token={}'.format(JOBS_TOKEN))
        print_log('{} requests ({}): {}'.format(label, r.status_code, r.text))
        if r.status_code != 200:
            message = 'failed ({}): {}'.format(r.status_code, r.text)
        else:
            result = r.json().get('result')
            count = sum(result.get(k) or 0 for k in count_keys)
            message = 'done: {}'.format(result)
            success = True
    except Exception as e:
        print_log('{} requests error: {}'.format(label, str(e)))
        message = 'error: {}'.format(str(e))

    print_log('{} {}'.format(label, message))
    # quiet runs stay off telegram: every 15 minutes would flood the chat
    if not success or count:
        telegram_send_message('{} {} {}'.format(chr(0x1F534) if not success else chr(9989), label, message))


while True:
    call_job('document cleanup', '/documents/cleanup', ('total_records',))
    call_job('deletion reminder', '/documents/cleanup/reminder', ('sent_records', 'failed_records'))

    print_log('waiting...')
    time.sleep(900) # 15 minutes
