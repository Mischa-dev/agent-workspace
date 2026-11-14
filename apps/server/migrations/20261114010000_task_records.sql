-- AWD-110, step 2 of moving tasks into collection records
-- (docs/plans/collections.md, "Tasks as collection records"): every task
-- gets a record in its project's built-in Tasks collection, with the same
-- ID, and database triggers keep the record in step with the task.
--
-- * The record copies what a record and a task share: the number, the title,
--   the custom field values (keyed by the same task field IDs the Tasks
--   collection lists), who made it, archiving and the times. The task stays
--   the one that changes: every task write still goes to work_items, and the
--   triggers below copy it onto the record in the same transaction. A Tasks
--   record can't be written any other way (the app refuses it, and so does
--   task_record_guard), so the two can't drift apart.
-- * Record titles are trimmed and have no control characters, which task
--   titles never promised, so the copy cleans them (task_record_title).
-- * The Tasks collection is made with the project's first task when it
--   doesn't exist yet (ensure_tasks_collection, which crate::collections::tasks
--   also calls when someone opens the collections list). Its next_number
--   isn't used: Tasks records take their task's number.
-- * Tasks never change projects (a move copies the task and archives the
--   original), so project_id isn't copied after the first insert.
--
-- The next step makes the record the owner of the shared columns and reads
-- tasks through one view, then drops these triggers.

-- What a task's title becomes on its record.
CREATE FUNCTION task_record_title(title TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT btrim(left(btrim(regexp_replace(title, '[[:cntrl:]]', ' ', 'g')), 300))
$$;

-- The project's Tasks collection, made (with an "All tasks" table view) if
-- it doesn't exist yet. A collection already called Tasks keeps its name:
-- the built-in one takes the first free name of "Tasks", "Tasks (built
-- in)", "Tasks (built in 2)" and so on.
CREATE FUNCTION ensure_tasks_collection(target_project UUID, actor UUID) RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    found_id UUID;
    candidate TEXT := 'Tasks';
    tries INTEGER := 1;
BEGIN
    SELECT id INTO found_id FROM collections
    WHERE project_id = target_project AND builtin = 'tasks';
    IF found_id IS NOT NULL THEN
        RETURN found_id;
    END IF;
    WHILE EXISTS (
        SELECT 1 FROM collections
        WHERE project_id = target_project AND archived_at IS NULL
          AND lower(name) = lower(candidate)
    ) LOOP
        tries := tries + 1;
        candidate := CASE WHEN tries = 2 THEN 'Tasks (built in)'
                          ELSE 'Tasks (built in ' || (tries - 1)::text || ')' END;
    END LOOP;
    found_id := gen_random_uuid();
    INSERT INTO collections (id, project_id, name, title_name, created_by_actor_id, builtin)
    VALUES (found_id, target_project, candidate, 'Title', actor, 'tasks')
    ON CONFLICT (project_id, builtin) WHERE builtin IS NOT NULL DO NOTHING;
    IF NOT FOUND THEN
        -- Someone else made it first.
        SELECT id INTO found_id FROM collections
        WHERE project_id = target_project AND builtin = 'tasks';
        RETURN found_id;
    END IF;
    INSERT INTO collection_views
        (id, project_id, collection_id, name, view_type, created_by_actor_id)
    VALUES (gen_random_uuid(), target_project, found_id, 'All tasks', 'table', actor);
    RETURN found_id;
END
$$;

-- Every project with tasks has its Tasks collection, made by whoever made
-- the project's first task.
SELECT ensure_tasks_collection(
    task.project_id,
    (array_agg(task.created_by_actor_id ORDER BY task.created_at, task.id))[1]
)
FROM work_items task
GROUP BY task.project_id;

INSERT INTO collection_records (
    id, project_id, collection_id, number, title, field_values,
    created_by_actor_id, updated_by_actor_id, archived_at, archived_by_actor_id,
    created_at, updated_at
)
SELECT task.id, task.project_id, collection.id, task.number,
       task_record_title(task.title), task.field_values,
       task.created_by_actor_id, task.created_by_actor_id,
       task.archived_at, task.archived_by_actor_id, task.created_at, task.updated_at
