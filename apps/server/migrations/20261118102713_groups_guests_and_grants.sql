-- K1.27: groups, guests and role grants.
--
-- * Four catalogue keys: `members.groups` (make and change groups, owners
--   and admins, from `members.manage`), and chat's `channels.browse`
--   (from `channels.post`), `channels.create_private` (from
--   `channels.create`) and `channels.mention_everyone` (from
--   `channels.moderate`). A custom role holding the parent gains the key.
-- * `role_grants`: a group holds a role in a space. Each of its members
--   then holds that space: a seat (`project_memberships.via_group`) the
--   database keeps in step with the grants and the group's members, so
--   every membership check finds it. Someone already in the space keeps
--   their own seat and holds both roles (`access::holding`). A direct grant
--   to one account stays the membership row itself.
-- * Guests: the starting role `guest` has no `spaces.browse` and no
--   `directory.view`. Its holders aren't in the workspace's own space and
--   don't read spaces open to the workspace; they see the spaces they are
--   added to, and the conversations there.
-- * Memberships may end: `expires_at` on workspace and space memberships,
--   which the `memberships.expire` job turns off once it passes.

INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES
    ('members.groups', 'members', 'workspace', false, ARRAY['owner', 'admin']::text[]),
    ('channels.browse', 'channels', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('channels.create_private', 'channels', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('channels.mention_everyone', 'channels', 'space', false, ARRAY['owner', 'admin']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

UPDATE workspace_roles AS role
SET permissions = role.permissions || ARRAY(
        SELECT backfill.key
        FROM (VALUES
            ('members.groups', 'members.manage'),
            ('channels.browse', 'channels.post'),
            ('channels.create_private', 'channels.create'),
            ('channels.mention_everyone', 'channels.moderate')
        ) AS backfill (key, parent)
        WHERE backfill.parent = ANY(role.permissions)
          AND NOT (backfill.key = ANY(role.permissions))
        ORDER BY backfill.key)
WHERE NOT role.built_in OR role.customized;

-- Starting roles nobody edited follow the catalogue again.
UPDATE workspace_roles SET version = version WHERE built_in AND NOT customized;

-- Group grants ----------------------------------------------------------------

CREATE TABLE role_grants (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    group_id UUID NOT NULL REFERENCES account_groups(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES workspace_roles(id),
    granted_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (space_id, group_id)
);
CREATE INDEX role_grants_by_group ON role_grants (group_id);
CREATE INDEX role_grants_by_workspace ON role_grants (workspace_id);
SELECT tenancy_protect('role_grants', 'workspace_id');

ALTER TABLE project_memberships
    ADD COLUMN via_group BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN expires_at TIMESTAMPTZ;
ALTER TABLE organization_memberships
    ADD COLUMN expires_at TIMESTAMPTZ;

CREATE INDEX project_memberships_expiring ON project_memberships (expires_at)
    WHERE expires_at IS NOT NULL AND enabled;
CREATE INDEX organization_memberships_expiring ON organization_memberships (expires_at)
    WHERE expires_at IS NOT NULL AND enabled;

-- Whether a workspace role row is the guest tier.
CREATE FUNCTION role_is_guest(role UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE((SELECT tier = 'guest' FROM workspace_roles WHERE id = role), false)
$$;

-- Brings `account`'s group seat in `space` in step with the grants: one
-- enabled seat with the best granted role (member before viewer, then the
-- oldest grant) while a grant reaches them and they have no seat of their
-- own; none otherwise. A seat of their own that was turned off becomes the
-- group's. Only enabled, active members of the workspace get one.
CREATE FUNCTION space_group_seat_sync(space UUID, account UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    best_role_id UUID;
    best_role TEXT;
    seat project_memberships%ROWTYPE;
    org UUID;
    name TEXT;
    wanted TEXT;
BEGIN
    SELECT organization_id INTO org FROM projects WHERE id = space AND archived_at IS NULL;
    IF org IS NULL THEN
        RETURN;
    END IF;
    SELECT grant_row.role_id, held.base_role INTO best_role_id, best_role
    FROM role_grants grant_row
    JOIN account_groups grp ON grp.id = grant_row.group_id AND grp.archived_at IS NULL
    JOIN account_group_members member
      ON member.group_id = grant_row.group_id AND member.account_id = account
    JOIN workspace_roles held ON held.id = grant_row.role_id
    JOIN organization_memberships workspace_membership
      ON workspace_membership.organization_id = org AND workspace_membership.actor_id = account
     AND workspace_membership.enabled
    JOIN actors actor ON actor.id = account AND actor.status = 'active'
    WHERE grant_row.space_id = space
    ORDER BY CASE held.base_role WHEN 'member' THEN 0 ELSE 1 END, grant_row.created_at, grant_row.id
    LIMIT 1;

    SELECT * INTO seat FROM project_memberships
    WHERE project_id = space AND actor_id = account
    FOR UPDATE;

    IF best_role_id IS NULL THEN
        IF FOUND AND seat.via_group AND seat.enabled THEN
            UPDATE project_memberships
            SET enabled = false, disabled_at = now(), disabled_by_actor_id = NULL,
                version = version + 1, updated_at = now()
            WHERE id = seat.id;
        END IF;
        RETURN;
    END IF;

    IF FOUND THEN
        IF seat.enabled AND NOT seat.via_group THEN
            RETURN;
        END IF;
        IF seat.enabled AND seat.role = best_role AND seat.role_id = best_role_id THEN
            RETURN;
        END IF;
        UPDATE project_memberships
        SET via_group = true, enabled = true, role = best_role, role_id = best_role_id,
            disabled_at = NULL, disabled_by_actor_id = NULL, expires_at = NULL,
            version = version + 1, updated_at = now()
        WHERE id = seat.id;
        RETURN;
    END IF;

    SELECT profile.display_name, profile.handle INTO name, wanted
    FROM organization_actor_profiles profile
    WHERE profile.organization_id = org AND profile.actor_id = account;
    IF name IS NULL OR wanted IS NULL THEN
        RETURN;
    END IF;
    IF EXISTS (
        SELECT 1 FROM project_memberships
        WHERE project_id = space AND enabled AND lower(handle) = lower(wanted)
    ) THEN
        wanted := left(wanted, 25) || '-' || right(replace(account::text, '-', ''), 6);
    END IF;
    INSERT INTO project_memberships (id, project_id, actor_id, role, role_id, display_name, handle, via_group)
    VALUES (gen_random_uuid(), space, account, best_role, best_role_id, name, wanted, true)
    ON CONFLICT (project_id, actor_id) DO NOTHING;
END;
$$;

-- Every seat a group's grants touch, for its members (or one member).
CREATE FUNCTION group_seats_sync(target_group UUID, account UUID)
RETURNS void
LANGUAGE sql
AS $$
    SELECT space_group_seat_sync(grant_row.space_id, member.account_id)
    FROM role_grants grant_row
    JOIN account_group_members member ON member.group_id = grant_row.group_id
    WHERE grant_row.group_id = target_group
      AND (account IS NULL OR member.account_id = account)
    ORDER BY grant_row.space_id, member.account_id
$$;

CREATE FUNCTION account_group_members_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM space_group_seat_sync(grant_row.space_id, OLD.account_id)
        FROM role_grants grant_row WHERE grant_row.group_id = OLD.group_id;
        RETURN NULL;
    END IF;
    PERFORM group_seats_sync(NEW.group_id, NEW.account_id);
    RETURN NULL;
END;
$$;

CREATE TRIGGER account_group_members_seats
AFTER INSERT OR DELETE ON account_group_members
FOR EACH ROW
EXECUTE FUNCTION account_group_members_seats();

CREATE FUNCTION role_grants_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP IN ('DELETE', 'UPDATE') THEN
        PERFORM space_group_seat_sync(OLD.space_id, member.account_id)
        FROM account_group_members member WHERE member.group_id = OLD.group_id
        ORDER BY member.account_id;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        PERFORM space_group_seat_sync(NEW.space_id, member.account_id)
        FROM account_group_members member WHERE member.group_id = NEW.group_id
        ORDER BY member.account_id;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER role_grants_seats
AFTER INSERT OR UPDATE OR DELETE ON role_grants
FOR EACH ROW
EXECUTE FUNCTION role_grants_seats();

CREATE FUNCTION account_groups_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM group_seats_sync(NEW.id, NULL);
    RETURN NULL;
END;
$$;

CREATE TRIGGER account_groups_seats
AFTER UPDATE OF archived_at ON account_groups
FOR EACH ROW
WHEN (OLD.archived_at IS DISTINCT FROM NEW.archived_at)
EXECUTE FUNCTION account_groups_seats();

-- Coming back to the workspace brings back group seats too.
CREATE FUNCTION organization_memberships_group_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM space_group_seat_sync(grant_row.space_id, NEW.actor_id)
    FROM account_group_members member
    JOIN role_grants grant_row ON grant_row.group_id = member.group_id
    WHERE member.account_id = NEW.actor_id AND member.workspace_id = NEW.organization_id
    ORDER BY grant_row.space_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_memberships_group_seats
AFTER UPDATE OF enabled ON organization_memberships
FOR EACH ROW
WHEN (NEW.enabled AND NOT OLD.enabled)
EXECUTE FUNCTION organization_memberships_group_seats();

-- Guests ------------------------------------------------------------------------

-- A guest isn't in the workspace's own space: they see only what they're
-- added to.
CREATE OR REPLACE FUNCTION workspace_space_add_member(space UUID, member UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    org UUID;
    held_role TEXT;
    held_role_id UUID;
    space_role TEXT;
    was_enabled BOOLEAN;
    name TEXT;
    wanted TEXT;
    suffix TEXT := right(replace(member::text, '-', ''), 6);
BEGIN
    SELECT organization_id INTO org FROM projects WHERE id = space AND kind = 'workspace';
    IF org IS NULL THEN
        RETURN;
    END IF;
    SELECT membership.role, membership.role_id INTO held_role, held_role_id
    FROM organization_memberships membership
    JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
    WHERE membership.organization_id = org AND membership.actor_id = member AND membership.enabled;
    IF NOT FOUND OR role_is_guest(held_role_id) THEN
        RETURN;
    END IF;
    space_role := CASE held_role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END;
    SELECT enabled INTO was_enabled
    FROM project_memberships WHERE project_id = space AND actor_id = member;
    IF FOUND THEN
        IF NOT was_enabled THEN
            UPDATE project_memberships
            SET enabled = true, role = space_role, role_id = held_role_id,
                disabled_by_actor_id = NULL, disabled_at = NULL,
                version = version + 1, updated_at = now()
            WHERE project_id = space AND actor_id = member;
        END IF;
        RETURN;
    END IF;
    SELECT named.display_name, named.handle INTO name, wanted
    FROM (
        SELECT profile.display_name, profile.handle, 0 AS rank, now() AS at
        FROM organization_actor_profiles profile
        WHERE profile.organization_id = org AND profile.actor_id = member
        UNION ALL
        SELECT person.display_name, left(replace(person.login_name, '.', '_'), 32), 1, now()
        FROM users person WHERE person.actor_id = member
        UNION ALL
        SELECT membership.display_name, membership.handle, 2, membership.created_at
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE membership.actor_id = member AND project.organization_id = org
    ) named
    ORDER BY named.rank, named.at
    LIMIT 1;
    IF name IS NULL OR wanted IS NULL THEN
        RETURN;
    END IF;
    IF EXISTS (
        SELECT 1 FROM project_memberships
        WHERE project_id = space AND enabled AND lower(handle) = lower(wanted)
    ) THEN
        wanted := left(wanted, 25) || '-' || suffix;
    END IF;
    INSERT INTO project_memberships (id, project_id, actor_id, role, role_id, display_name, handle)
    VALUES (gen_random_uuid(), space, member, space_role, held_role_id, name, wanted)
    ON CONFLICT (project_id, actor_id) DO NOTHING;
END;
$$;

-- The workspace space follows the workspace role: a guest leaves it, and
-- someone who stops being a guest joins it.
CREATE OR REPLACE FUNCTION workspace_space_follows_workspace_role()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    guest BOOLEAN := role_is_guest(NEW.role_id);
BEGIN
    UPDATE project_memberships membership
    SET role = CASE NEW.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
        role_id = NEW.role_id,
        enabled = membership.enabled AND NEW.enabled AND NOT guest,
        disabled_at = CASE WHEN membership.enabled AND NEW.enabled AND NOT guest
                           THEN membership.disabled_at ELSE COALESCE(membership.disabled_at, now()) END,
        version = membership.version + 1,
        updated_at = now()
    FROM projects project
    WHERE project.id = membership.project_id
      AND project.organization_id = NEW.organization_id
      AND project.kind = 'workspace'
      AND membership.actor_id = NEW.actor_id
      AND (
          membership.role_id IS DISTINCT FROM NEW.role_id
          OR membership.role IS DISTINCT FROM
              CASE NEW.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END
          OR (membership.enabled AND NOT (NEW.enabled AND NOT guest))
      );
    IF NEW.enabled AND NOT guest THEN
        PERFORM workspace_space_add_member(project.id, NEW.actor_id)
        FROM projects project
        WHERE project.organization_id = NEW.organization_id AND project.kind = 'workspace';
    END IF;
    RETURN NULL;
END;
$$;

-- The implicit workspace-space seat leaves guests out too.
CREATE OR REPLACE FUNCTION space_membership(target UUID, account UUID)
RETURNS TABLE (role TEXT, role_id UUID, implicit BOOLEAN)
LANGUAGE sql
STABLE
AS $$
    SELECT membership.role, membership.role_id, false
    FROM projects space
    JOIN project_memberships membership
      ON membership.project_id = space.id AND membership.actor_id = account
     AND membership.enabled
    JOIN organization_memberships workspace_membership
      ON workspace_membership.organization_id = space.organization_id
     AND workspace_membership.actor_id = account AND workspace_membership.enabled
    JOIN actors actor ON actor.id = account AND actor.status = 'active'
    WHERE space.id = target AND space.archived_at IS NULL
    UNION ALL
    SELECT CASE workspace_membership.role
               WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
           workspace_membership.role_id, true
    FROM projects space
    JOIN organization_memberships workspace_membership
      ON workspace_membership.organization_id = space.organization_id
     AND workspace_membership.actor_id = account AND workspace_membership.enabled
    JOIN actors actor ON actor.id = account AND actor.status = 'active'
    WHERE space.id = target AND space.kind = 'workspace' AND space.archived_at IS NULL
      AND NOT role_is_guest(workspace_membership.role_id)
      AND NOT EXISTS (
          SELECT 1 FROM project_memberships explicit
          WHERE explicit.project_id = space.id AND explicit.actor_id = account
            AND explicit.enabled)
$$;

-- A space open to the workspace is read by those who may browse spaces.
CREATE OR REPLACE FUNCTION space_readable_by(target UUID, account UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (SELECT 1 FROM space_membership(target, account))
        OR EXISTS (
            SELECT 1
            FROM projects space
            JOIN organization_memberships workspace_membership
              ON workspace_membership.organization_id = space.organization_id
             AND workspace_membership.actor_id = account AND workspace_membership.enabled
            JOIN actors actor ON actor.id = account AND actor.status = 'active'
            WHERE space.id = target AND space.archived_at IS NULL
              AND space.visibility = 'workspace'
              AND space.kind IN ('team', 'standard', 'workspace')
              AND role_allows(workspace_membership.role, workspace_membership.role_id, 'spaces.browse'))
$$;

CREATE OR REPLACE FUNCTION spaces_readable_by(account UUID, workspace UUID)
RETURNS SETOF UUID
LANGUAGE sql
STABLE
AS $$
    SELECT space.id
    FROM projects space
    JOIN organization_memberships workspace_membership
      ON workspace_membership.organization_id = space.organization_id
     AND workspace_membership.actor_id = account AND workspace_membership.enabled
    JOIN actors actor ON actor.id = account AND actor.status = 'active'
    WHERE space.organization_id = workspace
      AND space.archived_at IS NULL
      AND (
          (space.kind = 'workspace' AND NOT role_is_guest(workspace_membership.role_id))
          OR (space.visibility = 'workspace' AND space.kind IN ('team', 'standard')
              AND role_allows(workspace_membership.role, workspace_membership.role_id, 'spaces.browse'))
          OR EXISTS (
              SELECT 1 FROM project_memberships membership
              WHERE membership.project_id = space.id AND membership.actor_id = account
                AND membership.enabled)
      )
$$;
