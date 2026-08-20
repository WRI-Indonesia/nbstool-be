"""
check_stream.py - exercise the stream contract without touching the network.

    python check_stream.py

Every component is replaced by a stub, so this runs in about a second, needs no database, no
/vsicurl and no AOI, and gives the same answer every time. It covers the parts of the contract a
client depends on and that no amount of staring at a healthy response will show you: what a FAILED
component looks like, which states are retryable, and that a partial run really does re-run the
components its target depends on.

Why stubs. A component only raises when something is genuinely broken, and on a healthy deployment
none of them do -- so the crash path is the one part of the wire format that is never exercised in
practice until the day it matters. Replacing a component with a function that throws is the only
honest way to see the line a client will actually receive.

Note that a crash reports `state: "partial"`, not `"failed"`. The two states are instructions to
the client rather than descriptions of what happened: `partial` means try again, `failed` means
this is the answer. See pipeline.error_status.

FOR THE FRONTEND there is no need for any of this: the endpoint takes `?error_test=api|stream|data`
and fails on purpose. See ERROR_TEST_MODES in apis/geo_apis/feature/routes.py. This file is the
BACKEND check -- it verifies that the real classification is right, which the simulation cannot,
because the simulation just asserts states rather than deriving them.

To watch the same thing against real data, replace an entry of `COMPONENTS` in a scratch script and
run the endpoint normally:

    from application.utils.geos.v3.site_characterisation import run_site_characterisation as R

    def boom(aoi, *deps):
        raise RuntimeError("pretend the raster store is down")

    R.COMPONENTS['habitat area'] = (boom, ())

Real-world ways to produce a genuine failure, in rough order of how faithfully they reproduce
production:

  - drop the VPN. Every component raises OperationalError, `state` is `partial` and every card is
    retryable. That is what a database outage looks like end to end, and it is exactly why a crash
    reports `partial` rather than `failed`: retrying is the right move.
  - point one layer at a name that does not exist, in config.py. `habitat area` and `burned area`
    keep going and report `partial`, because a missing raster there is one species or one year of
    many; most other components raise, which is also `partial`.
  - ask for a Cambodian or Lao site, or an AOI at sea. Those report `failed`: not because
    anything broke, but because the answer does not exist and asking again cannot change it. On
    this endpoint `failed` means "this is the answer" -- see pipeline.error_status.
"""

from __future__ import annotations

import json
import logging
import threading

# `safe` logs every failure with its traceback, which is the point of it -- but the failures below
# are deliberate, so the tracebacks are noise here and would bury the results.
logging.disable(logging.ERROR)

try:
    from ..common import not_applicable
    from ..pipeline import error_status
    from . import run_site_characterisation as R
except ImportError:  # `python check_stream.py`: no package around it
    import pathlib
    import sys

    _here = pathlib.Path(__file__).resolve()
    sys.path.insert(0, str(_here.parents[1]))
    for _sub in ("general", "nature", "climate", "people"):
        sys.path.insert(0, str(_here.parent / _sub))
    sys.path.insert(0, str(_here.parent))
    import run_site_characterisation as R
    from common import not_applicable
    from pipeline import error_status

ENVELOPE = {'process', 'data', 'error_status', 'w', 'a', 'next'}
REAL = dict(R.COMPONENTS)
results: list[bool] = []


def check(label, condition, detail=""):
    results.append(bool(condition))
    print(f"  {'PASS' if condition else 'FAIL'}  {label}{('  ' + detail) if detail else ''}")


def stub(name):
    """A component that succeeds, recording which dependencies it was handed."""
    def fn(aoi, *deps):
        with _lock:
            calls.append((name, tuple(d[0].get('who') for d in deps)))
        return {'who': name}, {f'{name}_field': 1}
    return fn


def boom(aoi, *deps):
    """A component that dies with a secret in the message, as a real one would."""
    raise RuntimeError("postgresql://user:hunter2@10.0.0.5:5432/gis is unreachable")


def absent(aoi, *deps):
    """A component that ran fine and found nothing published for this area."""
    return {'missing': ["6.3: Laos publishes no household count for 'Attapeu'."],
            'who': 'social statistics'}, {}


def degrades(aoi, *deps):
    """1.6's real behaviour when 1.2 hands it nothing: a flag, not a crash."""
    return {'flags': ["1.6: no dominant country from 1.2."], 'who': 'deforestation risk'}, {}


