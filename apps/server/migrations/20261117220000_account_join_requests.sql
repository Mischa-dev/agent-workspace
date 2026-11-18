-- A2.18 (K1 3.11): an agent or app owned outside a workspace that asks an
-- admin first waits here, with the role and name it was added with, until
-- the approval request (subject `account_join`) is settled.
CREATE TABLE account_join_requests (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    asked_by_account_id UUID NOT NULL REFERENCES actors(id),
    role TEXT NOT NULL,
    role_id UUID,
    display_name TEXT NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120),
    handle TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'joined', 'refused', 'expired', 'cancelled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    settled_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX account_join_requests_one_pending
    ON account_join_requests (workspace_id, account_id) WHERE status = 'pending';
