-- AWD-74: roles are data.
--
-- * workspace_roles: every workspace's roles. The four built-in ones
--   (owner, admin, member, viewer) are rows in every workspace, made here for
--   the ones that exist and by a trigger for new ones. Their permissions live
--   in the server's code (policy.rs) so they mean exactly what they always
--   have; their name, description, defaults and agent limit can change. A
--   custom role starts from a built-in one ("base_role") and keeps a subset of
--   its permissions, so it can never do more than its base, here or in any
--   check that still reads the plain role.
-- * organization_memberships.role_id and project_memberships.role_id: the
--   custom role someone holds, or NULL for the built-in role named by `role`.
--   A trigger keeps `role` equal to the custom role's base, and clears
--   role_id when older code sets `role` directly.
-- * organization_memberships.agent_limit: what apps and agents acting for
--   this one person may do, narrower than their roles' limits. NULL leaves
--   the roles' limits as they are.
-- * project_layout_rules.role_id: a project's layout rule for a custom role.

CREATE TABLE workspace_roles (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    -- owner, admin, member or viewer for the built-in roles; a slug of the
    -- name for the others.
    key TEXT NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,39}$'),
    built_in BOOLEAN NOT NULL DEFAULT false,
    base_role TEXT NOT NULL CHECK (base_role IN ('owner', 'admin', 'member', 'viewer')),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 60 AND name = btrim(name) AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 300 AND description !~ '[[:cntrl:]]'
    ),
    -- Permission keys a custom role holds. Built-in roles keep none here:
    -- theirs come from the server's code.
    permissions TEXT[] NOT NULL DEFAULT '{}' CHECK (
        cardinality(permissions) <= 100
        AND array_position(permissions, NULL) IS NULL
    ),
    -- What apps and agents acting for holders may do. NULL: the same as the
    -- person.
    agent_limit TEXT[] CHECK (
        agent_limit IS NULL
        OR (cardinality(agent_limit) <= 100 AND array_position(agent_limit, NULL) IS NULL)
    ),
    -- Where holders start: {"layoutPreset", "channels", "pinnedViews",
    -- "homeHidden"}. Checked by the server.
    defaults JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(defaults) = 'object' AND octet_length(defaults::text) <= 8000
    ),
    -- The role invitations and new members start with.
    is_default BOOLEAN NOT NULL DEFAULT false,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID REFERENCES actors(id),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, key),
    UNIQUE (id, organization_id),
    CHECK (built_in = (key IN ('owner', 'admin', 'member', 'viewer'))),
    CHECK (NOT built_in OR (base_role = key AND cardinality(permissions) = 0)),
    -- Custom roles start from admin, member or viewer; nobody makes another
    -- owner.
    CHECK (built_in OR base_role <> 'owner'),
    -- The owner role's limit and defaults stay as they are: owners always
    -- hold everything.
    CHECK (key <> 'owner' OR (agent_limit IS NULL AND NOT is_default))
);

CREATE UNIQUE INDEX workspace_roles_one_default
    ON workspace_roles (organization_id)
    WHERE is_default;

CREATE UNIQUE INDEX workspace_roles_name
    ON workspace_roles (organization_id, lower(name));

CREATE FUNCTION create_built_in_workspace_roles(target UUID)
RETURNS void
LANGUAGE sql
AS $$
    INSERT INTO workspace_roles (id, organization_id, key, built_in, base_role, name, description, is_default)
    SELECT gen_random_uuid(), target, role.key, true, role.key, role.name, role.description,
           role.key = 'member'
    FROM (VALUES
        ('owner', 'Owner', 'Everything, including roles, ownership and the workspace itself. Every workspace keeps at least one.'),
        ('admin', 'Admin', 'Runs the workspace: settings, members, roles, code and every project.'),
        ('member', 'Member', 'Does the work: tasks, channels, pages, whiteboards, runs and pull requests.'),
        ('viewer', 'Viewer', 'Reads everything they''re given, and changes nothing.')
    ) AS role (key, name, description)
    ON CONFLICT (organization_id, key) DO NOTHING
$$;

SELECT create_built_in_workspace_roles(id) FROM organizations;

CREATE FUNCTION organizations_create_built_in_roles()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM create_built_in_workspace_roles(NEW.id);
    RETURN NEW;
END;
$$;

CREATE TRIGGER organizations_create_built_in_roles
AFTER INSERT ON organizations
FOR EACH ROW
EXECUTE FUNCTION organizations_create_built_in_roles();

-- A custom role's permissions; NULL for the built-in role (role_id NULL).
CREATE FUNCTION workspace_role_permissions(role UUID)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT permissions FROM workspace_roles WHERE id = role AND NOT built_in
$$;

-- For checks that read the plain role: whether a custom role (if any)
-- kept `permission`. The plain role already bounds it from above.
CREATE FUNCTION custom_role_allows(role UUID, permission TEXT)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT role IS NULL OR COALESCE(permission = ANY(workspace_role_permissions(role)), false)
$$;

ALTER TABLE organization_memberships
    ADD COLUMN role_id UUID REFERENCES workspace_roles(id),
    ADD COLUMN agent_limit TEXT[] CHECK (
        agent_limit IS NULL
        OR (cardinality(agent_limit) <= 100 AND array_position(agent_limit, NULL) IS NULL)
    );

