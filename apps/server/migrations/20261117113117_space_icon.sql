-- K4.7: a space's icon, an emoji shown beside its name in the sidebar, the
-- switcher and the command palette. The space operations set it.
ALTER TABLE projects
    ADD COLUMN icon TEXT CHECK (
        icon IS NULL OR (char_length(icon) BETWEEN 1 AND 16 AND icon !~ '[[:cntrl:][:space:]]')
    );

CREATE OR REPLACE VIEW spaces AS
    SELECT id, organization_id AS workspace_id, kind, project_key AS key, name, description,
           visibility, owner_actor_id, created_by_actor_id, shape, archived_at,
           archived_by_actor_id, version, created_at, updated_at, icon
    FROM projects;
