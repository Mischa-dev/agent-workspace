-- M1.6: built modules. A built module is a named bundle of existing parts
-- (collections, forms, automations, pages, navigation entries) that can be
-- switched off as one, hiding every part and keeping the data, and
-- archived (parts archived with it, or kept as plain objects).
--
-- * modules: one built module per key in a workspace, optionally in one
--   space. `package_install_id` arrives with M1.9's package_installs.
-- * module_parts: which module owns which object. One owner per object; a
--   nav entry is a part too (part_kind 'nav', its id a module_nav row).
-- * module_nav: a module's navigation entries.
-- * module_part_on(kind, id): whether an object is visible as far as built
--   modules go (not a part of a module that is switched off, in the
--   workspace or its space, or archived with its parts). Readers of
--   collections, records, forms and automations add it.
-- * permission_catalogue: modules.build and modules.manage (K1's
--   catalogue), owners and admins by default, grantable to any role.

CREATE TABLE modules (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL CHECK (
        module_key ~ '^[a-z][a-z0-9_]{0,39}\.[a-z][a-z0-9_]{0,39}$'
    ),
    kind TEXT NOT NULL DEFAULT 'built' CHECK (kind IN ('built', 'connected')),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 60 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    icon TEXT CHECK (icon IS NULL OR icon ~ '^[a-z0-9-]{1,40}$'),
    description_doc JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(description_doc) = 'object'
        AND octet_length(description_doc::text) <= 262144
    ),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    -- How it was archived: its parts archived with it, or kept as plain
    -- objects (then they are no longer its parts).
    archived_parts TEXT CHECK (archived_parts IN ('archive', 'keep')),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ,
    CHECK ((archived_at IS NULL) = (archived_parts IS NULL))
);

CREATE UNIQUE INDEX modules_key ON modules (workspace_id, module_key) WHERE archived_at IS NULL;
CREATE INDEX modules_space ON modules (space_id) WHERE space_id IS NOT NULL;

CREATE TABLE module_nav (
    id UUID PRIMARY KEY,
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK (
        char_length(label) BETWEEN 1 AND 40 AND label = btrim(label) AND label !~ '[[:cntrl:]]'
    ),
    icon TEXT CHECK (icon IS NULL OR icon ~ '^[a-z0-9-]{1,40}$'),
    target JSONB NOT NULL CHECK (
        jsonb_typeof(target) = 'object' AND octet_length(target::text) <= 2048
    ),
    position INT NOT NULL DEFAULT 0
);

CREATE INDEX module_nav_module ON module_nav (module_id, position);

CREATE TABLE module_parts (
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    part_kind TEXT NOT NULL CHECK (part_kind IN (
        'collection', 'form', 'automation', 'role', 'page', 'page_template', 'nav',
        'layout', 'skill', 'agent_template', 'channel'
    )),
    object_id UUID NOT NULL,
    package_key TEXT CHECK (package_key IS NULL OR package_key ~ '^[a-z][a-z0-9_]{0,63}$'),
    installed_hash TEXT CHECK (installed_hash IS NULL OR char_length(installed_hash) <= 100),
    position INT NOT NULL DEFAULT 0,
    PRIMARY KEY (module_id, part_kind, object_id),
    UNIQUE (part_kind, object_id)
);

CREATE INDEX module_parts_workspace ON module_parts (workspace_id);

-- Whether an object is on as far as built modules go: not a part of a
-- module switched off (in the workspace, or in the module's space) or
-- archived with its parts. An object in no module is always on.
CREATE FUNCTION module_part_on(kind TEXT, object UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT NOT EXISTS (
        SELECT 1
        FROM module_parts part
        JOIN modules module ON module.id = part.module_id
        -- $1 and $2, not the parameters' names: modules has a kind column,
        -- which a bare name would mean.
        WHERE part.part_kind = $1
          AND part.object_id = $2
          AND (
              module.archived_at IS NOT NULL
              OR EXISTS (
                  SELECT 1 FROM module_installs switch
                  WHERE switch.workspace_id = module.workspace_id
                    AND switch.module_key = module.module_key
                    AND switch.state = 'off'
                    AND (switch.space_id IS NULL OR switch.space_id = module.space_id)
              )
          )
    )
$$;

INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES
    ('modules.build', 'settings', 'both', false, ARRAY['owner', 'admin']::text[]),
    ('modules.manage', 'settings', 'both', false, ARRAY['owner', 'admin']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

-- Row security (H1.22): each table is protected by its workspace.
SELECT tenancy_protect('modules', 'workspace_id');
SELECT tenancy_protect('module_nav', 'workspace_id');
SELECT tenancy_protect('module_parts', 'workspace_id');
