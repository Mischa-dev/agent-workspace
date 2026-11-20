-- T1.25: a collection that acts like a task can be switched off again once
-- it has records. Its records keep their tasks, hidden: task_rows (and so
-- every task list, board, count and lookup through it) shows only tasks
-- whose collection acts like a task. Switching it on again brings them back
-- as they were, keys and history included, and makes tasks only for the
-- records added in between. While it is off its records are plain records:
-- task_record_guard already lets them change, since it asks the collection.

CREATE OR REPLACE VIEW task_rows AS
SELECT core.id, core.project_id, core.number, record.title, core.description,
       core.type_id, core.status_id, core.priority, core.assignee_actor_id,
       core.created_by_actor_id, record.archived_at, record.archived_by_actor_id,
       core.board_rank, core.version, core.created_at, core.updated_at,
       core.next_timeline_sequence, core.parent_work_item_id, core.blocked_reason,
       core.blocked_at, core.blocked_by_actor_id,
       record.search_vector || core.description_vector AS search_vector,
       core.milestone_id, core.start_date, core.due_date, core.estimate,
       record.field_values, core.collection_id,
       record.collection_id AS record_collection_id,
       record.updated_at AS record_updated_at,
       record.search_vector AS record_search_vector,
       core.description_vector,
       core.delegate_actor_id,
       core.cycle_id
FROM work_items core
JOIN collection_records record
  ON record.id = core.id AND record.project_id = core.project_id
-- A subquery rather than a join, so FOR UPDATE through the view locks the
-- task and its record, never the collection.
WHERE EXISTS (
    SELECT 1 FROM collections task_collection
    WHERE task_collection.id = record.collection_id AND task_collection.acts_like_task
);

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
    SELECT id INTO default_status FROM work_item_statuses
    WHERE project_id = target_project AND enabled AND is_default;
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
