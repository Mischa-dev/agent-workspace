-- K1.10: every membership names a role row (K1 §4.4 step 2).
--
-- * Starting roles hold their permissions as data. `permissions` is what an
--   admin ticked (for a starting role nobody edited: its catalogue
--   defaults); `effective_permissions` is that set closed under level
--   implication and the required keys, within the base role's defaults,
--   written on every save by `workspace_roles_effective` so SQL checks are
--   one `= ANY`. `customized` marks a starting role an admin edited; the
--   others follow the catalogue's defaults. `tier` is the role's standing
--   for ownership and "only owners give admin-tier roles".
-- * Two more starting roles in every workspace: `guest` (tier guest, the
--   required keys and what the catalogue gives guests; its screens are
--   K1.27's) and the hidden `worker` (tier worker, nothing, for today's
--   worker memberships until A1 retires them). A custom role already keyed
--   `guest` or `worker` gets a suffixed key; a custom role already named
--   Guest or Worker keeps its name and the starting role is "Guest (starting
--   role)".
-- * Every membership points at a role row: `role_id IS NULL` is backfilled
--   to the built-in row of its `role`. `memberships_sync_custom_role` keeps
--   `role` and `role_id` in step either way while old code remains: setting
--   `role` alone points `role_id` at that starting row; setting `role_id`
--   sets `role` from the row. `role_id IS NOT NULL` is added NOT VALID
--   (validated in K1.31).
-- * `workspace_role_permissions` answers for every row (a starting role's
--   effective set, the owner's NULL: everything), so `RoleGrant::stored`,
--   `role_allows` and `custom_role_allows` read edited starting roles, and
--   `custom_role_allows` is a full check of the row (finding 4).
-- * The server reconciles `effective_permissions` at start-up
--   (`roles::reconcile`), writing only rows whose value differs, so a new
--   catalogue key reaches every role without a per-role migration.

ALTER TABLE workspace_roles
    ADD COLUMN effective_permissions TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN tier TEXT NOT NULL DEFAULT 'member'
        CONSTRAINT workspace_roles_tier_check
        CHECK (tier IN ('owner', 'admin', 'member', 'viewer', 'guest', 'worker')),
    ADD COLUMN customized BOOLEAN NOT NULL DEFAULT false;

-- The old shape checks: four starting roles, holding no permissions as data.
DO $$
DECLARE
    found RECORD;
BEGIN
    FOR found IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'workspace_roles'::regclass AND contype = 'c'
          AND (pg_get_constraintdef(oid) LIKE '%built_in%'
               OR conname = 'workspace_roles_base_role_check')
    LOOP
        EXECUTE format('ALTER TABLE workspace_roles DROP CONSTRAINT %I', found.conname);
    END LOOP;
END
$$;

ALTER TABLE workspace_roles
    ADD CONSTRAINT workspace_roles_base_role_check
        CHECK (base_role IN ('owner', 'admin', 'member', 'viewer', 'worker')),
    ADD CONSTRAINT workspace_roles_starting_keys
        CHECK (built_in = (key IN ('owner', 'admin', 'member', 'viewer', 'guest', 'worker'))),
    ADD CONSTRAINT workspace_roles_starting_base
        CHECK (NOT built_in OR base_role = CASE key WHEN 'guest' THEN 'viewer' ELSE key END),
    -- Custom roles start from admin, member or viewer; nobody makes another
    -- owner, or a worker.
    ADD CONSTRAINT workspace_roles_custom_base
        CHECK (built_in OR base_role IN ('admin', 'member', 'viewer')),
    -- Only starting roles are marked as edited.
    ADD CONSTRAINT workspace_roles_customized_starting
        CHECK (built_in OR NOT customized);

-- The keys a role holds: `ticked` with the required keys, closed under
-- level implication (holding a level of a kind holds its lower levels),
-- within what `base` holds by default. The same as `policy::RoleGrant::stored`
-- after K1.9.
CREATE FUNCTION role_closed_permissions(ticked TEXT[], base TEXT)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    WITH levels (level, rank) AS (
        VALUES ('see', 1), ('comment', 2), ('suggest', 3), ('edit_own', 4), ('edit', 5), ('manage', 6)
    ),
    held AS (
        SELECT catalogue.key, catalogue.thing_kind, levels.rank
        FROM permission_catalogue catalogue
        LEFT JOIN levels ON levels.level = catalogue.level
        WHERE catalogue.key = ANY(ticked) OR catalogue.required
    )
    SELECT COALESCE(array_agg(catalogue.key ORDER BY catalogue.key), '{}')
    FROM permission_catalogue catalogue
    LEFT JOIN levels ON levels.level = catalogue.level
    WHERE base = ANY(catalogue.default_for)
      AND (catalogue.key IN (SELECT key FROM held)
           OR (catalogue.thing_kind IS NOT NULL AND EXISTS (
               SELECT 1 FROM held
               WHERE held.thing_kind = catalogue.thing_kind AND held.rank >= levels.rank)))
$$;

-- What a starting role nobody edited holds: the catalogue's defaults for
-- its key (the owner: every key; the worker: nothing).
CREATE FUNCTION starting_role_permissions(role_key TEXT)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(array_agg(catalogue.key ORDER BY catalogue.key), '{}')
    FROM permission_catalogue catalogue
    WHERE role_key <> 'worker'
      AND (role_key = 'owner' OR role_key = ANY(catalogue.default_for)
           OR catalogue.required)
$$;

CREATE FUNCTION workspace_roles_effective()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.tier := CASE WHEN NEW.built_in THEN NEW.key ELSE NEW.base_role END;
    IF NEW.built_in AND (NOT NEW.customized OR NEW.key IN ('owner', 'worker')) THEN
        NEW.customized := false;
        NEW.permissions := starting_role_permissions(NEW.key);
        NEW.effective_permissions := NEW.permissions;
    ELSE
        NEW.effective_permissions := role_closed_permissions(NEW.permissions, NEW.base_role);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER workspace_roles_effective
BEFORE INSERT OR UPDATE ON workspace_roles
FOR EACH ROW
EXECUTE FUNCTION workspace_roles_effective();

-- A custom role in the way of a new starting role's key moves aside.
UPDATE workspace_roles
SET key = left(key, 32) || '-custom'
WHERE NOT built_in AND key IN ('guest', 'worker');

CREATE OR REPLACE FUNCTION create_built_in_workspace_roles(target UUID)
RETURNS void
LANGUAGE sql
AS $$
    INSERT INTO workspace_roles (id, organization_id, key, built_in, base_role, name, description, is_default)
    SELECT gen_random_uuid(), target, role.key, true,
           CASE role.key WHEN 'guest' THEN 'viewer' ELSE role.key END,
           CASE WHEN EXISTS (
               SELECT 1 FROM workspace_roles taken
               WHERE taken.organization_id = target AND lower(taken.name) = lower(role.name)
                 AND taken.key <> role.key
           ) THEN role.name || ' (starting role)' ELSE role.name END,
           role.description,
           role.key = 'member' AND NOT EXISTS (
               SELECT 1 FROM workspace_roles chosen
               WHERE chosen.organization_id = target AND chosen.is_default
           )
    FROM (VALUES
        ('owner', 'Owner', 'Everything, including roles, ownership and the workspace itself. Every workspace keeps at least one.'),
        ('admin', 'Admin', 'Runs the workspace: settings, members, roles, code and every project.'),
        ('member', 'Member', 'Does the work: tasks, channels, pages, whiteboards, runs and pull requests.'),
        ('viewer', 'Viewer', 'Reads everything they''re given, and changes nothing.'),
        ('guest', 'Guest', 'Someone from outside: only the spaces and conversations they''re added to.'),
        ('worker', 'Worker', 'A run''s own account. Not given to anyone.')
    ) AS role (key, name, description)
    ON CONFLICT (organization_id, key) DO NOTHING
$$;

SELECT create_built_in_workspace_roles(id) FROM organizations;

-- Every existing row gets its effective set, tier and (for starting roles)
-- its permissions written by the trigger.
UPDATE workspace_roles SET version = version;

-- `role` and `role_id` in step, either way. A starting role is now named by
-- its row too; a custom one as before.
CREATE OR REPLACE FUNCTION memberships_sync_custom_role()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    target_organization UUID;
    held workspace_roles%ROWTYPE;
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.role_id IS NOT DISTINCT FROM OLD.role_id
       AND NEW.role IS DISTINCT FROM OLD.role THEN
        NEW.role_id := NULL;
    END IF;
    IF TG_TABLE_NAME = 'organization_memberships' THEN
        target_organization := NEW.organization_id;
    ELSE
        SELECT organization_id INTO target_organization FROM projects WHERE id = NEW.project_id;
    END IF;
    IF NEW.role_id IS NULL THEN
        SELECT * INTO held FROM workspace_roles
        WHERE organization_id = target_organization AND built_in AND key = NEW.role;
        IF FOUND THEN
            NEW.role_id := held.id;
        END IF;
        RETURN NEW;
    END IF;
    SELECT * INTO held FROM workspace_roles WHERE id = NEW.role_id;
    IF NOT FOUND OR held.organization_id <> target_organization THEN
        RAISE EXCEPTION 'role belongs to another workspace' USING ERRCODE = '23514';
    END IF;
    IF TG_TABLE_NAME = 'organization_memberships' AND held.built_in
       AND held.key IN ('viewer', 'worker') THEN
        RAISE EXCEPTION 'a workspace has no % role', held.key USING ERRCODE = '23514';
    END IF;
    IF TG_TABLE_NAME = 'organization_memberships' AND held.base_role = 'viewer' THEN
        NEW.role := 'member';
    ELSE
        NEW.role := held.base_role;
    END IF;
    RETURN NEW;
END;
$$;

-- Point every membership at its row. The trigger fills role_id from role.
UPDATE organization_memberships SET role_id = NULL WHERE role_id IS NULL;
UPDATE project_memberships SET role_id = NULL WHERE role_id IS NULL;

ALTER TABLE organization_memberships
    ADD CONSTRAINT organization_memberships_role_row CHECK (role_id IS NOT NULL) NOT VALID;
ALTER TABLE project_memberships
    ADD CONSTRAINT project_memberships_role_row CHECK (role_id IS NOT NULL) NOT VALID;

-- Every row answers now: a starting role's effective set as well as a
-- custom role's. NULL only for the owner (everything) and an unknown id.
CREATE OR REPLACE FUNCTION workspace_role_permissions(role UUID)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT effective_permissions FROM workspace_roles
    WHERE id = role AND NOT (built_in AND key = 'owner')
$$;

-- A full check of the row: whether the role named by `role` holds
-- `permission`. NULL (a membership old code wrote without a row) still
-- answers by its plain role alone, as before.
CREATE OR REPLACE FUNCTION custom_role_allows(role UUID, permission TEXT)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT role IS NULL OR COALESCE((
        SELECT (held.built_in AND held.key = 'owner') OR permission = ANY(held.effective_permissions)
        FROM workspace_roles held WHERE held.id = role
    ), false)
$$;

-- The custom role a membership holds, for readers written before roles were
-- data: a starting role (edited or not) reads as none, as it did when its
-- `role_id` was NULL. Permissions are read through
-- `workspace_role_permissions`, which sees the edits.
CREATE FUNCTION custom_role_id(role UUID)
RETURNS UUID
LANGUAGE sql
STABLE
AS $$
    SELECT id FROM workspace_roles WHERE id = role AND NOT built_in
$$;
