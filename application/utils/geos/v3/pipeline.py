"""
pipeline.py - the streaming envelope and failure contract every v3 module shares.

F02-P2 General is the first module ported; Nature, Climate and People follow, and each one is the
same shape: a fixed list of components, run together, each emitting the slice of the payload its
card needs. What differs between modules is which components exist and how they depend on each
other. What does not differ is the wire format, the per component failure rule and the progress
accounting, so those live here.

Wire format is NDJSON, one JSON object per line, matching luma's `/lulc-map`:

    {"process": "ecosystem type", "data": {...}, "error_status": null, "w": 10.0, "a": 1.5,
     "next": "administrative boundaries"}

`w` is the total weight of the run and `a` this step's share, so a progress bar advances by cost
rather than by step count. The opening line carries the whole `processes` list, so the frontend
knows the plan before any work lands.

`error_status` is null when the component ran clean, and otherwise carries the state, whether a
retry could plausibly change the answer, where to send that retry, and the component's own
messages:

    {"state": "partial", "retryable": true,
     "retry_url": "/geos/feature/site-characterisation?session_id=...&process=habitat+area",
     "messages": ["2.3: 4 of 236 candidate species rasters could not be read ..."]}

Two states, and the state IS the retryability: `partial` means asking again could change the
answer, `failed` means this is the answer. They are instructions to the client rather than
descriptions of what happened -- a crashed component reports `partial`, because a crash is the
most retryable thing here. `retry_url` is null whenever `retryable` is false, so a client draws
its button on the URL alone. See `error_status()` below, and note that permanent METHODOLOGY
caveats are a separate thing again, kept out of this field entirely.

A module supplies `processes` and a generator of view payloads. `processes` opens with a
`preparation` entry and closes with an `end` entry. Everything between the two is one component,
in emission order.

Both bookends are emitted. `preparation` carries the whole plan; `end` carries an empty `data` and
a null `next`, and is how a client knows the run finished rather than the connection dropping
part-way. That distinction cannot come from the HTTP status: the status is 200 from the moment the
first line is written, whatever happens afterwards.
"""

from __future__ import annotations

import json
import logging
import sys

try:
    from .common import to_jsonable
except ImportError:  # imported as a top-level module by a module run as a script
    from common import to_jsonable

logger = logging.getLogger(__name__)


def pack(data) -> str:
    """One NDJSON line. ASCII escaped, so the stream carries no charset assumption of its own."""
    return json.dumps(data) + "\n"


def safe(stage: str, fn, *args) -> tuple[dict, dict]:
    """Run one component, turning a failure into empty output plus a log line.

    A component that raises costs its own card rather than the whole response. That matters more
    under concurrency, where the exception surfaces on a worker thread and would otherwise appear
    only when its future is read. Returns the components' own `(results, view_results)` shape, so
    a caller that needs a value out of `results` can `.get` its way through an empty dict.

    `results` carries `failed`, which is what turns into `error_status.state == "failed"` on the
    wire. Without it a crashed component is indistinguishable from one that ran and had nothing to
    report: both are `data: {}`, and the HTTP status has been 200 since the first line.

    Only the exception TYPE goes into the message. `str(e)` on a rasterio or psycopg2 error carries
    the layer URL or the connection string, and this string is rendered in a browser; the full text
    stays in the log, where `logger.exception` also keeps the traceback.
    """
    try:
        return fn(*args)
    except Exception:
        logger.exception('%s failed', stage)
        return {
            'failed': True,
            'flags': [f"{stage} could not be computed ({sys.exc_info()[0].__name__})."],
        }, {}


def after(stage: str, fn, aoi, *futures) -> tuple[dict, dict]:
    """Run a component that needs another's output, once those futures have landed.

    `fn` is called as `fn(aoi, *dependency_results)`, each dependency being the whole
    `(results, view_results)` tuple its component returned -- including `({}, {})` if it failed,
    which every dependent component already handles by `.get`-ing its way through.

    THE WAIT HAPPENS ON A WORKER, not at submit time, and that is why the pool has to be sized for
    every component running at once. A waiter holding a thread while the component it waits on has
    none would deadlock the pair.

    A DEPENDENCY THAT CRASHED MAKES ITS DEPENDENT RETRYABLE, and this is the only place that rule
    is applied. 1.6 degrades gracefully to "no national comparison" when 1.2 gives it no country,
    and on its own that is not worth a retry button -- an AOI in open water would report it every
    time. But when the country is missing because 1.2 RAISED, a retry re-runs 1.2 and may well fix
    both. Only the dependency's own outcome separates those two, and only this function sees it.
    """
    deps = [f.result() for f in futures]
    results, view_results = safe(stage, fn, aoi, *deps)
    if any(dep_results.get('failed') for dep_results, _ in deps):
        results = dict(results, retryable=True)
    return results, view_results


