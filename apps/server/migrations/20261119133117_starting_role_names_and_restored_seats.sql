-- K1.19 and K1.27 follow-ups.
--
-- 1. A starting role's name, free in its workspace: its own name, or, when
--    a custom role already has it, "<name> (starting role)", then
--    "<name> (starting role 2)" and on until one is free. (Kept out of
--    `20261117001400_roles_as_data`, which databases have already applied.)
-- 2. A space brought back from the archive has its group seats worked out
--    again: a seat its group no longer gives (someone taken out of the group
--    while the space was archived) doesn't come back with it.

-- 1. ------------------------------------------------------------------------

CREATE FUNCTION starting_role_name(target UUID, role_key TEXT, wanted TEXT)
RETURNS TEXT
LANGUAGE plpgsql
AS $$
DECLARE
    candidate TEXT := wanted;
    attempt INT := 1;
BEGIN
    WHILE EXISTS (
        SELECT 1 FROM workspace_roles taken
        WHERE taken.organization_id = target AND lower(taken.name) = lower(candidate)
          AND taken.key <> role_key
    ) LOOP
        candidate := wanted || CASE attempt WHEN 1 THEN ' (starting role)'
                                            ELSE ' (starting role ' || attempt || ')' END;
        attempt := attempt + 1;
    END LOOP;
    RETURN candidate;
END;
$$;

CREATE OR REPLACE FUNCTION create_built_in_workspace_roles(target UUID)
RETURNS void
LANGUAGE sql
AS $$
    INSERT INTO workspace_roles (id, organization_id, key, built_in, base_role, name, description, is_default)
    SELECT gen_random_uuid(), target, role.key, true,
           CASE role.key WHEN 'guest' THEN 'viewer' ELSE role.key END,
           starting_role_name(target, role.key, role.name),
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

-- 2. ------------------------------------------------------------------------

CREATE FUNCTION space_restored_group_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM space_group_seat_sync(NEW.id, account.id)
    FROM (
        SELECT member.account_id AS id
        FROM role_grants grant_row
        JOIN account_group_members member ON member.group_id = grant_row.group_id
        WHERE grant_row.space_id = NEW.id
        UNION
        SELECT seat.actor_id FROM project_memberships seat
        WHERE seat.project_id = NEW.id AND seat.via_group AND seat.enabled
    ) account
    ORDER BY account.id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER projects_restored_group_seats
AFTER UPDATE OF archived_at ON projects
FOR EACH ROW
WHEN (OLD.archived_at IS NOT NULL AND NEW.archived_at IS NULL)
EXECUTE FUNCTION space_restored_group_seats();
