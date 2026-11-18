-- C2.18 (data from K1.27, K1 plan 3.4): user groups, so `@design` mentions
-- everyone in the group who can read the conversation. Lane A builds the
-- group operations, the members.groups permission and role grants to
-- groups on these tables; chat reads them for mentions.

CREATE TABLE account_groups (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    handle TEXT NOT NULL CHECK (handle ~ '^[a-z][a-z0-9_-]{1,31}$'),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 300),
    created_by_actor_id UUID REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,
    archived_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, handle)
);
SELECT tenancy_protect('account_groups', 'workspace_id');

CREATE TABLE account_group_members (
    group_id UUID NOT NULL REFERENCES account_groups(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    added_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, account_id)
);
CREATE INDEX account_group_members_by_account ON account_group_members (account_id);
CREATE INDEX account_group_members_by_workspace ON account_group_members (workspace_id);
SELECT tenancy_protect('account_group_members', 'workspace_id');