ALTER TABLE project_memberships
    ADD COLUMN role_id UUID REFERENCES workspace_roles(id);

CREATE INDEX organization_memberships_role
    ON organization_memberships (role_id)
    WHERE role_id IS NOT NULL;

CREATE INDEX project_memberships_role
    ON project_memberships (role_id)
    WHERE role_id IS NOT NULL;

-- Keeps `role` in step with a custom role: the base role, except that a
-- workspace has no viewer role of its own, so a custom role based on viewer
-- is a member there (members and viewers may do the same at workspace
-- level). Code that sets `role` without touching role_id means a built-in
-- role, so the custom one is cleared.
CREATE FUNCTION memberships_sync_custom_role()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    target_organization UUID;
    custom workspace_roles%ROWTYPE;
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.role_id IS NOT DISTINCT FROM OLD.role_id
       AND NEW.role IS DISTINCT FROM OLD.role THEN
        NEW.role_id := NULL;
    END IF;
    IF NEW.role_id IS NULL THEN
        RETURN NEW;
    END IF;
    IF TG_TABLE_NAME = 'organization_memberships' THEN
        target_organization := NEW.organization_id;
    ELSE
        SELECT organization_id INTO target_organization FROM projects WHERE id = NEW.project_id;
    END IF;
    SELECT * INTO custom FROM workspace_roles WHERE id = NEW.role_id;
    IF NOT FOUND OR custom.organization_id <> target_organization THEN
        RAISE EXCEPTION 'role belongs to another workspace' USING ERRCODE = '23514';
    END IF;
    IF custom.built_in AND TG_TABLE_NAME = 'organization_memberships'
       AND custom.base_role = 'viewer' THEN
        RAISE EXCEPTION 'a workspace has no viewer role' USING ERRCODE = '23514';
    END IF;
    IF custom.built_in THEN
        -- A built-in role is named by `role` alone.
        NEW.role := custom.base_role;
        NEW.role_id := NULL;
        RETURN NEW;
    END IF;
    IF TG_TABLE_NAME = 'organization_memberships' AND custom.base_role = 'viewer' THEN
        NEW.role := 'member';
    ELSE
        NEW.role := custom.base_role;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER organization_memberships_sync_custom_role
BEFORE INSERT OR UPDATE OF role, role_id ON organization_memberships
FOR EACH ROW
EXECUTE FUNCTION memberships_sync_custom_role();

CREATE TRIGGER project_memberships_sync_custom_role
BEFORE INSERT OR UPDATE OF role, role_id ON project_memberships
FOR EACH ROW
EXECUTE FUNCTION memberships_sync_custom_role();

-- Layout rules for a custom role in a project.
ALTER TABLE project_layout_rules
    ADD COLUMN role_id UUID;

ALTER TABLE project_layout_rules
    ADD CONSTRAINT project_layout_rules_role_id_workspace
    FOREIGN KEY (role_id, organization_id) REFERENCES workspace_roles (id, organization_id)
    ON DELETE CASCADE;

DO $$
DECLARE
    name TEXT;
BEGIN
    FOR name IN
        SELECT constraint_row.conname
        FROM pg_constraint constraint_row
        WHERE constraint_row.conrelid = 'project_layout_rules'::regclass
          AND constraint_row.contype = 'c'
          AND pg_get_constraintdef(constraint_row.oid) LIKE '%role IS NULL%actor_id IS NULL%'
    LOOP
        EXECUTE format('ALTER TABLE project_layout_rules DROP CONSTRAINT %I', name);
    END LOOP;
END;
$$;

ALTER TABLE project_layout_rules
    ADD CONSTRAINT project_layout_rules_one_target CHECK (
        (role IS NOT NULL)::int + (role_id IS NOT NULL)::int + (actor_id IS NOT NULL)::int = 1
    );

CREATE UNIQUE INDEX project_layout_rules_custom_role
    ON project_layout_rules (project_id, role_id)
    WHERE role_id IS NOT NULL;

-- Invitations can name a custom role too.
-- A role that is removed leaves its finished invitations naming the base
-- role; the server refuses to remove one an open invitation still names.
ALTER TABLE workspace_invitations
    ADD COLUMN role_id UUID REFERENCES workspace_roles(id) ON DELETE SET NULL;

ALTER TABLE project_invitations
    ADD COLUMN role_id UUID REFERENCES workspace_roles(id) ON DELETE SET NULL;

-- The views a project's member starts with pinned, from their role's
-- defaults; NULL when the role names none (My tasks and Ready for agents).
CREATE FUNCTION role_default_pinned_views(target_project UUID, target_actor UUID)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT ARRAY(SELECT jsonb_array_elements_text(held.defaults->'pinnedViews'))
    FROM project_memberships membership
    JOIN projects project ON project.id = membership.project_id
    JOIN workspace_roles held ON held.organization_id = project.organization_id
     AND (held.id = membership.role_id OR (membership.role_id IS NULL AND held.key = membership.role))
    WHERE membership.project_id = target_project AND membership.actor_id = target_actor
      AND jsonb_typeof(held.defaults->'pinnedViews') = 'array'
$$;
