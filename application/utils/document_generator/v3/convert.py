# application/utils/document_generator/v3/convert.py
#
# Turn the doc team's bracket-tagged draft into a docxtpl (jinja) template.
#
# The doc team authors and iterates the template in Word using human-readable placeholders:
#     [Site Characterisation: elevation class]   [User input: Project Proponent]
# Jinja tags do not survive Word editing (run splitting, smart quotes), so the bracket file stays
# the AUTHORING format and this converter is re-run per template revision:
#
#     python -m application.utils.document_generator.v3.convert \
#         "assets/Feasibility Study_Template Draft.docx" assets/feasibility_v3_template.docx
#
# What it does:
#   1. Every `[...]` placeholder becomes `{{ t("...") }}` -- one lookup function, no per-tag
#      variable names. The context's `t()` returns the mapped value or the literal bracket text,
#      so an unmapped or unfilled tag stays visible in the output for manual fill.
#   2. The land-cover "rows repeat for each remaining class" row and the species annex row become
#      `{%tr for %}` loops (see LOOP_ROWS).
#   3. `[only if X] ... [/only if X]` spans become `{% if %} ... {% endif %}`.
#
# Paragraphs containing a bracket are rewritten into their first run, so Word's run fragmentation
# cannot split a tag; the paragraph keeps the first run's formatting. Placeholder highlighting is
# lost by design -- it marks what this pipeline replaces.

from __future__ import annotations

import re
import sys
from copy import deepcopy

from docx import Document

BRACKET = re.compile(r"\[([^\[\]]+)\]")
ONLY_IF = re.compile(r"\[only if ([^\[\]]+?)\]")
END_ONLY_IF = re.compile(r"\[/only if [^\[\]]*\]")

# Conditions used by [only if ...] spans -> context flag names.
CONDITIONS = {
    "keystone species present": "keystone_present",
}

# Repeating table rows. A row whose text contains `marker` is turned into a loop over `var`:
# a `{%tr for %}` row above, `{%tr endfor %}` below, and each `[tag]` cell replaced by the
# matching jinja expression. Sibling paragraphs saying "(rows repeat...)" are dropped.
LOOP_ROWS = [
    {
        "marker": "n land cover class",
        "var": "land_cover_rest",
        "cells": {
            "n land cover class": "{{ r.name }}",
            "n land cover class area (ha)": "{{ r.area }}",
            "n land cover class area (% of AOI)": "{{ r.pct }}",
        },
        # The "No" column of the repeated row counts on from the three fixed rows.
        "literal": {"..": "{{ loop.index + 3 }}"},
    },
    {
        "marker": "Species taxonomic class",
        "var": "species_rows",
        "cells": {
            "Species taxonomic class": "{{ r.taxon_class }}",
            "Scientific name": "{{ r.scientific_name }}",
            "Species IUCN Red List Category": "{{ r.redlist_category }}",
        },
        "literal": {"[n]": "{{ loop.index }}"},
    },
]

REPEAT_NOTE = "(rows repeat"


def _to_jinja(text: str) -> str:
    """Bracket placeholders in one text blob -> jinja, conditionals first."""

    def _cond(match):
        flag = CONDITIONS.get(match.group(1).strip())
        # An unknown condition stays literal, visible in the output rather than silently eaten.
        return "{%% if %s %%}" % flag if flag else match.group(0)

    text = ONLY_IF.sub(_cond, text)
    text = END_ONLY_IF.sub("{% endif %}", text)
    # Escape quotes inside the tag text so the jinja string literal survives.
    return BRACKET.sub(lambda m: '{{ t("%s") }}' % m.group(1).replace('"', '\\"'), text)


def _rewrite_paragraph(paragraph, new_text: str) -> None:
    """Put `new_text` into the paragraph's first run and drop the rest.

    Word splits a placeholder across runs at will; collapsing to one run is what guarantees a
    jinja tag is never split. The paragraph keeps the first run's formatting.
    """
    if not paragraph.runs:
        if new_text:
            paragraph.add_run(new_text)
        return
    paragraph.runs[0].text = new_text
    for run in paragraph.runs[1:]:
        run._element.getparent().remove(run._element)


def _convert_paragraphs(paragraphs) -> None:
    for paragraph in paragraphs:
        text = paragraph.text
        if "[" in text or "]" in text:
            _rewrite_paragraph(paragraph, _to_jinja(text))


def _iter_cell_paragraphs(table):
    for row in table.rows:
        for cell in row.cells:
            yield from cell.paragraphs
            for nested in cell.tables:
                yield from _iter_cell_paragraphs(nested)


_NS = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _control_row(template_row):
    """A copy of `template_row` with every run removed, ready to hold one control tag."""
    tr = deepcopy(template_row._tr)
    for p in tr.findall(f".//{_NS}p"):
        for r in p.findall(f"{_NS}r"):
            p.remove(r)
    return tr


def _set_row_cell_texts(row, mapping: dict[str, str], literal: dict[str, str]) -> None:
    for cell in row.cells:
        for paragraph in cell.paragraphs:
            text = paragraph.text
            stripped = text.strip()
            if stripped in literal:
                _rewrite_paragraph(paragraph, literal[stripped])
                continue
            if REPEAT_NOTE in text:
                _rewrite_paragraph(paragraph, "")
                continue
            match = BRACKET.search(text)
            if match:
                tag = match.group(1)
                for key, expr in mapping.items():
                    if tag.endswith(key):
                        _rewrite_paragraph(paragraph, expr)
                        break


def _convert_loop_rows(table) -> bool:
    """Wrap this table's marker row in a {%tr%} loop, if it has one. True when converted."""
    converted = False
    for spec in LOOP_ROWS:
        for row in table.rows:
            if spec["marker"] in row.cells[0].text or any(
                    spec["marker"] in cell.text for cell in row.cells):
                _set_row_cell_texts(row, spec["cells"], spec.get("literal", {}))

                open_tr = _control_row(row)
                row._tr.addprevious(open_tr)
                _first_paragraph_text(open_tr, "{%%tr for r in %s %%}" % spec["var"])

                close_tr = _control_row(row)
                row._tr.addnext(close_tr)
                _first_paragraph_text(close_tr, "{%tr endfor %}")

                converted = True
                break
    return converted


def _first_paragraph_text(tr, text: str) -> None:
    p = tr.find(f".//{_NS}p")
    r = p.makeelement(f"{_NS}r", {})
    t = p.makeelement(f"{_NS}t", {})
    t.text = text
    r.append(t)
    p.append(r)


def convert(source_path: str, output_path: str) -> None:
    doc = Document(source_path)

    for table in doc.tables:
        _convert_loop_rows(table)
        _convert_paragraphs(_iter_cell_paragraphs(table))

    _convert_paragraphs(doc.paragraphs)

    for section in doc.sections:
        _convert_paragraphs(section.header.paragraphs)
        _convert_paragraphs(section.footer.paragraphs)

    doc.save(output_path)


if __name__ == "__main__":
    src = sys.argv[1] if len(sys.argv) > 1 else "assets/Feasibility Study_Template Draft.docx"
    out = sys.argv[2] if len(sys.argv) > 2 else "assets/feasibility_v3_template.docx"
    convert(src, out)
    print(f"converted: {src} -> {out}")
