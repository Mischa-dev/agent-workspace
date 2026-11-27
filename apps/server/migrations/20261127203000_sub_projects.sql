-- AWD-221: a project inside a project.
--
-- * `projects.parent_space_id`: the project or team space this one is
--   inside. A space and what is under it is a tree at most three levels
--   deep, all in one workspace; `spaces_parent_check` refuses anything else,
--   whichever edge wrote it. A space whose parent is deleted goes back to
--   the top.
-- * `projects.people_from_parent`: its people are its parent's. Everyone
--   with a seat in the parent then holds a seat here with the same role (a
--   derived seat, as a group's is: `project_memberships.via_group`, with
--   `via_parent` saying which source gave it), which the database keeps in
--   step with the parent's seats, so every membership check and the
--   conversation gate find it without knowing about parents. Someone added
--   here by hand keeps their own seat.
-- * `space_group_seat_sync` now takes the better of the two sources: a
--   group's grant here, or the parent's seat.
-- * The space's thing takes its parent (ruling R11: containment is a
--   parent).

ALTER TABLE projects
    ADD COLUMN parent_space_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    ADD COLUMN people_from_parent BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT projects_parent_not_self CHECK (parent_space_id <> id),
    ADD CONSTRAINT projects_people_need_a_parent
        CHECK (NOT people_from_parent OR parent_space_id IS NOT NULL);
CREATE INDEX projects_parent ON projects (parent_space_id) WHERE parent_space_id IS NOT NULL;

ALTER TABLE project_memberships
    ADD COLUMN via_parent BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT project_memberships_parent_seat_is_derived CHECK (NOT via_parent OR via_group);

CREATE OR REPLACE VIEW spaces AS
    SELECT id, organization_id AS workspace_id, kind, project_key AS key, name, description,
           visibility, owner_actor_id, created_by_actor_id, shape, archived_at,
           archived_by_actor_id, version, created_at, updated_at, icon,
           parent_space_id, people_from_parent
    FROM projects;

-- How many levels a space and what is under it may be.
CREATE FUNCTION space_tree_max_depth()
RETURNS INTEGER
LANGUAGE sql
IMMUTABLE
AS $$ SELECT 3 $$;

-- A space's parent is a project or team space of its workspace, never
-- itself or something under it, and the tree stays within its depth. Moves
-- in one workspace take turns, so two of them can't close a loop between
-- them.
CREATE FUNCTION spaces_parent_check()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    parent projects%ROWTYPE;
    above INTEGER;
    below INTEGER;
    loops BOOLEAN;
BEGIN
    IF NEW.parent_space_id IS NULL THEN
        NEW.people_from_parent := false;
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE'
       AND NEW.parent_space_id IS NOT DISTINCT FROM OLD.parent_space_id
       AND NEW.kind = OLD.kind THEN
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('space-tree:' || NEW.organization_id::text, 0));
    SELECT * INTO parent FROM projects WHERE id = NEW.parent_space_id;
    IF NOT FOUND
       OR parent.organization_id <> NEW.organization_id
       OR parent.kind NOT IN ('standard', 'team')
       OR NEW.kind NOT IN ('standard', 'team') THEN
        RAISE EXCEPTION 'a space goes inside a project or team space of its workspace'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'projects_parent_kind';
    END IF;
    WITH RECURSIVE up AS (
        SELECT space.id, space.parent_space_id, 1 AS level
        FROM projects space WHERE space.id = NEW.parent_space_id
        UNION ALL
        SELECT space.id, space.parent_space_id, up.level + 1
        FROM projects space JOIN up ON space.id = up.parent_space_id
        WHERE up.level < 16
    )
    SELECT max(level), bool_or(id = NEW.id) INTO above, loops FROM up;
    IF loops THEN
        RAISE EXCEPTION 'a space can''t go inside itself or something under it'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'projects_parent_loop';
    END IF;
    WITH RECURSIVE down AS (
        SELECT space.id, 1 AS level FROM projects space WHERE space.parent_space_id = NEW.id
        UNION ALL
        SELECT space.id, down.level + 1
        FROM projects space JOIN down ON space.parent_space_id = down.id
        WHERE down.level < 16
    )
    SELECT COALESCE(max(level), 0) INTO below FROM down;
    IF above + 1 + below > space_tree_max_depth() THEN
        RAISE EXCEPTION 'spaces nest at most % levels deep', space_tree_max_depth()
            USING ERRCODE = 'check_violation', CONSTRAINT = 'projects_parent_depth';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER spaces_parent_check
BEFORE INSERT OR UPDATE OF parent_space_id, people_from_parent, kind ON projects
FOR EACH ROW
EXECUTE FUNCTION spaces_parent_check();

-- How far a role reaches, for picking the better of two: lower is more.
CREATE FUNCTION space_role_rank(role TEXT)
RETURNS INTEGER
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 WHEN 'viewer' THEN 3 ELSE 2 END
$$;

