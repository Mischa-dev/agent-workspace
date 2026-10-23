-- Bug report links: other apps send bug reports straight into a project.
--
-- A link is a secret URL for one project. Anything that can POST to it (a
-- curl line, a fetch in a game's crash handler, a form) creates a task
-- there. The task is created as the person who made the link, through the
-- same task tools the app uses, and every report checks that person's
-- current access first; a link whose maker lost access turns itself off
-- with the reason. Only the token's SHA-256 digest is stored: the full link
-- is shown once, and replacing it makes a new one.
CREATE TABLE bug_report_links (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    owner_actor_id UUID NOT NULL REFERENCES actors(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    -- The token's last four characters, so people can tell links apart.
    token_hint TEXT NOT NULL CHECK (token_hint ~ '^[A-Za-z0-9_-]{4}$'),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'paused', 'disabled')),
    disabled_reason TEXT CHECK (disabled_reason IS NULL OR char_length(disabled_reason) <= 500),
    report_count BIGINT NOT NULL DEFAULT 0 CHECK (report_count >= 0),
    last_report_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX bug_report_links_project ON bug_report_links (project_id, created_at DESC);