FROM work_items task
JOIN collections collection
  ON collection.project_id = task.project_id AND collection.builtin = 'tasks';

-- Nothing is lost: every task has its record, with the same values.
DO $$
DECLARE
    missing BIGINT;
BEGIN
    SELECT count(*) INTO missing
    FROM work_items task
    LEFT JOIN collection_records record ON record.id = task.id
    LEFT JOIN collections collection
      ON collection.id = record.collection_id AND collection.builtin = 'tasks'
    WHERE collection.id IS NULL
       OR record.project_id <> task.project_id
       OR record.number <> task.number
       OR record.field_values <> task.field_values
       OR record.archived_at IS DISTINCT FROM task.archived_at;
    IF missing > 0 THEN
        RAISE EXCEPTION '% tasks have no matching record', missing;
    END IF;
END
$$;

CREATE FUNCTION task_record_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO collection_records (
        id, project_id, collection_id, number, title, field_values,
        created_by_actor_id, updated_by_actor_id, archived_at, archived_by_actor_id,
        created_at, updated_at
    ) VALUES (
        NEW.id, NEW.project_id,
        ensure_tasks_collection(NEW.project_id, NEW.created_by_actor_id),
        NEW.number, task_record_title(NEW.title), NEW.field_values,
        NEW.created_by_actor_id, NEW.created_by_actor_id,
        NEW.archived_at, NEW.archived_by_actor_id, NEW.created_at, NEW.updated_at
    );
    RETURN NULL;
END
$$;

CREATE TRIGGER work_items_record_insert
    AFTER INSERT ON work_items
    FOR EACH ROW EXECUTE FUNCTION task_record_insert();

CREATE FUNCTION task_record_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE collection_records
    SET number = NEW.number,
        title = task_record_title(NEW.title),
        field_values = NEW.field_values,
        archived_at = NEW.archived_at,
        archived_by_actor_id = NEW.archived_by_actor_id,
        updated_at = NEW.updated_at,
        version = version + 1
    WHERE id = NEW.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER work_items_record_update
    AFTER UPDATE OF number, title, field_values, archived_at, archived_by_actor_id, updated_at
    ON work_items
    FOR EACH ROW
    WHEN (
        OLD.number IS DISTINCT FROM NEW.number
        OR OLD.title IS DISTINCT FROM NEW.title
        OR OLD.field_values IS DISTINCT FROM NEW.field_values
        OR OLD.archived_at IS DISTINCT FROM NEW.archived_at
        OR OLD.archived_by_actor_id IS DISTINCT FROM NEW.archived_by_actor_id
        OR OLD.updated_at IS DISTINCT FROM NEW.updated_at
    )
    EXECUTE FUNCTION task_record_update();

CREATE FUNCTION task_record_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM collection_records WHERE id = OLD.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER work_items_record_delete
    AFTER DELETE ON work_items
    FOR EACH ROW EXECUTE FUNCTION task_record_delete();

-- A Tasks record changes only through its task: a write that doesn't come
-- from the triggers above is refused.
CREATE FUNCTION task_record_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    row_collection UUID := CASE WHEN TG_OP = 'DELETE' THEN OLD.collection_id
                                ELSE NEW.collection_id END;
BEGIN
    IF pg_trigger_depth() = 1 AND (
        EXISTS (SELECT 1 FROM collections WHERE id = row_collection AND builtin IS NOT NULL)
        OR (TG_OP = 'UPDATE' AND EXISTS (
            SELECT 1 FROM collections WHERE id = OLD.collection_id AND builtin IS NOT NULL
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

CREATE TRIGGER collection_records_task_guard
    BEFORE INSERT OR UPDATE OR DELETE ON collection_records
    FOR EACH ROW EXECUTE FUNCTION task_record_guard();
