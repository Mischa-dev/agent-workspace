-- S5: connect GitHub through the App's install flow instead of typed IDs.

-- One attempt to connect: a random state handed to GitHub, bound to the
-- person, the project and the browser that started it (a cookie only that
-- browser holds), used once and soon.
CREATE TABLE github_connect_attempts (
    state_digest BYTEA PRIMARY KEY CHECK (octet_length(state_digest) = 32),
    browser_digest BYTEA NOT NULL CHECK (octet_length(browser_digest) = 32),
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);

CREATE INDEX github_connect_attempts_expiry ON github_connect_attempts (expires_at);

-- Installations a workspace proved access to: someone in it signed in to
-- GitHub and GitHub listed the installation as theirs. Only these can be
-- linked to the workspace's projects, so knowing an installation's number
-- is no longer enough.
CREATE TABLE github_installation_claims (
    organization_id UUID NOT NULL REFERENCES organizations(id),
    installation_id BIGINT NOT NULL CHECK (installation_id > 0),
    account_login TEXT NOT NULL CHECK (
        char_length(account_login) BETWEEN 1 AND 100
        AND account_login !~ '[[:cntrl:]]'
    ),
    target_type TEXT NOT NULL CHECK (target_type IN ('User', 'Organization')),
    claimed_by_actor_id UUID REFERENCES actors(id),
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, installation_id)
);

-- Installations already linked by typing their number keep working.
INSERT INTO github_installation_claims (
    organization_id, installation_id, account_login, target_type, claimed_at
)
SELECT DISTINCT ON (project.organization_id, installation.installation_id)
       project.organization_id, installation.installation_id,
       installation.account_login, installation.target_type, installation.created_at
FROM github_installations installation
JOIN projects project ON project.id = installation.project_id
ORDER BY project.organization_id, installation.installation_id, installation.created_at
ON CONFLICT DO NOTHING;
