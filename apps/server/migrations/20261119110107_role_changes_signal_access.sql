-- K5.8: a change to a role's definition (its permissions, its modules, its
-- agent limit) changes what every holder of the role may read, so each
-- holder gets the same `account.access_changed` signal a membership change
-- gives: live connections work out their set again and drop watched
-- documents they can no longer open.

CREATE FUNCTION access_changed_role() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM stage_access_changed(OLD.organization_id, NULL, holder.actor_id)
    FROM (
        SELECT DISTINCT actor_id FROM organization_memberships WHERE role_id = OLD.id
    ) holder;
    PERFORM stage_access_changed(space.organization_id, space.id, holder.actor_id)
    FROM (
        SELECT DISTINCT project_id, actor_id FROM project_memberships WHERE role_id = OLD.id
    ) holder
    JOIN projects space ON space.id = holder.project_id;
    RETURN NULL;
END $$;

CREATE TRIGGER access_changed AFTER UPDATE ON workspace_roles
    FOR EACH ROW WHEN (OLD.* IS DISTINCT FROM NEW.*)
    EXECUTE FUNCTION access_changed_role();
