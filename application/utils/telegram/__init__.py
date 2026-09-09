# application/utils/telegram/__init__.py
import os
import requests


def send_message(text):
    '''
    post one HTML-formatted message to the configured Telegram chat/topic.
    env: TELEGRAM_BOT_API_KEY, TELEGRAM_BOT_CHAT_ID, TELEGRAM_BOT_CHAT_THREAD_ID (optional, forum topic)
    '''
    api_key = os.environ.get('TELEGRAM_BOT_API_KEY')
    chat_id = os.environ.get('TELEGRAM_BOT_CHAT_ID')
    thread_id = os.environ.get('TELEGRAM_BOT_CHAT_THREAD_ID')

    if not api_key or not chat_id:
        raise Exception('telegram not configured: TELEGRAM_BOT_API_KEY / TELEGRAM_BOT_CHAT_ID missing')

    payload = {
        'chat_id': chat_id,
        'text': text,
        'parse_mode': 'HTML',
        'disable_web_page_preview': True,
    }
    if thread_id:
        payload['message_thread_id'] = thread_id

    response = requests.post(
        'https://api.telegram.org/bot{}/sendMessage'.format(api_key),
        json=payload,
        timeout=10,
    )
    body = response.json()
    if not body.get('ok'):
        raise Exception('telegram send failed: {}'.format(body.get('description', response.text)))

    return body
