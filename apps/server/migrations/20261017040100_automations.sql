-- Automations (docs/plans/roadmap.md, workstream W14, X5).
--
-- A rule has a trigger (an event, a schedule or a capacity window),
-- conditions (a saved view's filters, or the same query inline) and up to
-- five actions. It acts as the person, app or agent that made it, and every
-- firing checks that actor's current access first; a rule that fails the
-- check is turned off with the reason, and its owner (or a project admin if
-- the owner left) is told.
CREATE TABLE automation_rules (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    owner_actor_id UUID NOT NULL REFERENCES actors(id),
    -- The connected app it was made through, if any; revoking the app stops it.
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    -- The agent connection an agent's rule acts through; removing it stops it.
    external_agent_connection_id UUID REFERENCES external_agent_connections(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name = btrim(name)),
    enabled BOOLEAN NOT NULL DEFAULT true,
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN (
        'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
        'message_posted', 'schedule', 'capacity_window'
    )),
    trigger JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(trigger) = 'object' AND octet_length(trigger::text) <= 4096
    ),
    -- A saved view whose filters are the conditions; checked when it fires.
    view_id UUID,
    -- Or the same list query inline, as the list page keeps it.
    conditions JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(conditions) = 'object' AND octet_length(conditions::text) <= 4096
    ),
    actions JSONB NOT NULL CHECK (
        jsonb_typeof(actions) = 'array'
        AND jsonb_array_length(actions) BETWEEN 1 AND 5
        AND octet_length(actions::text) <= 16384
    ),
    next_run_at TIMESTAMPTZ,
    disabled_reason TEXT CHECK (disabled_reason IS NULL OR char_length(disabled_reason) <= 500),
    last_fired_at TIMESTAMPTZ,
    fire_count BIGINT NOT NULL DEFAULT 0,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);

CREATE INDEX automation_rules_project ON automation_rules (project_id, created_at, id);
CREATE INDEX automation_rules_events
    ON automation_rules (project_id, trigger_kind)
    WHERE enabled;
CREATE INDEX automation_rules_due
    ON automation_rules (next_run_at)
    WHERE enabled AND next_run_at IS NOT NULL;
CREATE INDEX automation_rules_capacity
    ON automation_rules (owner_actor_id)
    WHERE enabled AND trigger_kind = 'capacity_window';

-- Each time a rule fired, what it did. The dedupe key makes one event or
-- schedule slot fire a rule once, however often its job runs.
CREATE TABLE automation_runs (
    id UUID PRIMARY KEY,
    rule_id UUID NOT NULL REFERENCES automation_rules(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id),
    dedupe_key TEXT NOT NULL CHECK (char_length(dedupe_key) BETWEEN 1 AND 300),
    trigger_summary TEXT NOT NULL CHECK (char_length(trigger_summary) BETWEEN 1 AND 300),
    subject JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(subject) = 'object'),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (
        state IN ('pending', 'succeeded', 'partial', 'failed', 'skipped')
    ),
    detail TEXT CHECK (detail IS NULL OR char_length(detail) <= 500),
    results JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(results) = 'array'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    UNIQUE (rule_id, dedupe_key)
);

CREATE INDEX automation_runs_log ON automation_runs (rule_id, created_at DESC, id DESC);
CREATE INDEX automation_runs_recent ON automation_runs (rule_id, created_at)
    WHERE state <> 'skipped';

-- Tasks a "task starts matching" rule last saw match, so it fires when a
-- task enters its conditions rather than on every change while it matches.
CREATE TABLE automation_rule_matches (
    rule_id UUID NOT NULL REFERENCES automation_rules(id) ON DELETE CASCADE,
    work_item_id UUID NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
    matched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (rule_id, work_item_id)
);

-- Launches an automation starts.
ALTER TABLE launches DROP CONSTRAINT launches_source_check;
ALTER TABLE launches ADD CONSTRAINT launches_source_check
    CHECK (source IN ('button', 'dialog', 'queue', 'tag', 'mcp', 'automation'));
