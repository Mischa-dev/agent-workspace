-- Joining a workspace's own space looked for a name the member already has
-- in the workspace by reading every space seat the member holds, in every
-- workspace, and keeping the ones in this workspace. For an account in many
-- workspaces that is a read of all its seats per join: making ten thousand
-- workspaces for one owner spent two minutes at commit here. The same name
-- is now found by looking in each of this workspace's spaces for the
-- member's seat, which the (project_id, actor_id) key answers directly.
-- Nothing else changes.
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
        FROM projects project
        CROSS JOIN LATERAL (
            SELECT seat.display_name, seat.handle, seat.created_at
            FROM project_memberships seat
            WHERE seat.project_id = project.id AND seat.actor_id = member
            LIMIT 1
        ) membership
        WHERE project.organization_id = org
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
