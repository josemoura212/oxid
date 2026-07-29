# Load test

Two scripts. `seed.mjs` builds the pool of shortcodes, `load.js` measures.

```bash
export TARGET=http://<host>:<nodeport>

node infra/k6/seed.mjs --base "$TARGET" --count 100000 > infra/k6/pool.json
k6 run --env TARGET="$TARGET" --env SCALE=0.5 infra/k6/load.js
```

`SCALE` is a fraction of the design target — 11,574 reads/s and 1,157 writes/s.
Start at `0.5` and only move up once a run is clean.

## Before trusting a single number

A load test measures the weakest link in the whole chain, and the generator is
part of that chain. Check these first; each one has silently invalidated
somebody's benchmark before.

- **`dropped_iterations` must be zero.** It is a threshold in the script, so a
  non-zero count fails the run outright. Dropped iterations mean the generator
  could not sustain the requested arrival rate — every percentile then describes
  the generator, not the target.
- **The generator needs CPU headroom.** Watch it during the run. A saturated
  generator adds its own scheduling delay to every measurement and reports it as
  server latency.
- **The target must be a release build.** A debug binary is not slow by a
  constant factor, it is slow unpredictably.
- **Do not measure through a CDN.** `TARGET` has to reach the origin directly. A
  CDN in the path returns its own cache hits and applies its own rate limiting,
  so the percentiles belong to it.
- **Know your network baseline.** Measure round-trip time to the target before
  the run and subtract it mentally from every client-side percentile. From
  outside the datacentre this is not a rounding error — it can be most of the
  latency budget.
- **Watch generator memory too, not just CPU.** Each VU costs a few megabytes.
  A generator that starts swapping reports the swap as request latency.

## Sizing the VUs

`EXPECTED_MS` drives it, and it is Little's law again — the same arithmetic as
the connection pool in stage 8. Virtual users needed to sustain an arrival rate
are `rate × latency`, not some fraction of the rate.

```
5,787 reads/s × 40 ms = 232 VUs, with 4× headroom for the tail
```

Sizing by a fraction of the rate is the tempting mistake: `rate / 2` allocates
thousands of VUs at a few megabytes each, pushes the generator into swap, and
the resulting delay gets reported as server latency. Set `EXPECTED_MS` to the
round trip you measured plus a generous guess at service time.

## Client-side and server-side are different measurements

`load.js` reports what the client experienced: service time plus network. The
metrics endpoint reports what the handler took, and nothing else.

Neither is more correct. **The gap between them is the network cost**, isolated
without a separate experiment — which is the whole reason the roadmap asks for
telemetry on both sides. A run where the server p95 is flat and the client p95
is not has a network problem, not a service problem, and the pair of numbers
says so immediately.

## What the read scenario actually exercises

Codes are drawn with a power-law bias toward the start of the pool, because real
shortener traffic is a few very hot links and a long cold tail. Drawing
uniformly would model a workload nobody has.

The consequence worth stating: **if the pool fits in the cache, this measures
the hot path only** — the router, the runtime, the cache and the network, with
the database never touched on reads. That is a legitimate first measurement and
a misleading only one. Forcing misses is a stage 10 variable, changed on its own
like every other.

`redirects: 0` matters more than it looks. k6 follows redirects by default, so
without it every iteration would chase the `Location` header out to the public
internet — measuring an unrelated server and sending it traffic it never agreed
to receive.

## The write scenario needs an exemption

`POST /v1/shorten` is rate limited per IP. A load generator is a single IP
standing in for the millions of clients the design targets assume, so it hits
that limit immediately and the run fills with 429s.

The script checks for this separately from other failures, because a 429 here is
the rate limiter working correctly rather than the service failing. Exempting
the generator's address restores the realism the generator destroys — it does
not weaken the limit for anyone else.

## Writes leave rows behind

Every write scenario iteration is a real insert with a unique URL. There is no
deletion path by design, so the rows are permanent. At the volumes this project
projects they are noise, but they are noise that stays.

Worth knowing about the id sequence: the insert uses `ON CONFLICT DO NOTHING`,
and a `bigserial` is consumed on every attempt, including the ones that conflict.
Duplicate writes therefore burn shortcode space without creating rows.
