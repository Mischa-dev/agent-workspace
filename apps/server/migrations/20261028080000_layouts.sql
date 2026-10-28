-- AWD-66: layouts, which parts of the app each person sees.
--
-- Visual only: hiding a part never changes what anyone may do. The parts and
-- the built-in presets live in apps/web/src/experience/registry.json. The
-- workspace's default layout, each project's own layout, each person's "My
-- view" and their "Show hidden parts" switch are settings (layout.default,
-- layout.my_view, layout.show_hidden). This adds the two things settings
-- can't hold:
--
-- * layout_presets: a workspace's own presets ("Save as preset"), each a
--   built-in preset with parts turned on or off and nouns renamed. Removing
--   one only hides it from pickers, so layouts that use it keep working.
-- * project_layout_rules: owners and admins give a role in a project, or one
--   person in it, a preset. A rule names a built-in preset or one of the
--   same workspace's saved presets, never another workspace's.

CREATE TABLE layout_presets (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 60 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 200 AND description !~ '[[:cntrl:]]'
    ),
    -- The built-in preset it starts from.
    base TEXT NOT NULL CHECK (base ~ '^[a-z][a-z0-9_]{0,39}$'),
    -- Part id -> shown, on top of the base.
    changes JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(changes) = 'object' AND octet_length(changes::text) <= 8000
    ),
    -- Noun -> {"one", "many"}.
    words JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(words) = 'object' AND octet_length(words::text) <= 4000
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    removed_at TIMESTAMPTZ,
    -- Lets a rule's foreign key require the same workspace.
    UNIQUE (id, organization_id)
);

CREATE INDEX layout_presets_organization
    ON layout_presets (organization_id, created_at)
    WHERE removed_at IS NULL;

CREATE UNIQUE INDEX layout_presets_name
    ON layout_presets (organization_id, lower(name))
    WHERE removed_at IS NULL;

CREATE TABLE project_layout_rules (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    -- Who it's for: a role in the project, or one person.
    role TEXT CHECK (role IN ('admin', 'member', 'viewer')),
    actor_id UUID REFERENCES actors(id),
    -- Which preset: a built-in one, or one this workspace saved.
    builtin_preset TEXT CHECK (builtin_preset ~ '^[a-z][a-z0-9_]{0,39}$'),
    saved_preset_id UUID,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((role IS NULL) <> (actor_id IS NULL)),
    CHECK ((builtin_preset IS NULL) <> (saved_preset_id IS NULL)),
    FOREIGN KEY (saved_preset_id, organization_id)
        REFERENCES layout_presets (id, organization_id)
);

CREATE UNIQUE INDEX project_layout_rules_role
    ON project_layout_rules (project_id, role)
    WHERE role IS NOT NULL;

CREATE UNIQUE INDEX project_layout_rules_person
    ON project_layout_rules (project_id, actor_id)
    WHERE actor_id IS NOT NULL;

CREATE INDEX project_layout_rules_saved_preset
    ON project_layout_rules (saved_preset_id)
    WHERE saved_preset_id IS NOT NULL;
