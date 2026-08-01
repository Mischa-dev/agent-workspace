-- M5 explicit publication of an already committed managed workspace. The
-- backend never stages or creates commits; this records the exact immutable
-- publication target before any credentialed network operation begins.

ALTER TABLE repository_workspaces
    ADD COLUMN published_commit TEXT,
    ADD COLUMN published_at TIMESTAMPTZ,
    ADD COLUMN published_by_actor_id UUID,
    ADD COLUMN publish_last_attempt_at TIMESTAMPTZ,
    ADD COLUMN publish_failure_category TEXT,
    ADD COLUMN publish_version BIGINT NOT NULL DEFAULT 1 CHECK (publish_version > 0),
    ADD CONSTRAINT repository_workspaces_published_commit_check CHECK (
        published_commit IS NULL OR published_commit ~ '^[0-9a-f]{40}$'
    ),
    ADD CONSTRAINT repository_workspaces_publish_failure_check CHECK (
        publish_failure_category IS NULL OR (
            char_length(publish_failure_category) BETWEEN 1 AND 64
            AND publish_failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    ADD CONSTRAINT repository_workspaces_publication_shape_check CHECK (
        (
            published_commit IS NULL
            AND published_at IS NULL
            AND published_by_actor_id IS NULL
        ) OR (
            published_commit IS NOT NULL
            AND published_at IS NOT NULL
            AND published_by_actor_id IS NOT NULL
        )
    ),
    ADD CONSTRAINT repository_workspaces_published_actor_fk
        FOREIGN KEY (project_id, published_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

ALTER TABLE github_jobs
    ADD COLUMN repository_workspace_id UUID,
    ADD COLUMN agent_run_id UUID,
    ADD COLUMN requesting_actor_id UUID,
    ADD COLUMN intended_commit TEXT,
    ADD CONSTRAINT github_jobs_intended_commit_check CHECK (
        intended_commit IS NULL OR intended_commit ~ '^[0-9a-f]{40}$'
    ),
    ADD CONSTRAINT github_jobs_workspace_publish_shape_check CHECK (
        kind <> 'workspace_publish' OR (
            repository_id IS NOT NULL
            AND repository_workspace_id IS NOT NULL
            AND agent_run_id IS NOT NULL
            AND requesting_actor_id IS NOT NULL
            AND intended_commit IS NOT NULL
            AND webhook_delivery_id IS NULL
        )
    ),
    ADD CONSTRAINT github_jobs_workspace_project_fk
        FOREIGN KEY (project_id, repository_workspace_id)
        REFERENCES repository_workspaces(project_id, id),
    ADD CONSTRAINT github_jobs_agent_run_project_fk
        FOREIGN KEY (project_id, agent_run_id)
        REFERENCES agent_runs(project_id, id),
    ADD CONSTRAINT github_jobs_requesting_actor_project_fk
        FOREIGN KEY (project_id, requesting_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

CREATE UNIQUE INDEX github_jobs_one_live_workspace_publish
    ON github_jobs (repository_workspace_id)
    WHERE kind = 'workspace_publish'
      AND state IN ('queued', 'running', 'failed');

CREATE OR REPLACE FUNCTION enforce_github_job_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.github_installation_id IS DISTINCT FROM OLD.github_installation_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.webhook_delivery_id IS DISTINCT FROM OLD.webhook_delivery_id
       OR NEW.kind IS DISTINCT FROM OLD.kind
       OR NEW.schema_version IS DISTINCT FROM OLD.schema_version
       OR NEW.payload IS DISTINCT FROM OLD.payload
       OR NEW.repository_workspace_id IS DISTINCT FROM OLD.repository_workspace_id
       OR NEW.agent_run_id IS DISTINCT FROM OLD.agent_run_id
       OR NEW.requesting_actor_id IS DISTINCT FROM OLD.requesting_actor_id
       OR NEW.intended_commit IS DISTINCT FROM OLD.intended_commit THEN
        RAISE EXCEPTION 'GitHub job identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER github_jobs_immutable_identity
BEFORE UPDATE ON github_jobs
FOR EACH ROW
EXECUTE FUNCTION enforce_github_job_identity();
