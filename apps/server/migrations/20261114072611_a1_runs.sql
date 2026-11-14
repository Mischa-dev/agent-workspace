-- A1.1 (build 1G): the run record, for every agent whoever runs it.
--
-- * runs: one piece of work by an agent account. A run the built-in host
--   drives keeps its detail row in agent_runs with the same id; a run an
--   outside agent or host reports has no detail row. host_id is a plain
--   UUID until A1.11 adds the hosts table and its foreign key.
-- * run_activities: typed entries in a run, named after AG-UI's events.
-- * usage_reports: what a reporter said it used, as it said it.
-- * run_usage: usage per run and model, as the run page and totals show it.
--
-- The built-in host still writes agent_runs, agent_run_events and
-- agent_run_usage. Three mirror triggers (R16; removed in A1.22) keep the new
-- tables equal to them, so every legacy writer stays in step without being
-- touched:
--
-- | table            | trigger                 | writes         |
-- | agent_runs       | agent_runs_mirror       | runs           |
-- | agent_run_events | agent_run_events_mirror | run_activities |
-- | agent_run_usage  | agent_run_usage_mirror  | run_usage      |
--
-- Runs and usage are copied here. Activities are copied by the resumable job
-- runs.backfill_activities (queued below), in batches, because
-- agent_run_events is the largest of the run tables.
--
-- space_id names a project until K4's rename; workspace_id an organization
-- (R1).

CREATE TABLE runs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    -- The agent account doing the work.
    account_id UUID NOT NULL REFERENCES actors(id),
    -- Null: the agent reports for itself. The foreign key arrives with hosts
    -- (A1.11).
    host_id UUID,
    host_run_ref TEXT CHECK (
        host_run_ref IS NULL
        OR (char_length(host_run_ref) BETWEEN 1 AND 200 AND host_run_ref !~ '[[:cntrl:]]')
    ),
    -- Only when the host discloses it.
    backend TEXT CHECK (backend IS NULL OR char_length(backend) BETWEEN 1 AND 80),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200 AND title !~ '[[:cntrl:]]'),
    -- Why it exists.
    cause_kind TEXT NOT NULL CHECK (cause_kind IN (
        'task', 'thread', 'message', 'schedule', 'host_trigger', 'automation', 'request', 'manual'
    )),
    cause_ref TEXT CHECK (cause_ref IS NULL OR char_length(cause_ref) <= 200),
    conversation_id UUID,
    work_item_id UUID,
    cause_message_id UUID,
    requested_by_account_id UUID REFERENCES actors(id),
    sponsored_by_account_id UUID REFERENCES actors(id),
    via_app_id UUID,
    parent_run_id UUID REFERENCES runs(id),
    predecessor_run_id UUID REFERENCES runs(id),
    launch_id UUID,
    -- Where it is.
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'starting', 'start_uncertain', 'accepted', 'working', 'waiting',
        'cancel_requested', 'done', 'failed', 'cancelled', 'refused', 'interrupted'
    )),
    liveness TEXT NOT NULL DEFAULT 'ok' CHECK (liveness IN ('ok', 'quiet', 'not_responding')),
    -- The oldest open request raised by this run; derived.
    waiting_on_request_id UUID,
    status_line TEXT CHECK (status_line IS NULL OR char_length(status_line) <= 200),
    outcome_summary TEXT CHECK (outcome_summary IS NULL OR char_length(outcome_summary) <= 2000),
    failure_reason TEXT CHECK (failure_reason IS NULL OR char_length(failure_reason) <= 2000),
    external_url TEXT CHECK (
        external_url IS NULL OR external_url ~ '^https://[^[:space:][:cntrl:]]+$'
    ),
    -- Reporting.
    client_run_id TEXT CHECK (
        client_run_id IS NULL OR char_length(client_run_id) BETWEEN 1 AND 200
    ),
    report_interval_seconds INTEGER NOT NULL DEFAULT 900
        CHECK (report_interval_seconds BETWEEN 30 AND 86400),
    last_report_at TIMESTAMPTZ,
    activity_seq BIGINT NOT NULL DEFAULT 0 CHECK (activity_seq >= 0),
    gap_count INTEGER NOT NULL DEFAULT 0 CHECK (gap_count >= 0),
    -- Settings the run used, frozen once set (the rule agent_runs.config has).
    settings JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(settings) = 'object' AND octet_length(settings::text) <= 32768
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    cancel_requested_at TIMESTAMPTZ,
    cancel_requested_by_account_id UUID REFERENCES actors(id),
    finished_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (space_id, id),
    UNIQUE (conversation_id, id),
    FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (space_id, work_item_id) REFERENCES work_items(project_id, id),
    CHECK (parent_run_id IS NULL OR parent_run_id <> id),
    CHECK (predecessor_run_id IS NULL OR predecessor_run_id <> id),
    CHECK ((state IN ('done', 'failed', 'cancelled', 'refused', 'interrupted')) = (finished_at IS NOT NULL))
);

