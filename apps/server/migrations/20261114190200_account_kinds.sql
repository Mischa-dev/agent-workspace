-- K1.5: account kinds and owners (K1 plan, sections 3.1 and 4.2; rulings
-- R19 and R45).
--
-- * actors.account_kind: person, agent, app or system, kept in step with the
--   older `kind` by a trigger while old code still inserts `kind` only.
--   `kind` goes in K1.31.
-- * actors.owner_account_id / owner_workspace_id: who an agent or app
--   account belongs to (structural: who is accountable and who may delete
--   it, never a permission). Exactly one is set for agents and apps, neither
--   for people and the system. An owning account may be any kind of
--   account, but the chain of owners ends at a person or a workspace, never
--   loops, and is at most `accounts.owner_depth` (3) long (R45).
-- * actors.listed: false for plumbing accounts not shown in directories
--   (today's bare per-project execution workers). Not a permission.
-- * actors.created_by_actor_id: who made it, where known.
--
-- Backfill (nothing merges, ids never change): today's agents and apps
-- belong to their workspace: an external agent to its one workspace
-- membership (the connection's project's workspace when it has several), a
-- profile agent to its profile's workspace, a bare worker and an
-- integration to their project's workspace. Every statement only touches
-- rows still unset, so a stopped run resumes by running again.
--
-- The CHECK that agents and apps have exactly one owner is added NOT VALID
-- in K1.8, once the worker accounts are backfilled there; until then the
-- two triggers below give every new agent or app the workspace of its
-- first membership.

ALTER TABLE actors
    ADD COLUMN account_kind TEXT CHECK (account_kind IN ('person', 'agent', 'app', 'system')),
    ADD COLUMN owner_account_id UUID REFERENCES actors(id),
    ADD COLUMN owner_workspace_id UUID REFERENCES organizations(id),
    ADD COLUMN listed BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN created_by_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT actors_one_owner CHECK (num_nonnulls(owner_account_id, owner_workspace_id) <= 1),
    ADD CONSTRAINT actors_owner_not_self CHECK (owner_account_id IS NULL OR owner_account_id <> id);

CREATE FUNCTION actors_account_kind()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.kind IS DISTINCT FROM OLD.kind OR NEW.account_kind IS NULL THEN
        NEW.account_kind := CASE NEW.kind
            WHEN 'human' THEN 'person'
            WHEN 'external_agent' THEN 'agent'
            WHEN 'execution_worker' THEN 'agent'
            WHEN 'integration' THEN 'app'
            ELSE 'system' END;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER actors_account_kind
BEFORE INSERT OR UPDATE OF kind, account_kind ON actors
FOR EACH ROW EXECUTE FUNCTION actors_account_kind();

UPDATE actors SET account_kind = CASE kind
    WHEN 'human' THEN 'person'
    WHEN 'external_agent' THEN 'agent'
    WHEN 'execution_worker' THEN 'agent'
    WHEN 'integration' THEN 'app'
    ELSE 'system' END
WHERE account_kind IS NULL;

ALTER TABLE actors ALTER COLUMN account_kind SET NOT NULL;

-- An owner chain ends at a person or a workspace, never loops, and is at
-- most three accounts long (R45; the server setting `accounts.owner_depth`
-- starts at 3).
CREATE FUNCTION actors_owner_chain()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    current UUID := NEW.owner_account_id;
    depth INTEGER := 0;
    next_owner UUID;
BEGIN
    IF NEW.owner_account_id IS NOT NULL AND NEW.account_kind NOT IN ('agent', 'app') THEN
        RAISE EXCEPTION 'only agents and apps have an owner' USING ERRCODE = '23514';
    END IF;
    IF NEW.owner_workspace_id IS NOT NULL AND NEW.account_kind NOT IN ('agent', 'app') THEN
        RAISE EXCEPTION 'only agents and apps have an owner' USING ERRCODE = '23514';
    END IF;
    WHILE current IS NOT NULL LOOP
        depth := depth + 1;
        IF current = NEW.id OR depth > 3 THEN
            RAISE EXCEPTION 'an owner chain loops or is longer than 3' USING ERRCODE = '23514';
        END IF;
        SELECT owner_account_id INTO next_owner FROM actors WHERE id = current;
        current := next_owner;
    END LOOP;
    RETURN NEW;
END;
$$;

CREATE TRIGGER actors_owner_chain
BEFORE INSERT OR UPDATE OF owner_account_id, owner_workspace_id ON actors
FOR EACH ROW EXECUTE FUNCTION actors_owner_chain();

-- External agents: their workspace membership, or their connection's
-- project's workspace when they have more than one.
UPDATE actors actor
SET owner_workspace_id = COALESCE(
    (SELECT project.organization_id
     FROM external_agent_connections connection
     JOIN projects project ON project.id = connection.project_id
     WHERE connection.actor_id = actor.id
     ORDER BY connection.created_at
     LIMIT 1),
    (SELECT membership.organization_id
     FROM organization_memberships membership
     WHERE membership.actor_id = actor.id
     ORDER BY membership.created_at
     LIMIT 1))
WHERE actor.kind = 'external_agent' AND actor.owner_workspace_id IS NULL
  AND actor.owner_account_id IS NULL;

-- Profile agents: their profile's workspace.
UPDATE actors actor
SET owner_workspace_id = profile.organization_id
FROM agent_profiles profile
WHERE profile.actor_id = actor.id AND actor.owner_workspace_id IS NULL
  AND actor.owner_account_id IS NULL;

-- Bare per-project workers: their project's workspace, and not listed.
UPDATE actors actor
SET owner_workspace_id = (
        SELECT project.organization_id
        FROM execution_workers worker
        JOIN projects project ON project.id = worker.project_id
        WHERE worker.actor_id = actor.id
        ORDER BY worker.created_at
        LIMIT 1),
    listed = false
WHERE actor.kind = 'execution_worker' AND actor.owner_workspace_id IS NULL
  AND actor.owner_account_id IS NULL
  AND NOT EXISTS (SELECT 1 FROM agent_profiles profile WHERE profile.actor_id = actor.id);

-- Integrations and anything left: the workspace of a project they are in,
-- else of a workspace membership.
UPDATE actors actor
SET owner_workspace_id = COALESCE(
    (SELECT project.organization_id
     FROM project_memberships membership
     JOIN projects project ON project.id = membership.project_id
     WHERE membership.actor_id = actor.id
     ORDER BY membership.created_at
     LIMIT 1),
    (SELECT membership.organization_id
     FROM organization_memberships membership
     WHERE membership.actor_id = actor.id
     ORDER BY membership.created_at
     LIMIT 1))
WHERE actor.account_kind IN ('agent', 'app') AND actor.owner_workspace_id IS NULL
  AND actor.owner_account_id IS NULL;

-- New agents and apps made by today's code get the workspace of their
-- first membership. Removed in K1.6, when `accounts::create_agent` names
-- the owner itself.
CREATE FUNCTION memberships_fill_account_owner()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    workspace UUID;
BEGIN
    IF TG_TABLE_NAME = 'organization_memberships' THEN
        workspace := NEW.organization_id;
    ELSE
        SELECT organization_id INTO workspace FROM projects WHERE id = NEW.project_id;
    END IF;
    UPDATE actors
    SET owner_workspace_id = workspace
    WHERE id = NEW.actor_id
      AND account_kind IN ('agent', 'app')
      AND owner_account_id IS NULL
      AND owner_workspace_id IS NULL;
    RETURN NULL;
END;
$$;

CREATE TRIGGER organization_memberships_fill_account_owner
AFTER INSERT ON organization_memberships
FOR EACH ROW EXECUTE FUNCTION memberships_fill_account_owner();

CREATE TRIGGER project_memberships_fill_account_owner
AFTER INSERT ON project_memberships
FOR EACH ROW EXECUTE FUNCTION memberships_fill_account_owner();

CREATE INDEX actors_owner_account ON actors (owner_account_id) WHERE owner_account_id IS NOT NULL;
CREATE INDEX actors_owner_workspace ON actors (owner_workspace_id) WHERE owner_workspace_id IS NOT NULL;
