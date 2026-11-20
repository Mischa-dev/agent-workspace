-- T1.25: each collection that acts like a task has its own workflow within
-- its space's: the statuses its tasks use (NULL is all of them) and the one
-- a new task starts in (NULL is the space's default). Statuses stay the
-- space's, so boards and filters across collections still line up.

ALTER TABLE collections
    ADD COLUMN task_statuses UUID[],
    ADD COLUMN start_status_id UUID;

ALTER TABLE collections
    ADD CONSTRAINT collections_task_statuses_some
    CHECK (task_statuses IS NULL OR cardinality(task_statuses) BETWEEN 1 AND 64);

-- Switching on again starts the new tasks in the collection's own start status.
CREATE OR REPLACE FUNCTION collection_make_task_like(target UUID, actor UUID) RETURNS BIGINT
LANGUAGE plpgsql AS $$
DECLARE
    target_project UUID;
    default_type UUID;
    default_status UUID;
    first_number BIGINT;
    made BIGINT;
BEGIN
    SELECT project_id INTO target_project FROM collections
    WHERE id = target AND NOT acts_like_task AND builtin IS NULL
    FOR UPDATE;
    IF target_project IS NULL THEN
        RETURN 0;
    END IF;
    SELECT id INTO default_type FROM work_item_types
    WHERE project_id = target_project AND enabled AND is_default;
    -- Its start status, else the space's default if it uses it, else the
    -- first status it uses.
    SELECT status.id INTO default_status
    FROM work_item_statuses status, collections collection
    WHERE collection.id = target AND status.project_id = target_project AND status.enabled
      AND (collection.task_statuses IS NULL OR status.id = ANY(collection.task_statuses))
    ORDER BY status.id = collection.start_status_id IS TRUE DESC, status.is_default DESC,
             status.position
    LIMIT 1;
    IF default_type IS NULL OR default_status IS NULL THEN
        RAISE EXCEPTION 'the project has no default task type or status'
            USING ERRCODE = 'check_violation';
    END IF;
    -- Only records without a task yet need one (and a new number).
    SELECT count(*) INTO made FROM collection_records record
    WHERE record.collection_id = target
      AND NOT EXISTS (SELECT 1 FROM work_items core WHERE core.id = record.id);
    -- Out of the way of the task numbers the records are about to take.
    UPDATE collection_records SET number = number + 1000000000000
    WHERE collection_id = target;
    -- A record that was a task before takes its task's number back.
    UPDATE collection_records record
    SET number = core.number
    FROM work_items core
    WHERE record.collection_id = target AND core.id = record.id;
    UPDATE collection_records
    SET title = COALESCE(NULLIF(btrim(left(title, 240)), ''), 'Untitled task'),
        version = version + 1
    WHERE collection_id = target
      AND title IS DISTINCT FROM COALESCE(NULLIF(btrim(left(title, 240)), ''), 'Untitled task');
    UPDATE project_counters
    SET next_work_item_number = next_work_item_number + made, updated_at = now()
    WHERE project_id = target_project
    RETURNING next_work_item_number - made INTO first_number;
    INSERT INTO work_items (
        id, project_id, collection_id, number, type_id, status_id,
        created_by_actor_id, board_rank, created_at, updated_at
    )
    SELECT record.id, record.project_id, target,
           first_number + row_number() OVER (ORDER BY record.number, record.id) - 1,
           default_type, default_status, record.created_by_actor_id,
           (SELECT COALESCE(max(board_rank), 0) FROM work_items
            WHERE project_id = target_project AND status_id = default_status)
             + 1024 * row_number() OVER (ORDER BY record.number, record.id),
           record.created_at, record.updated_at
    FROM collection_records record
    WHERE record.collection_id = target
      AND NOT EXISTS (SELECT 1 FROM work_items core WHERE core.id = record.id);
    -- The new tasks' records take their numbers too.
    UPDATE collection_records record
    SET number = core.number
    FROM work_items core
    WHERE record.collection_id = target AND core.id = record.id
      AND record.number <> core.number;
    UPDATE collections
    SET acts_like_task = true, version = version + 1, updated_at = now()
    WHERE id = target;
    RETURN made;
END
$$;
