# application/apis/geo_apis/features/routes.py
from flask import jsonify, request, make_response, g as g_var
from flask import Response, stream_with_context
from flask_login import current_user
from .. import geo_apis_blueprint
from .... import db
from ....models.geos_models.models import Polygons, DataAnalyzer
from ....models.user_models.models import SessionsAuth, UserSessions

from datetime import datetime, timedelta
from flask_cors import cross_origin
from urllib.parse import urlencode
from werkzeug.utils import secure_filename

import os
import gc
import uuid
import json
import threading

from shapely.geometry import Polygon

from ....utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ....utils.common import app_exception_handler, success_handler
from ....utils.geos import GeoUtils
# off: v2 current-condition/benefit, replaced by site-characterisation and the upcoming benefit
# from ....utils.geos.current_condition import get_eligible_intervention, get_current_condition, set_intervention
# from ....utils.geos.current_condition import process_input_data_analyzer_result
# from ....utils.geos.benefit import run_benefit
from ....utils.geos.v3.common import prepare_aoi_from_session, to_jsonable
from ....utils.geos.v3.benefit.run_benefit import stream_benefit
from ....utils.geos.v3.benefit.run_benefit import validate as benefit_validate
from ....utils.geos.v3.pathway.run_pathway import run_pathway
from ....utils.geos.v3.run_analysis import COMPONENTS as ANALYSIS_COMPONENTS
from ....utils.geos.v3.run_analysis import stream_analysis
from ....utils.geos.v3.site_characterisation.run_site_characterisation import (
    COMPONENTS,
    stream_site_characterisation,
)
from ....utils.geos.v3.threat.run_threat import SECTIONS, stream_threat
from .persist import ANALYSIS_COLUMNS, SITECHAR_COLUMNS, persist_ndjson, save_v3_sections

from ..utils import GeoLogic


# off: v2 benefit, until the new benefit logic lands.
# legacy: /nbsapi/feature/benefits [POST]
# @geo_apis_blueprint.route('/feature/benefits', methods=['GET'])
# @cross_origin()
# def geo_feature_benefits():
#     g_var.__api_name__ = 'geo_feature_benefits'
#
#     try:
#         data = request.args
#
#         session_id = data.get('session_id')
#
#         known_polygons = Polygons.query.filter_by(session_id=session_id).first()
#
#         if not known_polygons:
#             raise AppMessageException('fail, session id Not found')
#
#         known_polygons.assert_area_size()
#
#         set_intervention(session_id)
#
#         known_map_explorer = MapExplorer.query.filter_by(session_id=session_id).first()
#         project_duration = known_map_explorer.project_duration
#         estimated_unplanned_deforestation = known_map_explorer.estimated_unplanned_deforestation
#         rest_target = known_map_explorer.rest_target
#
#         geom = db.session.query(db.func.ST_AsGeoJSON(known_polygons.geom)).first()
#
#         new_geometry = json.loads(geom[0])
#         geometry_type = new_geometry['type']
#         geometry_coordinate = new_geometry['coordinates']
#
#         if geometry_type.lower() == "polygon":
#             new_geometry = {
#                 "type": geometry_type,
#                 "coordinates": geometry_coordinate
#             }
#
#         new_ses_data = {
#             "type": "Feature",
#             "session_id": session_id,
#             "geometry": new_geometry,
#             "properties": {
#                 "project_duration": project_duration,
#                 "unavoided_def_rate": int(estimated_unplanned_deforestation),
#                 "rest_target": rest_target
#             }
#         }
#
#         # resp_from_geo_service = requests.post(current_app.config.get("BENEFIT_API_URL"), json=new_ses_data, timeout=300)
#         # results = resp_from_geo_service.json()
#
#         results = run_benefit(new_ses_data)
#
#         process_input_data_analyzer_result(session_id=session_id, section="benefit", data=results)
#
#         return make_response(jsonify(success_handler(results)), 200)
#     except AppMessageException as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
#     except Exception as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# off: v2 current-condition, replaced by /feature/site-characterisation.
# legacy: /nbsapi/feature/current-condition-new [POST]
# @geo_apis_blueprint.route('/feature/current-condition', methods=['GET'])
# @cross_origin()
# def geo_feature_current_condition():
#     g_var.__api_name__ = 'geo_feature_current_condition'
#
#     g_var.__log_it__ = False
#     g_var.__session_id__ = None
#     g_var.__description_data__ = {}
#     try:
#         g_var.__request_data__ = request.args.to_dict()
#     except:
#         pass
#
#     try:
#         data = request.args
#
#         session_id = data.get('session_id')
#         section_type = data.get('section_type')
#
#         g_var.__session_id__ = session_id
#
#         current_condition_data = get_current_condition(session_id, section_type)
#
#         return make_response(jsonify(success_handler({ 'result': current_condition_data })), 200)
#     except AppMessageException as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
#     except Exception as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# v3: F02-P2 site characterisation. ONE route for all four modules -- General, Nature, Climate and
# People -- as a single NDJSON stream: the plan, then one line per component, then done. The
# per-module runners still exist for running a module on its own, but nothing here calls them.
#
# The AOI is built BEFORE the response opens, not inside the generator. Once the first line is
# written the status is already 200, so anything that can legitimately fail the request -- unknown
# session, oversized polygon, an unrecognised `process` name -- has to fail while a 400 is still
# possible. An exception raised inside the generator can only truncate the stream.
#
# RETRY IS THIS SAME ROUTE, not a second one. `?process=` names the components to run and may be
# repeated; absent, everything runs. A frontend retry button for a card whose `error_status` said
# `retryable` sends the component's own `process` name back, and "retry all failed" is one request
# with several, running on one pool instead of one request each.
#
# The response shape is identical either way -- `preparation` carrying the plan for THIS run, one
# line per component, `end` -- so nothing on the client needs a second code path, and `w`/`a` give
# the retry its own progress bar. Dependencies of a requested component are re-run but not
# re-emitted; see run_site_characterisation.COMPONENTS.

