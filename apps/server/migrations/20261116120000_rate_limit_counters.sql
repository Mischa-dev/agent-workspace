-- K5.6: request budgets shared by every server instance.
--
-- One row per bucket and key (a SHA-256 digest; the key itself is never
-- stored). A window starts with the key's first use and ends at
-- expires_at; the next use after that starts a new one. Rows for "seen
-- lately" (a name that signed in, a token that resolved) use the same shape
-- with expires_at at the end of the time they count.
--
-- Unlogged: the counters only matter for minutes, and a crash or failover
-- resetting them is the stated bound (better than every restart, as
-- before). A job deletes ended rows every minute.
CREATE UNLOGGED TABLE rate_limit_counters (
    bucket TEXT NOT NULL CHECK (bucket ~ '^[a-z][a-z0-9_.]{0,62}$'),
    key_digest BYTEA NOT NULL CHECK (octet_length(key_digest) = 32),
    window_start TIMESTAMPTZ NOT NULL,
    window_seconds INTEGER NOT NULL CHECK (window_seconds BETWEEN 1 AND 604800),
    used INTEGER NOT NULL CHECK (used >= 0),
    expires_at TIMESTAMPTZ NOT NULL,
    -- Set for per-workspace buckets, for clean-up and isolation.
    workspace_id UUID,
    PRIMARY KEY (bucket, key_digest)
);

CREATE INDEX rate_limit_counters_expiry ON rate_limit_counters (expires_at);