calls: list[tuple] = []
_lock = threading.Lock()


def install(**overrides):
    calls.clear()
    R.COMPONENTS.clear()
    for name, (_, deps) in REAL.items():
        R.COMPONENTS[name] = (overrides.get(name) or stub(name), deps)


def url_for(process):
    from urllib.parse import urlencode
    return '/geos/feature/site-characterisation?' + urlencode(
        {'session_id': 'test', 'process': process})


def run(wanted=None, retry_url=url_for):
    return [json.loads(line) for line in
            R.stream_site_characterisation(object(), wanted, retry_url)]


print("\n== error_status: two states, and the state IS the retryability")
check("a clean component says nothing", error_status({'values': {}}) is None)
check("a permanent caveat in `notes` says nothing",
      error_status({'notes': ["belowground biomass is derived, not mapped"]}) is None,
      "otherwise 3.1 and 2.6 would report a state on every single request")
check("a crash is partial, because a crash is the most retryable thing here",
      error_status({'failed': True, 'flags': ['x']})
      == {'state': 'partial', 'retryable': True, 'retry_url': None, 'messages': ['x']},
      "`partial` means TRY AGAIN, not `the data is incomplete`")
check("a degraded number that cannot be improved is failed",
      error_status({'flags': ['the biomass raster covers 40% of the AOI']})
      == {'state': 'failed', 'retryable': False, 'retry_url': None,
          'messages': ['the biomass raster covers 40% of the AOI']},
      "40% coverage is permanent; a button would reproduce it exactly")
check("a component whose problem was I/O is partial",
      error_status({'flags': ['raster unreadable'], 'retryable': True})['state'] == 'partial')
check("data that does not exist is failed",
      error_status({'missing': ["lao_number_of_households has no row for 'Attapu'"]})
      == {'state': 'failed', 'retryable': False, 'retry_url': None,
          'messages': ["lao_number_of_households has no row for 'Attapu'"]},
      "absence is a permanent answer")
check("state and retryable can never disagree",
      all(error_status(r) is None
          or (error_status(r)['state'] == 'partial') == error_status(r)['retryable']
          for r in ({}, {'failed': True}, {'flags': ['a']}, {'missing': ['b']},
                    {'flags': ['a'], 'retryable': True}, {'missing': ['b'], 'retryable': True},
                    {'flags': ['a'], 'missing': ['b']})),
      "one is computed from the other")
check("messages carry flags and missing together",
      error_status({'flags': ['degraded'], 'missing': ['absent']})['messages']
      == ['degraded', 'absent'])
check("absence caused by a crashed dependency is promoted to partial",
      error_status({'missing': ['nothing resolved'], 'retryable': True})['state'] == 'partial',
      "`after` promotes it, because re-running this re-runs the dependency")
check("a component with nothing to measure is not silent",
      error_status(dict(flags=not_applicable("3.6", "No soil data here.").flags,
                        missing=not_applicable("3.6", "No soil data here.").missing))
      == {'state': 'failed', 'retryable': False, 'retry_url': None,
          'messages': ['No soil data here.']},
      "not_applicable used to leave error_status null")

print("\n== a healthy full run")
install()
lines = run()
check("one line per step plus both bookends", len(lines) == len(R.processes),
      f"{len(lines)} lines")
check("every line carries exactly the six envelope keys",
      all(set(line) == ENVELOPE for line in lines))
check("the plan matches what is emitted",
      [p['name'] for p in lines[0]['data']['processes']] == [ln['process'] for ln in lines])
check("nothing reports an error", all(ln['error_status'] is None for ln in lines))
check("`next` chains to the end and stops",
      all(lines[i]['next'] == lines[i + 1]['process'] for i in range(len(lines) - 1))
      and lines[-1]['next'] is None)
finish = [R._finishes_at(p['name']) for p in R.processes[1:-1]]
check("components are emitted soonest-finishing first", finish == sorted(finish),
      f"{R.processes[1]['name']} ~{finish[0]}s .. {R.processes[-2]['name']} ~{finish[-1]}s")
independent = [p['w'] for p in R.processes[1:-1] if not R.COMPONENTS[p['name']][1]]
check("which for a component with no dependencies means cheapest first",
      independent == sorted(independent))
