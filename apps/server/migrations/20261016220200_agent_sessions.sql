-- A3: the session an agent works in for a thread, whichever way the work
-- started: a harness run (a Codex Cloud task) or an external runner that
-- binds its own Claude Code or Codex session. One per agent per thread, so a
-- runner resumes it instead of starting cold. Separate from Agent Runs and
-- provider sessions, as AGENTS.md requires.
CREATE TABLE agent_sessions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    conversation_id UUID NOT NULL,
    agent_actor_id UUID NOT NULL REFERENCES actors(id),
    source TEXT NOT NULL CHECK (source IN ('harness_run', 'runner')),
    -- What runs the session, such as codex_cloud, claude_code or codex.
    harness TEXT NOT NULL CHECK (harness ~ '^[a-z][a-z0-9_]{0,63}$'),
    external_ref TEXT NOT NULL CHECK (
        char_length(external_ref) BETWEEN 1 AND 200 AND external_ref !~ '[[:cntrl:]]'
    ),
    url TEXT CHECK (
        url IS NULL OR (char_length(url) <= 2000 AND url ~ '^https://[^[:space:][:cntrl:]]+$')
    ),
    state TEXT NOT NULL DEFAULT 'active' CHECK (
        state IN ('active', 'idle', 'ended', 'cancelled')
    ),
    agent_run_id UUID,
    reply_request_id UUID REFERENCES reply_requests(id),
    last_activity_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, agent_actor_id),
    FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, agent_run_id) REFERENCES agent_runs(project_id, id)
);

CREATE INDEX agent_sessions_conversation ON agent_sessions (conversation_id, updated_at DESC);
CREATE INDEX agent_sessions_agent ON agent_sessions (agent_actor_id, updated_at DESC);

CREATE FUNCTION agent_sessions_match_workspace()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM projects
        WHERE id = NEW.project_id AND organization_id = NEW.organization_id
    ) THEN
        RAISE EXCEPTION 'Agent session workspace must match its project' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_sessions_match_workspace
    BEFORE INSERT OR UPDATE OF organization_id, project_id ON agent_sessions
    FOR EACH ROW EXECUTE FUNCTION agent_sessions_match_workspace();
