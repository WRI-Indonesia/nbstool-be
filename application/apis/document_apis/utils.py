# application/apis/document_apis/utils.py
#
# The shared save/load flow behind the v3 document endpoints (feasibility, monitoring).
#
# One DocumentData row per (session, certification_type) holds that template's `form` and
# `user_input` draft. Monitoring layers ON TOP of feasibility: the socio-economic answers are
# collected once in the feasibility form, so monitoring's draft view and render read
# prefill < FeasibilityV3 < MonitoringV3, later sources winning key by key.

from __future__ import annotations

import json

from sqlalchemy.dialects.postgresql import JSONB

from ... import db
from ...models.geos_models.models import DataAnalyzer
from ...models.master_models.models import DocumentData
from ...utils.common import AppMessageException
from ...utils.document_generator.v3.prefill import feasibility_prefill, merge_form


def _stored_dict(value):
    """A stored jsonb value, as the dict it is supposed to be. A frontend that once POSTed a
    STRINGIFIED form got it jsonb-`||`-wrapped into an array (object || string concatenates);
    such a row must read as empty rather than crash every draft reader."""
    return value if isinstance(value, dict) else {}


def jsonb_merge(column, patch: dict):
    """`coalesce(column, '{}') || patch` -- an atomic per-key merge evaluated by Postgres, so
    two concurrent saves cannot lose each other's fields the way a read-modify-write of the
    whole dict would."""
    return db.func.coalesce(column, db.cast('{}', JSONB)).op('||')(
        db.cast(json.dumps(patch), JSONB))


def load_draft(session_id: str, cert_type: str, base_types: tuple = ()):
    """`(analyzer, form, user_input)` for one template: stored drafts merged over the analyser
    prefill, `base_types` first so `cert_type`'s own answers win."""
    analyzer = DataAnalyzer.find_by_session_id(session_id)
    form = feasibility_prefill(analyzer)
    user_input: dict = {}
    for template_type in (*base_types, cert_type):
        row = DocumentData.find_by_session_id_and_type(session_id, template_type)
        if row:
            form = merge_form(_stored_dict(row.form), form)
            user_input = {**user_input, **_stored_dict(row.user_input)}
    return analyzer, form, user_input


def save_draft(session_id: str, cert_type: str, patch_form: dict, patch_user_input: dict):
    """Upsert one template's draft row, merging the patch key-by-key in the database. Flushes
    and re-reads the row so callers see the actual merged values, not SQL expressions."""
    # A non-dict patch (a STRINGIFIED form, a bare list) would not merge -- jsonb `||` wraps
    # object || string into an ARRAY, silently corrupting the row for every later reader.
    if not isinstance(patch_form, dict) or not isinstance(patch_user_input, dict):
        raise AppMessageException('fail, form and user_input must be json objects')
    stored = DocumentData.find_by_session_id_and_type(session_id, cert_type)
    if not stored:
        stored = DocumentData(session_id=session_id, certification_type=cert_type,
                              form=patch_form, user_input=patch_user_input)
        db.session.add(stored)
    else:
        # An already-corrupted (non-dict) column self-heals: replace it, `||` on an array
        # would only append to the corruption.
        if patch_form:
            stored.form = (jsonb_merge(DocumentData.form, patch_form)
                           if isinstance(stored.form, dict) else patch_form)
        if patch_user_input:
            stored.user_input = (jsonb_merge(DocumentData.user_input, patch_user_input)
                                 if isinstance(stored.user_input, dict) else patch_user_input)
    db.session.flush()
    db.session.refresh(stored)
    return stored
