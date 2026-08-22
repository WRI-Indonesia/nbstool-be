# application/apis/geo_apis/feature/persist.py
#
# Persist v3 analysis payloads into DataAnalyzer's JSONB columns, so modules that read stored
# results (project, document, future benefit) can use them without re-running the analysis.
#
# v3 sections map onto the existing v2 columns; only threat needed a new one:
#
#     site-characterisation  General -> site_information_json   Nature -> nature_json
#                            Climate -> climate_json            People -> people_json
#     threat                 threat_json, nested {section name: data}
#     pathway                intervention_eligibility_json
#
# JSONB columns only -- the PickleType columns stay legacy-v2. Every save also stamps
# UserSessions.analyzer_version = 'v3', so readers know which payload shape the row carries.
# Writes MERGE into the stored value rather than replace it: a retry run re-emits only the
# requested components, and replacing the column would wipe the siblings a client already holds.
import json
import logging

from .... import db
from ....models.geos_models.models import DataAnalyzer
from ....models.user_models.models import UserSessions
from ....utils.common import sanitize_for_jsonb
from ....utils.geos.v3.site_characterisation.climate.run_climate import processes as climate_processes
from ....utils.geos.v3.site_characterisation.general.run_general import processes as general_processes
from ....utils.geos.v3.site_characterisation.nature.run_nature import processes as nature_processes
from ....utils.geos.v3.site_characterisation.people.run_people import processes as people_processes

logger = logging.getLogger(__name__)

# component name -> DataAnalyzer column, derived from the module process lists so a component
# added to a module lands in the right column without a change here
SITECHAR_COLUMNS = {
    p['name']: column
    for module_processes, column in (
        (general_processes, 'site_information_json'),
        (nature_processes, 'nature_json'),
        (climate_processes, 'climate_json'),
        (people_processes, 'people_json'),
    )
    for p in module_processes[1:-1]
}


def _merge(base, patch):
    """One level of dict merge: People's three components all write into `social_demography`
    with disjoint inner fields, so a dict value merges key-by-key; anything else replaces."""
    merged = dict(base or {})
    for key, value in patch.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = {**merged[key], **value}
        else:
            merged[key] = value
    return merged


def save_v3_sections(session_id: str, updates: dict):
    """Merge `{column name: patch dict}` into the session's DataAnalyzer row and stamp the
    session as v3. Creates the row if the polygon-upload pre-create is missing."""
    if not updates:
        return

    analyzer = DataAnalyzer.find_by_session_id(session_id)
    if not analyzer:
        analyzer = DataAnalyzer(session_id=session_id)
        db.session.add(analyzer)

    for column, patch in updates.items():
        setattr(analyzer, column, _merge(getattr(analyzer, column), sanitize_for_jsonb(patch)))

    user_session = UserSessions.query.filter_by(session_id=session_id).first()
    if user_session:
        user_session.analyzer_version = 'v3'

    db.session.commit()


def persist_ndjson(lines, session_id: str, column_for, nest_by_process: bool = False):
    """Tee an NDJSON stream: yield every line unchanged, accumulate each component's `data`,
    save once on the `end` line.

    `column_for` maps a process name to a column (None skips the line). `nest_by_process` stores
    `{process: data}` instead of merging `data` flat -- threat's four sections reuse field names
    like `total_area_ha`, so they must not share a namespace.

    A crashed component (`data: {}`) is skipped, so it never wipes a stored value. Saving only at
    `end` means a dropped connection persists nothing, which is what its retry expects. A save
    failure is logged and swallowed: the stream is already 200 and the analysis itself succeeded.
    """
    updates: dict = {}
    for line in lines:
        payload = json.loads(line)
        process, data = payload['process'], payload['data']
        if process == 'end':
            try:
                save_v3_sections(session_id, updates)
            except Exception:
                db.session.rollback()
                logger.exception('failed to persist v3 results for session %s', session_id)
        elif process != 'preparation' and data:
            column = column_for(process)
            if column:
                updates[column] = _merge(updates.get(column), {process: data} if nest_by_process else data)
        yield line
