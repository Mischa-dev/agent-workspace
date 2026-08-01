CREATE TABLE work_item_worker_invocations (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    requesting_actor_id UUID NOT NULL REFERENCES actors(id),
    idempotency_key TEXT NOT NULL CHECK (char_length(idempotency_key) BETWEEN 1 AND 200),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    repository_id UUID NOT NULL,
    agent_run_id UUID,
    response_body JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (requesting_actor_id, work_item_id, idempotency_key),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    FOREIGN KEY (project_id, agent_run_id)
        REFERENCES agent_runs(project_id, id)
);

CREATE UNIQUE INDEX work_item_worker_invocations_one_run
    ON work_item_worker_invocations (agent_run_id)
    WHERE agent_run_id IS NOT NULL;
