-- AWD-74: the hidden Messages (direct) space follows each person's
-- workspace role. Its membership was written once, when the person first
-- opened Messages, so someone who was an owner then kept "owner" there
-- after transferring ownership or being given a custom role, and an
-- owner's role ignores custom restrictions. Every reader of project
-- memberships (sending, MCP reach, search, notifications) then saw the
-- stale role.
--
-- A trigger copies the workspace role onto the direct-space membership
-- whenever it changes, in the same transaction, and the existing rows are
-- brought in line once. A custom role is carried by role_id (the project
-- trigger memberships_sync_custom_role sets `role` to its base); a
-- built-in one by `role`.
--
-- People only: an agent keeps the direct-space role it was given (Viewer,
-- when it's joined to a direct message), whatever its workspace role.

CREATE FUNCTION direct_space_follows_workspace_role()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE project_memberships membership
    SET role = CASE NEW.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
        role_id = NEW.role_id,
        version = membership.version + 1,
        updated_at = now()
    FROM projects project
    WHERE project.id = membership.project_id
      AND project.organization_id = NEW.organization_id
      AND project.kind = 'direct'
      AND membership.actor_id = NEW.actor_id
      -- Execution workers keep the worker role, and agents theirs.
      AND membership.role <> 'worker'
      AND EXISTS (SELECT 1 FROM actors person WHERE person.id = NEW.actor_id AND person.kind = 'human')
      AND (
          membership.role_id IS DISTINCT FROM NEW.role_id
          OR (
              NEW.role_id IS NULL
              AND membership.role IS DISTINCT FROM
                  CASE NEW.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END
          )
      );
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_memberships_direct_space_role
AFTER INSERT OR UPDATE OF role, role_id ON organization_memberships
FOR EACH ROW
EXECUTE FUNCTION direct_space_follows_workspace_role();

UPDATE project_memberships membership
SET role = CASE organization_membership.role
               WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
    role_id = organization_membership.role_id,
    version = membership.version + 1,
    updated_at = now()
FROM projects project
JOIN organization_memberships organization_membership
  ON organization_membership.organization_id = project.organization_id
WHERE project.id = membership.project_id
  AND project.kind = 'direct'
  AND organization_membership.actor_id = membership.actor_id
  AND membership.role <> 'worker'
  AND EXISTS (SELECT 1 FROM actors person WHERE person.id = membership.actor_id AND person.kind = 'human')
  AND (
      membership.role_id IS DISTINCT FROM organization_membership.role_id
      OR (
          organization_membership.role_id IS NULL
          AND membership.role IS DISTINCT FROM
              CASE organization_membership.role
                  WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END
      )
  );
