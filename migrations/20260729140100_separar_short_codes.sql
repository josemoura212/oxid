-- Splits what `urls` was doing twice: storing the long URL, and defining the
-- shortcode. They need different uniqueness rules.
--
-- `urls` keeps its global dedupe, so a long URL is stored once no matter how
-- many people shorten it — that is what makes 365 billion rows plausible.
-- `short_codes` gives each (owner, url) pair its own code, which is what makes
-- per-owner click counts possible at all: without it two owners share one code
-- and there is nothing to attribute a click to.

CREATE TABLE short_codes (
    id BIGSERIAL PRIMARY KEY,
    url_id BIGINT NOT NULL REFERENCES urls (id),
    -- NULL means anonymous. Deliberately not a sentinel user row: "no owner" is
    -- an absence, and encoding it as a fake account would leak into every join.
    owner_id BIGINT REFERENCES users (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- NULLS NOT DISTINCT is load-bearing. A plain UNIQUE treats NULL as never
    -- equal to NULL, so it would happily allow two anonymous codes for the same
    -- URL — silently breaking the idempotence that works today. Nothing errors;
    -- duplicates just start accumulating.
    CONSTRAINT short_codes_owner_url_key UNIQUE NULLS NOT DISTINCT (owner_id, url_id)
);

-- The listing: one owner's codes, newest first.
CREATE INDEX short_codes_owner_created_idx ON short_codes (owner_id, created_at DESC);

-- Every published link has to keep resolving, so each existing row takes the id
-- it already has. The code is a pure function of this id, so reusing it is the
-- difference between a migration nobody notices and every link in the wild
-- changing meaning at once.
INSERT INTO short_codes (id, url_id, owner_id, created_at)
SELECT id, id, NULL, created_at FROM urls;

-- Without this the sequence still starts at 1 and the next insert collides with
-- a row that was just backfilled. From here the two sequences diverge on
-- purpose: `urls.id` counts distinct URLs, `short_codes.id` counts codes.
SELECT setval(
    pg_get_serial_sequence('short_codes', 'id'),
    (SELECT COALESCE(max(id), 1) FROM short_codes)
);
