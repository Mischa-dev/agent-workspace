-- T1.24 Cycles: a space's work in fixed-length periods. A job keeps the
-- next cycles made, closes one when it ends (rolling its unfinished tasks
-- into the next), and keeps a daily count of each running cycle's tasks
-- for its burn-up chart.

CREATE TABLE cycle_settings (
    space_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT false,
    length_days INTEGER NOT NULL DEFAULT 14 CHECK (length_days BETWEEN 7 AND 56),
    -- ISO weekday a cycle starts on: 1 is Monday, 7 is Sunday.
    starts_on_weekday SMALLINT NOT NULL DEFAULT 1 CHECK (starts_on_weekday BETWEEN 1 AND 7),
    -- How many cycles after the running one are kept made.
    upcoming INTEGER NOT NULL DEFAULT 2 CHECK (upcoming BETWEEN 1 AND 6),
    roll_over BOOLEAN NOT NULL DEFAULT true,
    -- Whoever last changed the settings: the job rolls tasks over as them.
    updated_by_actor_id UUID NOT NULL REFERENCES actors(id),
    -- The last day the job planned this space; it runs once a day.
    planned_on DATE,
    version BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX cycle_settings_by_workspace ON cycle_settings (workspace_id);
CREATE INDEX cycle_settings_due ON cycle_settings (planned_on) WHERE enabled;
SELECT tenancy_protect('cycle_settings', 'workspace_id');

CREATE TABLE cycles (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    number INTEGER NOT NULL CHECK (number > 0),
    name TEXT NOT NULL DEFAULT ''
        CHECK (char_length(name) <= 80 AND name !~ '[[:cntrl:]]'),
    -- The first day, and the day after the last: [starts_on, ends_on).
    starts_on DATE NOT NULL,
    ends_on DATE NOT NULL CHECK (ends_on > starts_on),
    completed_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (space_id, number),
    UNIQUE (space_id, id)
);

CREATE INDEX cycles_by_workspace ON cycles (workspace_id);
CREATE INDEX cycles_open ON cycles (space_id, ends_on) WHERE completed_at IS NULL;
SELECT tenancy_protect('cycles', 'workspace_id');

-- One row per running cycle per day: its tasks, started and finished.
CREATE TABLE cycle_snapshots (
    cycle_id UUID NOT NULL REFERENCES cycles(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    on_date DATE NOT NULL,
    scope INTEGER NOT NULL,
    started INTEGER NOT NULL,
    completed INTEGER NOT NULL,
    scope_points DOUBLE PRECISION,
    completed_points DOUBLE PRECISION,
    PRIMARY KEY (cycle_id, on_date)
);

CREATE INDEX cycle_snapshots_by_workspace ON cycle_snapshots (workspace_id);
SELECT tenancy_protect('cycle_snapshots', 'workspace_id');

ALTER TABLE work_items
    ADD COLUMN cycle_id UUID,
    ADD CONSTRAINT work_items_cycle_fk
        FOREIGN KEY (project_id, cycle_id)
        REFERENCES cycles(space_id, id);

CREATE INDEX work_items_cycle ON work_items (project_id, cycle_id)
    WHERE cycle_id IS NOT NULL;

-- Tasks read through task_rows show their cycle too (a column added at the
-- end, so the view keeps its others).
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
  ON record.id = core.id AND record.project_id = core.project_id;