# At most this many site characterisation runs per process. Each run spawns a thread pool of one
# worker per component and holds several float64 AOI windows at once, and the instances have 4 GB:
# more concurrent runs risks the OOM killer, not slowness. Requests beyond the cap block in
# order inside their stream until a slot frees.
#
# WAS 2, AND 2 WAS A GUESS. Measured 2026-08-10 on AOI1, peak RSS over a 138 MB baseline:
#
#     concurrency      1        4        8
#     added RSS      324 MB   832 MB  1,374 MB
#     per run        324 MB   208 MB    172 MB      <- sublinear, the runs share
#     wall clock      44 s     62 s      92 s       <- I/O bound, degrades gently
#
# So the 4 GB holds far more than two runs, and at 2 the third user waited out a whole run before
# starting. 6 is measured at roughly 1.2 GB, a third of the box.
#
# SIX AND NOT EIGHT, even though gunicorn runs `-w 1 --threads 8` and eight is what the threads
# allow: a cap equal to the thread count is not a cap at all, and it would let site
# characterisation take every thread on the instance, leaving nothing for health checks or any
# other endpoint. Two threads are kept back on purpose.
#
# The database is sized to match rather than left to SQLAlchemy's defaults -- see `_POOL_SIZE` in
# utils/geos/v3/db.py, which is what actually bounds the load six concurrent runs put on Postgres.
#
# Retries take a slot too, which is over-conservative -- a one-component retry sizes its pool to
# one worker and costs a fraction of the memory. It is deliberate: the cap exists because of the
# 4 GB, and letting retries past it means a user clicking three buttons can OOM the box out from
# under somebody else's run. Weight the count if the queueing ever bites.
_SITECHAR_SLOTS = threading.BoundedSemaphore(6)


def _limit_slots(gen):
    """Run `gen` while holding a slot. Acquired lazily at the first NDJSON line, released on
    exhaustion and on close(), which stream_with_context calls when the client disconnects."""
    with _SITECHAR_SLOTS:
        yield from gen


