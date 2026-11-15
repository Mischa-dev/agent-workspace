-- T1.4d (owner step 3c of moving tasks into collection records,
-- docs/plans/master/plans/T1-fields-views-tasks.md section 4): every
-- reader reads a task's title, field values and archive state from
-- task_rows (T1.4a-c), so work_items' copies go.
--
-- * The copy trigger and the guard on the copies go with the columns.
-- * A task is inserted after its record (crate::task_records::create, or a
--   collection being switched on); one inserted without is refused.
-- * Search: a task's words are its record's (title and text values, which
--   collection_records.search_vector already holds) and its description,
--   which stays on the core with its own tsvector. task_rows combines the
--   two as `search_vector`; a query that wants the indexes asks each.
--   Title trigram search uses collection_records_title_trgm (T1.7).
-- * The partial indexes that read "not archived" on work_items are made
--   again without that condition (archive state is the record's now).
-- * task_rows no longer shows the record's own version (ruling R53: a task
--   has one public revision, work_items.version, which every change to it
--   advances once; the record's counter is internal).
-- * Database functions that read the copies read task_rows instead:
--   the GitHub key matchers, the definition guard and task references.

DROP VIEW task_rows;

DROP TRIGGER collection_records_task_cache ON collection_records;
DROP FUNCTION task_record_to_cache();
DROP TRIGGER work_items_task_cache_guard ON work_items;
DROP FUNCTION task_cache_guard();

-- Task references: the description is the core's, the archive state the
-- record's, so both writes refresh them.
DROP TRIGGER thing_references_from_task ON work_items;

ALTER TABLE work_items
    DROP COLUMN search_vector,
    DROP COLUMN title,
    DROP COLUMN field_values,
    DROP COLUMN archived_at,
    DROP COLUMN archived_by_actor_id;

ALTER TABLE work_items
    ADD COLUMN description_vector TSVECTOR
        GENERATED ALWAYS AS (setweight(to_tsvector('english', description), 'B')) STORED;

CREATE INDEX work_items_description_search ON work_items USING GIN (description_vector);

CREATE INDEX IF NOT EXISTS work_items_project_status_rank
    ON work_items (project_id, status_id, board_rank, id);
CREATE INDEX IF NOT EXISTS work_items_project_assignee
    ON work_items (project_id, assignee_actor_id, updated_at DESC)
    WHERE assignee_actor_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS work_items_project_due
    ON work_items (project_id, due_date, number)
    WHERE due_date IS NOT NULL;

CREATE VIEW task_rows AS
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
       core.description_vector
FROM work_items core
JOIN collection_records record
  ON record.id = core.id AND record.project_id = core.project_id;

-- A task comes after its record; inserting it only gives the record the
-- task's number when they differ (a collection being switched on).
CREATE OR REPLACE FUNCTION task_record_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE collection_records
    SET number = NEW.number, version = version + 1
    WHERE id = NEW.id AND project_id = NEW.project_id AND number <> NEW.number;
    IF NOT EXISTS (
        SELECT 1 FROM collection_records WHERE id = NEW.id AND project_id = NEW.project_id
    ) THEN
        RAISE EXCEPTION 'a task is written after its record'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END
$$;

CREATE OR REPLACE FUNCTION thing_references_from_task() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references WHERE source_kind = 'task' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'task', NEW.id, 'task', NEW.id,
        (SELECT CASE WHEN task.archived_at IS NULL THEN task.description END
         FROM task_rows task WHERE task.id = NEW.id)
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER thing_references_from_task
AFTER INSERT OR UPDATE OF description ON work_items
FOR EACH ROW EXECUTE FUNCTION thing_references_from_task();

-- Archiving or restoring a task's record refreshes the task's references.
CREATE FUNCTION thing_references_from_task_record() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM thing_references_replace(
        task.project_id, 'task', task.id, 'task', task.id,
        CASE WHEN NEW.archived_at IS NULL THEN task.description END
    )
    FROM work_items task
    WHERE task.id = NEW.id AND task.project_id = NEW.project_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER thing_references_from_task_record
AFTER UPDATE OF archived_at ON collection_records
FOR EACH ROW
WHEN (OLD.archived_at IS DISTINCT FROM NEW.archived_at)
EXECUTE FUNCTION thing_references_from_task_record();

CREATE OR REPLACE FUNCTION prevent_disabling_referenced_work_item_definition()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.enabled AND NOT NEW.enabled AND (
        (TG_TABLE_NAME = 'work_item_types' AND EXISTS (
            SELECT 1
              FROM task_rows
             WHERE project_id = OLD.project_id
               AND type_id = OLD.id
               AND archived_at IS NULL
        ))
        OR
        (TG_TABLE_NAME = 'work_item_statuses' AND EXISTS (
            SELECT 1
              FROM task_rows
             WHERE project_id = OLD.project_id
               AND status_id = OLD.id
               AND archived_at IS NULL
        ))
    ) THEN
        RAISE EXCEPTION 'an active Work Item definition cannot be disabled while referenced'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

-- The GitHub key matchers (20261114041000) find open tasks through task_rows.

CREATE OR REPLACE FUNCTION github_suggest_task_links(
    p_project_id UUID,
    p_pull_request_id UUID,
    p_branch_keys TEXT[],
    p_body_keys TEXT[],
    p_actor_id UUID
)
RETURNS INTEGER
LANGUAGE plpgsql
AS $$
DECLARE
    added INTEGER := 0;
    batch INTEGER;
    source_name TEXT;
    candidate_keys TEXT[];
BEGIN
    FOREACH source_name IN ARRAY ARRAY['branch_key', 'body_marker'] LOOP
        candidate_keys := CASE source_name
            WHEN 'branch_key' THEN COALESCE(p_branch_keys, ARRAY[]::TEXT[])
            ELSE COALESCE(p_body_keys, ARRAY[]::TEXT[])
        END;
        IF cardinality(candidate_keys) > 20 THEN
            candidate_keys := candidate_keys[1:20];
        END IF;
        INSERT INTO github_pull_request_work_item_links (
            project_id, github_pull_request_id, work_item_id,
            source, confirmed, created_by_actor_id
        )
        SELECT DISTINCT p_project_id, p_pull_request_id, item.id,
               source_name, false, p_actor_id
        FROM unnest(candidate_keys) AS named(task_key)
        CROSS JOIN LATERAL (
            SELECT CASE
                WHEN upper(named.task_key) ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]{0,8}$'
                THEN upper(named.task_key)
            END AS task_key
        ) checked
        JOIN projects project
          ON project.id = p_project_id
         AND project.project_key = split_part(checked.task_key, '-', 1)
        JOIN task_rows item
          ON item.project_id = p_project_id
         AND item.number = CASE
                WHEN checked.task_key IS NOT NULL
                THEN split_part(checked.task_key, '-', 2)::BIGINT
             END
         AND item.archived_at IS NULL
        WHERE EXISTS (
                SELECT 1 FROM github_pull_requests pull_request
                WHERE pull_request.project_id = p_project_id
                  AND pull_request.id = p_pull_request_id
            )
          AND NOT EXISTS (
                SELECT 1 FROM github_pull_request_dismissed_links dismissed
                WHERE dismissed.github_pull_request_id = p_pull_request_id
                  AND dismissed.work_item_id = item.id
            )
        ON CONFLICT (github_pull_request_id, work_item_id) DO NOTHING;
        GET DIAGNOSTICS batch = ROW_COUNT;
        added := added + batch;
    END LOOP;
    RETURN added;
