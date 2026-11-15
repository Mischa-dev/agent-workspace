-- T1.3 (owner step 3a of moving tasks into collection records,
-- docs/plans/master/plans/T1-fields-views-tasks.md section 4): a task's
-- record owns its title, field values and archive state.
--
-- * The app writes those on the record (crate::task_records), and the
--   trigger below copies them back onto work_items, which keeps them as a
--   read cache until every reader has moved to task_rows (T1.4a-c) and the
--   cache is dropped (T1.4d).
-- * work_items refuses a direct write of a cached column: only the copy
--   from the record may change them. A new task's record is written first;
--   inserting the task fills its cache from the record. A task inserted
--   with no record (data written straight into the table) still gets one
--   made from it, as before.
-- * Record and task versions are separate: a record write bumps the
--   record's version, a task write the task's, and nothing copies one into
--   the other. So the task-to-record update trigger goes.
-- * A record of a collection that acts like a task can't be deleted, moved
--   to another collection or renumbered except through its task, and one
--   left without a task when its transaction ends is refused.
-- * Before any of that, the step 2 check runs again and repairs records
--   from work_items where they differ, so this is safe on a database where
--   20261114010000 ran in the same batch. Running it twice changes nothing.

-- No more task-to-record copying on update.
DROP TRIGGER IF EXISTS work_items_record_update ON work_items;
DROP FUNCTION IF EXISTS task_record_update();

-- Records of a collection that acts like a task: written by the app, but
-- deleted, moved or renumbered only through their tasks.
CREATE OR REPLACE FUNCTION task_record_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() = 1 AND (
        (TG_OP = 'DELETE' AND EXISTS (
            SELECT 1 FROM collections WHERE id = OLD.collection_id AND acts_like_task
        ))
        OR (TG_OP = 'UPDATE'
            AND (OLD.collection_id IS DISTINCT FROM NEW.collection_id
                 OR OLD.project_id IS DISTINCT FROM NEW.project_id
                 OR OLD.number IS DISTINCT FROM NEW.number)
            AND EXISTS (
                SELECT 1 FROM collections
                WHERE id IN (OLD.collection_id, NEW.collection_id) AND acts_like_task
            ))
    ) THEN
        RAISE EXCEPTION 'a Tasks record changes only through its task'
            USING ERRCODE = 'check_violation';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER collection_records_task_guard ON collection_records;
CREATE TRIGGER collection_records_task_guard
    BEFORE UPDATE OR DELETE ON collection_records
    FOR EACH ROW EXECUTE FUNCTION task_record_guard();

-- Step 2's check, repaired from work_items. Titles first: a task title the
-- record can't hold as is (from before titles were cleaned) is cleaned on
-- the task too, so the two can agree.
UPDATE work_items
SET title = COALESCE(NULLIF(task_record_title(title), ''), 'Untitled task')
WHERE title IS DISTINCT FROM COALESCE(NULLIF(task_record_title(title), ''), 'Untitled task');

INSERT INTO collection_records (
    id, project_id, collection_id, number, title, field_values,
    created_by_actor_id, updated_by_actor_id, archived_at, archived_by_actor_id,
    created_at, updated_at
)
SELECT task.id, task.project_id,
       COALESCE(task.collection_id,
                ensure_tasks_collection(task.project_id, task.created_by_actor_id)),
       task.number, task.title, task.field_values,
       task.created_by_actor_id, task.created_by_actor_id,
       task.archived_at, task.archived_by_actor_id, task.created_at, task.updated_at
FROM work_items task
WHERE NOT EXISTS (SELECT 1 FROM collection_records record WHERE record.id = task.id);

UPDATE collection_records record
SET title = task.title,
    field_values = task.field_values,
    archived_at = task.archived_at,
    archived_by_actor_id = task.archived_by_actor_id,
    version = record.version + 1
FROM work_items task
WHERE task.id = record.id
  AND (record.title IS DISTINCT FROM task.title
       OR record.field_values IS DISTINCT FROM task.field_values
       OR record.archived_at IS DISTINCT FROM task.archived_at
       OR record.archived_by_actor_id IS DISTINCT FROM task.archived_by_actor_id);

DO $$
DECLARE
    unmatched BIGINT;
BEGIN
    SELECT count(*) INTO unmatched
    FROM work_items task
    LEFT JOIN collection_records record ON record.id = task.id
    LEFT JOIN collections collection
      ON collection.id = record.collection_id AND collection.acts_like_task
    WHERE collection.id IS NULL
       OR record.project_id <> task.project_id
       OR record.collection_id IS DISTINCT FROM
          COALESCE(task.collection_id, record.collection_id)
       OR record.number <> task.number
       OR record.title <> task.title
       OR record.field_values <> task.field_values
       OR record.archived_at IS DISTINCT FROM task.archived_at
       OR record.archived_by_actor_id IS DISTINCT FROM task.archived_by_actor_id;
    IF unmatched > 0 THEN
        RAISE EXCEPTION '% tasks have no matching record', unmatched;
    END IF;
