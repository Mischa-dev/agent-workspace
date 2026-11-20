-- T1.26: initiatives group work across spaces. An initiative is a record of
-- the workspace's Initiatives collection, in the workspace's own space; it
-- is not a space. What it holds is its milestones: a milestone's parent is
-- the initiative record (containment, R11; no link kind), so a space takes
-- part in an initiative through its milestones there.
--
-- * collections.role: what the app knows a collection as. Only
--   'initiatives' so far, at most one per space. Otherwise it is a plain
--   collection: fields, views, comments and history work as anywhere.
-- * project_milestones.initiative_id: the initiative record a milestone is
--   part of, shown as the milestone thing's parent.

ALTER TABLE collections
    ADD COLUMN role TEXT CHECK (role IN ('initiatives'));

CREATE UNIQUE INDEX collections_one_per_role
    ON collections (project_id, role) WHERE role IS NOT NULL;

ALTER TABLE project_milestones
    ADD COLUMN initiative_id UUID REFERENCES collection_records(id) ON DELETE SET NULL;

CREATE INDEX project_milestones_initiative
    ON project_milestones (initiative_id) WHERE initiative_id IS NOT NULL;

CREATE OR REPLACE VIEW thing_source_project_milestones AS
SELECT milestone.id, 'milestone'::text AS kind, project.organization_id AS workspace_id,
       milestone.project_id AS space_id,
       thing_if_registered(milestone.initiative_id) AS parent_id,
       NULL::uuid AS access_parent_id,
       left(milestone.name, 300) AS title, NULL::text AS status,
       milestone.created_by_actor_id AS owner_account_id,
       milestone.created_by_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, milestone.version,
       milestone.created_at, milestone.updated_at, milestone.archived_at,
       NULL::timestamptz AS deleted_at
FROM project_milestones milestone
JOIN projects project ON project.id = milestone.project_id;
