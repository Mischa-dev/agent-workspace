-- W10: AI planning (docs/plans/roadmap.md, AI1 to AI6 and V7).
--
-- The model gateway logs every call here with its token counts and cost,
-- never the prompt or the reply. AI suggestions (dependencies, blockers,
-- triage fields, missing criteria) wait in a review queue, keep the sources
-- they were drawn from, and are only shown to someone who can still see
-- every one of those sources. Plan imports keep their own record of what
-- each batch created, so a resumed import never creates anything twice.

-- AI1: one row per model call. Workspace-scoped; the monthly budget sums it.
CREATE TABLE ai_usage (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    -- Who asked, when a person did; background runs have none.
    actor_id UUID REFERENCES actors(id),
    prompt_id TEXT NOT NULL CHECK (prompt_id ~ '^[a-z][a-z0-9_]{0,63}$'),
    prompt_version INTEGER NOT NULL CHECK (prompt_version > 0),
    provider TEXT NOT NULL CHECK (provider ~ '^[a-z][a-z0-9_-]{0,63}$'),
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    tier TEXT NOT NULL CHECK (tier IN ('quick', 'deep')),
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    -- Millionths of the price's currency (USD), from the model's prices.
    cost_micros BIGINT NOT NULL DEFAULT 0 CHECK (cost_micros >= 0),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('succeeded', 'invalid_output', 'provider_error', 'refused')
    ),
    -- A short category, never a response body.
    error TEXT CHECK (error IS NULL OR char_length(error) <= 300),
    latency_ms INTEGER NOT NULL DEFAULT 0 CHECK (latency_ms >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ai_usage_organization_month ON ai_usage (organization_id, created_at);
CREATE INDEX ai_usage_project ON ai_usage (project_id, created_at) WHERE project_id IS NOT NULL;

-- AI2 and AI4: suggestions waiting for a person.
--
-- work_item_id is the task the suggestion is about. For link kinds the other
-- task is other_work_item_id: "blocks" means the other task blocks this one,
-- "duplicates" that this one duplicates the other, "parent" that the other
-- is this one's parent. Field kinds (blocked_reason, label, priority, type,
-- criteria) carry their value.
CREATE TABLE ai_suggestions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'blocks', 'relates_to', 'duplicates', 'parent',
        'blocked_reason', 'label', 'priority', 'type', 'criteria'
    )),
    other_project_id UUID,
    other_work_item_id UUID,
    value JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(value) = 'object' AND octet_length(value::text) <= 8192
    ),
    confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 300),
    -- What it was drawn from: [{"kind": "task", "id": ...},
    -- {"kind": "message", "messageId": ..., "conversationId": ...},
    -- {"kind": "signal", "name": ...}]. Rechecked on every read.
    sources JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(sources) = 'array' AND octet_length(sources::text) <= 8192
    ),
    -- One suggestion per pair of tasks (any link kind, either direction) or
    -- per task, field and value. A dismissed one keeps its key for good.
    pair_key TEXT NOT NULL CHECK (char_length(pair_key) BETWEEN 1 AND 300),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'accepted', 'dismissed', 'applied', 'stale')
    ),
    -- Set when only this person may see it: their own Analyze drew on
    -- something not every project member can see.
    audience_actor_id UUID REFERENCES actors(id),
    origin TEXT NOT NULL CHECK (
        origin IN ('edit', 'analyze', 'nightly', 'create', 'conversation', 'request')
    ),
    provider TEXT NOT NULL CHECK (char_length(provider) BETWEEN 1 AND 64),
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    prompt_id TEXT NOT NULL CHECK (char_length(prompt_id) BETWEEN 1 AND 64),
    prompt_version INTEGER NOT NULL CHECK (prompt_version > 0),
    requested_by_actor_id UUID REFERENCES actors(id),
    decided_by_actor_id UUID REFERENCES actors(id),
    decided_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (other_project_id, other_work_item_id) REFERENCES work_items(project_id, id),
    CHECK ((other_work_item_id IS NULL) = (other_project_id IS NULL)),
    CHECK (
        (kind IN ('blocks', 'relates_to', 'duplicates', 'parent'))
        = (other_work_item_id IS NOT NULL)
    ),
    CHECK (other_work_item_id IS NULL OR other_work_item_id <> work_item_id),
    CHECK ((status IN ('accepted', 'dismissed')) = (decided_at IS NOT NULL))
);

CREATE UNIQUE INDEX ai_suggestions_once
    ON ai_suggestions (
        project_id, pair_key,
        COALESCE(audience_actor_id, '00000000-0000-0000-0000-000000000000'::uuid)
    )
    WHERE status <> 'stale';

