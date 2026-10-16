-- S2: settings that inherit. One row per value set at a scope; the typed
-- registry in settings.rs says which keys exist, their type, the scopes
-- they may be set at, and the built-in default. A value resolves from a
-- launch override, then the project, then the person, then the workspace,
-- then the default.
CREATE TABLE settings (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK (scope IN ('workspace', 'project', 'user')),
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    actor_id UUID REFERENCES actors(id),
    key TEXT NOT NULL CHECK (
        char_length(key) BETWEEN 1 AND 120
        AND key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'
    ),
    value JSONB NOT NULL CHECK (octet_length(value::text) <= 16384),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((scope = 'project') = (project_id IS NOT NULL)),
    CHECK ((scope = 'user') = (actor_id IS NOT NULL))
);

CREATE UNIQUE INDEX settings_workspace_key
    ON settings (organization_id, key) WHERE scope = 'workspace';

CREATE UNIQUE INDEX settings_project_key
    ON settings (project_id, key) WHERE scope = 'project';

CREATE UNIQUE INDEX settings_user_key
    ON settings (organization_id, actor_id, key) WHERE scope = 'user';
