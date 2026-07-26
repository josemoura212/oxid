# Production image. Development lives in Dockerfile.dev.
#
# Two stages so the toolchain never reaches the runtime: the builder is ~2 GB and
# carries the CVEs of a full Rust install; the final image ships one static binary.

FROM rust:1.97-bookworm AS builder

WORKDIR /build

# Compile against the committed .sqlx cache instead of a live database — the
# builder has no network access to Postgres.
ENV SQLX_OFFLINE=true

# Dependencies first, in their own layer: they change far less often than the
# source, so editing a handler does not rebuild the whole dependency tree.
COPY Cargo.toml Cargo.lock ./
COPY crates/oxid-api/Cargo.toml crates/oxid-api/
COPY crates/oxid-shared/Cargo.toml crates/oxid-shared/
COPY crates/oxid-web/Cargo.toml crates/oxid-web/
# Every target declared in a Cargo.toml needs a file here, or cargo refuses to
# resolve — including `src/bin/`, which is easy to forget when a binary is added.
RUN mkdir -p crates/oxid-api/src/bin crates/oxid-shared/src crates/oxid-web/src \
    && echo 'fn main() {}' > crates/oxid-api/src/main.rs \
    && echo 'fn main() {}' > crates/oxid-api/src/bin/migrate.rs \
    && touch crates/oxid-api/src/lib.rs crates/oxid-shared/src/lib.rs \
    && echo 'fn main() {}' > crates/oxid-web/src/main.rs \
    && cargo build --release -p oxid-api --bins \
    && rm -rf crates/*/src

COPY .sqlx .sqlx
COPY migrations migrations
COPY crates crates

# Touch every source, not just the entry points: cargo decides what to rebuild by
# mtime, and COPY preserves the timestamps from the build context. Miss one crate
# and it silently links the empty stub compiled in the layer above — which shows
# up as "no `ShortenResponse` in the root", not as a cache problem.
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release -p oxid-api --bins

# ---

FROM debian:bookworm-slim

# ca-certificates is required for rustls to validate TLS to Postgres.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged: a shortener has no reason to run anything as root.
RUN useradd --system --uid 10001 --no-create-home oxid
USER 10001

WORKDIR /app
COPY --from=builder /build/target/release/oxid /usr/local/bin/oxid
# Migrations run as a Job before the rollout, from this same image — so the
# schema and the code that expects it are always the same build.
COPY --from=builder /build/target/release/oxid-migrate /usr/local/bin/oxid-migrate
COPY configuration configuration

EXPOSE 3000
ENV APP_ENVIRONMENT=production

ENTRYPOINT ["/usr/local/bin/oxid"]
