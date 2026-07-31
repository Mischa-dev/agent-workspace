-- M0 walking-skeleton domain schema.
-- Internal identifiers are application-generated UUIDv7 values.

CREATE TABLE actors (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (
        kind IN ('human', 'external_agent', 'execution_worker', 'integration', 'system')
    ),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE organizations (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]+(?:-[a-z0-9]+)*$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE projects (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_key TEXT NOT NULL CHECK (project_key ~ '^[A-Z][A-Z0-9]{1,11}$'),
    name TEXT NOT NULL,
    archived_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX projects_organization_key_unique
    ON projects (organization_id, lower(project_key));

CREATE TABLE project_memberships (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    role TEXT NOT NULL CHECK (role IN ('owner', 'member', 'worker')),
    display_name TEXT NOT NULL,
    handle TEXT NOT NULL CHECK (handle ~ '^[a-z0-9][a-z0-9_-]{1,31}$'),
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, actor_id)
);

CREATE UNIQUE INDEX project_memberships_active_handle_unique
    ON project_memberships (project_id, lower(handle))
    WHERE enabled;

CREATE TABLE repositories (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    github_repository_id BIGINT,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    clone_url TEXT NOT NULL,
    default_branch TEXT NOT NULL DEFAULT 'main',
    github_app_installation_id BIGINT,
    mirror_path TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    trusted BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, owner, name)
);

CREATE TABLE conversations (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind IN ('channel', 'thread')),
    parent_channel_id UUID REFERENCES conversations(id),
    repository_id UUID REFERENCES repositories(id),
    name TEXT NOT NULL,
    next_entry_sequence BIGINT NOT NULL DEFAULT 1 CHECK (next_entry_sequence > 0),
    archived_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (kind = 'channel' AND parent_channel_id IS NULL AND repository_id IS NULL)
        OR
        (kind = 'thread' AND parent_channel_id IS NOT NULL)
    )
);

CREATE OR REPLACE FUNCTION validate_conversation_parent()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    parent_kind TEXT;
    parent_project_id UUID;
BEGIN
    IF NEW.kind = 'channel' THEN
        RETURN NEW;
    END IF;

    SELECT kind, project_id
      INTO parent_kind, parent_project_id
      FROM conversations
     WHERE id = NEW.parent_channel_id;

    IF parent_kind IS DISTINCT FROM 'channel'
       OR parent_project_id IS DISTINCT FROM NEW.project_id THEN
        RAISE EXCEPTION 'thread parent must be a channel in the same project';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER conversations_validate_parent
BEFORE INSERT OR UPDATE OF kind, parent_channel_id, project_id
ON conversations
FOR EACH ROW
EXECUTE FUNCTION validate_conversation_parent();

CREATE UNIQUE INDEX conversations_active_channel_name_unique
    ON conversations (project_id, lower(name))
    WHERE kind = 'channel' AND archived_at IS NULL;

CREATE UNIQUE INDEX conversations_active_thread_name_unique
    ON conversations (parent_channel_id, lower(name))
    WHERE kind = 'thread' AND archived_at IS NULL;

CREATE TABLE provider_connections (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind = 'codex_app_server'),
    host TEXT NOT NULL CHECK (host = 'local_project_host'),
    enabled BOOLEAN NOT NULL DEFAULT true,
    capabilities JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX provider_connections_project_kind_host_unique
    ON provider_connections (project_id, kind, host);

CREATE TABLE execution_profiles (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    provider_connection_id UUID NOT NULL REFERENCES provider_connections(id),
    owner_actor_id UUID NOT NULL REFERENCES actors(id),
    name TEXT NOT NULL,
    codex_home_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unavailable' CHECK (
        status IN ('available', 'unavailable', 'authentication_required', 'disabled')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, name)
);

CREATE TABLE execution_workers (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    provider_connection_id UUID NOT NULL REFERENCES provider_connections(id),
    execution_profile_id UUID NOT NULL REFERENCES execution_profiles(id),
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, actor_id)
);

