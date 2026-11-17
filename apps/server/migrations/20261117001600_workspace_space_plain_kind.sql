-- K4.10: the workspace's own space is a plain kind.
--
-- * Every workspace has its space from the start: workspace_space_make makes
--   it (key WSPACE, or WSPACE and four characters of its id when a project
--   took that key; its counter and instructions rows; every enabled member
--   of the workspace in it), a trigger calls it for each new workspace, and
--   it is called here for every workspace that has none yet. Made lazily, a
--   space appeared in the middle of a request, after the limits of an app
--   acting for someone were read, so the app was refused in it.
--   spaces::workspace::ensure calls the same function. Spaces the server
--   made since K4.1 were left closed by default; the workspace's space is
--   open to the workspace, so they are opened.
-- * Boards that belonged to a whole workspace (whiteboards.project_id NULL)
--   move into its workspace space, which every enabled workspace member is a
--   member of with their workspace role. whiteboard_visible_to gives the
--   same answers: membership of the workspace space follows workspace
--   membership (migration 20261113203000). New boards across a workspace go
--   there too, so project_id becomes NOT NULL: every board lives in a space.

-- A member joins the workspace's space under their workspace name. Made
-- eagerly, the space now exists when a membership is written before the
-- name (in its own transaction), so a member with no name yet is left out
-- until their profile is written, instead of joining as "Agent".
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
    IF NOT FOUND THEN
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
    -- Nobody knows their name yet: they join when their workspace profile
    -- is written (organization_actor_profiles_workspace_space).
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

CREATE FUNCTION organization_actor_profiles_workspace_space()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM workspace_space_add_member(project.id, NEW.actor_id)
    FROM projects project
    WHERE project.organization_id = NEW.organization_id AND project.kind = 'workspace';
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_actor_profiles_workspace_space
AFTER INSERT ON organization_actor_profiles
FOR EACH ROW
EXECUTE FUNCTION organization_actor_profiles_workspace_space();

CREATE FUNCTION workspace_space_make(workspace UUID)
RETURNS UUID
LANGUAGE plpgsql
AS $$
DECLARE
    space UUID;
    space_key TEXT;
BEGIN
    SELECT id INTO space FROM projects WHERE organization_id = workspace AND kind = 'workspace';
    IF FOUND THEN
        RETURN space;
    END IF;
    space := gen_random_uuid();
    space_key := CASE
        WHEN EXISTS (
            SELECT 1 FROM projects taken
            WHERE taken.organization_id = workspace AND lower(taken.project_key) = 'wspace'
        )
        THEN 'WSPACE' || upper(right(replace(space::text, '-', ''), 4))
        ELSE 'WSPACE'
    END;
    INSERT INTO projects (id, organization_id, project_key, name, kind, visibility)
    VALUES (space, workspace, space_key, 'Workspace', 'workspace', 'workspace')
    ON CONFLICT DO NOTHING;
    -- Someone else made it at the same moment: theirs is the one.
    SELECT id INTO space FROM projects WHERE organization_id = workspace AND kind = 'workspace';
    INSERT INTO project_counters (project_id, next_work_item_number)
    VALUES (space, 1)
    ON CONFLICT DO NOTHING;
    INSERT INTO project_instructions (project_id, content)
    VALUES (space, '')
    ON CONFLICT DO NOTHING;
    PERFORM workspace_space_add_member(space, membership.actor_id)
    FROM organization_memberships membership
    WHERE membership.organization_id = workspace AND membership.enabled
    ORDER BY membership.created_at, membership.actor_id;
    RETURN space;
END;
$$;

CREATE FUNCTION organizations_make_workspace_space()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM workspace_space_make(NEW.id);
    RETURN NULL;
END;
$$;

CREATE TRIGGER organizations_make_workspace_space
AFTER INSERT ON organizations
FOR EACH ROW
EXECUTE FUNCTION organizations_make_workspace_space();

SELECT workspace_space_make(organization.id)
FROM organizations organization
ORDER BY organization.created_at, organization.id;

-- The workspace's space is open to the workspace; ones made by the server
-- since K4.1 were left at the default.
UPDATE projects SET visibility = 'workspace'
WHERE kind = 'workspace' AND visibility <> 'workspace';

UPDATE whiteboards board
SET project_id = space.id
FROM projects space
WHERE board.project_id IS NULL
  AND space.organization_id = board.organization_id
  AND space.kind = 'workspace';

ALTER TABLE whiteboards ALTER COLUMN project_id SET NOT NULL;
