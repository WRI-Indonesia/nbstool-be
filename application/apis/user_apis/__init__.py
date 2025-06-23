# application/apis/user_apis/__init__.py
from flask import Blueprint, Response, request

user_apis_blueprint = Blueprint('user_apis', __name__)

from . import routes

