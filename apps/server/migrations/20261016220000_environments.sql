-- C1: a project's environments. An environment says where a run executes:
-- a harness (this host's Codex, Codex Cloud, a connected runner) plus, for a
-- provider's cloud, the provider's own environment ID, and the repository
-- and branch it works on. The project's default is the `run.environment`
-- setting (S2), which holds an environment's ID.
CREATE TABLE project_environments (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    harness TEXT NOT NULL CHECK (harness ~ '^[a-z][a-z0-9_]{0,63}$'),
    provider_environment_id TEXT CHECK (
        provider_environment_id IS NULL
        OR (
            char_length(provider_environment_id) BETWEEN 1 AND 200
            AND provider_environment_id !~ '[[:cntrl:]]'
        )
    ),
    repository_id UUID,
    branch TEXT CHECK (
        branch IS NULL OR (char_length(branch) BETWEEN 1 AND 200 AND branch !~ '[[:cntrl:][:space:]]')
    ),
    setup_notes TEXT NOT NULL DEFAULT '' CHECK (char_length(setup_notes) <= 4000),
    last_checked_at TIMESTAMPTZ,
    last_check_ok BOOLEAN,
    last_check_message TEXT CHECK (
        last_check_message IS NULL OR char_length(last_check_message) <= 300
    ),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    archived_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, repository_id) REFERENCES repositories(project_id, id)
);

CREATE UNIQUE INDEX project_environments_active_name
    ON project_environments (project_id, lower(name)) WHERE archived_at IS NULL;

-- The workspace of an environment is its project's, always.
CREATE FUNCTION project_environments_match_workspace()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM projects
        WHERE id = NEW.project_id AND organization_id = NEW.organization_id
    ) THEN
        RAISE EXCEPTION 'Environment workspace must match its project' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER project_environments_match_workspace
    BEFORE INSERT OR UPDATE OF organization_id, project_id ON project_environments
    FOR EACH ROW EXECUTE FUNCTION project_environments_match_workspace();
