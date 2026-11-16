-- H1.16 (build 1H): leases for server-wide maintenance that must not
-- overlap, such as a database backup and a master key rotation. A lease is a
-- row with a holder and an expiry on the database's clock, taken by a
-- conditional insert, renewed while the work runs and released at the end,
-- so it holds through a transaction-mode pooler where a session advisory
-- lock would not (R24). Global: the work is the server's, not a workspace's.
-- K5.2's `leases` table takes these names over.

CREATE TABLE maintenance_leases (
    name TEXT PRIMARY KEY CHECK (
        char_length(name) <= 80 AND name ~ '^[a-z][a-z0-9_.-]*$'
    ),
    holder UUID NOT NULL,
    acquired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at >= acquired_at)
);
