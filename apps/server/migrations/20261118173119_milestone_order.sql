-- T1.42: milestones keep a manual order within their space. Existing ones
-- take the order the lists showed (by target date, then name), archived
-- ones last.

ALTER TABLE project_milestones ADD COLUMN position INTEGER;

UPDATE project_milestones milestone
SET position = ordered.place
FROM (
    SELECT id,
           row_number() OVER (
               PARTITION BY project_id
               ORDER BY archived_at IS NOT NULL, target_date NULLS LAST, lower(name), id
           )::int AS place
    FROM project_milestones
) ordered
WHERE ordered.id = milestone.id;

ALTER TABLE project_milestones
    ALTER COLUMN position SET DEFAULT 0,
    ALTER COLUMN position SET NOT NULL;

CREATE INDEX project_milestones_order ON project_milestones (project_id, position);
