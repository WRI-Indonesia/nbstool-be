# application/apis/document_apis/__init__.py
from flask import Blueprint, Response, request
from ...utils.cloud_storage import CloudStorage

document_apis_blueprint = Blueprint('document_apis', __name__)
gcs = CloudStorage()

from . import routes

from .feasibility import routes
from .ccb import routes
from .download import routes
from .cleanup import routes