-- Brings `account`'s derived seat in `space` in step with what gives it
-- one: a group's grant there (member before viewer, then the oldest grant),
-- or, where the space's people are its parent's, their seat in the parent.
-- The better role wins, and a grant made here wins a tie. One enabled seat
-- while a source reaches them and they have no seat of their own; none
-- otherwise. A seat of their own that was turned off becomes the derived
-- one. Only enabled, active members of the workspace get one.
CREATE OR REPLACE FUNCTION space_group_seat_sync(space UUID, account UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    best_role_id UUID;
    best_role TEXT;
    parent_role_id UUID;
    parent_role TEXT;
    from_parent BOOLEAN := false;
    seat project_memberships%ROWTYPE;
    org UUID;
    parent UUID;
    name TEXT;
    wanted TEXT;
BEGIN
    SELECT organization_id, CASE WHEN people_from_parent THEN parent_space_id END
    INTO org, parent
    FROM projects WHERE id = space AND archived_at IS NULL;
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

    IF parent IS NOT NULL THEN
        SELECT held.role, held.role_id INTO parent_role, parent_role_id
        FROM project_memberships held
        JOIN projects above ON above.id = held.project_id AND above.archived_at IS NULL
        JOIN organization_memberships workspace_membership
          ON workspace_membership.organization_id = org AND workspace_membership.actor_id = account
         AND workspace_membership.enabled
        JOIN actors actor ON actor.id = account AND actor.status = 'active'
        WHERE held.project_id = parent AND held.actor_id = account AND held.enabled;
        IF parent_role IS NOT NULL
           AND (best_role IS NULL OR space_role_rank(parent_role) < space_role_rank(best_role)) THEN
            best_role := parent_role;
            best_role_id := parent_role_id;
            from_parent := true;
        END IF;
    END IF;

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
            IF seat.via_parent IS DISTINCT FROM from_parent THEN
                UPDATE project_memberships SET via_parent = from_parent WHERE id = seat.id;
            END IF;
            RETURN;
        END IF;
        UPDATE project_memberships
        SET via_group = true, via_parent = from_parent, enabled = true,
            role = best_role, role_id = best_role_id,
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
    INSERT INTO project_memberships (
        id, project_id, actor_id, role, role_id, display_name, handle, via_group, via_parent
    )
    VALUES (gen_random_uuid(), space, account, best_role, best_role_id, name, wanted, true, from_parent)
    ON CONFLICT (project_id, actor_id) DO NOTHING;
END;
$$;

-- A seat that becomes someone's own is no longer the parent's.
CREATE FUNCTION project_memberships_own_seat()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT NEW.via_group THEN
        NEW.via_parent := false;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER project_memberships_own_seat
BEFORE UPDATE OF via_group ON project_memberships
FOR EACH ROW
EXECUTE FUNCTION project_memberships_own_seat();

-- Every derived seat of a space, for everyone a source could reach: the
-- members of groups granted a role there, those with a seat in its parent,
-- and those holding a derived seat now.
CREATE FUNCTION space_derived_seats_sync(space UUID)
RETURNS void
LANGUAGE sql
AS $$
    SELECT space_group_seat_sync(space, account.id)
    FROM (
        SELECT member.account_id AS id
        FROM role_grants grant_row
        JOIN account_group_members member ON member.group_id = grant_row.group_id
        WHERE grant_row.space_id = space
        UNION
        SELECT held.actor_id
        FROM projects target
        JOIN project_memberships held
          ON held.project_id = target.parent_space_id AND held.enabled
        WHERE target.id = space AND target.people_from_parent
        UNION
        SELECT seat.actor_id FROM project_memberships seat
        WHERE seat.project_id = space AND seat.via_group AND seat.enabled
    ) account
    ORDER BY account.id
$$;

-- A seat changing in a space reaches the spaces under it whose people are
-- its people (and, through their own seats changing, the ones under those).
CREATE FUNCTION project_memberships_child_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    changed_space UUID;
    account UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        changed_space := OLD.project_id;
        account := OLD.actor_id;
    ELSE
        changed_space := NEW.project_id;
        account := NEW.actor_id;
    END IF;
    PERFORM space_group_seat_sync(child.id, account)
    FROM projects child
    WHERE child.parent_space_id = changed_space AND child.people_from_parent
    ORDER BY child.id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER project_memberships_child_seats
AFTER INSERT OR DELETE OR UPDATE OF enabled, role, role_id ON project_memberships
FOR EACH ROW
EXECUTE FUNCTION project_memberships_child_seats();

-- Moving a space, or changing whose people it has, brings its derived
-- seats in step. Seats the parent gave become their holders' own when the
-- space takes its own people (`spaces::manage` keeps them first); what is
-- left here is whatever no source reaches any more.
CREATE FUNCTION spaces_parent_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM space_derived_seats_sync(NEW.id);
    RETURN NULL;
END;
$$;

CREATE TRIGGER spaces_parent_seats
AFTER INSERT OR UPDATE OF parent_space_id, people_from_parent ON projects
FOR EACH ROW
EXECUTE FUNCTION spaces_parent_seats();

-- A restored space gets its derived seats again, the parent's included.
CREATE OR REPLACE FUNCTION space_restored_group_seats()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM space_derived_seats_sync(NEW.id);
    RETURN NULL;
END;
$$;

-- The space's thing sits under its parent's.
CREATE OR REPLACE VIEW thing_source_projects AS
    SELECT id,
           'project'::text AS kind,
           organization_id AS workspace_id,
           id AS space_id,
           thing_if_registered(parent_space_id) AS parent_id,
           NULL::uuid AS access_parent_id,
           "left"(name, 300) AS title,
           NULL::text AS status,
           NULL::uuid AS owner_account_id,
           NULL::uuid AS created_by_account_id,
           NULL::uuid AS updated_by_account_id,
           version,
           created_at,
           updated_at,
           archived_at,
           NULL::timestamp with time zone AS deleted_at
    FROM projects project;
