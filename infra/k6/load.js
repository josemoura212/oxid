// Load test for stage 9.
//
//   node infra/k6/seed.mjs --base $TARGET --count 100000 > infra/k6/pool.json
//   k6 run --env TARGET=$TARGET --env SCALE=0.5 infra/k6/load.js
//
// TARGET has to bypass the CDN. Measuring through it would measure its cache
// and its rate limiting, not this service.
import http from 'k6/http'
import { check } from 'k6'
import { SharedArray } from 'k6/data'
import { Trend, Rate } from 'k6/metrics'

const TARGET = __ENV.TARGET || 'http://127.0.0.1:3000'
const SCALE = Number(__ENV.SCALE || 0.5)
const DURATION = __ENV.DURATION || '3m'

// The design targets: 100M URLs/day at a 1:10 write-to-read ratio.
const READS_AT_FULL = 11574
const WRITES_AT_FULL = 1157

const reads = Math.round(READS_AT_FULL * SCALE)
const writes = Math.round(WRITES_AT_FULL * SCALE)

// Loaded once and shared across VUs. A plain array would be copied into every
// VU — at 100k entries that is the generator running out of memory before the
// target runs out of CPU.
const pool = new SharedArray('codes', function () {
  return JSON.parse(open('./pool.json')).codes
})

const redirectLatency = new Trend('redirect_latency', true)
const redirectOk = new Rate('redirect_ok')

// Little's law again, same as the connection pool in stage 8: the VUs needed to
// sustain an arrival rate are `rate × latency`. Sizing by a fraction of the rate
// instead — the obvious-looking `rate / 2` — allocates thousands of VUs at a few
// megabytes each and puts the generator into swap, which shows up as latency and
// gets blamed on the target.
//
// EXPECTED_MS should be the round trip you measured plus a generous estimate of
// service time. HEADROOM covers the tail.
const EXPECTED_MS = Number(__ENV.EXPECTED_MS || 40)
const HEADROOM = 4

// Pre-allocated with slack rather than at the computed figure. Growing the VU
// pool is not instantaneous, and an arrival-rate executor that finds no free VU
// drops the iteration instead of delaying it — which fails the dropped_iterations
// threshold on startup alone, while the steady state would have been fine.
const PREALLOC_SLACK = 3

function vusFor(rate) {
  return Math.max(8, Math.ceil((rate * EXPECTED_MS) / 1000))
}

// Zipf-ish: a power law over the pool index. Real shortener traffic is a few
// very hot codes and a long cold tail, and drawing uniformly would model a
// workload nobody has. EXPONENT above 1 concentrates on the hot end.
//
// An exact Zipf draw needs a precomputed CDF and a binary search per iteration.
// At 11k iterations a second the generator's own cost matters, so this trades
// exactness for O(1) — the shape is right even if the constant is not.
const EXPONENT = Number(__ENV.ZIPF_EXPONENT || 2.5)

function hotIndex() {
  return Math.floor(pool.length * Math.pow(Math.random(), EXPONENT))
}

export const options = {
  // Nothing here reads a response body. Keeping them would spend generator
  // memory and CPU on bytes the test never looks at.
  discardResponseBodies: true,

  scenarios: {
    reads: {
      executor: 'ramping-arrival-rate',
      exec: 'read',
      startRate: 0,
      timeUnit: '1s',
      // 30s ramp. A constant executor from zero puts cold start into the same
      // percentiles as steady state, and p95 never recovers from it.
      stages: [
        { target: reads, duration: '30s' },
        { target: reads, duration: DURATION },
      ],
      preAllocatedVUs: vusFor(reads) * PREALLOC_SLACK,
      maxVUs: vusFor(reads) * HEADROOM,
    },
    writes: {
      executor: 'ramping-arrival-rate',
      exec: 'write',
      startRate: 0,
      timeUnit: '1s',
      stages: [
        { target: writes, duration: '30s' },
        { target: writes, duration: DURATION },
      ],
      preAllocatedVUs: vusFor(writes) * PREALLOC_SLACK,
      maxVUs: vusFor(writes) * HEADROOM,
    },
  },

  thresholds: {
    // The stage 10 goal, measured client-side. It includes network round trip,
    // so from outside the datacentre this reads high by exactly the RTT — see
    // the note in the roadmap about telemetry on both sides.
    'redirect_latency': ['p(95)<50'],
    'redirect_ok': ['rate>0.999'],
    'http_req_failed': ['rate<0.001'],
    // Non-negotiable. Dropped iterations mean the generator could not keep the
    // requested arrival rate, and every percentile above becomes a measurement
    // of the generator instead of the target.
    'dropped_iterations': ['count===0'],
  },
}

export function read() {
  const code = pool[hotIndex()]

  const res = http.get(`${TARGET}/${code}`, {
    // k6 follows redirects by default. Left on, every iteration would chase the
    // Location header out to the public internet — measuring someone else's
    // server and generating traffic nobody asked for.
    redirects: 0,
    tags: { name: 'redirect' },
  })

  redirectLatency.add(res.timings.duration)
  redirectOk.add(res.status === 301)

  check(res, {
    'redirect is 301': (r) => r.status === 301,
    'has location': (r) => !!r.headers['Location'],
  })
}

export function write() {
  // Unique per iteration, so every write is a real insert. Repeating a URL
  // would exercise the idempotent path instead — worth measuring, but as its
  // own scenario, not blended into this one.
  const unique = `${__VU}-${__ITER}-${Date.now()}`
  const long = `https://example.com/load/${unique}`

  const res = http.post(
    `${TARGET}/v1/shorten`,
    JSON.stringify({ url: long }),
    { headers: { 'Content-Type': 'application/json' }, tags: { name: 'shorten' } },
  )

  check(res, {
    'shorten is 200': (r) => r.status === 200,
    // Called out separately: a 429 here is the per-IP write limit, not the
    // service failing. One load generator is one IP standing in for millions.
    'not rate limited': (r) => r.status !== 429,
  })
}
