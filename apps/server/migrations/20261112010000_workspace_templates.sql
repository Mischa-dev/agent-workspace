-- AWD-76: workspace templates.
--
-- A template sets a workspace's modules, its default layout, its roles and
-- the role new members start with, and can bring a starter project with
-- channels and collections. The built-in templates live in the repository
-- (modules/templates/*.json); these tables hold the rest.
--
-- * workspace_templates: templates a workspace saved from itself ("Save
--   this workspace as a template"). `definition` is the same portable shape
--   the built-in ones use and workspace_template_list returns; the server
--   checks it when it's saved and again when it's used.
-- * workspace_template_uses: each time a template was used in a workspace,
--   by whom, and what it did. The setup path's first step is done once a
--   workspace has one.

CREATE TABLE workspace_templates (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 60 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 300 AND description !~ '[[:cntrl:]]'
    ),
    definition JSONB NOT NULL CHECK (
        jsonb_typeof(definition) = 'object' AND octet_length(definition::text) <= 200000
    ),
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX workspace_templates_name
    ON workspace_templates (organization_id, lower(name))
    WHERE archived_at IS NULL;

CREATE INDEX workspace_templates_organization
    ON workspace_templates (organization_id, created_at)
    WHERE archived_at IS NULL;

CREATE TABLE workspace_template_uses (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- A built-in template's id (such as "software_team"), or a saved one's ID.
    template TEXT NOT NULL CHECK (char_length(template) BETWEEN 1 AND 80),
    template_name TEXT NOT NULL CHECK (char_length(template_name) BETWEEN 1 AND 60),
    -- The starter project it made or reused, if any.
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    -- What it did, in words, as the person saw it.
    summary TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(summary) <= 100),
    used_by_actor_id UUID NOT NULL REFERENCES actors(id),
    used_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX workspace_template_uses_organization
    ON workspace_template_uses (organization_id, used_at DESC);