CREATE UNIQUE INDEX runs_host_ref ON runs (host_id, host_run_ref) WHERE host_run_ref IS NOT NULL;
CREATE UNIQUE INDEX runs_client_ref ON runs (account_id, client_run_id) WHERE client_run_id IS NOT NULL;
CREATE INDEX runs_workspace_recent ON runs (workspace_id, created_at DESC, id DESC);
CREATE INDEX runs_space_recent ON runs (space_id, created_at DESC, id DESC);
CREATE INDEX runs_active_conversation ON runs (conversation_id)
    WHERE state NOT IN ('done', 'failed', 'cancelled', 'refused', 'interrupted');
CREATE INDEX runs_active_account ON runs (account_id)
    WHERE state NOT IN ('done', 'failed', 'cancelled', 'refused', 'interrupted');
CREATE INDEX runs_active_host ON runs (host_id)
    WHERE host_id IS NOT NULL AND state NOT IN ('done', 'failed', 'cancelled', 'refused', 'interrupted');
CREATE INDEX runs_liveness_due ON runs (last_report_at)
    WHERE state IN ('accepted', 'working', 'waiting', 'cancel_requested');

-- Settings are frozen once set, as agent_runs.config is.
CREATE FUNCTION runs_freeze_settings()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.settings <> '{}'::jsonb AND NEW.settings IS DISTINCT FROM OLD.settings THEN
        RAISE EXCEPTION 'a run''s settings are frozen once set' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER runs_freeze_settings
BEFORE UPDATE OF settings ON runs
FOR EACH ROW
EXECUTE FUNCTION runs_freeze_settings();

CREATE TABLE run_activities (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    -- The order Workspace received them in.
    seq BIGINT NOT NULL CHECK (seq > 0),
    client_id TEXT CHECK (client_id IS NULL OR char_length(client_id) BETWEEN 1 AND 200),
    kind TEXT NOT NULL CHECK (kind IN (
        'run_started', 'run_finished', 'run_error',
        'step_started', 'step_finished',
        'text_message', 'reasoning', 'tool_call',
        'plan', 'state_snapshot', 'state_delta',
        'interrupt', 'interrupt_resolved',
        'usage', 'result', 'gap', 'custom'
    )),
    step_ref TEXT CHECK (step_ref IS NULL OR char_length(step_ref) <= 200),
    parent_ref TEXT CHECK (parent_ref IS NULL OR char_length(parent_ref) <= 200),
    summary TEXT NOT NULL CHECK (char_length(summary) <= 2000),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 131072
    ),
    occurred_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reported_by_account_id UUID NOT NULL REFERENCES actors(id),
    in_thread BOOLEAN NOT NULL DEFAULT false,
    -- agent_run_events.id when copied from there.
    legacy_event_id UUID UNIQUE,
    UNIQUE (run_id, seq),
    UNIQUE (run_id, client_id)
);

CREATE INDEX run_activities_run_order ON run_activities (run_id, seq);

