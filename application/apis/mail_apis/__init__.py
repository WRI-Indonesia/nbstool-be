# application/apis/mail_apis/__init__.py
from flask import Blueprint, Response, request

mail_apis_blueprint = Blueprint('mail_apis', __name__, template_folder='templates')

from . import routes

