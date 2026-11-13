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
-- its space with their workspace role. Membership is added the first time
-- someone reaches the space (`workspace_space::join`); removing someone
-- from the workspace disables it with their other memberships, and this
-- trigger keeps their role in step.

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
