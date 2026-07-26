# oxid

High-performance URL shortener written in Rust. Live at **[oxid.uk](https://oxid.uk)**.

> 🇧🇷 [Leia em português](README-ptbr.md)

The shortener is not the point — **learning Rust and system design under real load
is**. The project is modelled on a case study that reached ~12,700 req/s
(100 million URLs per day) with read p95 in single-digit milliseconds.

## Design targets

| Requirement | Number |
|---|---|
| URLs created per day | 100 million (~1,157 writes/s) |
| Reads | ~11,574/s (1:10 write-to-read ratio) |
| Shortcode length | 7 Base62 characters (62⁷ ≈ 3.5 trillion) |
| Retention | 10 years (~365 billion rows) |
| Read latency | p95 < 50 ms |
| Errors under 2–3× peak | zero |

## How a shortcode is made

```
bigserial id  →  obfuscate  →  Base62 encode  →  zero-pad to 7  →  eDrBKMi
```

The database id goes through a **bijection** — modular multiplication by a prime
coprime with 62⁷ — so consecutive ids land far apart and the sequence is not
guessable from the code. Being a bijection, collisions are impossible by
construction: there is no retry loop and no uniqueness check on the code itself.

Resolution is the same path backwards, ending in a primary-key lookup.

Two properties are worth knowing:

- **Idempotent.** The same long URL always returns the same code, guaranteed by a
  unique constraint in Postgres — not by the application. Concurrent requests for
  the same URL cannot produce two codes.
- **Immutable.** A code never changes meaning, which is why the cache needs no
  invalidation and carries no TTL.

## Stack

| Layer | Choice |
|---|---|
| HTTP | [Axum](https://github.com/tokio-rs/axum) on Tokio |
| Database | PostgreSQL 18 via [sqlx](https://github.com/launchbadge/sqlx) (compile-time checked queries) |
| Cache | Redis, cache-aside with `allkeys-lru`, plus a negative cache |
| Rate limiting | [`tower_governor`](https://github.com/benwis/tower-governor) on writes only |
| Front end | [Leptos](https://leptos.dev) CSR compiled to WebAssembly, served by nginx |
| Config | YAML layers via [`config`](https://github.com/rust-cli/config-rs) + [`secrecy`](https://github.com/iqlusioninc/crates) |
| Production | k3s on arm64, GitHub Actions, Traefik in front |
| Observability | `tracing` + Prometheus *(stage 7)* |
| Load testing | k6 *(stage 9)* |

Reads are deliberately unlimited: the redirect is the path the cache absorbs and
the one stages 9–10 push to 11k req/s. Limiting it would punish exactly the
traffic the system exists to serve.

## Layout

```
crates/
  oxid-api/       back end: routes, codec, repository, cache, config
  oxid-shared/    API contract — types shared by both sides
  oxid-web/       front end (Leptos, wasm32) + subset fonts
configuration/    base.yaml → <environment>.yaml → APP_* env vars
migrations/       sqlx migrations
infra/            Dockerfiles, compose, nginx, k8s manifests
.github/          CI (every push and PR) and deploy (main only)
```

`oxid-shared` is where full-stack Rust pays off: request and response types live
in one place, so **changing a field in the back end breaks the front end at
compile time**. The contract is enforced by the compiler, not by an integration
test.

> The decision log lives in `docs/DECISOES.md` (Portuguese) and is **not in this
> repository** — `docs/` is ignored globally on the author's machine. Every
> decision below is summarised here or in `ROADMAP.md`.

## Front end

One page: paste a URL, get a code. While you type, a meter shows the length
you are at collapsing onto the seven characters it will become.

Links you create are kept in **`localStorage`**, so closing the tab does not lose
them. Not a cookie: a cookie for the origin would ride along on every `/{code}`
redirect, adding bytes to the one path the whole system is tuned around. The list
never touches the network — which also means it does not follow you to another
browser or device, and clearing site data clears it.

Removing a link from the list does **not** disable it. Codes are immutable and
shared: the same long URL yields the same code for everyone, so "deleting" one
would be deleting someone else's link. It is also why the redirect can be a 301.

The typeface is JetBrains Mono, self-hosted and subset to ASCII plus ten symbols
— 5 KB per weight, against ~90 KB for the full family.

## Running it

Requires Docker. Everything else lives inside the containers.

```bash
cp .env.example .env

docker compose --env-file .env \
  -f infra/docker-compose.yml \
  -f infra/docker-compose.local.yml up
```

| Service | URL |
|---|---|
| API | http://127.0.0.1:3000 |
| Front end | http://127.0.0.1:8080 |
| Postgres | `localhost:5432` |
| Redis | `localhost:6379` |

Both services hot reload from the host: the source is bind-mounted, `watchexec`
restarts the API and `trunk` rebuilds the front end. `Ctrl+C` tears everything
down in well under a second.

### Without Docker

Postgres and Redis still come from compose; the rest runs natively.

```bash
docker compose --env-file .env -f infra/docker-compose.yml up -d
cargo sqlx migrate run

cargo run -p oxid-api                      # API
cd crates/oxid-web && trunk serve          # front end
```

## API

### `POST /v1/shorten`

```bash
curl -X POST http://127.0.0.1:3000/v1/shorten \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://example.com/a/very/long/path"}'
```

```json
{
  "code": "eDrBKMi",
  "short_url": "http://127.0.0.1:3000/eDrBKMi",
  "long_url": "https://example.com/a/very/long/path"
}
```

Only `http` and `https` are accepted. `javascript:` and `data:` URLs parse fine
but would turn the shortener into an XSS vector on a trusted domain, so the
scheme is checked explicitly.

### `GET /{code}`

Answers **301** with the original URL in `Location`. The redirect sits at the
root on purpose: `/v1/urls/` would spend nine characters on a URL whose entire
job is being short.

A malformed code answers 404 rather than 400 — separating the two would leak the
shortcode format to anyone probing the endpoint.

### `GET /health`

### Errors

Errors follow [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457), served as
`application/problem+json`:

```json
{
  "type": "/problems/invalid-url",
  "title": "Invalid URL",
  "status": 400,
  "detail": "only http and https urls are accepted"
}
```

`title` is stable and safe for clients to match on; `detail` describes the single
occurrence. Database errors never reach the client — table, column and constraint
names would hand out a map of the schema.

## Observability

Metrics are served on **their own port**, never on the public router:

```bash
curl http://127.0.0.1:9090/metrics
```

That separation is deliberate. Traefik forwards everything that is not the front
end to this service, so a `/metrics` route would publish request volumes, latency
distributions and cache behaviour to the internet. In the cluster the port is
declared on the Deployment and left out of the Service.

Prometheus and Grafana are a separate compose file, so the everyday `up` stays
light:

```bash
docker compose --env-file .env \
  -f infra/docker-compose.yml \
  -f infra/docker-compose.local.yml \
  -f infra/docker-compose.observability.yml up
```

| Service | URL |
|---|---|
| Grafana (anonymous, admin) | http://127.0.0.1:3001 |
| Prometheus | http://127.0.0.1:9091 |

The dashboard is provisioned from `infra/grafana/provisioning/` — p50/p95/p99 per
route, requests per second by status, cache hit rate, lookups by outcome, and pool
connections. A dashboard clicked together in the UI would live only in Grafana's
volume and could never be reviewed in a pull request.

Three details worth knowing:

- **The route label comes from the matched path**, not the URL. On `/{code}` those
  differ on every request, and labelling with the real path would create one time
  series per shortcode.
- **Latency buckets are chosen, not default.** Stage 10 targets p95 under 50 ms,
  so the resolution sits below 100 ms where the answers actually land.
- **Pool gauges are read at scrape time**, so they are never staler than the
  scrape. `total - idle` is the number that matters: pinned at `max_connections`
  means requests are queueing on the pool rather than on the database.

## Development

```bash
# tests (Postgres must be up — each test gets its own temporary database)
cargo test --workspace --exclude oxid-web

# lint: fmt, back end, and the front end on its wasm target
cargo fmt --all --check \
  && cargo clippy --workspace --exclude oxid-web --all-targets -- -D warnings \
  && cargo clippy -p oxid-web --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Warnings are errors here. Clippy runs with `pedantic` denied, and `unwrap`,
`expect`, `panic`, indexing and unchecked arithmetic are all denied outside
tests — every panic has to be a deliberate, argued decision.

The front end needs its own clippy run: lints that only exist on `wasm32` stay
invisible from the host target. Both crates are library + binary rather than a
plain binary, because in a pure binary `unreachable_pub` and `redundant_pub_crate`
contradict each other.

### Changing queries

`sqlx` validates SQL against the real schema at compile time. After touching a
query or a migration:

```bash
cargo sqlx prepare
```

That refreshes `.sqlx/`, which is committed so CI builds without a database
(`SQLX_OFFLINE=true`).

## Deployment

Every push to `main` builds both images on a **native arm64 runner**, runs the
migrations as a Kubernetes `Job`, and rolls out API then front end **by digest**
— never by `latest`, which cannot tell you what is running nor what to roll back
to. CI itself runs on every pull request.

The cluster is a single-node k3s box (Oracle Ampere, arm64). Traefik runs outside
the cluster and reaches it over a NodePort. Details in `infra/k8s/README.md`
(local only, same `docs/` caveat).

## Roadmap

| Stage | Status |
|---|---|
| 1. Async foundation | ✅ |
| 2. Base62 + obfuscating bijection | ✅ |
| 3. Persistence with sqlx | ✅ |
| 4. Write and read routes | ✅ |
| 5. Redis cache (cache-aside + negative cache) | ✅ |
| 5.1 Saved links in the browser | ✅ |
| 5.2 Lighthouse fixes | partly — contrast, ARIA and touch targets done |
| 6. Configuration and pool sizing | ✅ (pulled forward) |
| 7. Observability (Prometheus + Grafana) | |
| 8. Full topology (2 instances behind Nginx) | |
| 9. Load testing with k6 | |
| 10. Optimization loop to full scale | |
| 11. Accounts and sessions | |
| 12. Click analytics | |

Details in [`ROADMAP.md`](ROADMAP.md).

## License

MIT