check("no component is ordered before something it depends on",
      all(R._ORDER.index(d) < R._ORDER.index(n)
          for n, (_, deps) in R.COMPONENTS.items() for d in deps))

print("\n== dependencies reach the component that needs them")
handed = {name: deps for name, deps in calls if deps}
check("1.6 <- 1.2", handed['deforestation risk'] == ('administrative boundaries',))
check("3.2 <- 1.1", handed['soil organic carbon'] == ('ecosystem type',))
check("carbon shares <- 3.1 and 3.2",
      handed['carbon shares'] == ('current carbon storage', 'soil organic carbon'))
check("6.3 <- 1.2", handed['social statistics'] == ('administrative boundaries',))

print("\n== a component that raises")
install(**{'administrative boundaries': boom})
lines = run(['administrative boundaries'])
status = lines[1]['error_status']
check("a crash reports state partial", status['state'] == 'partial', "because it is retryable")
check("but its data is empty", lines[1]['data'] == {}, "partial by state, empty by content")
check("retryable", status['retryable'] is True)
check("the credential in the exception does NOT reach the client",
      all('hunter2' not in m and '10.0.0.5' not in m for m in status['messages']),
      status['messages'][0])
check("retry_url names this component and nothing else",
      status['retry_url'] == url_for('administrative boundaries'), status['retry_url'])
check("retry_url is a path, not an absolute URL", status['retry_url'].startswith('/'))

print("\n== a crash makes the components that depend on it retryable too")
install(**{'administrative boundaries': boom, 'deforestation risk': degrades})
lines = run(['deforestation risk'])
check("1.6 is partial and retryable when 1.2 crashed",
      lines[1]['error_status']['state'] == 'partial'
      and lines[1]['error_status']['retryable'] is True,
      str(lines[1]['error_status']))

install(**{'deforestation risk': degrades})
lines = run(['deforestation risk'])
check("the same 1.6 flag is failed when 1.2 succeeded",
      lines[1]['error_status']['state'] == 'failed'
      and lines[1]['error_status']['retryable'] is False
      and lines[1]['error_status']['retry_url'] is None,
      "an AOI at sea reports this every time")

print("\n== absent data through the whole envelope")
install(**{'social statistics': absent})
lines = run(['social statistics'])
st = lines[1]['error_status']
check("state is failed", st['state'] == 'failed', str(st))
check("and carries no retry button", st['retryable'] is False and st['retry_url'] is None)

install(**{'administrative boundaries': boom, 'social statistics': absent})
lines = run(['social statistics'])
st = lines[1]['error_status']
check("but the same absence turns partial when the dependency CRASHED",
      st['state'] == 'partial' and st['retry_url'] is not None, str(st))

print("\n== a partial run")
install()
lines = run(['social statistics'])
check("only what was asked for is emitted",
      [ln['process'] for ln in lines] == ['preparation', 'social statistics', 'end'])
check("but its dependency really ran", 'administrative boundaries' in dict(calls))
check("and nothing else did", len(calls) == 2, str(sorted(n for n, _ in calls)))
check("`w` is this run's own total, so the progress bar is self-contained",
      lines[0]['w'] == sum(p['w'] for p in lines[0]['data']['processes']))

install()
run(['carbon shares'])
check("a deep dependent pulls its whole chain",
      sorted(n for n, _ in calls) == sorted(['ecosystem type', 'current carbon storage',
                                             'soil organic carbon', 'carbon shares']))

install()
asked = ['land cover', 'social statistics']
lines = run(list(reversed(asked)))
check("several at once come back in plan order, not request order",
      [ln['process'] for ln in lines[1:-1]]
      == [p['name'] for p in R.processes[1:-1] if p['name'] in set(asked)],
      str([ln['process'] for ln in lines[1:-1]]))

print("\n== no URL builder, as when a module is run as a script")
install(**{'administrative boundaries': boom})
lines = run(['administrative boundaries'], retry_url=None)
check("retry_url is null but retryable still tells the truth",
      lines[1]['error_status']['retry_url'] is None
      and lines[1]['error_status']['retryable'] is True)

R.COMPONENTS.clear()
R.COMPONENTS.update(REAL)
print(f"\n{sum(results)}/{len(results)} checks passed")
raise SystemExit(0 if all(results) else 1)
