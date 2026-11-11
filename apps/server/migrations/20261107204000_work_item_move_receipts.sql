-- AWD-102: the original result of each confirmed task move, so a retry of
-- the same request (same person, same request ID, same task, target,
-- preview snapshot and choices) gets that result back however late it
-- comes, and the same request ID with anything changed is refused.
--
-- It keeps only what the move made: the moved tasks' old and new keys and
-- new IDs as they were then, and how many custom field values stayed on the
-- originals. No field values, names or choices: the request is kept as a
-- SHA-256 digest, as is the request ID.
--
-- A receipt lives as long as the archived original it belongs to (the same
-- as that task's key alias, migration 0043) and goes with it if the task is
-- ever deleted.
CREATE TABLE work_item_move_receipts (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    source_project_id UUID NOT NULL REFERENCES projects(id),
    -- The moved task's archived original. A task moves once.
    source_work_item_id UUID NOT NULL UNIQUE
        REFERENCES work_items(id) ON DELETE CASCADE,
    target_project_id UUID NOT NULL REFERENCES projects(id),
    -- Who moved it; a request ID only ever matches its own sender.
    actor_id UUID NOT NULL REFERENCES actors(id),
    request_key_hash BYTEA NOT NULL CHECK (octet_length(request_key_hash) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    -- [{"from": "AW-7", "to": "AWD-12", "id": "<new task ID>"}, ...], the
    -- task first, then its subtasks.
    moved JSONB NOT NULL CHECK (
        jsonb_typeof(moved) = 'array' AND jsonb_array_length(moved) BETWEEN 1 AND 100
    ),
    left_behind INTEGER NOT NULL CHECK (left_behind >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (source_project_id <> target_project_id)
);