CREATE TABLE provider_sessions (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    execution_profile_id UUID NOT NULL REFERENCES execution_profiles(id),
    provider_thread_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'new' CHECK (
        state IN ('new', 'active', 'idle', 'archived', 'unavailable')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (execution_profile_id, provider_thread_id),
    UNIQUE (conversation_id, execution_profile_id)
);

CREATE TABLE repository_workspaces (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id),
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    branch_name TEXT NOT NULL,
    base_commit TEXT NOT NULL CHECK (base_commit ~ '^[0-9a-f]{40,64}$'),
    state TEXT NOT NULL DEFAULT 'active' CHECK (
        state IN ('active', 'retained', 'cleaned', 'failed')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (repository_id, branch_name)
);

CREATE UNIQUE INDEX repository_workspaces_active_conversation_unique
    ON repository_workspaces (repository_id, conversation_id)
    WHERE state = 'active';

CREATE TABLE git_worktrees (
    id UUID PRIMARY KEY,
    repository_workspace_id UUID NOT NULL REFERENCES repository_workspaces(id),
    path TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL DEFAULT 'creating' CHECK (
        state IN ('creating', 'ready', 'dirty', 'missing', 'removed', 'failed')
    ),
    dirty BOOLEAN NOT NULL DEFAULT false,
    last_inspected_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX git_worktrees_one_live_per_workspace
    ON git_worktrees (repository_workspace_id)
    WHERE state IN ('creating', 'ready', 'dirty', 'missing', 'failed');

CREATE TABLE agent_runs (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    execution_worker_id UUID NOT NULL REFERENCES execution_workers(id),
    requesting_actor_id UUID NOT NULL REFERENCES actors(id),
    execution_profile_id UUID NOT NULL REFERENCES execution_profiles(id),
    provider_session_id UUID REFERENCES provider_sessions(id),
    repository_id UUID NOT NULL REFERENCES repositories(id),
    repository_workspace_id UUID REFERENCES repository_workspaces(id),
    state TEXT NOT NULL CHECK (
        state IN (
            'created', 'queued', 'starting', 'working', 'blocked', 'recovering',
            'completed', 'failed', 'canceled', 'interrupted'
        )
    ),
    prompt TEXT NOT NULL CHECK (char_length(prompt) BETWEEN 1 AND 20000),
    authorization_snapshot JSONB NOT NULL,
    failure_reason TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX agent_runs_one_active_per_thread
    ON agent_runs (conversation_id)
    WHERE state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering');

CREATE TABLE messages (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    author_actor_id UUID NOT NULL REFERENCES actors(id),
    content_version INTEGER NOT NULL DEFAULT 1 CHECK (content_version = 1),
    content JSONB NOT NULL,
    plain_text TEXT NOT NULL CHECK (char_length(plain_text) <= 200000),
    agent_run_id UUID REFERENCES agent_runs(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE agent_run_events (
    id UUID PRIMARY KEY,
    agent_run_id UUID NOT NULL REFERENCES agent_runs(id),
    event_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'run_state_changed',
            'command_started', 'command_completed', 'file_change',
            'approval_requested', 'approval_resolved', 'question_requested',
            'question_resolved', 'test_result', 'run_completed', 'run_failed',
            'run_interrupted', 'recovery'
        )
    ),
    summary TEXT NOT NULL CHECK (char_length(summary) <= 2000),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_run_id, event_key)
);

CREATE TABLE agent_approvals (
    id UUID PRIMARY KEY,
    agent_run_id UUID NOT NULL REFERENCES agent_runs(id),
    requesting_actor_id UUID NOT NULL REFERENCES actors(id),
    responded_by_actor_id UUID REFERENCES actors(id),
    provider_request_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('approval', 'question')),
    summary TEXT NOT NULL CHECK (char_length(summary) <= 2000),
    options JSONB NOT NULL,
    eligible_role TEXT NOT NULL DEFAULT 'owner',
    human_required BOOLEAN NOT NULL DEFAULT true,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'resolved', 'superseded', 'canceled')
    ),
    response JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    responded_at TIMESTAMPTZ,
    UNIQUE (agent_run_id, provider_request_id)
);

CREATE TABLE conversation_entries (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    kind TEXT NOT NULL CHECK (kind IN ('message', 'worker_event', 'system')),
    message_id UUID REFERENCES messages(id),
    agent_run_event_id UUID REFERENCES agent_run_events(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, sequence),
    CHECK (
        (kind = 'message' AND message_id IS NOT NULL AND agent_run_event_id IS NULL)
        OR
        (kind = 'worker_event' AND message_id IS NULL AND agent_run_event_id IS NOT NULL)
        OR
        (kind = 'system' AND message_id IS NULL AND agent_run_event_id IS NULL)
    )
);

CREATE INDEX conversation_entries_timeline
    ON conversation_entries (conversation_id, sequence);
CREATE INDEX messages_conversation_created
    ON messages (conversation_id, created_at);
CREATE INDEX agent_runs_conversation_created
    ON agent_runs (conversation_id, created_at DESC);
CREATE INDEX agent_approvals_pending_run
    ON agent_approvals (agent_run_id)
    WHERE status = 'pending';
