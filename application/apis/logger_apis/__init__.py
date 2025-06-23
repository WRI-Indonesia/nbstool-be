# application/apis/logger_apis/__init__.py
from flask import Blueprint, Response, request

logger_apis_blueprint = Blueprint('logger_apis', __name__)

from . import routes

