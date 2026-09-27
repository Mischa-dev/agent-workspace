-- Provider IDs identify credentials; local actors and memberships own access.
CREATE TABLE workos_organizations (
    client_id TEXT NOT NULL,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    workos_organization_id TEXT NOT NULL,
    PRIMARY KEY (client_id, organization_id),
    UNIQUE (client_id, workos_organization_id)
);
CREATE TABLE workos_agent_bindings (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    connection_id UUID NOT NULL REFERENCES external_agent_connections(id),
    credential_id UUID NOT NULL REFERENCES mcp_client_credentials(id),
    kind TEXT NOT NULL CHECK (kind IN ('blueprint', 'registration')),
    subject_id TEXT NOT NULL CHECK (char_length(subject_id) BETWEEN 1 AND 100),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    FOREIGN KEY (client_id, organization_id) REFERENCES workos_organizations(client_id, organization_id),
    UNIQUE (client_id, subject_id)
);
CREATE UNIQUE INDEX workos_agent_one_active_binding
    ON workos_agent_bindings(client_id, connection_id, kind) WHERE revoked_at IS NULL;
