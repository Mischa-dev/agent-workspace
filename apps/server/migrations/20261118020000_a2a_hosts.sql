-- A1.20: the A2A binding. A person links an agent that speaks A2A by its
-- Agent Card's address; it becomes a host whose connection is `a2a`. The
-- server is its A2A client: it sends the host's commands as A2A calls
-- (on the job queue) and turns the agent's task updates into run
-- reports. The agent runs elsewhere; nothing of it runs here.
--
-- * hosts.a2a_card_url: the card's https address, as the person gave it.
-- * hosts.a2a_endpoint: the JSON-RPC address the card names.
-- * hosts.a2a_token_secret_id: a bearer token for the agent, sealed in
--   `secrets`, when its card asks for one.
-- * host_a2a_tasks: one row per run on an A2A host: the agent's task and
--   context ids and when to look at the task next. Due rows are polled
--   (`tasks/get`); a finished task's row goes.

ALTER TABLE hosts
    DROP CONSTRAINT hosts_connection_check,
    ADD CONSTRAINT hosts_connection_check CHECK (connection IN ('dial_out', 'push', 'a2a')),
    ADD COLUMN a2a_card_url TEXT CHECK (
        a2a_card_url IS NULL
        OR (char_length(a2a_card_url) <= 1000 AND a2a_card_url ~ '^https?://[^[:space:][:cntrl:]]+$')
    ),
    ADD COLUMN a2a_endpoint TEXT CHECK (
        a2a_endpoint IS NULL
        OR (char_length(a2a_endpoint) <= 1000 AND a2a_endpoint ~ '^https?://[^[:space:][:cntrl:]]+$')
    ),
    ADD COLUMN a2a_token_secret_id UUID,
    ADD CONSTRAINT hosts_a2a_endpoint CHECK ((connection = 'a2a') = (a2a_endpoint IS NOT NULL));

CREATE TABLE host_a2a_tasks (
    run_id UUID PRIMARY KEY REFERENCES runs(id),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    host_id UUID NOT NULL REFERENCES hosts(id),
    task_id TEXT NOT NULL CHECK (char_length(task_id) BETWEEN 1 AND 200 AND task_id !~ '[[:cntrl:]]'),
    context_id TEXT CHECK (
        context_id IS NULL OR (char_length(context_id) BETWEEN 1 AND 200 AND context_id !~ '[[:cntrl:]]')
    ),
    task_state TEXT NOT NULL DEFAULT 'submitted' CHECK (char_length(task_state) BETWEEN 1 AND 40),
    polls INTEGER NOT NULL DEFAULT 0 CHECK (polls >= 0),
    next_poll_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (host_id, task_id)
);

CREATE INDEX host_a2a_tasks_due ON host_a2a_tasks (next_poll_at);
CREATE INDEX host_a2a_tasks_workspace ON host_a2a_tasks (workspace_id);
SELECT tenancy_protect('host_a2a_tasks', 'workspace_id');