# FRONTEND ERROR-PATH REHEARSAL. `?error_test=` makes this endpoint fail ON PURPOSE, so the three
# things a client has to handle can be built, reviewed and demoed without waiting for a real
# outage. They are genuinely hard to see otherwise: on a healthy deployment nothing fails, and the
# most important of them -- a stream that dies mid-flight -- cannot be provoked at all from
# outside.
#
#   api     the request fails BEFORE the stream opens: 500, normal JSON error body, no NDJSON.
#           This is the only failure that can still use an HTTP status.
#   stream  the stream opens, sends a few lines, then STOPS WITHOUT `end`. A dropped connection.
#           The status went 200 with the first line, so `end` is the only way to tell a finished
#           run from a truncated one, and this is how a client proves it does.
#   data    a real, complete run whose lines are given synthetic `error_status` values, cycling
#           partial-with-no-data -> partial-with-data -> failed -> clean, so every card state a
#           client has to draw appears within one response.
#
# EVERY SYNTHETIC MESSAGE SAYS "SIMULATED" and names the mode, so one can never be mistaken for a
# real failure in a screenshot, a bug report or a log. The first of the four also gets its `data`
# emptied, because that is what a crashed component really carries and a client should not be built
# against a combination that cannot occur.
#
# Not gated to non-production: the frontend has to rehearse against whatever environment it is
# pointed at, and nothing here mutates state, reveals anything or costs more than an ordinary
# request -- the worst a caller can do is break their own response. Add a `current_app.debug` guard
# if that ever stops being true.
ERROR_TEST_MODES = ('api', 'stream', 'data')

_ERROR_TEST_KEEP = 4          # `stream` mode: preparation plus three components, then silence


def _error_test_truncate(gen):
    """`error_test=stream`: stop mid-response without `end`, as a dropped connection would."""
    for index, line in enumerate(gen):
        if index >= _ERROR_TEST_KEEP:
            return
        yield line


def _error_test_states(retry_url):
    """`error_test=data`: every card state a client has to draw, in rotation.

    Four, not two, because `partial` reaches a client in two shapes that look nothing alike -- a
    crash carries NO data and a degraded component carries all of it -- and a card has to handle
    both. The `data` emptying for the crash case is done by the caller.
    """
    return (
        lambda name: {'state': 'partial', 'retryable': True, 'retry_url': retry_url(name),
                      'messages': [f"SIMULATED: {name} was made to crash by error_test=data, so "
                                   "it carries no data. Nothing is actually wrong."]},
        lambda name: {'state': 'partial', 'retryable': True, 'retry_url': retry_url(name),
                      'messages': [f"SIMULATED: {name} was made to report an incomplete result by "
                                   "error_test=data. Nothing is actually wrong."]},
        lambda name: {'state': 'failed', 'retryable': False, 'retry_url': None,
                      'messages': [f"SIMULATED: {name} was made to report that this is the final "
                                   "answer by error_test=data. Nothing is actually wrong."]},
        lambda name: None,
    )


def _error_test_overlay(gen, retry_url):
    """`error_test=data`: rewrite each component line's `error_status`, leaving the payload real."""
    states = _error_test_states(retry_url)
    for index, line in enumerate(gen):
        payload = json.loads(line)
        if payload['process'] not in ('preparation', 'end'):
            slot = (index - 1) % len(states)
            payload['error_status'] = states[slot](payload['process'])
            if slot == 0:
                payload['data'] = {}      # the crash case: a crashed component carries nothing
            line = json.dumps(payload) + "\n"
        yield line


