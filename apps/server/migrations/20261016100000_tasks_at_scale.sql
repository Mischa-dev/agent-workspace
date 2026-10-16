-- Tasks at project scale (docs/plans/roadmap.md, workstream W3).
--
-- Dates, estimates and watchers on tasks (T5); blocks across projects of one
-- workspace plus relates-to and duplicates links (T3); workflows a project can
-- change (T4); and the indexes that keep lists fast at 10,000 tasks (T6).

-- T6: title search through a trigram index. The extension lives in public so
-- every schema (tests run one per test) finds the same operator class.
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

CREATE INDEX work_items_title_trgm
    ON work_items USING GIN (title public.gin_trgm_ops);

CREATE INDEX work_items_project_number_open
    ON work_items (project_id, number DESC)
    WHERE archived_at IS NULL;

-- T5: dates and an estimate. The estimate is in the project's unit.
ALTER TABLE work_items
    ADD COLUMN start_date DATE,
    ADD COLUMN due_date DATE,
    ADD COLUMN estimate DOUBLE PRECISION CHECK (
        estimate IS NULL OR (estimate >= 0 AND estimate <= 100000)
    ),
    ADD CONSTRAINT work_items_dates_in_order CHECK (
        start_date IS NULL OR due_date IS NULL OR start_date <= due_date
    );

CREATE INDEX work_items_project_due
    ON work_items (project_id, due_date, number)
    WHERE archived_at IS NULL AND due_date IS NOT NULL;

-- Per-project task settings. A project without a row uses the defaults.
CREATE TABLE project_task_settings (
    project_id UUID PRIMARY KEY REFERENCES projects(id),
    estimate_unit TEXT NOT NULL DEFAULT 'points'
        CHECK (estimate_unit IN ('points', 'hours')),
    -- Bumped by every workflow change, so two editors can't overwrite each
    -- other's statuses.
    workflow_version BIGINT NOT NULL DEFAULT 1 CHECK (workflow_version > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- People and agents who hear about changes to a task. The creator and the
-- assignee start watching; anyone in the project can watch or stop.
CREATE TABLE work_item_watchers (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (work_item_id, actor_id),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX work_item_watchers_actor ON work_item_watchers (actor_id, project_id);

INSERT INTO work_item_watchers (project_id, work_item_id, actor_id)
SELECT item.project_id, item.id, item.created_by_actor_id
FROM work_items item
WHERE item.archived_at IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO work_item_watchers (project_id, work_item_id, actor_id)
SELECT item.project_id, item.id, item.assignee_actor_id
FROM work_items item
WHERE item.archived_at IS NULL AND item.assignee_actor_id IS NOT NULL
ON CONFLICT DO NOTHING;

-- T3: a task can be blocked by a task in another project of the same
-- workspace. project_id stays the blocked task's project.
ALTER TABLE work_item_dependencies ADD COLUMN blocking_project_id UUID;
UPDATE work_item_dependencies SET blocking_project_id = project_id;
ALTER TABLE work_item_dependencies
    ALTER COLUMN blocking_project_id SET NOT NULL,
    DROP CONSTRAINT work_item_dependencies_project_id_blocking_work_item_id_fkey,
    ADD CONSTRAINT work_item_dependencies_blocking_fk
        FOREIGN KEY (blocking_project_id, blocking_work_item_id)
        REFERENCES work_items(project_id, id);

CREATE INDEX work_item_dependencies_blocking
    ON work_item_dependencies (blocking_work_item_id);

-- Both ends of a cross-project link must be in one workspace.
CREATE FUNCTION validate_cross_project_task_link()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    first_project UUID;
    second_project UUID;
    same_workspace BOOLEAN;
BEGIN
    IF TG_TABLE_NAME = 'work_item_dependencies' THEN
        first_project := NEW.project_id;
        second_project := NEW.blocking_project_id;
    ELSE
        first_project := NEW.project_id;
        second_project := NEW.other_project_id;
    END IF;
    IF first_project = second_project THEN
        RETURN NEW;
    END IF;
    SELECT first.organization_id = second.organization_id
      INTO same_workspace
      FROM projects first, projects second
     WHERE first.id = first_project AND second.id = second_project;
    IF same_workspace IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'linked tasks must be in the same workspace'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_dependencies_same_workspace
BEFORE INSERT OR UPDATE ON work_item_dependencies
FOR EACH ROW EXECUTE FUNCTION validate_cross_project_task_link();

-- T3: relates-to (either way round, stored once) and duplicates (this task
-- duplicates the other, the original). A task duplicates at most one task.
CREATE TABLE work_item_relations (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('relates_to', 'duplicates')),
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    other_project_id UUID NOT NULL,
    other_work_item_id UUID NOT NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (other_project_id, other_work_item_id)
        REFERENCES work_items(project_id, id),
    CHECK (work_item_id <> other_work_item_id)
);

CREATE UNIQUE INDEX work_item_relations_pair_unique
    ON work_item_relations (
        kind,
        LEAST(work_item_id, other_work_item_id),
        GREATEST(work_item_id, other_work_item_id)
    );
CREATE UNIQUE INDEX work_item_relations_one_original
    ON work_item_relations (work_item_id) WHERE kind = 'duplicates';
CREATE INDEX work_item_relations_other ON work_item_relations (other_work_item_id);
CREATE INDEX work_item_relations_task ON work_item_relations (work_item_id);

CREATE TRIGGER work_item_relations_same_workspace
BEFORE INSERT OR UPDATE ON work_item_relations
FOR EACH ROW EXECUTE FUNCTION validate_cross_project_task_link();

ALTER TABLE work_item_activity
    DROP CONSTRAINT work_item_activity_kind_check,
    ADD CONSTRAINT work_item_activity_kind_check CHECK (kind IN (
        'work_item_created', 'work_item_updated', 'work_item_archived',
        'work_item_restored', 'criterion_created', 'criterion_updated',
        'criterion_completed', 'criterion_reopened', 'criterion_removed',
        'comment_created', 'thread_linked', 'thread_unlinked',
        'repository_linked', 'repository_unlinked', 'repository_primary_changed',
        'worker_invoked', 'github_automation_transition',
        'dependency_added', 'dependency_removed',
        'relation_added', 'relation_removed'
    ));

-- T4: a project orders its own statuses and types. Positions are renumbered
-- 1..n by every workflow change, so they no longer need a unique index that
-- a reorder would trip over halfway through.
DROP INDEX work_item_statuses_active_position_unique;
DROP INDEX work_item_types_active_position_unique;
