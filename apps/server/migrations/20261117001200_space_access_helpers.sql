-- K4.2: the space access helpers (K4 §3.4). K4 owns "which spaces exist and
-- who is in them"; K1's access composes on top. The helpers answer
-- membership and readability only, never what a role allows.
--
-- * space_membership(space, account): the account's membership of a space,
--   explicit (an enabled space membership, with an enabled workspace
--   membership and an active account), or implicit for the workspace space
--   (every enabled workspace member is in it, with their workspace role's
--   tier). No row when they aren't a member, or the space is archived.
-- * space_readable_by(space, account): a member, or the space is open to
--   the workspace (`visibility = 'workspace'`, a team space or a project) and
--   they're an enabled member of the workspace.
-- * spaces_readable_by(account, workspace): every space in a workspace the
--   account may read, for lists: `WHERE s.id IN (SELECT spaces_readable_by($1, $2))`.
--
-- Behaviour is unchanged: existing spaces are `members`, and the workspace
-- space was always open to every workspace member.

CREATE FUNCTION space_membership(target UUID, account UUID)
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
      AND NOT EXISTS (
          SELECT 1 FROM project_memberships explicit
          WHERE explicit.project_id = space.id AND explicit.actor_id = account
            AND explicit.enabled)
$$;

CREATE FUNCTION space_readable_by(target UUID, account UUID)
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
              AND space.kind IN ('team', 'standard', 'workspace'))
$$;

CREATE FUNCTION spaces_readable_by(account UUID, workspace UUID)
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
      AND space.kind <> 'direct'
      AND (
          space.kind = 'workspace'
          OR space.visibility = 'workspace' AND space.kind IN ('team', 'standard')
          OR EXISTS (
              SELECT 1 FROM project_memberships membership
              WHERE membership.project_id = space.id AND membership.actor_id = account
                AND membership.enabled)
      )
$$;
