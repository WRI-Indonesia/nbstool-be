"""
settings.py - runtime settings the v3 modules read from tbl_master_settings.

The raster root used to be a literal in config.py, so moving between the assets-geo bucket and a
local copy of the layers meant editing code. It now comes from the `V3_BUCKET` row of
tbl_master_settings, the same table MAX_DRAW_AREA and FE_URL live in, so each environment points
itself at the right storage.

Connection. Built from DB_SQLALCHEMY_URI directly rather than through Flask-SQLAlchemy, for the
same reason db.py does it for the GIS database: a component has to run as
`python ecosystem_type.py` with no Flask app. Inside the app this is a second small pool
alongside `db.session`, and it reads a table nothing else in this module writes.

Caching. The value is read once per process, and the read is LOCKED so that "once per process"
holds on the first request too, not just afterwards. Editing the row takes effect at the next
restart, which is the same reach the literal had.
"""

from __future__ import annotations

import os
import pathlib
import threading

from sqlalchemy import create_engine, text

try:
    from .config import V3_BUCKET_SETTING
except ImportError:  # imported as a top-level module by a component run as a script
    from config import V3_BUCKET_SETTING

SETTINGS_TABLE = "tbl_master_settings"

_engine = None
_engine_lock = threading.Lock()
_cache: dict[str, str] = {}

# Guards the CACHE MISS, not just the engine. See get_setting: without it the first request on a
# process opens one connection per component instead of one per process.
#
# One lock for every setting name rather than one per name. There is only ever a handful of them,
# they are read once each, and the query is a primary-key lookup -- a second name waiting behind
# the first costs a millisecond on a cold process and nothing afterwards.
_cache_lock = threading.Lock()

# How long a component waits for whoever is already fetching the setting before giving up and
# fetching it itself. Long enough that a healthy database always wins the race -- the query is a
# single indexed row and takes milliseconds -- and short enough that an unreachable one costs one
# wait rather than one timeout per component. See get_setting.
_CACHE_WAIT_S = 10


def _get_engine():
    """One lazily built engine per process, so importing this module never opens a connection.

    Locked because the components run on a thread pool and every one of them resolves a layer
    path, so on a cold process they all arrive here at once. Without it several would build an
    engine and all but one pool would be orphaned with its connections still open.
    """
    global _engine
    if _engine is not None:
        return _engine

    with _engine_lock:
        if _engine is not None:   # built while this thread waited for the lock
            return _engine

        uri = os.environ.get("DB_SQLALCHEMY_URI")
        if not uri:
            # Running outside the app, so the repo .env is the only place the URI can come from.
            from dotenv import load_dotenv

            load_dotenv(pathlib.Path(__file__).resolve().parents[4] / ".env")
            uri = os.environ.get("DB_SQLALCHEMY_URI")

        if not uri:
            raise RuntimeError(
                "No application database URI. Set DB_SQLALCHEMY_URI, or run from a checkout whose "
                ".env carries it."
            )

        # TINY ON PURPOSE. With the cache miss locked this pool serves one query per setting per
        # process -- one connection on the first request, none ever again -- so SQLAlchemy's
        # default 5 + 10 would reserve fifteen slots of the database's connection budget, per
        # instance, to do nothing. `pool_size=1` is the honest size; the overflow is there only so
        # a second setting name added later cannot deadlock behind the first.
        _engine = create_engine(uri, pool_pre_ping=True, pool_size=1, max_overflow=2,
                                connect_args={"connect_timeout": 15})
        return _engine


def get_setting(name: str) -> str:
    """One row of tbl_master_settings by name, cached for the life of the process.

    Raises rather than falling back to a default. A missing row means the environment has not
    been configured, and a silent default would send every raster read somewhere unintended.

    THE MISS IS LOCKED, and that is the whole point of the lock. Every component resolves a layer
    path, and they run together on a thread pool, so on a cold process all of them arrive here
    within a few milliseconds of each other and all of them miss. Unlocked, that is one connection
    PER COMPONENT rather than one per process: measured at 15 of the pool's 15 checkouts on a
    SINGLE run, so two concurrent requests just after a restart could exhaust the pool and fail
    components with OperationalError. Warm, this function opens nothing at all.

    Double-checked: the `in` test is repeated inside the lock, because the thread that held it may
    have filled the cache while this one waited, and re-running the query would defeat the fix.

    THE WAIT IS BOUNDED, and that matters more than it looks. A thread that gives up waiting goes
    on to query anyway, which is the unlocked behaviour -- so the lock can only ever make things
    slower by `_CACHE_WAIT_S`, never block a request outright. Without the timeout an unreachable
    database would be far WORSE than no lock at all: every component would queue and take its own
    turn to time out, twenty five of them in series at `connect_timeout` each, which outlasts
    gunicorn's own timeout and would have the worker killed rather than the components failing.
    That is not hypothetical -- it is what a dropped VPN looks like from here. The winner of the
    lock needs milliseconds on a healthy database, so the timeout costs the happy path nothing.
    """
    if name in _cache:
        return _cache[name]

    leading = _cache_lock.acquire(timeout=_CACHE_WAIT_S)
    try:
        if name in _cache:     # filled while this thread waited for the lock
            return _cache[name]

        with _get_engine().connect() as conn:
            row = conn.execute(
                text(f"select value from {SETTINGS_TABLE} where name = :name"), {"name": name}
            ).first()

        if row is None or not row[0]:
            raise RuntimeError(
                f"{SETTINGS_TABLE} has no usable '{name}' row. Insert it before running the v3 "
                "site characterisation."
            )

        _cache[name] = str(row[0])
        return _cache[name]
    finally:
        if leading:
            _cache_lock.release()


def layer_path(layer: str) -> str:
    """Full path of one v3 layer, from the layer file name that config.py holds.

    In the bucket the objects are plain `<name>_v3.tif`. The local copies of the same files carry
    an extra `v3_` prefix, so a local root gets it added and an https root does not. That is the
    only difference between the two roots, so `V3_BUCKET` can be set to either
    `https://storage.googleapis.com/assets-geo/v3` or a directory of downloaded layers.

    A layer given as an ALREADY COMPLETE url is returned unchanged. Not every layer the tool reads
    lives under the v3 root: the burned-area history reads v2's own objects under
    `assets-geo/baseline/`, which V3_BUCKET must not move. Config holds those as full urls, and
    this is what lets a component call `load_raster_clipped` on them like any other layer.
    """
    if layer.startswith("http") or layer.startswith("/vsi"):
        return layer

    root = get_setting(V3_BUCKET_SETTING).rstrip("/")
    return f"{root}/{layer}" if root.startswith("http") else f"{root}/v3_{layer}"
