# application/apis/geo_apis/__init__.py
from flask import Blueprint, Response, request

geo_apis_blueprint = Blueprint('geo_apis', __name__)


@geo_apis_blueprint.before_request
def _refresh_v3_settings():
    """Make the v3 settings cache request scoped: one database read per setting per request.

    tbl_master_settings is otherwise read once per process and kept forever, so repointing
    V3_BUCKET needed a pm2 reload -- and a reload under load cuts in-flight streams short at
    kill_timeout. Dropping the cache here costs one query per request and lets a change take
    effect on the very next one.

    It is not removed outright because `layer_path` resolves the root for every layer, and
    habitat area calls it once per species: roughly 473 reads in one analysis run, against a
    settings engine sized `pool_size=1`.
    """
    from ...utils.geos.v3 import settings
    settings.refresh()

from .polygon import routes
from .layers import routes
from .map import routes
from .feature import routes