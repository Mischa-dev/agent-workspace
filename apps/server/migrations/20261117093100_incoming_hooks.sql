-- K3.26: incoming webhooks as an automation trigger.
--
-- An incoming hook is an address in one space that an outside service posts
-- to. Its signing secret lives in the secrets store (`secret_id`) and is
-- shown once, to whoever made the hook. A delivery whose signature checks
-- out is kept in `incoming_hook_deliveries` with its body, and becomes an
-- `incoming_hook.received` fact in the log that carries the hook's and the
-- delivery's ids, never the body; rules triggered by the hook read the body
-- live from here. A refused delivery is kept too, without its body, so the
-- hook's owner can see what was refused and why. One delivery id is
-- accepted once per hook, so a sender's retry never runs a rule twice.

CREATE TABLE incoming_hooks (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120 AND name !~ '[[:cntrl:]]'),
    secret_id UUID NOT NULL,
    created_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    -- A retried create finds the hook it made (without its secret, which is
    -- never kept for a replay) instead of making another.
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_delivery_at TIMESTAMPTZ,
    UNIQUE (created_by_account_id, request_id)
);

CREATE INDEX incoming_hooks_space ON incoming_hooks (space_id, created_at DESC);
CREATE INDEX incoming_hooks_workspace ON incoming_hooks (workspace_id);

CREATE TABLE incoming_hook_deliveries (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    hook_id UUID NOT NULL REFERENCES incoming_hooks(id) ON DELETE CASCADE,
    -- The sender's own id for the delivery (`X-Hook-Delivery`).
    delivery_key TEXT NOT NULL CHECK (
        char_length(delivery_key) BETWEEN 1 AND 200 AND delivery_key !~ '[[:cntrl:]]'
    ),
    outcome TEXT NOT NULL CHECK (outcome IN ('accepted', 'refused')),
    -- Why it was refused: a short code, never the body.
    reason TEXT CHECK (reason IS NULL OR char_length(reason) <= 80),
    content_type TEXT CHECK (content_type IS NULL OR char_length(content_type) <= 120),
    body BYTEA CHECK (body IS NULL OR octet_length(body) <= 65536),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((outcome = 'refused') <= (body IS NULL)),
    CHECK ((outcome = 'refused') = (reason IS NOT NULL))
);

CREATE UNIQUE INDEX incoming_hook_deliveries_once
    ON incoming_hook_deliveries (hook_id, delivery_key) WHERE outcome = 'accepted';
CREATE INDEX incoming_hook_deliveries_recent
    ON incoming_hook_deliveries (hook_id, received_at DESC);
CREATE INDEX incoming_hook_deliveries_received
    ON incoming_hook_deliveries (received_at);

-- Rules can trigger on a hook.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'page_changed', 'sheet_changed',
    'whiteboard_changed', 'incoming_hook', 'schedule', 'capacity_window'
));

-- Row security (the tenancy decision of 2026-11-16).
SELECT tenancy_protect('incoming_hooks', 'workspace_id');
SELECT tenancy_protect('incoming_hook_deliveries', 'workspace_id');
