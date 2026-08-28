// k6 load test for the v3 analyzer endpoints:
//   GET  /geos/feature/analysis    (union NDJSON stream: sitechar + threat + pathway --
//                                   the one call the frontend makes after the polygon)
//   POST /geos/feature/benefit     (NDJSON stream)
//
// Usage:
//   k6 run -e BASE_URL=https://<prod-host> -e SEED=1 loadtest/k6_v3_analyzer.js   # first run
//   k6 run -e BASE_URL=https://<prod-host> loadtest/k6_v3_analyzer.js             # reuse
//   k6 run -e BASE_URL=... -e VUS=4 -e DURATION=10m loadtest/k6_v3_analyzer.js
//
// Session ids are DETERMINISTIC -- loadtest-1 .. loadtest-<SESSIONS> (default 100) -- so
// they survive between runs. SEED=1 creates them via POST /geos/polygon, each with a
// random AOI from aoi_pool.json (build with: venv/Scripts/python loadtest/build_aoi_pool.py);
// posting an existing id just updates its geometry, so reseeding is idempotent. Without
// SEED, setup() only verifies which sessions exist and reuses them. Cleanup:
//   DELETE ... WHERE session_id LIKE 'loadtest-%'      (polygons / sessions_auth / data_analyzer)
// Each iteration picks a random session, so run sizes vary like real traffic.
//
// Server caps to know when reading results: _SITECHAR_SLOTS = 6 (analysis and benefit
// queue inside the stream past 6 concurrent runs) and gunicorn runs 8 threads per
// instance. Default VUS=6 probes the knee without drowning it.

import http from 'k6/http';
import { check, fail } from 'k6';
import { Counter } from 'k6/metrics';
import { SharedArray } from 'k6/data';

const BASE_URL = __ENV.BASE_URL || fail('set -e BASE_URL=https://<host>');
const VUS = parseInt(__ENV.VUS || '6');
const DURATION = __ENV.DURATION || '5m';
const SESSIONS = parseInt(__ENV.SESSIONS || '100');
const SEED = __ENV.SEED === '1';

const AOI_POOL = new SharedArray('aois', () =>
    JSON.parse(open('./aoi_pool.json')));

// One raster run takes ~40 s alone and degrades gently under load; queued behind six
// others it can take several minutes. Generous per-request timeout, not k6's 60 s.
const STREAM_TIMEOUT = '600s';

export const options = {
    scenarios: {
        analyzer_flow: {
            executor: 'constant-vus',
            vus: VUS,
            duration: DURATION,
            gracefulStop: STREAM_TIMEOUT,
        },
    },
    // 100 sequential polygon POSTs; the default 60 s is not enough.
    setupTimeout: '600s',
    thresholds: {
        http_req_failed: ['rate<0.05'],
        truncated_streams: ['count==0'],
    },
    summaryTrendStats: ['min', 'med', 'p(95)', 'max'],
};

const truncatedStreams = new Counter('truncated_streams');

// Component lines whose error_status is set. A stream can end cleanly while every
// component inside it crashed in milliseconds -- which makes latency numbers garbage --
// so a run is only trustworthy when this stays near zero (a few known data-gap
// partials, e.g. social statistics on Indonesia, are normal).
const failedComponents = new Counter('failed_components');

export function setup() {
    const sessions = [];
    let failed = 0;
    for (let i = 1; i <= SESSIONS; i++) {
        const sessionId = `loadtest-${i}`;
        if (SEED) {
            const aoi = AOI_POOL[Math.floor(Math.random() * AOI_POOL.length)];
            const res = http.post(`${BASE_URL}/geos/polygon`,
                JSON.stringify({
                    session_id: sessionId,
                    type: 'Feature',
                    geometry: aoi.geometry,
                    properties: {},
                }),
                { headers: { 'Content-Type': 'application/json' } });
            if (res.status !== 200) {
                failed++;
                console.warn(`polygon failed for ${aoi.name}: ${res.status} ${res.body}`);
                continue;
            }
        } else {
            const res = http.get(
                `${BASE_URL}/geos/polygon?session_id=${sessionId}`);
            if (res.status !== 200) {
                failed++;
                continue;
            }
        }
        sessions.push(sessionId);
    }
    if (sessions.length === 0) {
        fail(SEED ? 'no session survived seeding'
                  : 'no seeded session found -- run once with -e SEED=1');
    }
    console.log(`sessions ready: ${sessions.length}, missing/failed: ${failed}`);
    return { sessions };
}

// A finished NDJSON stream ends with an `end` line; anything else is a truncated run
// (the status is already 200 by then, so this is the only way to tell).
function checkStream(res, name) {
    const ok = check(res, {
        [`${name} status 200`]: (r) => r.status === 200,
        [`${name} stream complete`]: (r) => {
            if (r.status !== 200 || !r.body) return false;
            const lines = r.body.trim().split('\n');
            try {
                return JSON.parse(lines[lines.length - 1]).process === 'end';
            } catch (e) {
                return false;
            }
        },
    });
    if (res.status === 200 && !ok) truncatedStreams.add(1);
    if (res.status === 200 && res.body) {
        for (const line of res.body.trim().split('\n')) {
            try {
                if (JSON.parse(line).error_status) failedComponents.add(1);
            } catch (e) { /* counted by the stream-complete check */ }
        }
    }
}

export default function (data) {
    const sessionId = data.sessions[Math.floor(Math.random() * data.sessions.length)];
    const params = { timeout: STREAM_TIMEOUT };

    const analysis = http.get(
        `${BASE_URL}/geos/feature/analysis?session_id=${sessionId}`,
        { ...params, tags: { name: 'analysis' } });
    checkStream(analysis, 'analysis');

    const benefit = http.post(`${BASE_URL}/geos/feature/benefit`,
        JSON.stringify({
            session_id: sessionId,
            duration_years: 30,
            ecosystem_class: 1,
            carbon_project: 'yes',
        }),
        {
            ...params,
            headers: { 'Content-Type': 'application/json' },
            tags: { name: 'benefit' },
        });
    checkStream(benefit, 'benefit');
}
