-- Task dependencies and milestones (docs/plans/mcp-tools.md, Phase 5).
--
-- A dependency says one task blocks another in the same project. Cycles are
-- refused by the command that adds them. A milestone is a named target in a
-- project (a release or checkpoint) that tasks can belong to.

CREATE TABLE work_item_dependencies (
    project_id UUID NOT NULL,
    blocking_work_item_id UUID NOT NULL,
    blocked_work_item_id UUID NOT NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, blocking_work_item_id, blocked_work_item_id),
    FOREIGN KEY (project_id, blocking_work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, blocked_work_item_id) REFERENCES work_items(project_id, id),
    CHECK (blocking_work_item_id <> blocked_work_item_id)
);

CREATE INDEX work_item_dependencies_blocked
    ON work_item_dependencies (project_id, blocked_work_item_id);

CREATE TABLE project_milestones (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120 AND name = btrim(name)
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    target_date DATE,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    archived_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);

CREATE UNIQUE INDEX project_milestones_active_name_unique
    ON project_milestones (project_id, lower(name))
    WHERE archived_at IS NULL;

ALTER TABLE work_items
    ADD COLUMN milestone_id UUID,
    ADD CONSTRAINT work_items_milestone_fk
        FOREIGN KEY (project_id, milestone_id)
        REFERENCES project_milestones(project_id, id);

CREATE INDEX work_items_milestone ON work_items (project_id, milestone_id)
    WHERE milestone_id IS NOT NULL;
