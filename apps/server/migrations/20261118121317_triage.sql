-- T1.23 Triage: a status category for work that arrived and hasn't been
-- looked at yet, a project's triage rules, and a task's snooze.
--
-- A project uses triage once it has a status of the `triage` category
-- (the triage setup operation adds one). Tasks made through a form, a bug
-- report link or an import land there; the triage rules then run in order
-- over each one that lands and the first that matches acts.

-- The category, first in workflow order.
DO $$
DECLARE
    found TEXT;
BEGIN
    FOR found IN
        SELECT conname FROM pg_constraint
         WHERE conrelid = 'work_item_statuses'::regclass AND contype = 'c'
           AND pg_get_constraintdef(oid) LIKE '%semantic_category%'
    LOOP
        EXECUTE format('ALTER TABLE work_item_statuses DROP CONSTRAINT %I', found);
    END LOOP;
END
$$;

ALTER TABLE work_item_statuses
    ADD CONSTRAINT work_item_statuses_semantic_category_check CHECK (
        semantic_category IN (
            'triage', 'backlog', 'unstarted', 'started', 'review', 'verification',
            'completed', 'canceled'
        )
    );

-- Snoozed out of the triage queue until a time; who looked at it last.
ALTER TABLE work_items
    ADD COLUMN snoozed_until TIMESTAMPTZ,
    ADD COLUMN triaged_at TIMESTAMPTZ,
    ADD COLUMN triaged_by_actor_id UUID REFERENCES actors(id);

-- The wake job finds snoozes that are due.
CREATE INDEX work_items_snoozed ON work_items (snoozed_until)
    WHERE snoozed_until IS NOT NULL;

CREATE TABLE triage_rules (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 1 AND 50),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'),
    -- A view filter over the arriving task, as views store theirs (a list
    -- of clauses, or "and" and "or" groups).
    filter JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- {"kind": "accept", "status": id or name?} | {"kind": "assign", "to": account}
    -- | {"kind": "keep"}
    action JSONB NOT NULL CHECK (jsonb_typeof(action) = 'object'),
    enabled BOOLEAN NOT NULL DEFAULT true,
    version BIGINT NOT NULL DEFAULT 1,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX triage_rules_by_space ON triage_rules (space_id, position);
CREATE INDEX triage_rules_by_workspace ON triage_rules (workspace_id);
SELECT tenancy_protect('triage_rules', 'workspace_id');

-- The built-in Triage view every Tasks collection has: what's in a triage
-- status and isn't snoozed, oldest first. 0192f5c0-…-014 is the Snoozed
-- system field (`collections::tasks::SNOOZED`), true while a snooze runs.
ALTER TABLE collection_views DROP CONSTRAINT collection_views_builtin_check;
ALTER TABLE collection_views ADD CONSTRAINT collection_views_builtin_check CHECK (
    builtin IS NULL
    OR builtin IN ('my_tasks', 'ready_for_agents', 'blocked', 'recently_changed', 'triage')
);

CREATE OR REPLACE FUNCTION add_builtin_task_views(target_collection UUID) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO collection_views
        (id, project_id, collection_id, name, view_type, config, position, created_by_actor_id, builtin)
    SELECT gen_random_uuid(), collection.project_id, collection.id, builtin.name, 'table',
           builtin.config,
           (SELECT COALESCE(max(view.position) + 1, 0) FROM collection_views view
            WHERE view.collection_id = collection.id AND view.owner_actor_id IS NULL) + builtin.place,
           collection.created_by_actor_id, builtin.key
    FROM collections collection
    CROSS JOIN (VALUES
        ('my_tasks', 'My tasks', '{"filters": [{"field": "0192f5c0-0000-7000-8000-000000000002", "op": "is", "values": ["me"]}, {"field": "0192f5c0-0000-7000-8000-000000000001", "op": "categoryIsNot", "values": ["completed", "canceled"]}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000005", "direction": "asc"}, {"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}]}'::jsonb, 0),
        ('ready_for_agents', 'Ready for agents', '{"filters": [{"field": "0192f5c0-0000-7000-8000-00000000000b", "op": "is", "values": ["true"]}, {"field": "0192f5c0-0000-7000-8000-00000000000c", "op": "empty"}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}, {"field": "number", "direction": "desc"}]}'::jsonb, 1),
        ('blocked', 'Blocked', '{"filters": [{"field": "0192f5c0-0000-7000-8000-00000000000a", "op": "is", "values": ["true"]}, {"field": "0192f5c0-0000-7000-8000-000000000001", "op": "categoryIsNot", "values": ["completed", "canceled"]}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}, {"field": "number", "direction": "desc"}]}'::jsonb, 2),
        ('recently_changed', 'Recently changed', '{"sort": [{"field": "updated", "direction": "desc"}]}'::jsonb, 3),
        ('triage', 'Triage', '{"filters": [{"field": "0192f5c0-0000-7000-8000-000000000001", "op": "categoryIs", "values": ["triage"]}, {"field": "0192f5c0-0000-7000-8000-000000000014", "op": "empty"}], "sort": [{"field": "number", "direction": "asc"}]}'::jsonb, 4)
    ) AS builtin(key, name, config, place)
    WHERE collection.id = target_collection
    ON CONFLICT DO NOTHING
$$;

SELECT add_builtin_task_views(id) FROM collections WHERE builtin = 'tasks';
