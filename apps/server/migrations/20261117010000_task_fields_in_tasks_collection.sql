-- Lane D (T1.11): a project's task fields are the fields of its built-in
-- Tasks collection, like any collection's. Their IDs don't change, so the
-- values records keep by field ID stay as they are. `record_kind` goes:
-- every field now belongs to a collection.
SELECT ensure_tasks_collection(field.project_id, min(field.created_by_actor_id::text)::uuid)
FROM field_definitions field
WHERE field.record_kind = 'task'
GROUP BY field.project_id;

DROP INDEX field_definitions_active_name_unique;

ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_collection_check;

UPDATE field_definitions field
SET collection_id = tasks.id
FROM collections tasks
WHERE field.record_kind = 'task'
  AND tasks.project_id = field.project_id AND tasks.builtin = 'tasks';

ALTER TABLE field_definitions
    DROP COLUMN record_kind,
    ALTER COLUMN collection_id SET NOT NULL;

-- A record's link fields are its collection's own now (the Tasks
-- collection's included), so the links that follow them read only those.
CREATE OR REPLACE FUNCTION links_record_field_ids(r collection_records) RETURNS UUID[]
LANGUAGE sql STABLE AS $$
    SELECT ARRAY(
        SELECT definition.id FROM field_definitions definition
        WHERE definition.project_id = r.project_id AND definition.field_type = 'link'
          AND definition.collection_id = r.collection_id)
$$;