@geo_apis_blueprint.route('/feature/site-characterisation', methods=['GET'])
@cross_origin()
def geo_feature_site_characterisation():
    g_var.__api_name__ = 'geo_feature_site_characterisation'

    try:
        session_id = request.args.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        # None means the whole run. A typo has to 400 here rather than silently emitting a shorter
        # plan: once the stream opens the status is 200 and a client would read the missing card as
        # a component that never arrived.
        wanted = request.args.getlist('process') or None
        if wanted:
            unknown = [name for name in wanted if name not in COMPONENTS]
            if unknown:
                raise AppMessageException(
                    f"fail, unknown process: {', '.join(unknown)}")

        # See ERROR_TEST_MODES. An unknown value is a 400 rather than being ignored: a typo that
        # silently returned a healthy stream would look like the error path was broken.
        error_test = request.args.get('error_test')
        if error_test and error_test not in ERROR_TEST_MODES:
            raise AppMessageException(
                f"fail, unknown error_test: {error_test}. "
                f"Expected one of: {', '.join(ERROR_TEST_MODES)}")

        if error_test == 'api':
            # Deliberately not an AppMessageException: this rehearses the 500 path, the one where
            # the client gets no stream at all and has only the status to go on.
            raise RuntimeError("SIMULATED: this request was made to fail by error_test=api. "
                               "Nothing is actually wrong.")

        aoi = prepare_aoi_from_session(session_id)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error

    # The retry URL is a PATH, not an absolute URL: `request.url_root` behind a proxy is the
    # internal host unless every X-Forwarded-* header and ProxyFix line is right, and none of that
    # is configured here. `script_root` is the WSGI SCRIPT_NAME, empty today and the mount prefix
    # if this is ever served under one, so the path follows the deployment on its own.
    #
    # A CLIENT SHOULD STILL RESOLVE IT AGAINST THE URL IT CALLED (`new URL(retry_url, streamUrl)`)
    # rather than against the site root. If a reverse proxy rewrites a prefix away without setting
    # SCRIPT_NAME -- the legacy `/nbsapi` routes suggest one might -- nothing on this side can see
    # that prefix, and resolving relatively is what makes it not matter.
    path = request.script_root + request.path

    def retry_url(process):
        return f"{path}?{urlencode({'session_id': session_id, 'process': process})}"

    # Persist wraps the RAW stream, before the error_test overlays: what lands in DataAnalyzer is
    # what the analysis produced, not a simulated failure.
    lines = _limit_slots(persist_ndjson(
        stream_site_characterisation(aoi, wanted, retry_url), session_id, SITECHAR_COLUMNS.get))
    if error_test == 'stream':
        lines = _error_test_truncate(lines)
    elif error_test == 'data':
        lines = _error_test_overlay(lines, retry_url)

    return Response(
        stream_with_context(lines),
        mimetype='application/x-ndjson',
    )


# v3: THE UNION STREAM. Site characterisation + threat + pathway as one NDJSON response -- the
# only analysis call the frontend makes after the polygon. Same envelope, same retry contract;
# component names are unique across the three stages, so `?process=` addresses any card. The
# individual endpoints below remain for tooling and single-stage runs.
#
# Takes a _SITECHAR_SLOTS slot: this run holds site characterisation's ~25 raster windows PLUS
# threat's twelve rasters and pathway's band reads.
@geo_apis_blueprint.route('/feature/analysis', methods=['GET'])
@cross_origin()
def geo_feature_analysis():
    g_var.__api_name__ = 'geo_feature_analysis'

    try:
        session_id = request.args.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        # None means the whole union. A typo has to 400 before the stream opens.
        wanted = request.args.getlist('process') or None
        if wanted:
            unknown = [name for name in wanted if name not in ANALYSIS_COMPONENTS]
            if unknown:
                raise AppMessageException(
                    f"fail, unknown process: {', '.join(unknown)}")

        aoi = prepare_aoi_from_session(session_id)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error

    path = request.script_root + request.path

    def retry_url(process):
        return f"{path}?{urlencode({'session_id': session_id, 'process': process})}"

    lines = _limit_slots(persist_ndjson(
        stream_analysis(aoi, wanted, retry_url), session_id, ANALYSIS_COLUMNS.get))

    return Response(
        stream_with_context(lines),
        mimetype='application/x-ndjson',
    )


# v3: F02-P3 Threat. The Threat Profile screen: four tabs, one NDJSON line each.
#
# STREAMED, and with the SAME ENVELOPE as site-characterisation rather than pathway's single JSON
# document. The four sections are the four tabs, the user lands on Overview, and Overview is the
# cheapest of them (two rasters against twelve for the whole profile) -- so it can be drawn while
# the rest are still reading. It also inherits `error_status` and `retry_url` per tab, which a
# single document has nowhere to put.
#
# IT DOES TAKE A SLOT FROM _SITECHAR_SLOTS, unlike pathway. Twelve rasters and a vector distance
# over a 1.28 GB canal layer is the same order of memory as a characterisation run, not a fraction
# of it, so it queues with them rather than past them.
@geo_apis_blueprint.route('/feature/threat', methods=['GET'])
@cross_origin()
def geo_feature_threat():
    g_var.__api_name__ = 'geo_feature_threat'

    try:
        session_id = request.args.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        # None means every tab. A typo has to 400 here rather than silently emitting a shorter
        # plan, for the same reason as site-characterisation: once the stream opens the status is
        # 200 and a client would read the missing line as a tab that never arrived.
        wanted = request.args.getlist('process') or None
        if wanted:
            unknown = [name for name in wanted if name not in SECTIONS]
            if unknown:
                raise AppMessageException(
                    f"fail, unknown process: {', '.join(unknown)}")

        aoi = prepare_aoi_from_session(session_id)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error

    # Relative for the same reason as site-characterisation: `request.url_root` behind a proxy is
    # the internal host, and `script_root` follows a mount prefix on its own. Resolve it client
    # side against the URL you called.
    path = request.script_root + request.path

    def retry_url(process):
        return f"{path}?{urlencode({'session_id': session_id, 'process': process})}"

    # Nested by section name: the four tabs reuse field names (`total_area_ha` and friends), so
    # they cannot share one flat namespace in threat_json.
    lines = _limit_slots(persist_ndjson(
        stream_threat(aoi, wanted, retry_url), session_id,
        lambda name: ('threat_json', True)))

    return Response(
        stream_with_context(lines),
        mimetype='application/x-ndjson',
    )