CREATE INDEX ai_suggestions_queue
    ON ai_suggestions (project_id, status, confidence DESC, id);
CREATE INDEX ai_suggestions_task ON ai_suggestions (work_item_id, status);
CREATE INDEX ai_suggestions_other ON ai_suggestions (other_work_item_id)
    WHERE other_work_item_id IS NOT NULL;

-- The one rule for who may see a suggestion: it's for everyone in the
-- project or for them, they can reach both tasks' projects, and they can
-- still see every task and message it cites (conversation_visible_to,
-- migration 0037). Every read and every accept goes through this.
CREATE FUNCTION ai_suggestion_visible_to(target UUID, viewer UUID, connection UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM ai_suggestions suggestion
        WHERE suggestion.id = target
          AND (suggestion.audience_actor_id IS NULL OR suggestion.audience_actor_id = viewer)
          AND actor_can_access_project(viewer, connection, suggestion.project_id)
          AND (suggestion.other_project_id IS NULL
               OR actor_can_access_project(viewer, connection, suggestion.other_project_id))
          AND NOT EXISTS (
              SELECT 1
              FROM jsonb_array_elements(suggestion.sources) source
              WHERE (source->>'kind' = 'task' AND NOT EXISTS (
                        SELECT 1 FROM work_items item
                        WHERE item.id = (source->>'id')::uuid
                          AND actor_can_access_project(viewer, connection, item.project_id)))
                 OR (source->>'kind' = 'message' AND NOT EXISTS (
                        SELECT 1
                        FROM messages message
                        JOIN conversations conversation
                          ON conversation.id = message.conversation_id
                        WHERE message.id = (source->>'messageId')::uuid
                          AND message.removed_at IS NULL
                          AND actor_can_access_project(viewer, connection, conversation.project_id)
                          AND conversation_visible_to(conversation.id, viewer)))
                 OR (source->>'kind' NOT IN ('task', 'message', 'signal'))
          )
    )
$$;

-- AI2: runs of detection (on edits, nightly, a person's Analyze), triage
-- and the rest, so the web can say what's happening and what it found.
CREATE TABLE ai_runs (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('edit', 'analyze', 'nightly', 'triage')),
    requested_by_actor_id UUID REFERENCES actors(id),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (
        state IN ('queued', 'running', 'succeeded', 'failed')
    ),
    tasks_considered INTEGER NOT NULL DEFAULT 0 CHECK (tasks_considered >= 0),
    suggestions_created INTEGER NOT NULL DEFAULT 0 CHECK (suggestions_created >= 0),
    suggestions_applied INTEGER NOT NULL DEFAULT 0 CHECK (suggestions_applied >= 0),
    -- A sentence for the person, never a provider's response body.
    error TEXT CHECK (error IS NULL OR char_length(error) <= 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX ai_runs_project ON ai_runs (project_id, created_at DESC);

-- Where background detection got to in each project.
CREATE TABLE ai_project_scans (
    project_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    last_scan_at TIMESTAMPTZ NOT NULL,
    last_nightly_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- AI3: a plan turned into tasks. The draft is the editable preview; each
-- created milestone, task and dependency is recorded by the draft's ref in
-- the same transaction that made it.
CREATE TABLE ai_plan_imports (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('text', 'page', 'structured')),
    -- The pasted plan, kept while the draft is made, then cleared.
    source_text TEXT CHECK (source_text IS NULL OR char_length(source_text) <= 400000),
    state TEXT NOT NULL CHECK (
        state IN ('drafting', 'draft', 'committing', 'committed', 'failed')
    ),
    draft JSONB NOT NULL DEFAULT '{"milestones": [], "tasks": []}'::jsonb CHECK (
        jsonb_typeof(draft) = 'object' AND octet_length(draft::text) <= 4000000
    ),
    committed_by_actor_id UUID REFERENCES actors(id),
    error TEXT CHECK (error IS NULL OR char_length(error) <= 500),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    committed_at TIMESTAMPTZ
);

CREATE INDEX ai_plan_imports_project ON ai_plan_imports (project_id, created_at DESC);

CREATE TABLE ai_plan_import_items (
    import_id UUID NOT NULL REFERENCES ai_plan_imports(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('milestone', 'task', 'dependency')),
    ref TEXT NOT NULL CHECK (char_length(ref) BETWEEN 1 AND 200),
    work_item_id UUID,
    milestone_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (import_id, kind, ref)
);
