# application/apis/project_apis/__init__.py
from flask import Blueprint, Response, request

project_apis_blueprint = Blueprint('project_apis', __name__)

from . import routes

