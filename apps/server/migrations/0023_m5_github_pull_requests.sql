-- M5 durable pull-request identity and explicit Project-owned links. External
-- bodies, comments, diffs, logs, and webhook payloads are intentionally not
-- stored in these canonical projections.

CREATE TABLE github_pull_requests (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    github_installation_id UUID NOT NULL,
    github_repository_id BIGINT NOT NULL CHECK (github_repository_id > 0),
    pull_request_number BIGINT NOT NULL CHECK (pull_request_number > 0),
    node_id TEXT NOT NULL CHECK (char_length(node_id) BETWEEN 1 AND 256),
    html_url TEXT NOT NULL CHECK (
        char_length(html_url) BETWEEN 1 AND 2048
        AND html_url ~ '^https://github\.com/'
    ),
    head_ref TEXT NOT NULL CHECK (char_length(head_ref) BETWEEN 1 AND 255),
    head_sha TEXT NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}$'),
    base_ref TEXT NOT NULL CHECK (char_length(base_ref) BETWEEN 1 AND 255),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 256),
    author_login TEXT NOT NULL CHECK (char_length(author_login) BETWEEN 1 AND 100),
    draft BOOLEAN NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'closed', 'merged')),
    merged_at TIMESTAMPTZ,
    merge_commit_sha TEXT CHECK (
        merge_commit_sha IS NULL OR merge_commit_sha ~ '^[0-9a-f]{40}$'
    ),
    external_created_at TIMESTAMPTZ NOT NULL,
    external_updated_at TIMESTAMPTZ NOT NULL,
    last_sync_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_success_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sync_failure_category TEXT CHECK (
        sync_failure_category IS NULL OR (
            char_length(sync_failure_category) BETWEEN 1 AND 64
            AND sync_failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (repository_id, pull_request_number),
    UNIQUE (project_id, id),
    UNIQUE (project_id, node_id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    FOREIGN KEY (project_id, github_installation_id)
        REFERENCES github_installations(project_id, id),
    CONSTRAINT github_pull_requests_merge_shape_check CHECK (
        (state <> 'merged' AND merged_at IS NULL AND merge_commit_sha IS NULL)
        OR (state = 'merged' AND merged_at IS NOT NULL AND merge_commit_sha IS NOT NULL)
    )
);

CREATE TABLE github_pull_request_requests (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_installation_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    repository_workspace_id UUID NOT NULL,
    agent_run_id UUID NOT NULL,
    requesting_actor_id UUID NOT NULL,
    head_ref TEXT NOT NULL CHECK (char_length(head_ref) BETWEEN 1 AND 255),
    head_sha TEXT NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}$'),
    base_ref TEXT NOT NULL CHECK (char_length(base_ref) BETWEEN 1 AND 255),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 256),
    body TEXT NOT NULL CHECK (char_length(body) <= 10000),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (
        state IN ('queued', 'running', 'completed', 'failed', 'exhausted')
    ),
    failure_category TEXT CHECK (
        failure_category IS NULL OR (
            char_length(failure_category) BETWEEN 1 AND 64
            AND failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    mutation_attempted_at TIMESTAMPTZ,
    github_pull_request_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE (repository_workspace_id),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, github_installation_id)
        REFERENCES github_installations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    FOREIGN KEY (project_id, repository_workspace_id)
        REFERENCES repository_workspaces(project_id, id),
    FOREIGN KEY (project_id, agent_run_id)
        REFERENCES agent_runs(project_id, id),
    FOREIGN KEY (project_id, requesting_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    CONSTRAINT github_pull_request_requests_completion_shape_check CHECK (
        (state = 'completed' AND completed_at IS NOT NULL AND github_pull_request_id IS NOT NULL)
        OR (state <> 'completed' AND completed_at IS NULL AND github_pull_request_id IS NULL)
    )
);

ALTER TABLE github_jobs
    ADD COLUMN pull_request_request_id UUID,
    ADD CONSTRAINT github_jobs_pull_request_request_fk
        FOREIGN KEY (project_id, pull_request_request_id)
        REFERENCES github_pull_request_requests(project_id, id),
    ADD CONSTRAINT github_jobs_pull_request_create_shape_check CHECK (
        kind <> 'pull_request_create' OR (
            repository_id IS NOT NULL
            AND repository_workspace_id IS NOT NULL
            AND agent_run_id IS NOT NULL
            AND requesting_actor_id IS NOT NULL
            AND intended_commit IS NOT NULL
            AND pull_request_request_id IS NOT NULL
            AND webhook_delivery_id IS NULL
        )
    );

CREATE UNIQUE INDEX github_jobs_one_pull_request_create
    ON github_jobs (pull_request_request_id)
    WHERE kind = 'pull_request_create';

CREATE TABLE github_pull_request_work_item_links (
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('explicit', 'run_artifact', 'branch_key', 'body_marker')),
    confirmed BOOLEAN NOT NULL,
    created_by_actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (github_pull_request_id, work_item_id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CONSTRAINT github_pull_request_work_item_confirmation_check CHECK (
        (source IN ('explicit', 'run_artifact') AND confirmed)
        OR (source IN ('branch_key', 'body_marker') AND NOT confirmed)
    )
);

CREATE TABLE github_pull_request_conversation_links (
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('explicit', 'run_artifact')),
    created_by_actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (github_pull_request_id, conversation_id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE TABLE github_pull_request_workspace_links (
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    repository_workspace_id UUID NOT NULL,
    agent_run_id UUID NOT NULL,
    commit_sha TEXT NOT NULL CHECK (commit_sha ~ '^[0-9a-f]{40}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (github_pull_request_id, repository_workspace_id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    FOREIGN KEY (project_id, repository_workspace_id)
        REFERENCES repository_workspaces(project_id, id),
    FOREIGN KEY (project_id, agent_run_id)
        REFERENCES agent_runs(project_id, id)
);

CREATE OR REPLACE FUNCTION enforce_github_pull_request_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.github_installation_id IS DISTINCT FROM OLD.github_installation_id
       OR NEW.github_repository_id IS DISTINCT FROM OLD.github_repository_id
       OR NEW.pull_request_number IS DISTINCT FROM OLD.pull_request_number
       OR NEW.node_id IS DISTINCT FROM OLD.node_id
       OR NEW.html_url IS DISTINCT FROM OLD.html_url
       OR NEW.head_ref IS DISTINCT FROM OLD.head_ref
       OR NEW.base_ref IS DISTINCT FROM OLD.base_ref
       OR NEW.external_created_at IS DISTINCT FROM OLD.external_created_at THEN
        RAISE EXCEPTION 'GitHub pull request identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER github_pull_requests_immutable_identity
BEFORE UPDATE ON github_pull_requests
FOR EACH ROW
EXECUTE FUNCTION enforce_github_pull_request_identity();

CREATE OR REPLACE FUNCTION validate_github_pull_request_thread_link()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_kind TEXT;
BEGIN
    SELECT kind INTO conversation_kind
      FROM conversations
     WHERE project_id = NEW.project_id AND id = NEW.conversation_id;
    IF conversation_kind IS DISTINCT FROM 'thread' THEN
        RAISE EXCEPTION 'GitHub pull requests may link only to Threads';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER github_pull_request_conversation_links_validate_thread
BEFORE INSERT OR UPDATE ON github_pull_request_conversation_links
FOR EACH ROW EXECUTE FUNCTION validate_github_pull_request_thread_link();

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
       OR NEW.intended_commit IS DISTINCT FROM OLD.intended_commit
       OR NEW.pull_request_request_id IS DISTINCT FROM OLD.pull_request_request_id THEN
        RAISE EXCEPTION 'GitHub job identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
