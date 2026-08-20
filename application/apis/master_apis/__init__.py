# application/apis/master_apis/__init__.py
from flask import Blueprint, Response, request

master_apis_blueprint = Blueprint('master_apis', __name__)

from .organization import routes
from .faq import routes
from .dictionary import routes