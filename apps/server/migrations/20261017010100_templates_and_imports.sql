-- Templates, recurring tasks, imports and exports (docs/plans/roadmap.md,
-- workstream W11, T9 and T10).

-- A task template: what a new task starts with, its acceptance criteria and
-- its subtasks. Labels are kept by name, so a template outlives a label
-- being archived and recreated.
CREATE TABLE task_templates (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name = btrim(name)
    ),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 240),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 20000),
    type_id UUID,
    priority TEXT NOT NULL DEFAULT 'no_priority' CHECK (
        priority IN ('no_priority', 'low', 'medium', 'high', 'urgent')
    ),
    labels TEXT[] NOT NULL DEFAULT ARRAY[]::text[] CHECK (cardinality(labels) <= 20),
    acceptance_criteria TEXT[] NOT NULL DEFAULT ARRAY[]::text[] CHECK (
        cardinality(acceptance_criteria) <= 50
    ),
    -- [{"title": "...", "description": "..."}], in order.
    subtasks JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(subtasks) = 'array'
        AND jsonb_array_length(subtasks) <= 50
        AND octet_length(subtasks::text) <= 65536
    ),
    estimate DOUBLE PRECISION CHECK (estimate IS NULL OR (estimate >= 0 AND estimate <= 100000)),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    archived_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, type_id) REFERENCES work_item_types(project_id, id)
);

CREATE UNIQUE INDEX task_templates_active_name
    ON task_templates (project_id, lower(name))
    WHERE archived_at IS NULL;

-- A project template belongs to a workspace: a snapshot of one project's
-- workflow, labels, channels, project settings and task templates, which a
-- new project can start from.
CREATE TABLE project_templates (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name = btrim(name)
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    content JSONB NOT NULL CHECK (
        jsonb_typeof(content) = 'object' AND octet_length(content::text) <= 262144
    ),
    source_project_id UUID REFERENCES projects(id),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    archived_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX project_templates_active_name
    ON project_templates (organization_id, lower(name))
    WHERE archived_at IS NULL;

-- A task made from a template on a schedule. It acts as the person who set
-- it up, and checks they can still create tasks in the project each time
-- (principle 9); if they can't, it pauses and says why.
CREATE TABLE recurring_tasks (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    template_id UUID NOT NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    frequency TEXT NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    repeat_every INTEGER NOT NULL DEFAULT 1 CHECK (repeat_every BETWEEN 1 AND 52),
    -- Weekly: ISO weekdays, 1 (Monday) to 7 (Sunday).
    weekdays SMALLINT[] NOT NULL DEFAULT ARRAY[]::smallint[] CHECK (
        weekdays <@ ARRAY[1, 2, 3, 4, 5, 6, 7]::smallint[]
    ),
    -- Monthly: the day of the month; a shorter month uses its last day.
    month_day SMALLINT CHECK (month_day IS NULL OR month_day BETWEEN 1 AND 31),
    time_of_day TIME NOT NULL DEFAULT '09:00',
    -- An IANA zone such as Europe/Amsterdam, checked against the database.
    timezone TEXT NOT NULL DEFAULT 'UTC' CHECK (char_length(timezone) BETWEEN 1 AND 64),
    -- The first local date it may run on.
    starts_on DATE NOT NULL,
    assignee_actor_id UUID,
    -- The new task is due this many days after it's created.
    due_in_days INTEGER CHECK (due_in_days IS NULL OR due_in_days BETWEEN 0 AND 365),
    enabled BOOLEAN NOT NULL DEFAULT true,
    paused_reason TEXT CHECK (paused_reason IS NULL OR char_length(paused_reason) <= 200),
    next_run_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    last_work_item_id UUID,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    CHECK (frequency <> 'weekly' OR cardinality(weekdays) >= 1),
    CHECK (frequency <> 'monthly' OR month_day IS NOT NULL),
    FOREIGN KEY (project_id, template_id) REFERENCES task_templates(project_id, id),
    FOREIGN KEY (project_id, assignee_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, last_work_item_id) REFERENCES work_items(project_id, id)
);

CREATE INDEX recurring_tasks_due
    ON recurring_tasks (next_run_at)
    WHERE enabled AND next_run_at IS NOT NULL;

-- One row per occurrence that made a task, so a job that runs twice for the
-- same occurrence (delivery is at least once) makes one task.
CREATE TABLE recurring_task_occurrences (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    recurring_task_id UUID NOT NULL,
    occurrence_at TIMESTAMPTZ NOT NULL,
    work_item_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (recurring_task_id, occurrence_at),
    FOREIGN KEY (project_id, recurring_task_id)
        REFERENCES recurring_tasks(project_id, id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id)
);

-- An import: what was read (rows ready to become tasks), how its columns map
-- to task fields, and how far creating them got. A GitHub import starts
-- `fetching` while a job reads the issues. The source text is kept
-- while it's a draft so a changed mapping can be applied again, and dropped
-- once it's committed.
CREATE TABLE task_imports (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    source TEXT NOT NULL CHECK (source IN ('markdown', 'csv', 'github')),
    source_name TEXT NOT NULL DEFAULT '' CHECK (char_length(source_name) <= 200),
    -- The GitHub repository, for GitHub imports.
    repository_id UUID,
    raw_text TEXT CHECK (raw_text IS NULL OR octet_length(raw_text) <= 4194304),
    mapping JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(mapping) = 'object'),
    options JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(options) = 'object'),
    rows JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(rows) = 'array'),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (
        status IN ('fetching', 'draft', 'queued', 'running', 'completed', 'failed', 'canceled')
    ),
    row_count INTEGER NOT NULL DEFAULT 0 CHECK (row_count BETWEEN 0 AND 5000),
    -- Rows the commit job has worked through, created or skipped.
    processed_count INTEGER NOT NULL DEFAULT 0,
    created_count INTEGER NOT NULL DEFAULT 0,
    skipped_count INTEGER NOT NULL DEFAULT 0,
    error TEXT CHECK (error IS NULL OR char_length(error) <= 500),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, repository_id) REFERENCES repositories(project_id, id)
);

CREATE INDEX task_imports_project_created ON task_imports (project_id, created_at DESC);

-- Tasks made by imports, by where they came from, so importing the same
-- issues again skips the ones already here.
CREATE TABLE task_import_links (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    -- Such as "github:1296269" (a repository) or "csv:Linear".
    source_key TEXT NOT NULL CHECK (char_length(source_key) BETWEEN 1 AND 120),
    external_id TEXT NOT NULL CHECK (char_length(external_id) BETWEEN 1 AND 200),
    work_item_id UUID NOT NULL,
    import_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, source_key, external_id),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, import_id) REFERENCES task_imports(project_id, id)
);

CREATE INDEX task_import_links_import ON task_import_links (import_id);
