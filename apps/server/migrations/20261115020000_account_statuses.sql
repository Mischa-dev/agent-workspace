-- C2.15: one status for every account (people and agents alike). A status
-- set without a workspace is the account's own, shown in every workspace it
-- is in; one set for a workspace (an agent's line about work there) shows
-- only in that workspace. A2's columns (state, the paused_* ones, set_by)
-- are here so agents need no table of their own (R12, R19).

CREATE TABLE account_statuses (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- NULL: the account's own status, in every workspace (R19).
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    emoji TEXT CHECK (emoji IS NULL OR (char_length(emoji) BETWEEN 1 AND 16
                                        AND emoji !~ '[[:cntrl:][:space:]]')),
    text TEXT CHECK (text IS NULL OR (char_length(text) BETWEEN 1 AND 120
                                      AND text !~ '[[:cntrl:]]')),
    state TEXT CHECK (state IS NULL OR state IN
        ('available', 'working', 'asleep', 'paused', 'unresponsive')),
    clear_at TIMESTAMPTZ,
    away_until TIMESTAMPTZ,
    set_by TEXT NOT NULL DEFAULT 'self' CHECK (set_by IN ('self', 'host', 'workspace')),
    set_by_account_id UUID REFERENCES actors(id),
    paused_by_account_id UUID REFERENCES actors(id),
    paused_reason TEXT CHECK (paused_reason IS NULL OR char_length(paused_reason) <= 200),
    paused_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (account_id, workspace_id)
);

-- The clearing job finds what is due without scanning every status.
CREATE INDEX account_statuses_clear_due
    ON account_statuses (clear_at)
    WHERE clear_at IS NOT NULL;
