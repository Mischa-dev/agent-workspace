-- M1.9: package preview and install (M1 section 3.6).
--
-- * packages: the packages this workspace has seen, in their inline form
--   (one per id and version).
-- * package_installs: each install, with the plan the installer approved
--   and its hash (an install whose plan changed since the preview is
--   refused).
-- * package_objects: what each install made or reused, by the part's key,
--   for removal and (M1.10) upgrades.
-- * agent_templates: agent templates a package brings. A template is a
--   starting point for "Add an agent", never an account.
-- * modules.package_install_id: the install that made a built module.

CREATE TABLE packages (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    package_key TEXT NOT NULL CHECK (
        package_key ~ '^[a-z][a-z0-9_]{0,63}(\.[a-z][a-z0-9_]{0,63})?$'
    ),
    version TEXT NOT NULL CHECK (char_length(version) BETWEEN 1 AND 40),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    document JSONB NOT NULL CHECK (
        jsonb_typeof(document) = 'object' AND octet_length(document::text) <= 5242880
    ),
    digest TEXT NOT NULL CHECK (char_length(digest) <= 100),
    source TEXT NOT NULL CHECK (
        source IN ('builtin', 'directory', 'upload', 'export', 'legacy_template')
    ),
    source_ref TEXT CHECK (source_ref IS NULL OR char_length(source_ref) <= 2000),
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, package_key, version)
);

CREATE TABLE package_installs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    package_id UUID NOT NULL REFERENCES packages(id),
    state TEXT NOT NULL CHECK (
        state IN ('installing', 'installed', 'failed', 'superseded', 'removed')
    ),
    plan JSONB NOT NULL CHECK (octet_length(plan::text) <= 1048576),
    plan_hash TEXT NOT NULL CHECK (char_length(plan_hash) <= 100),
    summary TEXT[] NOT NULL DEFAULT '{}',
    installed_by_actor_id UUID NOT NULL REFERENCES actors(id),
    via_app_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX package_installs_workspace ON package_installs (workspace_id, created_at DESC);
CREATE INDEX package_installs_space ON package_installs (space_id) WHERE space_id IS NOT NULL;

CREATE TABLE package_objects (
    install_id UUID NOT NULL REFERENCES package_installs(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    package_key TEXT NOT NULL CHECK (char_length(package_key) BETWEEN 1 AND 100),
    object_kind TEXT NOT NULL CHECK (char_length(object_kind) BETWEEN 1 AND 40),
    object_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('created', 'reused', 'updated')),
    installed_hash TEXT NOT NULL CHECK (char_length(installed_hash) <= 100),
    PRIMARY KEY (install_id, package_key, object_kind)
);

CREATE INDEX package_objects_workspace ON package_objects (workspace_id);

CREATE TABLE agent_templates (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    install_id UUID REFERENCES package_installs(id) ON DELETE SET NULL,
    template_key TEXT NOT NULL CHECK (template_key ~ '^[a-z][a-z0-9_]{0,63}$'),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    purpose TEXT NOT NULL DEFAULT '' CHECK (char_length(purpose) <= 500),
    definition JSONB NOT NULL CHECK (
        jsonb_typeof(definition) = 'object' AND octet_length(definition::text) <= 65536
    ),
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX agent_templates_key ON agent_templates (workspace_id, template_key)
    WHERE archived_at IS NULL;

ALTER TABLE modules
    ADD COLUMN package_install_id UUID REFERENCES package_installs(id) ON DELETE SET NULL;