END;
$$;

-- Whether a webhook job's `task_keys` (the keys its pull request's branch,
-- title or body names) include one of the project's open tasks. Only such a
-- pull request is read from GitHub and picked up when the app isn't
-- tracking it yet.
CREATE OR REPLACE FUNCTION github_job_names_open_task(p_project_id UUID, p_payload JSONB)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN jsonb_typeof(p_payload -> 'task_keys') IS DISTINCT FROM 'array' THEN false
        ELSE EXISTS (
            SELECT 1
            FROM jsonb_array_elements_text(p_payload -> 'task_keys')
                 WITH ORDINALITY AS named(task_key, position)
            CROSS JOIN LATERAL (
                SELECT CASE
                    WHEN named.position <= 20
                     AND upper(named.task_key) ~ '^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]{0,8}$'
                    THEN upper(named.task_key)
                END AS task_key
            ) checked
            JOIN projects project
              ON project.id = p_project_id
             AND project.project_key = split_part(checked.task_key, '-', 1)
            JOIN task_rows item
              ON item.project_id = p_project_id
             AND item.number = CASE
                    WHEN checked.task_key IS NOT NULL
                    THEN split_part(checked.task_key, '-', 2)::BIGINT
                 END
             AND item.archived_at IS NULL
        )
    END
$$;
