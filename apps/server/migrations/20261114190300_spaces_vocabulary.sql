-- K4.1: space words in the database (K4 plan, sections 3.2 and 3.3).
--
-- * Views with the new names over today's tables: `workspaces`
--   (organizations), `workspace_members` (organization_memberships),
--   `spaces` (projects) and `space_members` (project_memberships). Each is a
--   plain view of one table, so PostgreSQL writes through it (INSERT,
--   UPDATE, DELETE, ON CONFLICT, row locks). New code reads and writes these
--   names; old code is untouched. The base tables are renamed only in
--   K4.22, because a foreign key can't point at a view. The view-parity
--   guard (G3, `spaces::views_match_tables`) fails when a branch adds a
--   column to a table and not to its view.
-- * New columns on projects: a description, who may find it (`members`:
--   only members; `workspace`: every workspace member may read it and join
--   it), the owner of a personal space, who made it, and its starting shape
--   (K4.20).
-- * Two new kinds, `team` and `personal`; one personal space per account per
--   workspace. The stored `standard` stays until K4.22 (`SpaceKind` maps it).
-- * Workspace spaces are open to the workspace, as they always were in
--   effect. Existing projects stay `members`, so nobody sees more.

ALTER TABLE projects
    ADD COLUMN description TEXT NOT NULL DEFAULT ''
        CHECK (char_length(description) <= 500 AND description !~ '[[:cntrl:]]'),
    ADD COLUMN visibility TEXT NOT NULL DEFAULT 'members'
        CHECK (visibility IN ('members', 'workspace')),
    ADD COLUMN owner_actor_id UUID REFERENCES actors(id),
    ADD COLUMN created_by_actor_id UUID REFERENCES actors(id),
    ADD COLUMN shape JSONB CHECK (
        shape IS NULL OR (jsonb_typeof(shape) = 'object' AND octet_length(shape::text) <= 16000)
    );

ALTER TABLE projects DROP CONSTRAINT projects_kind_check,
    ADD CONSTRAINT projects_kind_check
        CHECK (kind IN ('standard', 'direct', 'workspace', 'team', 'personal')),
    ADD CONSTRAINT projects_personal_owner_check
        CHECK ((kind = 'personal') = (owner_actor_id IS NOT NULL));

CREATE UNIQUE INDEX projects_one_personal_space
    ON projects (organization_id, owner_actor_id) WHERE kind = 'personal';

UPDATE projects SET visibility = 'workspace'
WHERE kind = 'workspace' AND visibility <> 'workspace';

CREATE VIEW workspaces AS
    SELECT id, name, slug, version, created_at, updated_at
    FROM organizations;

CREATE VIEW workspace_members AS
    SELECT organization_id AS workspace_id, actor_id, role, role_id, role_explicit,
           agent_limit, enabled, version, created_at, updated_at
    FROM organization_memberships;

CREATE VIEW spaces AS
    SELECT id, organization_id AS workspace_id, kind, project_key AS key, name, description,
           visibility, owner_actor_id, created_by_actor_id, shape, archived_at,
           archived_by_actor_id, version, created_at, updated_at
    FROM projects;

CREATE VIEW space_members AS
    SELECT id, project_id AS space_id, actor_id, role, role_id, display_name, handle, enabled,
           invited_by_actor_id, accepted_invitation_id, accepted_at, disabled_by_actor_id,
           disabled_at, version, created_at, updated_at
    FROM project_memberships;
