-- A9: an agent is a profile people create (often from a template such as
-- Reviewer): a name, an avatar, a harness and its default model settings,
-- instructions, allowed tools and an environment. Each profile is its own
-- execution worker in its project, so people tag it and its replies post as
-- it.
CREATE TABLE agent_profiles (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL UNIQUE REFERENCES actors(id),
    execution_worker_id UUID NOT NULL UNIQUE REFERENCES execution_workers(id),
    template TEXT CHECK (template IS NULL OR template ~ '^[a-z][a-z0-9_]{0,31}$'),
    -- An emoji or up to two letters shown in the agent's avatar.
    avatar TEXT CHECK (avatar IS NULL OR char_length(avatar) BETWEEN 1 AND 8),
    color TEXT CHECK (color IS NULL OR color ~ '^[a-z]{3,12}$'),
    description TEXT CHECK (description IS NULL OR char_length(description) <= 500),
    -- Null runs on the project's default harness.
    harness TEXT CHECK (harness IS NULL OR harness ~ '^[a-z][a-z0-9_]{0,63}$'),
    -- Model, reasoning and speed, as a run's settings take them.
    run_defaults JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(run_defaults) = 'object' AND octet_length(run_defaults::text) <= 4096
    ),
    instructions TEXT NOT NULL DEFAULT '' CHECK (char_length(instructions) <= 8000),
    allowed_tools TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(allowed_tools) <= 64),
    environment_id UUID,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, environment_id) REFERENCES project_environments(project_id, id)
);

CREATE INDEX agent_profiles_project ON agent_profiles (project_id, created_at, id);

-- A launch or run started for a profile (a tag, or picked in the Launch
-- dialog) carries it, so the run uses its settings and posts as it.
ALTER TABLE launches ADD COLUMN agent_profile_id UUID REFERENCES agent_profiles(id) ON DELETE SET NULL;