# v3: F02-P4 Pathway. The Pathway Selection screen: one card per ecosystem carrying its area, how
# much of it is disturbed, and which of Protect / Manage / Restore can be selected on it with the
# activities under each.
#
# A PLAIN JSON RESPONSE, not the NDJSON of site-characterisation, and deliberately so. The screen
# cannot render anything until every ecosystem card exists -- a user choosing interventions cannot
# act on half an answer the way they can read a half-filled report -- and the whole run is ~3.5 s
# of one raster, so streaming would add a client code path and buy nothing. A failure is therefore
# an ordinary 500 rather than a degraded card, which is why there is no `error_status` here.
#
# NO SLOT FROM _SITECHAR_SLOTS. That cap exists because a characterisation run holds ~25 float64
# AOI windows at once on a 4 GB box; this reads four bands of one raster in sequence and costs a
# fraction of it. Guarding it would make a 3 s request queue behind 40 s ones for no memory reason.
@geo_apis_blueprint.route('/feature/pathway', methods=['GET'])
@cross_origin()
def geo_feature_pathway():
    g_var.__api_name__ = 'geo_feature_pathway'

    try:
        session_id = request.args.get('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        aoi = prepare_aoi_from_session(session_id)
        # `to_jsonable` because the components return dataclasses and numpy scalars, exactly as
        # they do in the notebook, and jsonify takes neither.
        result = to_jsonable(run_pathway(aoi))

        save_v3_sections(session_id, {'intervention_eligibility_json': result})
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error

    return make_response(jsonify(success_handler({'result': result})), 200)


# v3: F02-P5 Benefit, carbon components 5.2 / 5.3 / 5.4 / 5.5. A PLAIN JSON RESPONSE like
# STREAMED like the analysis endpoints: the carbon components land in the first ~6 s while the
# two habitat components (~25 s each) are still reading, so the cards render progressively
# instead of waiting ~30 s for one document.
#
# 5.2 needs the deforestation rate from a completed site-characterisation run, read from the
# persisted DataAnalyzer row -- absent, 5.2 reports not-applicable rather than failing, exactly
# as the notebook does with a missing stage file. The carbon-risk deductions default to
# CARBON_RISK_DEFAULTS and may be overridden per request; their sum over 100% is a 400 (the
# notebook's own 4.4 validation).
# The "Calculate potential benefit" button on the Pathway screen POSTs the screen's state: the
# per-ecosystem pathway toggles and chosen activities (`selections`), the duration slider, the
# carbon-project toggle and its three deduction inputs. `selections` is echoed into the
# persisted result -- it does not gate the quantification (5.2/5.3 follow the notebook and
# quantify ELIGIBILITY; whether a de-toggled pathway should drop out of the numbers is an open
# product decision).
@geo_apis_blueprint.route('/feature/benefit', methods=['POST'])
@cross_origin()
def geo_feature_benefit_v3():
    g_var.__api_name__ = 'geo_feature_benefit_v3'

    try:
        if not request.is_json:
            raise AppMessageException('please provide json data')
        payload = request.get_json()

        def param(name, default=None):
            return payload.get(name, default)

        session_id = param('session_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()

        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        try:
            duration_years = int(param('duration_years', 30))
            ecosystem_class = int(param('ecosystem_class', 1))
            leakage = param('leakage')
            uncertainty = param('uncertainty')
            buffer = param('buffer')
            leakage = float(leakage) if leakage is not None else None
            uncertainty = float(uncertainty) if uncertainty is not None else None
            buffer = float(buffer) if buffer is not None else None
        except (TypeError, ValueError):
            raise AppMessageException('fail, duration_years/ecosystem_class/leakage/uncertainty/buffer must be numbers')
        if ecosystem_class not in (1, 2, 3):
            raise AppMessageException('fail, ecosystem_class must be 1 (forest), 2 (mangrove) or 3 (peatland)')
        carbon_project = str(param('carbon_project', 'yes')).lower() in ('yes', 'y', 'true', '1')
        selections = payload.get('selections')
        if selections is not None and not isinstance(selections, dict):
            raise AppMessageException('fail, selections must be an object keyed by ecosystem')

        # Partial re-runs: `process` in the body names the components to re-emit, mirroring the
        # `?process=` contract of the GET streams. The endpoint is POST, so `retry_url` on the
        # lines stays null; a client re-POSTs the same body with this list instead.
        wanted = payload.get('process') or None
        if wanted:
            from ....utils.geos.v3.benefit.run_benefit import _W as _BENEFIT_NAMES
            unknown = [n for n in wanted
                       if n not in _BENEFIT_NAMES or n == 'activity_stage']
            if unknown:
                raise AppMessageException(f"fail, unknown process: {', '.join(unknown)}")

        # Validation fires BEFORE the stream opens, so bad inputs are still an honest 400.
        try:
            benefit_validate(duration_years,
                             leakage if leakage is not None else 15.0,
                             uncertainty if uncertainty is not None else 10.0,
                             buffer if buffer is not None else 12.0)
        except ValueError as e:
            raise AppMessageException(f'fail, {e}')

        analyzer = DataAnalyzer.find_by_session_id(session_id)
        site = (analyzer.site_information_json if analyzer else None) or {}
        rate_pct = site.get('historical_deforestation_percentage')

        aoi = prepare_aoi_from_session(session_id)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error

    # Every line persists into benefit_json under its process name -- the same underscore keys
    # the old single-document response used, so the stored shape and its readers are unchanged.
    lines = _limit_slots(persist_ndjson(
        stream_benefit(aoi, duration_years, rate_pct, carbon_project,
                       leakage, uncertainty, buffer, ecosystem_class,
                       selections, wanted),
        session_id, lambda name: ('benefit_json', True)))

    return Response(
        stream_with_context(lines),
        mimetype='application/x-ndjson',
    )


# off: v2 intervention eligibility, read the v2 current-condition data.
# legacy: /nbsapi/feature/intervention-eligibility [POST]
# @geo_apis_blueprint.route('/feature/intervention-eligibility', methods=['GET'])
# @cross_origin()
# def geo_feature_intervention_eligibility():
#     g_var.__api_name__ = 'geo_feature_intervention_eligibility'
#
#     try:
#         data = request.args
#
#         session_id = data.get('session_id')
#
#         interventions = get_eligible_intervention(session_id)
#
#         return make_response(jsonify(success_handler({ 'result': interventions })), 200)
#     except AppMessageException as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
#     except Exception as e:
#         return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/feature/data-analyzer-result [POST]
@geo_apis_blueprint.route('/feature/data-analyzer', methods=['GET'])
@cross_origin()
def geo_feature_data_analyzer_result():
    g_var.__api_name__ = 'geo_feature_data_analyzer_result'

    try:
        data = request.args

        session_id = data.get('session_id')

        known_data_analyzer = DataAnalyzer.query.filter_by(session_id=session_id).first()
        if not known_data_analyzer:
            raise AppMessageException('fail, session id Not found')
        
        geom = Polygons.get_geometry(session_id).first()
        geom = json.loads(geom[0])['coordinates'][0]

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if not known_polygons:
            raise AppMessageException('fail, session id Not found')

        known_polygons.assert_area_size()

        results = {
            'session_id': session_id,
            'polygon': geom,
            'site_information': known_data_analyzer.site_information,
            'nature': known_data_analyzer.nature,
            'climate': known_data_analyzer.climate,
            'people': known_data_analyzer.people,
            'benefit': known_data_analyzer.benefit,
            'eligibility': known_data_analyzer.intervention_eligibility,
        }

        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error



