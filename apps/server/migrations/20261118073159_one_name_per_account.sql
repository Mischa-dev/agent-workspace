-- K1.28: one name per account per workspace. `organization_actor_profiles`
-- is the account's name and handle in a workspace; every membership in the
-- workspace's spaces carries that name, kept in step by the database.
-- A rename written on any seat is the account's rename in the workspace,
-- so it reaches every space instead of being reverted. People, agents
-- and apps take part. A profile agent's handle belongs to its project,
-- where turning it off frees the handle for another profile; its name
-- follows its profile (agent_profiles.rs), and bare workers are unlisted.

CREATE OR REPLACE FUNCTION canonical_member_profile() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    org UUID;
    canonical organization_actor_profiles%ROWTYPE;
    wanted TEXT;
BEGIN
    -- A rename is written through after the statement (below).
    IF TG_OP = 'UPDATE' THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM actors
        WHERE id = NEW.actor_id AND listed AND account_kind <> 'system'
          AND kind <> 'execution_worker'
    ) THEN
        RETURN NEW;
    END IF;
    SELECT organization_id INTO org FROM projects WHERE id = NEW.project_id;
    IF org IS NULL THEN
        RETURN NEW;
    END IF;
    SELECT * INTO canonical FROM organization_actor_profiles
    WHERE organization_id = org AND actor_id = NEW.actor_id;
    IF NOT FOUND THEN
        wanted := NEW.handle;
        IF EXISTS (
            SELECT 1 FROM organization_actor_profiles
            WHERE organization_id = org AND handle = wanted
        ) THEN
            wanted := left(wanted, 25) || '-' || right(replace(NEW.actor_id::text, '-', ''), 6);
        END IF;
        INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle)
        VALUES (org, NEW.actor_id, NEW.display_name, wanted)
        ON CONFLICT (organization_id, actor_id) DO NOTHING;
        SELECT * INTO canonical FROM organization_actor_profiles
        WHERE organization_id = org AND actor_id = NEW.actor_id;
    END IF;
    NEW.display_name := canonical.display_name;
    NEW.handle := canonical.handle;
    RETURN NEW;
END;
$$;

-- A rename written on a seat is the account's rename in its workspace:
-- once the statement is done, the workspace profile takes it, and from
-- there every other seat (the trigger below).
CREATE FUNCTION member_rename_reaches_profile() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;
    UPDATE organization_actor_profiles profile
    SET display_name = NEW.display_name, handle = NEW.handle
    FROM projects space
    WHERE space.id = NEW.project_id
      AND profile.organization_id = space.organization_id
      AND profile.actor_id = NEW.actor_id
      AND (profile.display_name IS DISTINCT FROM NEW.display_name
           OR profile.handle IS DISTINCT FROM NEW.handle);
    RETURN NULL;
END;
$$;

CREATE TRIGGER project_memberships_rename_reaches_profile
AFTER UPDATE OF display_name, handle ON project_memberships
FOR EACH ROW
WHEN (OLD.display_name IS DISTINCT FROM NEW.display_name OR OLD.handle IS DISTINCT FROM NEW.handle)
EXECUTE FUNCTION member_rename_reaches_profile();

-- The one name reaches every seat in the workspace.
CREATE FUNCTION member_profile_reaches_every_space() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE project_memberships membership
    SET display_name = NEW.display_name, handle = NEW.handle, updated_at = now()
    FROM projects space
    WHERE space.id = membership.project_id
      AND space.organization_id = NEW.organization_id
      AND membership.actor_id = NEW.actor_id
      AND (membership.display_name IS DISTINCT FROM NEW.display_name
           OR membership.handle IS DISTINCT FROM NEW.handle);
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_actor_profiles_every_space
AFTER UPDATE OF display_name, handle ON organization_actor_profiles
FOR EACH ROW EXECUTE FUNCTION member_profile_reaches_every_space();

-- Listed accounts without a name in a workspace they have a seat in get
-- the name of their first seat there (a taken handle gets a suffix).
INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle)
SELECT DISTINCT ON (space.organization_id, membership.actor_id)
       space.organization_id, membership.actor_id, membership.display_name,
       CASE WHEN EXISTS (
                SELECT 1 FROM organization_actor_profiles taken
                WHERE taken.organization_id = space.organization_id
                  AND taken.handle = membership.handle)
            THEN left(membership.handle, 25) || '-'
                 || right(replace(membership.actor_id::text, '-', ''), 6)
            ELSE membership.handle END
FROM project_memberships membership
JOIN projects space ON space.id = membership.project_id
JOIN actors account ON account.id = membership.actor_id
WHERE account.listed AND account.account_kind <> 'system'
  AND account.kind <> 'execution_worker'
  AND membership.handle ~ '^[a-z0-9][a-z0-9_-]{1,31}$'
  AND NOT EXISTS (
      SELECT 1 FROM organization_actor_profiles existing
      WHERE existing.organization_id = space.organization_id
        AND existing.actor_id = membership.actor_id)
ORDER BY space.organization_id, membership.actor_id, membership.created_at, membership.id
ON CONFLICT DO NOTHING;

-- Every seat now shows its account's one name.
UPDATE project_memberships membership
SET display_name = profile.display_name, handle = profile.handle
FROM projects space, organization_actor_profiles profile
WHERE space.id = membership.project_id
  AND profile.organization_id = space.organization_id
  AND profile.actor_id = membership.actor_id
  AND (membership.display_name IS DISTINCT FROM profile.display_name
       OR membership.handle IS DISTINCT FROM profile.handle);
