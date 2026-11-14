-- AWD-110, step 6 of moving tasks into collection records
-- (docs/plans/collections.md, "Collections that act like tasks"): any
-- collection can switch on "acts like a task", and its records become
-- tasks, the way the built-in Tasks collection's are.
--
-- * collections.acts_like_task is the switch. The built-in Tasks collection
--   always has it on.
-- * work_items.collection_id says which collection a task's record is in:
--   NULL for the project's Tasks collection, else a collection with the
--   switch on. It never changes after the task is made.
-- * Switching it on (collection_make_task_like) gives each of the
--   collection's records a task with the same ID: the project's default
--   type and status, a key from the project's task numbers (which the
--   record takes as its number too), its title and field values. The
--   triggers from 20261114010000 then keep the record in step with its
--   task, and task_record_guard refuses other writes to it.
-- * Switching it off is only allowed while the collection has no records,
--   so a task never loses its record (crate::collections::task_like).

ALTER TABLE collections
    ADD COLUMN acts_like_task BOOLEAN NOT NULL DEFAULT false;

UPDATE collections SET acts_like_task = true WHERE builtin = 'tasks';

ALTER TABLE collections
    ADD CONSTRAINT collections_builtin_acts_like_task
    CHECK (builtin IS NULL OR acts_like_task);

ALTER TABLE work_items
    ADD COLUMN collection_id UUID,
    ADD CONSTRAINT work_items_collection_fk
        FOREIGN KEY (project_id, collection_id) REFERENCES collections(project_id, id);

CREATE INDEX work_items_collection
    ON work_items (project_id, collection_id)
    WHERE collection_id IS NOT NULL;

CREATE OR REPLACE FUNCTION ensure_tasks_collection(target_project UUID, actor UUID) RETURNS UUID
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
    INSERT INTO collections
        (id, project_id, name, title_name, created_by_actor_id, builtin, acts_like_task)
    VALUES (found_id, target_project, candidate, 'Title', actor, 'tasks', true)
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

-- A new task's record goes in its own collection. When the record is
-- already there (a collection being switched on), it takes the task's
-- number, title and values instead.
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
        title = EXCLUDED.title,
        field_values = EXCLUDED.field_values,
        archived_at = EXCLUDED.archived_at,
        archived_by_actor_id = EXCLUDED.archived_by_actor_id,
        version = collection_records.version + 1;
    RETURN NULL;
END
$$;

-- Records of a collection that acts like a task change only through their
-- tasks.
CREATE OR REPLACE FUNCTION task_record_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    row_collection UUID := CASE WHEN TG_OP = 'DELETE' THEN OLD.collection_id
                                ELSE NEW.collection_id END;
BEGIN
    IF pg_trigger_depth() = 1 AND (
        EXISTS (SELECT 1 FROM collections WHERE id = row_collection AND acts_like_task)
        OR (TG_OP = 'UPDATE' AND EXISTS (
            SELECT 1 FROM collections WHERE id = OLD.collection_id AND acts_like_task
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

-- Switches a collection on: each record gets a task with its ID, as
-- `actor`. Returns how many tasks were made. The caller has checked who
-- may, and locked the collection.
CREATE FUNCTION collection_make_task_like(target UUID, actor UUID) RETURNS BIGINT
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
    UPDATE project_counters
    SET next_work_item_number = next_work_item_number + made, updated_at = now()
    WHERE project_id = target_project
    RETURNING next_work_item_number - made INTO first_number;
    INSERT INTO work_items (
        id, project_id, collection_id, number, title, type_id, status_id,
        created_by_actor_id, board_rank, field_values,
        archived_at, archived_by_actor_id, created_at, updated_at
    )
    SELECT record.id, record.project_id, target,
           first_number + row_number() OVER (ORDER BY record.number, record.id) - 1,
           COALESCE(NULLIF(btrim(left(record.title, 240)), ''), 'Untitled task'),
           default_type, default_status, record.created_by_actor_id,
           (SELECT COALESCE(max(board_rank), 0) FROM work_items
            WHERE project_id = target_project AND status_id = default_status)
             + 1024 * row_number() OVER (ORDER BY record.number, record.id),
           record.field_values, record.archived_at, record.archived_by_actor_id,
           record.created_at, record.updated_at
    FROM collection_records record
    WHERE record.collection_id = target;
    UPDATE collections
    SET acts_like_task = true, version = version + 1, updated_at = now()
    WHERE id = target;
    RETURN made;
END
$$;
