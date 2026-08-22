# application/utils/document_generator/v3/prefill.py
#
# Initial values for the F03 feasibility form, taken from the session's analyser results.
#
# The form asks PROJECT-AREA questions; the analyser holds REGIONAL statistics, so only fields
# where the regional figure is an honest starting point are prefilled -- the user is expected to
# correct them from local knowledge. Everything else starts empty. Stored answers always win
# over prefill: this only fills gaps.

from __future__ import annotations


def feasibility_prefill(analyzer) -> dict:
    """`{se* key: value}` for the fields the analyser can seed. Empty when nothing is stored."""
    if analyzer is None:
        return {}
    people = analyzer.people_json or {}
    demography = people.get("social_demography", {})
    economy = people.get("economy", {})

    prefill = {
        # Regional household count -- the form asks for the project area's own.
        "seHouseholds": demography.get("household_number"),
        # Regional average household income (country-dependent availability).
        "seIncome": economy.get("avg_household_income"),
    }
    return {key: value for key, value in prefill.items() if value not in (None, "", [], {})}


def merge_form(stored: dict | None, prefill: dict) -> dict:
    """Prefill fills gaps only; a stored answer -- even a deliberate blank -- is never replaced."""
    merged = dict(prefill)
    merged.update(stored or {})
    return merged