CREATE TABLE usage_reports (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    reported_by_account_id UUID NOT NULL REFERENCES actors(id),
    -- Whose work it was.
    account_id UUID NOT NULL REFERENCES actors(id),
    host_id UUID,
    -- Null: a report for a time window.
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    report_key TEXT NOT NULL CHECK (char_length(report_key) BETWEEN 1 AND 200),
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    window_start TIMESTAMPTZ,
    window_end TIMESTAMPTZ,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    reported_cost DOUBLE PRECISION CHECK (reported_cost IS NULL OR reported_cost >= 0),
    reported_currency TEXT CHECK (reported_currency IS NULL OR reported_currency ~ '^[A-Z]{3}$'),
    attributed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (reported_by_account_id, report_key),
    CHECK (run_id IS NOT NULL OR (window_start IS NOT NULL AND window_end > window_start))
);

CREATE INDEX usage_reports_unattributed ON usage_reports (created_at)
    WHERE run_id IS NULL AND attributed_at IS NULL;
CREATE INDEX usage_reports_workspace ON usage_reports (workspace_id, created_at DESC);

CREATE TABLE run_usage (
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    attribution TEXT NOT NULL CHECK (attribution IN ('reported', 'window')),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    host_id UUID,
    backend TEXT CHECK (backend IS NULL OR char_length(backend) BETWEEN 1 AND 80),
    usage_date DATE NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    estimated_cost DOUBLE PRECISION CHECK (estimated_cost IS NULL OR estimated_cost >= 0),
    currency TEXT CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    reported_cost DOUBLE PRECISION CHECK (reported_cost IS NULL OR reported_cost >= 0),
    reported_currency TEXT CHECK (reported_currency IS NULL OR reported_currency ~ '^[A-Z]{3}$'),
    confidence TEXT NOT NULL CHECK (confidence IN ('exact', 'estimated')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, model, attribution)
);

CREATE INDEX run_usage_workspace_day ON run_usage (workspace_id, usage_date);
CREATE INDEX run_usage_account_day ON run_usage (account_id, usage_date);

-- The new state of a built-in run, from its agent_runs state (A1 3.2).
-- "interrupted" is "cancelled" when the person who asked for the run pressed
-- Stop (the reason interrupt_run_in records), otherwise "interrupted".
CREATE FUNCTION runs_state_from_legacy(legacy TEXT, reason TEXT)
RETURNS TEXT
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE legacy
        WHEN 'created' THEN 'queued'
        WHEN 'queued' THEN 'queued'
        WHEN 'starting' THEN 'starting'
        WHEN 'working' THEN 'working'
        WHEN 'blocked' THEN 'waiting'
        WHEN 'recovering' THEN 'working'
        WHEN 'completed' THEN 'done'
        WHEN 'failed' THEN 'failed'
        WHEN 'canceled' THEN 'cancelled'
        WHEN 'interrupted' THEN
            CASE WHEN reason = 'Interrupted by the requesting Actor.' THEN 'cancelled'
                 ELSE 'interrupted' END
    END
$$;

-- A run's title from its prompt: the first line, on one line, at most 200
-- characters.
CREATE FUNCTION runs_title_from_prompt(prompt TEXT)
RETURNS TEXT
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT COALESCE(
        NULLIF(left(btrim(regexp_replace(split_part(btrim(prompt), E'\n', 1), '[[:cntrl:]]+', ' ', 'g')), 200), ''),
        'Run'
    )
$$;

