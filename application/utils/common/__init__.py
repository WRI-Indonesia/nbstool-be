# application/utils/__init__.py

from flask import render_template
from datetime import timedelta
from string import ascii_lowercase, digits
from sqlalchemy.orm.collections import InstrumentedList
from datetime import datetime, UTC
import requests

from Crypto.PublicKey import RSA
from Crypto.Cipher import PKCS1_v1_5, PKCS1_OAEP
from Crypto import Random
from base64 import b64encode
import ast
import math
from decimal import Decimal

import os

from .handler import *

def get_date():
    # return (datetime.utcnow() + timedelta(hours=7)).replace(tzinfo=pytz.timezone('Asia/Jakarta'))
    # return datetime.utcnow() + timedelta(hours=7)
    # return datetime.utcnow()
    return datetime.now(UTC).replace(tzinfo=None)

def map_attr(data, map, nullify=[]):
    j = {}
    for n in map:
        if n in nullify: # jika ingin nullify user.password berarti input nullify user dan user.password
            continue
        s = []
        if '.' in n:
            s = n.split('.')
        if s:
            fmt = 'j{}'
            for d in range(len(s)):
                q = fmt.format(''.join(['''['{}']'''.format(s[x]) for x in range(d+1)]))
                vq = fmt.format(''.join(['''.get('{}')'''.format(s[x]) for x in range(d+1)]))
                if not eval(vq):
                    exec('{}{}'.format(q, ' = {}'))
                if d == len(s)-1:
                    exec('{}{}'.format(q, ' = {}'.format('data.{}'.format(n))))
        else:
            j[n] = eval('data.{}'.format(n))
            if type(j[n]) == InstrumentedList:
                j[n] = eval('[i.to_json() for i in data.{} if i.rowstatus==1] if data.{} else []'.format(n, n))
            elif type(j[n]) == datetime:
                j[n] = eval('(data.{}.isoformat() + ".000Z") if data.{} else None'.format(n, n))
    return j

def set_attr(attr):
    if not attr:
        return None
    a = ascii_lowercase + digits + '.,_'
    if all([n in a for n in set(attr)]):
        return [n.strip() for n in attr.split(',')]
    return None

def get_default_list_param(args):
    page_index = args.get('page_index')
    page_size = args.get('page_size')
    search_by = args.get('search_by') if args.get('search_by') else ''
    keywords = args.get('keywords') if args.get('keywords') else ''
    order_by_col = args.get('order_by_col') if args.get('order_by_col') else ''
    order_by_type = args.get('order_by_type') if args.get('order_by_type') else ''
    filter_by_col = args.get('filter_by_col') if args.get('filter_by_col') else ''
    filter_by_text = args.get('filter_by_text') if args.get('filter_by_text') else ''
    
    try:
        int(page_index)
    except:
        page_index = 1
    
    try:
        int(page_size)
    except:
        page_size = 10
    
    page_index = int(page_index)
    page_size = int(page_size)

    return {
        'page_index': page_index,
        'page_size': page_size,
        'search_by': search_by[:1000],
        'keywords': keywords[:1000],
        'order_by_col': order_by_col[:1000],
        'order_by_type': order_by_type[:1000],
        'filter_by_col': filter_by_col[:1000],
        'filter_by_text': filter_by_text[:1000],
    }


def allowed_image_file(filename):
    return '.' in filename and filename.rsplit('.', 1)[1].lower() in {'jpg', 'jpeg', 'png', 'svg'}


def sanitize_for_jsonb(obj):
    if isinstance(obj, float):
        if math.isnan(obj) or math.isinf(obj):
            return None
        return obj
    
    if isinstance(obj, Decimal):
        f = float(obj)
        if math.isnan(f) or math.isinf(f):
            return None
        return f
    
    if isinstance(obj, dict):
        return {k: sanitize_for_jsonb(v) for k, v in obj.items()}
    
    if isinstance(obj, list):
        return [sanitize_for_jsonb(v) for v in obj]
    
    return obj