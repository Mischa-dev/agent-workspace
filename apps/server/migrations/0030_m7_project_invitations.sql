-- M7 private-host Project invitation links and optimistic membership versions.

ALTER TABLE projects
    ADD CONSTRAINT projects_organization_id_id_unique
        UNIQUE (organization_id, id);

ALTER TABLE project_memberships
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);

CREATE TABLE project_invitations (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL,
    project_id UUID NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member', 'viewer')),
    token_prefix TEXT NOT NULL UNIQUE CHECK (
        char_length(token_prefix) = 16
        AND token_prefix ~ '^aw_inv_[A-Za-z0-9_-]{9}$'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    status TEXT NOT NULL DEFAULT 'active' CHECK (
        status IN ('active', 'accepted', 'revoked', 'replaced')
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    issued_by_actor_id UUID NOT NULL,
    accepted_by_actor_id UUID,
    replaced_by_invitation_id UUID UNIQUE REFERENCES project_invitations(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (organization_id, project_id)
        REFERENCES projects(organization_id, id),
    FOREIGN KEY (project_id, issued_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, accepted_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        expires_at > created_at
        AND expires_at <= created_at + interval '7 days'
    ),
    CHECK (
        (status = 'active'
            AND accepted_by_actor_id IS NULL
            AND accepted_at IS NULL
            AND revoked_at IS NULL
            AND replaced_by_invitation_id IS NULL)
        OR (status = 'accepted'
            AND accepted_by_actor_id IS NOT NULL
            AND accepted_at IS NOT NULL
            AND revoked_at IS NULL
            AND replaced_by_invitation_id IS NULL)
        OR (status = 'revoked'
            AND accepted_by_actor_id IS NULL
            AND accepted_at IS NULL
            AND revoked_at IS NOT NULL
            AND replaced_by_invitation_id IS NULL)
        OR (status = 'replaced'
            AND accepted_by_actor_id IS NULL
            AND accepted_at IS NULL
            AND revoked_at IS NOT NULL
            AND replaced_by_invitation_id IS NOT NULL)
    )
);

CREATE INDEX project_invitations_project_created
    ON project_invitations (project_id, created_at DESC, id DESC);

CREATE INDEX project_invitations_active_expiry
    ON project_invitations (expires_at)
    WHERE status = 'active';
