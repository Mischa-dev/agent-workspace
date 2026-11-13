-- AWD-96: pages outside projects.
--
-- Each workspace gets one hidden "workspace" project, its workspace space,
-- that holds the pages that belong to the workspace rather than to one
-- project. Like the hidden Messages space (migration 0037), reusing a
-- project row keeps every page path working unchanged: revisions, live
-- editing, comments, suggestions, search, links and access checks. It's
-- never listed as a project.
--
-- Every enabled member of the workspace, people and agents, is a member of
-- its space with their workspace role, so search, links and every other
-- membership check find its pages. Everyone in the workspace is added when
-- the space is made (`workspace_space::join`), and anyone who joins or
-- comes back later as their workspace membership commits. Leaving the
-- workspace disables it with their other memberships, and a trigger keeps
-- their role in step.

ALTER TABLE projects DROP CONSTRAINT projects_kind_check;
ALTER TABLE projects ADD CONSTRAINT projects_kind_check
    CHECK (kind IN ('standard', 'direct', 'workspace'));

CREATE UNIQUE INDEX projects_one_workspace_space_per_organization
    ON projects (organization_id)
    WHERE kind = 'workspace';

CREATE FUNCTION workspace_space_follows_workspace_role()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE project_memberships membership
    SET role = CASE NEW.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
        role_id = NEW.role_id,
        enabled = membership.enabled AND NEW.enabled,
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
          OR (membership.enabled AND NOT NEW.enabled)
      );
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_memberships_workspace_space_role
AFTER UPDATE OF role, role_id, enabled ON organization_memberships
FOR EACH ROW
EXECUTE FUNCTION workspace_space_follows_workspace_role();

-- Adds someone to the workspace's space with their workspace role, or turns
-- their membership back on; does nothing unless they're an enabled, active
-- member of the workspace. Their name is the one they have in the workspace
-- (a person's account, or an agent's name in its projects), and a handle
-- that's taken in the space gets their ID's end.
CREATE FUNCTION workspace_space_add_member(space UUID, member UUID)
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
    name := COALESCE(name, 'Agent');
    wanted := COALESCE(wanted, 'agent-' || suffix);
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

-- Someone joining the workspace, or coming back to it, joins its space as
-- the change commits, once their other memberships (and so their name) are
-- in place.
CREATE FUNCTION workspace_space_follows_workspace_membership()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.enabled THEN
        PERFORM workspace_space_add_member(project.id, NEW.actor_id)
        FROM projects project
        WHERE project.organization_id = NEW.organization_id AND project.kind = 'workspace';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER organization_memberships_workspace_space_member
AFTER INSERT OR UPDATE OF enabled ON organization_memberships
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION workspace_space_follows_workspace_membership();
