-- S4 and S8: manage the workspace, and read its audit log.

-- Workspace roles used to follow project roles only. A role an owner sets
-- on the Members page sticks: project role changes no longer move it.
ALTER TABLE organization_memberships
    ADD COLUMN role_explicit BOOLEAN NOT NULL DEFAULT false;

-- The workspace a person last chose. Messages (the direct space) lives
-- there; unset or no longer a member, the earliest one is used.
ALTER TABLE users
    ADD COLUMN default_organization_id UUID REFERENCES organizations(id);

-- Invitations to the workspace rather than to one project. Accepting one
-- makes the person a workspace member; admins then add them to projects.
CREATE TABLE workspace_invitations (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    note TEXT CHECK (note IS NULL OR (char_length(note) BETWEEN 1 AND 200 AND note !~ '[[:cntrl:]]')),
    token_prefix TEXT NOT NULL UNIQUE CHECK (
        char_length(token_prefix) = 16
        AND token_prefix ~ '^aw_wsi_[A-Za-z0-9_-]{9}$'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'accepted', 'revoked')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    issued_by_actor_id UUID NOT NULL REFERENCES actors(id),
    accepted_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at),
    CHECK ((status = 'accepted') = (accepted_by_actor_id IS NOT NULL AND accepted_at IS NOT NULL)),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL))
);

CREATE INDEX workspace_invitations_organization
    ON workspace_invitations (organization_id, created_at DESC);

-- The audit log is read per workspace. Events about a project take its
-- workspace; workspace-level events say theirs in metadata or set it.
ALTER TABLE audit_events
    ADD COLUMN organization_id UUID REFERENCES organizations(id);

UPDATE audit_events event
SET organization_id = project.organization_id
FROM projects project
WHERE project.id = event.project_id AND event.organization_id IS NULL;

UPDATE audit_events event
SET organization_id = organization.id
FROM organizations organization
WHERE event.organization_id IS NULL
  AND event.metadata ? 'organizationId'
  AND event.metadata->>'organizationId' = organization.id::text;

CREATE FUNCTION audit_event_organization() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.organization_id IS NULL AND NEW.project_id IS NOT NULL THEN
        SELECT organization_id INTO NEW.organization_id FROM projects WHERE id = NEW.project_id;
    END IF;
    IF NEW.organization_id IS NULL
       AND NEW.metadata ? 'organizationId'
       AND NEW.metadata->>'organizationId' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
        SELECT id INTO NEW.organization_id FROM organizations
        WHERE id = (NEW.metadata->>'organizationId')::uuid;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER audit_events_organization
BEFORE INSERT ON audit_events
FOR EACH ROW EXECUTE FUNCTION audit_event_organization();

CREATE INDEX audit_events_organization_occurred
    ON audit_events (organization_id, occurred_at DESC, id DESC)
    WHERE organization_id IS NOT NULL;