END
$$;

-- The record's shared columns, copied onto its task's cache.
CREATE FUNCTION task_record_to_cache() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE work_items
    SET title = NEW.title,
        field_values = NEW.field_values,
        archived_at = NEW.archived_at,
        archived_by_actor_id = NEW.archived_by_actor_id
    WHERE id = NEW.id AND project_id = NEW.project_id;
    RETURN NULL;
END
$$;

CREATE TRIGGER collection_records_task_cache
    AFTER UPDATE OF title, field_values, archived_at, archived_by_actor_id
    ON collection_records
    FOR EACH ROW
    WHEN (
        OLD.title IS DISTINCT FROM NEW.title
        OR OLD.field_values IS DISTINCT FROM NEW.field_values
        OR OLD.archived_at IS DISTINCT FROM NEW.archived_at
        OR OLD.archived_by_actor_id IS DISTINCT FROM NEW.archived_by_actor_id
    )
    EXECUTE FUNCTION task_record_to_cache();

-- work_items' cached columns: filled from the record when a task is
-- inserted after its record, and otherwise changed only by the copy above
-- (a trigger's write runs one level deeper than the statement's).
CREATE FUNCTION task_cache_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    owner collection_records%ROWTYPE;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT * INTO owner FROM collection_records WHERE id = NEW.id;
        IF FOUND THEN
            NEW.title := owner.title;
            NEW.field_values := owner.field_values;
            NEW.archived_at := owner.archived_at;
            NEW.archived_by_actor_id := owner.archived_by_actor_id;
        END IF;
        RETURN NEW;
    END IF;
    IF pg_trigger_depth() = 1 AND (
        OLD.title IS DISTINCT FROM NEW.title
        OR OLD.field_values IS DISTINCT FROM NEW.field_values
        OR OLD.archived_at IS DISTINCT FROM NEW.archived_at
        OR OLD.archived_by_actor_id IS DISTINCT FROM NEW.archived_by_actor_id
    ) THEN
        RAISE EXCEPTION 'a task''s title, field values and archive state are written on its record'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER work_items_task_cache_guard
    BEFORE INSERT OR UPDATE OF title, field_values, archived_at, archived_by_actor_id
    ON work_items
    FOR EACH ROW EXECUTE FUNCTION task_cache_guard();

-- A task inserted after its record leaves the record as it is, except for
-- taking the task's number (a collection being switched on). One inserted
-- with no record gets one made from it.
CREATE OR REPLACE FUNCTION task_record_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO collection_records (
        id, project_id, collection_id, number, title, field_values,
        created_by_actor_id, updated_by_actor_id, archived_at, archived_by_actor_id,
        created_at, updated_at
    ) VALUES (
        NEW.id, NEW.project_id,
        COALESCE(NEW.collection_id,
                 ensure_tasks_collection(NEW.project_id, NEW.created_by_actor_id)),
        NEW.number, task_record_title(NEW.title), NEW.field_values,
        NEW.created_by_actor_id, NEW.created_by_actor_id,
        NEW.archived_at, NEW.archived_by_actor_id, NEW.created_at, NEW.updated_at
    )
    ON CONFLICT (id) DO UPDATE
    SET number = EXCLUDED.number,
        version = collection_records.version + 1
    WHERE collection_records.number <> EXCLUDED.number;
    RETURN NULL;
END
$$;

-- A record in a collection that acts like a task has its task by the time
-- the transaction ends (the task is inserted just after its record).
CREATE FUNCTION task_record_has_task() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM collection_records record
        JOIN collections collection
          ON collection.id = record.collection_id AND collection.acts_like_task
        WHERE record.id = NEW.id
          AND NOT EXISTS (SELECT 1 FROM work_items task WHERE task.id = record.id)
    ) THEN
        RAISE EXCEPTION 'a Tasks record changes only through its task'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER collection_records_task_exists
    AFTER INSERT ON collection_records
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION task_record_has_task();

-- Switching a collection on: its records' titles become ones a task can
-- hold first, since each new task takes its record's.
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
    SELECT count(*) INTO made FROM collection_records WHERE collection_id = target;
    -- Out of the way of the task numbers the records are about to take.
    UPDATE collection_records SET number = number + 1000000000000
    WHERE collection_id = target;
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
    WHERE record.collection_id = target;
    UPDATE collections
    SET acts_like_task = true, version = version + 1, updated_at = now()
    WHERE id = target;
    RETURN made;
END
$$;
