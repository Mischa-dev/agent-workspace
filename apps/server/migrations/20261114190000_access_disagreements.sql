-- K1.2: old and new access checks side by side (K1 plan, section 4.3).
--
-- While an area moves onto the access module it runs in compare mode: the
-- old check decides and the new one runs beside it. Each difference is one
-- row per fingerprint (area, operation, account, permission, kind of
-- target, the two answers), counted, so an operator sees what would change
-- before the area switches. Ids, keys and kinds only, never content. Rows
-- older than 30 days are dropped by the database retention job.

CREATE TABLE access_disagreements (
    id UUID PRIMARY KEY,
    area TEXT NOT NULL CHECK (char_length(area) BETWEEN 1 AND 40),
    operation TEXT NOT NULL CHECK (char_length(operation) BETWEEN 1 AND 120),
    -- Where the check was made: the workspace (and space) located for it.
    -- NULL for a call about the account's own things.
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),
    -- For analysis only; never read by a decision.
    account_kind TEXT,
    permission TEXT NOT NULL CHECK (char_length(permission) BETWEEN 1 AND 120),
    target_kind TEXT,
    target_id UUID,
    old_allowed BOOLEAN NOT NULL,
    new_allowed BOOLEAN NOT NULL,
    new_reason TEXT CHECK (new_reason IS NULL OR char_length(new_reason) <= 120),
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    count BIGINT NOT NULL DEFAULT 1 CHECK (count > 0),
    fingerprint BYTEA NOT NULL UNIQUE CHECK (octet_length(fingerprint) = 32)
);

CREATE INDEX access_disagreements_workspace
    ON access_disagreements (workspace_id, last_seen_at DESC);
CREATE INDEX access_disagreements_last_seen ON access_disagreements (last_seen_at);
