-- run_list and the briefing list a project's Codex runs newest first.
CREATE INDEX agent_runs_project_created
    ON agent_runs (project_id, created_at DESC, id DESC);
