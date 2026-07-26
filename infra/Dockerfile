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
RUN mkdir -p crates/oxid-api/src crates/oxid-shared/src crates/oxid-web/src \
    && echo 'fn main() {}' > crates/oxid-api/src/main.rs \
    && touch crates/oxid-api/src/lib.rs crates/oxid-shared/src/lib.rs \
    && echo 'fn main() {}' > crates/oxid-web/src/main.rs \
    && cargo build --release -p oxid-api \
    && rm -rf crates/*/src

COPY .sqlx .sqlx
COPY crates crates

# Touch the sources so cargo does not reuse the stub artifacts from the layer above.
RUN touch crates/oxid-api/src/main.rs crates/oxid-api/src/lib.rs \
    && cargo build --release -p oxid-api

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
COPY configuration configuration

EXPOSE 3000
ENV APP_ENVIRONMENT=production

ENTRYPOINT ["/usr/local/bin/oxid"]
