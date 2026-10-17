-- C4/C5: every launch, whether it starts at once or waits in the queue.
-- A launch belongs to the person who asked for it and is checked again
-- (their project access, their account, the environment) when it starts,
-- so one queued by someone who has since lost access never runs.
CREATE TABLE launches (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    work_item_id UUID,
    conversation_id UUID,
    source_message_id UUID,
    requested_by_actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID,
    source TEXT NOT NULL CHECK (source IN ('button', 'dialog', 'queue', 'tag', 'mcp')),
    environment_id UUID,
    -- Launch overrides (harness, model, reasoning, speed, attempts).
    overrides JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(overrides) = 'object' AND octet_length(overrides::text) <= 16384
    ),
    -- An edited prompt; null means the project's template renders it when
    -- the launch starts.
    prompt TEXT CHECK (prompt IS NULL OR char_length(prompt) BETWEEN 1 AND 100000),
    start_when TEXT NOT NULL CHECK (
        start_when IN ('now', 'at', 'window_reset', 'blockers_done')
    ),
    start_at TIMESTAMPTZ,
    provider_account_id UUID REFERENCES provider_accounts(id),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (
        state IN ('queued', 'started', 'cancelled', 'failed')
    ),
    -- Why it is waiting, or why it was cancelled or failed, in plain words.
    status_reason TEXT CHECK (status_reason IS NULL OR char_length(status_reason) <= 500),
    agent_run_id UUID,
    position BIGINT NOT NULL,
    idempotency_key TEXT CHECK (
        idempotency_key IS NULL OR char_length(idempotency_key) BETWEEN 1 AND 200
    ),
    cancelled_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    UNIQUE (project_id, id),
    UNIQUE (requested_by_actor_id, idempotency_key),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, source_message_id) REFERENCES messages(project_id, id),
    FOREIGN KEY (project_id, environment_id) REFERENCES project_environments(project_id, id),
    FOREIGN KEY (project_id, agent_run_id) REFERENCES agent_runs(project_id, id),
    CHECK (work_item_id IS NOT NULL OR conversation_id IS NOT NULL),
    CHECK ((start_when IN ('at', 'window_reset')) = (start_at IS NOT NULL)),
    CHECK ((state = 'started') = (agent_run_id IS NOT NULL))
);

CREATE INDEX launches_queue ON launches (project_id, position) WHERE state = 'queued';
CREATE INDEX launches_due ON launches (start_at) WHERE state = 'queued';
CREATE INDEX launches_work_item ON launches (project_id, work_item_id, created_at DESC);
-- A tag launches once per message.
CREATE UNIQUE INDEX launches_one_per_tag ON launches (source_message_id)
    WHERE source = 'tag';

CREATE FUNCTION launches_match_workspace()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM projects
        WHERE id = NEW.project_id AND organization_id = NEW.organization_id
    ) THEN
        RAISE EXCEPTION 'Launch workspace must match its project' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER launches_match_workspace
    BEFORE INSERT OR UPDATE OF organization_id, project_id ON launches
    FOR EACH ROW EXECUTE FUNCTION launches_match_workspace();