def error_status(results: dict) -> dict | None:
    """The `error_status` field for one component's line, or None when it ran clean.

    "OK" is the ABSENCE of the object, so a client tests `if (line.error_status)` first and only
    then looks at `state`. TWO STATES, and THE STATE IS THE RETRYABILITY -- one is computed from
    the other, so they can never disagree:

        partial  asking again could plausibly give a different answer. Retry.
        failed   this IS the answer. Asking again returns exactly the same thing.

    READ THE NAMES AS INSTRUCTIONS TO THE CLIENT, not as descriptions of what happened. They do
    not line up with intuition, and that is the deliberate trade:

      - a component that CRASHED reports `partial`, with `data: {}`. Nothing about it is partial,
        but a crash here is almost always the network -- when the database became unreachable
        during development all 25 components raised OperationalError and a retry was exactly the
        right move -- so it is the most retryable thing this endpoint produces, and the state has
        to say so.
      - "the biomass raster covers 40% of the AOI" and "Laos publishes no household count for
        Attapeu" both report `failed` while carrying real numbers. Neither will ever change, so
        neither gets a button.

    The classification underneath keeps all three distinctions; only the wire collapses them:

        results['failed']   the component raised. Set by `safe`. Always retryable.
        results['flags']    it produced numbers, but they are degraded or incomplete. Retryable
                            only when the component says so, which means when what failed was I/O.
        results['missing']  it ran correctly and there is nothing to report -- no such table, no
                            row for this province, the layer does not reach this AOI. Never
                            retryable on its own.

    Keeping them apart costs nothing and means a third state is one line here and nothing anywhere
    else. `missing` also closes a hole worth keeping on its own account: a component with nothing
    to measure returns through `common.not_applicable`, whose `flags` are empty, so before
    `missing` existed those twelve components reported `error_status: null` -- indistinguishable
    from a healthy run that found something.

    `after` promotes any of these to retryable when the reason a component has nothing to say is
    that the component it depends on crashed.

    `retry_url` is filled in by `stream`, the only layer that knows how this run was reached. It
    stays null here and for any caller with no URL to give -- a module run as a script, for one.
    `retryable` is now redundant with `state`; it is kept because it is the primitive this function
    computes and `state` is a presentation of it, and because a script has a `retryable` to read
    when it has no `retry_url`.

    Permanent methodology caveats do NOT belong here at all. They live in `results['notes']` and
    never reach this function: a component whose only remark is "belowground biomass is a fixed
    multiple of aboveground" is not degraded, and if it reported a state on every run the field
    would be ignored within a week.
    """
    flags = results.get('flags') or []
    missing = results.get('missing') or []

    if not (results.get('failed') or flags or missing):
        return None

    # A crash is retryable by definition. Anything else only if the component or `after` said so.
    retryable = bool(results.get('failed') or results.get('retryable'))
    return {
        'state': 'partial' if retryable else 'failed',
        'retryable': retryable,
        'retry_url': None,
        'messages': flags + missing,
    }


def stream(processes: list[dict], views, retry_url=None):
    """Wrap a module's component output in the NDJSON envelope.

    `views` yields one `(view_results, error_status)` pair per entry of `processes` between
    `preparation` and `end`, in that order. The view is passed through `to_jsonable`, so components
    can return dataclasses and numpy scalars and no module has to remember to convert.

    `processes` need not be the module's whole list. A retry passes the subset that was asked for,
    and every line -- including `preparation`'s plan and the `w` totals -- describes THAT run, so a
    client needs no second code path to read one.

    `retry_url` is an optional `name -> url` function, supplied by whatever reached this stream.
    Only a retryable component gets one. It is a function rather than a base string because this
    module knows nothing about HTTP -- every runner here also runs as a plain script -- so the web
    layer keeps ownership of what a URL to itself looks like, and a caller with none passes
    nothing and every `retry_url` stays null.
    """
    # Rounded because the weights are one-decimal seconds and summing them in binary floating point
    # does not stay that way: 21.9 + 6.7 + 0.1 + 0.1 lands on 28.799999999999997, which is what a
    # client would then have to display or divide by.
    total_w = round(sum(p['w'] for p in processes), 2)
    last = len(processes) - 1

    def forge(step: int, data: dict, status: dict | None = None) -> str:
        if status is not None and status['retryable'] and retry_url is not None:
            status = dict(status, retry_url=retry_url(processes[step]['name']))
        return pack({
            'process': processes[step]['name'],
            'data': data,
            # Null on a clean component, and on both bookends: neither runs any analysis.
            'error_status': status,
            'w': total_w,
            'a': processes[step]['w'],
            # `end` has nothing after it, so its `next` is null rather than running off the list.
            'next': processes[step + 1]['name'] if step < last else None,
        })

    yield forge(0, {'processes': processes})

    for step, (view, status) in enumerate(views, start=1):
        yield forge(step, to_jsonable(view), status)

    # `end` IS emitted, carrying no data. It is the client's signal that the stream finished
    # rather than being cut off mid-flight: NDJSON over a chunked response gives no other way to
    # tell a complete run from a dropped connection, and by then the status is long since 200.
    yield forge(last, {})


def component_count(processes: list[dict]) -> int:
    """Number of real components: everything except `preparation` and `end`.

    Modules size their thread pool with this. It has to fit every component at once when one of
    them waits on another's future, or the waiter can take the last worker and the two deadlock.
    """
    return len(processes) - 2
