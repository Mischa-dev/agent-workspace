-- A2.19 (A2 3.9, rulings R19 and R21): an agent a host made and proposed
-- for a workspace waits here, with the profile and the host's name for it,
-- until its approval request (subject `agent_register`) is settled. A host
-- retrying with the same client_request_id gets the same proposal.
CREATE TABLE agent_proposals (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    host_id UUID NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    host_agent_ref TEXT NOT NULL CHECK (char_length(host_agent_ref) BETWEEN 1 AND 200),
    display_name TEXT NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 80),
    handle TEXT NOT NULL CHECK (char_length(handle) BETWEEN 1 AND 40),
    summary TEXT CHECK (summary IS NULL OR char_length(summary) <= 280),
    ref_issuer TEXT CHECK (ref_issuer IS NULL OR char_length(ref_issuer) <= 200),
    ref_id TEXT CHECK (ref_id IS NULL OR char_length(ref_id) <= 200),
    ref_lineage TEXT CHECK (ref_lineage IS NULL OR char_length(ref_lineage) <= 200),
    asked_account_id UUID REFERENCES actors(id),
    client_request_id TEXT NOT NULL CHECK (char_length(client_request_id) BETWEEN 1 AND 200),
    request_id UUID REFERENCES requests(id),
    account_id UUID REFERENCES actors(id),
    status TEXT NOT NULL DEFAULT 'proposed'
        CHECK (status IN ('proposed', 'approved', 'refused', 'expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    settled_at TIMESTAMPTZ,
    UNIQUE (host_id, client_request_id)
);

CREATE INDEX agent_proposals_workspace ON agent_proposals (workspace_id);
SELECT tenancy_protect('agent_proposals', 'workspace_id');

-- An action a person ran on a host (A2.19): only they read its reply.
ALTER TABLE host_commands ADD COLUMN invoked_by_account_id UUID REFERENCES actors(id);

-- A2.18's waiting joins get the same row rules.
SELECT tenancy_protect('account_join_requests', 'workspace_id');
