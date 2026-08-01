-- Credentials for clients that are not a browser tab — the extension first.
--
-- Not the session cookie, and not because a cookie would be inelegant: an
-- extension does not share cookies with the site in any way that holds across
-- browsers. A token is also revocable on its own, so losing a laptop with the
-- extension installed does not mean signing out everywhere.
CREATE TABLE api_tokens (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- SHA-256 of the token, hex. Deliberately *not* Argon2, which guards this
    -- table's neighbour `users.password_hash` — the reasoning inverts here.
    --
    -- Argon2 is slow on purpose, to make guessing a low-entropy human secret
    -- expensive. A token is 256 bits from the OS random source: there is no
    -- dictionary to walk, so the slowness would buy nothing. It would also cost
    -- something real, because a per-row salt means you cannot *look a token up* —
    -- authentication would have to try every row. A plain digest is a key.
    --
    -- The column is still a hash, so the database never holds anything that can
    -- be replayed against the API.
    token_hash TEXT NOT NULL UNIQUE,

    -- What the person called it, so a list of tokens is a list of decisions
    -- ("laptop", "work phone") rather than of opaque ids they cannot tell apart
    -- when deciding which to revoke.
    name TEXT NOT NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Written on use, and the reason is revocation: "this one has not been used
    -- in eight months" is what turns an unfamiliar entry into a safe delete.
    -- Nullable because a token that was never used has no honest value here.
    last_used_at TIMESTAMPTZ
);

-- Every list is scoped to one owner, newest first — the same access pattern as
-- `short_codes`, and the same reason for the index.
CREATE INDEX api_tokens_owner_idx ON api_tokens (user_id, created_at DESC);
