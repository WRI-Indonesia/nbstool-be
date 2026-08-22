# application/utils/document_generator/v3/__init__.py
#
# Generate the v3 feasibility document: fill the converted jinja template (see convert.py) with
# the session's persisted analysis results plus the F03 form and user-input payloads.
#
# assets/Feasibility Study_Template Draft.docx is the doc team's bracket-tagged AUTHORING file;
# assets/feasibility_v3_template.docx is the machine-generated docxtpl template. When the doc
# team revises the draft, re-run convert.py -- never hand-edit the generated file.

from __future__ import annotations

import os
from datetime import datetime

from docxtpl import DocxTemplate

from .context import build_context

FEASIBILITY_TEMPLATE = "assets/feasibility_v3_template.docx"
OUTPUT_FOLDER = "generated-file/docx-v3/"


def generate_feasibility_v3(session_id: str, analyzer, form: dict | None,
                            user_input: dict | None) -> str:
    """Render the feasibility docx for one session. Returns the saved file path.

    Unfilled tags render as their literal `[bracket]` text on purpose: the team fills those
    manually in the output document.
    """
    context = build_context(analyzer, form, user_input)

    os.makedirs(OUTPUT_FOLDER, exist_ok=True)
    stamp = datetime.now().strftime("%Y%m%d%H%M%S")
    output_path = os.path.join(OUTPUT_FOLDER, f"{session_id}-{stamp}-feasibility.docx")

    doc = DocxTemplate(FEASIBILITY_TEMPLATE)
    doc.render(context)
    doc.save(output_path)
    return output_path