-- Upsert the runs row for one agent_runs row. The run service marks the
-- transaction (runs.service_write = the run's id) when it writes a built-in
-- run itself; the mirror then leaves that row alone.
CREATE FUNCTION runs_mirror_agent_run()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    mapped TEXT;
    terminal BOOLEAN;
BEGIN
    IF current_setting('runs.service_write', true) = NEW.id::text THEN
        RETURN NEW;
    END IF;
    mapped := runs_state_from_legacy(NEW.state, NEW.failure_reason);
    terminal := mapped IN ('done', 'failed', 'cancelled', 'refused', 'interrupted');
    INSERT INTO runs (
        id, workspace_id, space_id, account_id, title, cause_kind,
        conversation_id, requested_by_account_id, sponsored_by_account_id,
        predecessor_run_id, state, status_line, failure_reason, last_report_at,
        settings, created_at, started_at, finished_at, updated_at
    )
    SELECT NEW.id, project.organization_id, NEW.project_id, worker.actor_id,
           runs_title_from_prompt(NEW.prompt),
           CASE WHEN NEW.external_execution_request_id IS NULL THEN 'thread' ELSE 'request' END,
           NEW.conversation_id, NEW.requesting_actor_id, NEW.sponsoring_actor_id,
           NEW.predecessor_run_id, mapped,
           CASE WHEN NEW.state = 'recovering' THEN 'Recovering after a restart' END,
           left(NEW.failure_reason, 2000), NEW.updated_at, NEW.config,
           NEW.created_at, NEW.started_at,
           CASE WHEN terminal THEN COALESCE(NEW.finished_at, NEW.updated_at) END,
           NEW.updated_at
    FROM projects project
    JOIN execution_workers worker ON worker.id = NEW.execution_worker_id
    WHERE project.id = NEW.project_id
    ON CONFLICT (id) DO UPDATE SET
        title = EXCLUDED.title,
        state = EXCLUDED.state,
        status_line = EXCLUDED.status_line,
        failure_reason = EXCLUDED.failure_reason,
        last_report_at = EXCLUDED.last_report_at,
        settings = CASE WHEN runs.settings = '{}'::jsonb THEN EXCLUDED.settings ELSE runs.settings END,
        started_at = EXCLUDED.started_at,
        finished_at = EXCLUDED.finished_at,
        liveness = 'ok',
        version = runs.version + 1,
        updated_at = EXCLUDED.updated_at
    WHERE (runs.title, runs.state, runs.status_line, runs.failure_reason, runs.started_at,
           runs.finished_at)
          IS DISTINCT FROM
          (EXCLUDED.title, EXCLUDED.state, EXCLUDED.status_line, EXCLUDED.failure_reason,
           EXCLUDED.started_at, EXCLUDED.finished_at);
    RETURN NEW;
END;
$$;

-- The activity an agent_run_events row becomes (A1 3.3).
CREATE FUNCTION runs_activity_kind_from_legacy(legacy TEXT, payload JSONB)
RETURNS TEXT
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE
        WHEN legacy = 'run_state_changed' THEN
            CASE
                WHEN payload->>'to' IN ('starting', 'working') THEN 'run_started'
                WHEN payload->>'to' IN ('completed', 'failed', 'canceled', 'interrupted') THEN 'run_finished'
                ELSE 'custom'
            END
        WHEN legacy IN ('command_started', 'command_completed', 'file_change', 'tool_used', 'test_result')
            THEN 'tool_call'
        WHEN legacy IN ('approval_requested', 'question_requested') THEN 'interrupt'
        WHEN legacy IN ('approval_resolved', 'question_resolved') THEN 'interrupt_resolved'
        WHEN legacy IN ('run_completed', 'run_failed', 'run_interrupted') THEN 'run_finished'
        WHEN legacy = 'agent_note' THEN 'text_message'
        ELSE 'custom'
    END
$$;

-- The payload an agent_run_events row becomes: the old payload with its kind
-- kept beside it, and the outcome a finished run reports.
CREATE FUNCTION runs_activity_payload_from_legacy(legacy TEXT, payload JSONB)
RETURNS JSONB
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE
        WHEN octet_length(merged::text) > 60000
            THEN jsonb_build_object('legacyKind', legacy, 'truncated', true)
        ELSE merged
    END
    FROM (
        SELECT COALESCE(payload, '{}'::jsonb)
               || jsonb_build_object('legacyKind', legacy)
               || CASE
                      WHEN legacy = 'run_completed' OR (legacy = 'run_state_changed' AND payload->>'to' = 'completed')
                          THEN jsonb_build_object('outcome', 'done')
                      WHEN legacy = 'run_failed' OR (legacy = 'run_state_changed' AND payload->>'to' = 'failed')
                          THEN jsonb_build_object('outcome', 'failed')
                      WHEN legacy = 'run_interrupted' OR (legacy = 'run_state_changed' AND payload->>'to' IN ('canceled', 'interrupted'))
                          THEN jsonb_build_object('outcome', 'cancelled')
                      WHEN legacy = 'agent_note' AND NOT (payload ? 'text')
                          THEN jsonb_build_object('role', 'note')
                      ELSE '{}'::jsonb
                  END AS merged
    ) built
$$;

-- Copy one agent_run_events row into run_activities, once.
CREATE FUNCTION runs_copy_legacy_event(event agent_run_events)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    next_seq BIGINT;
    run_workspace UUID;
    run_account UUID;
BEGIN
    IF EXISTS (SELECT 1 FROM run_activities WHERE legacy_event_id = event.id) THEN
        RETURN;
    END IF;
    UPDATE runs
       SET activity_seq = activity_seq + 1
     WHERE id = event.agent_run_id
    RETURNING activity_seq, workspace_id, account_id
         INTO next_seq, run_workspace, run_account;
    IF next_seq IS NULL THEN
        RETURN;
    END IF;
    INSERT INTO run_activities (
        id, run_id, workspace_id, seq, kind, summary, payload, occurred_at,
        received_at, reported_by_account_id, legacy_event_id
    )
    VALUES (
        event.id, event.agent_run_id, run_workspace, next_seq,
        runs_activity_kind_from_legacy(event.kind, event.payload),
        left(event.summary, 2000),
        runs_activity_payload_from_legacy(event.kind, event.payload),
        event.created_at, event.created_at, run_account, event.id
    )
    ON CONFLICT DO NOTHING;
END;
$$;

CREATE FUNCTION runs_mirror_agent_run_event()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        UPDATE run_activities
           SET kind = runs_activity_kind_from_legacy(NEW.kind, NEW.payload),
               summary = left(NEW.summary, 2000),
               payload = runs_activity_payload_from_legacy(NEW.kind, NEW.payload)
         WHERE legacy_event_id = NEW.id;
        IF FOUND THEN
            RETURN NEW;
        END IF;
    END IF;
    PERFORM runs_copy_legacy_event(NEW);
    RETURN NEW;
END;
$$;

-- Upsert one agent_run_usage row into run_usage, as reported, exact.
CREATE FUNCTION runs_mirror_agent_run_usage()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO run_usage (
        run_id, model, attribution, workspace_id, space_id, account_id, backend,
        usage_date, input_tokens, output_tokens, cached_input_tokens,
        estimated_cost, currency, reported_cost, reported_currency, confidence, updated_at
    )
    SELECT NEW.agent_run_id, NEW.model, 'reported', NEW.organization_id, NEW.project_id,
           run.account_id, left(NEW.adapter_kind, 80), NEW.usage_date, NEW.input_tokens,
           NEW.output_tokens, NEW.cached_input_tokens, NEW.estimated_cost, NEW.currency,
           NEW.reported_cost_usd,
           CASE WHEN NEW.reported_cost_usd IS NOT NULL THEN 'USD' END,
           'exact', NEW.updated_at
    FROM runs run
    WHERE run.id = NEW.agent_run_id
    ON CONFLICT (run_id, model, attribution) DO UPDATE SET
        usage_date = EXCLUDED.usage_date,
        input_tokens = EXCLUDED.input_tokens,
        output_tokens = EXCLUDED.output_tokens,
        cached_input_tokens = EXCLUDED.cached_input_tokens,
        estimated_cost = EXCLUDED.estimated_cost,
        currency = EXCLUDED.currency,
        reported_cost = EXCLUDED.reported_cost,
        reported_currency = EXCLUDED.reported_currency,
        updated_at = EXCLUDED.updated_at;
    RETURN NEW;
END;
$$;

-- Copy every run there is now.
INSERT INTO runs (
    id, workspace_id, space_id, account_id, title, cause_kind, conversation_id,
    requested_by_account_id, sponsored_by_account_id, state, status_line,
    failure_reason, last_report_at, settings, created_at, started_at, finished_at,
    updated_at
)
SELECT run.id, project.organization_id, run.project_id, worker.actor_id,
       runs_title_from_prompt(run.prompt),
       CASE WHEN run.external_execution_request_id IS NULL THEN 'thread' ELSE 'request' END,
       run.conversation_id, run.requesting_actor_id, run.sponsoring_actor_id,
       runs_state_from_legacy(run.state, run.failure_reason),
       CASE WHEN run.state = 'recovering' THEN 'Recovering after a restart' END,
       left(run.failure_reason, 2000), run.updated_at, run.config, run.created_at,
       run.started_at,
       CASE WHEN runs_state_from_legacy(run.state, run.failure_reason)
                 IN ('done', 'failed', 'cancelled', 'refused', 'interrupted')
            THEN COALESCE(run.finished_at, run.updated_at) END,
       run.updated_at
FROM agent_runs run
JOIN projects project ON project.id = run.project_id
JOIN execution_workers worker ON worker.id = run.execution_worker_id;

-- Retries point at the run they repeat, once both rows exist.
UPDATE runs SET predecessor_run_id = run.predecessor_run_id
FROM agent_runs run
WHERE run.id = runs.id AND run.predecessor_run_id IS NOT NULL;

INSERT INTO run_usage (
    run_id, model, attribution, workspace_id, space_id, account_id, backend,
    usage_date, input_tokens, output_tokens, cached_input_tokens, estimated_cost,
    currency, reported_cost, reported_currency, confidence, updated_at
)
SELECT usage.agent_run_id, usage.model, 'reported', usage.organization_id,
       usage.project_id, run.account_id, left(usage.adapter_kind, 80), usage.usage_date,
       usage.input_tokens, usage.output_tokens, usage.cached_input_tokens,
       usage.estimated_cost, usage.currency, usage.reported_cost_usd,
       CASE WHEN usage.reported_cost_usd IS NOT NULL THEN 'USD' END, 'exact',
       usage.updated_at
FROM agent_run_usage usage
JOIN runs run ON run.id = usage.agent_run_id;

CREATE TRIGGER agent_runs_mirror
AFTER INSERT OR UPDATE ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION runs_mirror_agent_run();

CREATE TRIGGER agent_run_events_mirror
AFTER INSERT OR UPDATE ON agent_run_events
FOR EACH ROW
EXECUTE FUNCTION runs_mirror_agent_run_event();

CREATE TRIGGER agent_run_usage_mirror
AFTER INSERT OR UPDATE ON agent_run_usage
FOR EACH ROW
EXECUTE FUNCTION runs_mirror_agent_run_usage();

-- Copy up to `batch` agent_run_events rows after `after` (by id), and return
-- the last id looked at (NULL when there were none). Safe to run twice:
-- legacy_event_id is unique and copied rows are skipped.
CREATE FUNCTION runs_backfill_activities(after UUID, batch INTEGER)
RETURNS UUID
LANGUAGE plpgsql
AS $$
DECLARE
    event agent_run_events;
    last_id UUID;
BEGIN
    FOR event IN
        SELECT * FROM agent_run_events
        WHERE after IS NULL OR id > after
        ORDER BY id
        LIMIT batch
    LOOP
        PERFORM runs_copy_legacy_event(event);
        last_id := event.id;
    END LOOP;
    RETURN last_id;
END;
$$;

-- Copy the activities there are now, in batches, from the job queue.
INSERT INTO background_jobs (id, kind, payload, unique_key, max_attempts)
SELECT gen_random_uuid(), 'runs.backfill_activities', '{}'::jsonb, 'runs.backfill_activities', 50
WHERE EXISTS (SELECT 1 FROM agent_run_events);
