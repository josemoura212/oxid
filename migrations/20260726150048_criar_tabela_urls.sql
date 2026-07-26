CREATE FUNCTION url_sha256(url text) RETURNS bytea
    LANGUAGE sql
    IMMUTABLE
    STRICT
    PARALLEL SAFE
    RETURN sha256(convert_to(url, 'UTF8'));

CREATE TABLE urls (
    id BIGSERIAL PRIMARY KEY,
    long_url TEXT NOT NULL CHECK (length(long_url) <= 2048),
    url_hash BYTEA NOT NULL GENERATED ALWAYS AS (url_sha256(long_url)) STORED UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
