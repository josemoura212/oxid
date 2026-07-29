-- `citext` makes the unique index case-insensitive at the storage layer, so
-- "Ana@x.com" and "ana@x.com" cannot become two accounts. Doing it in the
-- application would mean every lookup has to remember to lower() first, and one
-- forgotten call is a duplicate account.
CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email CITEXT NOT NULL UNIQUE,
    -- PHC string: algorithm, parameters and salt travel with the hash, so the
    -- cost can be raised later without invalidating existing passwords.
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
