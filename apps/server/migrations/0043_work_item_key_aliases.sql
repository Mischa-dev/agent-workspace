-- Tasks moved to another project (docs/plans/mcp-tools.md, decision 1).
--
-- A task can't change projects in place: its comments are immutable and its
-- links and authors are tied to its project. A move instead carries the
-- task over into the target project and archives the original. This table
-- keeps the original's key (such as AW-7) pointing at where it went, so old
-- links and `task_open AW-7` still find it.

CREATE TABLE work_item_key_aliases (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_key TEXT NOT NULL CHECK (project_key ~ '^[A-Z][A-Z0-9]{1,11}$'),
    number BIGINT NOT NULL CHECK (number > 0),
    -- The archived original, which keeps its history and links.
    source_work_item_id UUID NOT NULL UNIQUE REFERENCES work_items(id),
    -- Where it went.
    work_item_id UUID NOT NULL REFERENCES work_items(id),
    moved_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (source_work_item_id <> work_item_id)
);

CREATE UNIQUE INDEX work_item_key_aliases_key_unique
    ON work_item_key_aliases (organization_id, upper(project_key), number);

CREATE INDEX work_item_key_aliases_target
    ON work_item_key_aliases (work_item_id);
