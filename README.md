# oxid

High-performance URL shortener written in Rust.

> 🇧🇷 [Leia em português](README-ptbr.md)

The shortener is not the point — **learning Rust and system design under real load
is**. The project is modelled on a case study that reached ~12,700 req/s
(100 million URLs per day) with read p95 in single-digit milliseconds. Every
decision here is written down with its trade-off in
[`docs/DECISOES.md`](docs/DECISOES.md), including the ones that turned out wrong.

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
- **Immutable.** A code never changes meaning, which is why the cache (stage 5)
  needs no invalidation.

## Stack

| Layer | Choice |
|---|---|
| HTTP | [Axum](https://github.com/tokio-rs/axum) on Tokio |
| Database | PostgreSQL 18 via [sqlx](https://github.com/launchbadge/sqlx) (compile-time checked queries) |
| Cache | Redis, cache-aside with `allkeys-lru` *(stage 5)* |
| Front end | [Leptos](https://leptos.dev) CSR, compiled to WebAssembly |
| Config | YAML layers via [`config`](https://github.com/rust-cli/config-rs) + [`secrecy`](https://github.com/iqlusioninc/crates) |
| Observability | `tracing` + Prometheus *(stage 7)* |
| Load balancer | Nginx *(stage 8)* |
| Load testing | k6 *(stage 9)* |

## Layout

```
crates/
  oxid-api/       back end: routes, codec, repository, config
  oxid-shared/    API contract — types shared by both sides
  oxid-web/       front end (Leptos, wasm32)
configuration/    base.yaml → <environment>.yaml → APP_* env vars
migrations/       sqlx migrations
infra/            docker compose, dev image
docs/DECISOES.md  decision log (Portuguese)
```

`oxid-shared` is where full-stack Rust pays off: request and response types live
in one place, so **changing a field in the back end breaks the front end at
compile time**. The contract is enforced by the compiler, not by an integration
test.

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

Both services hot reload from the host: the source is bind-mounted, `watchexec`
restarts the API and `trunk` rebuilds the front end. `Ctrl+C` tears everything
down in well under a second.

### Without Docker

Postgres still comes from compose; the rest runs natively.

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

## Development

```bash
# tests (Postgres must be up — each test gets its own temporary database)
cargo test

# lint: fmt, back end, and the front end on its wasm target
cargo fmt --all --check \
  && cargo clippy --workspace --exclude oxid-web --all-targets -- -D warnings \
  && cargo clippy -p oxid-web --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Warnings are errors here. Clippy runs with `pedantic` denied, and `unwrap`,
`expect`, `panic`, indexing and unchecked arithmetic are all denied outside
tests — every panic has to be a deliberate, argued decision.

The front end needs its own clippy run: lints that only exist on `wasm32` stay
invisible from the host target.

### Changing queries

`sqlx` validates SQL against the real schema at compile time. After touching a
query or a migration:

```bash
cargo sqlx prepare
```

That refreshes `.sqlx/`, which is committed so CI builds without a database
(`SQLX_OFFLINE=true`).

## Roadmap

| Stage | Status |
|---|---|
| 1. Async foundation | ✅ |
| 2. Base62 + obfuscating bijection | ✅ |
| 3. Persistence with sqlx | ✅ |
| 4. Write and read routes | ✅ |
| 5. Redis cache (cache-aside + negative cache) | |
| 6. Configuration and pool sizing | ✅ (pulled forward) |
| 7. Observability (Prometheus + Grafana) | |
| 8. Full topology (2 instances behind Nginx) | |
| 9. Load testing with k6 | |
| 10. Optimization loop to full scale | |

Details in [`ROADMAP.md`](ROADMAP.md).

## License

MIT
