-- K2.21: long operations run as jobs (the MCP Tasks extension).
--
-- * operation_tasks: one call of a long operation that was answered
--   "started" and finished by a job. It keeps who called (the caller, as
--   the edge resolved it, never a token), what they called it with until it
--   has run, and the answer, so any server process can tell the caller
--   (tasks/get, job_get) and the answer survives a restart. Only the caller
--   reads it. Finished rows go after seven days (retention).

CREATE TABLE operation_tasks (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),
    op TEXT NOT NULL CHECK (op ~ '^[a-z][a-z0-9_]{0,99}$'),
    status TEXT NOT NULL DEFAULT 'queued' CHECK (
        status IN ('queued', 'working', 'completed', 'cancelled')
    ),
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) BETWEEN 1 AND 128),
    input_hash TEXT NOT NULL CHECK (char_length(input_hash) <= 100),
    -- Cleared once the call has run: what it was called with is in the
    -- records it made.
    input JSONB CHECK (input IS NULL OR octet_length(input::text) <= 8388608),
    caller JSONB NOT NULL CHECK (jsonb_typeof(caller) = 'object'),
    result JSONB CHECK (result IS NULL OR jsonb_typeof(result) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    CHECK ((status IN ('completed', 'cancelled')) = (finished_at IS NOT NULL))
);

-- A retried call with the same request id finds the task it started.
CREATE UNIQUE INDEX operation_tasks_request
    ON operation_tasks (account_id, op, request_id)
    WHERE request_id IS NOT NULL;
CREATE INDEX operation_tasks_account ON operation_tasks (account_id, created_at DESC);
CREATE INDEX operation_tasks_finished ON operation_tasks (finished_at)
    WHERE finished_at IS NOT NULL;

SELECT tenancy_protect('operation_tasks', 'workspace_id');
