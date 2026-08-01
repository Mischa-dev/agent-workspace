-- M5 canonical pull-request checks and review projection. Webhook payloads are
-- only invalidation hints; these rows contain bounded fields fetched from the
-- installation API and never retain logs, annotations, review bodies, or diffs.

ALTER TABLE github_pull_requests
    ADD COLUMN checks_state TEXT NOT NULL DEFAULT 'unknown' CHECK (
        checks_state IN ('unknown', 'pending', 'success', 'failure')
    ),
    ADD COLUMN review_state TEXT NOT NULL DEFAULT 'unknown' CHECK (
        review_state IN ('unknown', 'pending', 'approved', 'changes_requested')
    ),
    ADD COLUMN mergeability_state TEXT NOT NULL DEFAULT 'unknown' CHECK (
        mergeability_state IN ('unknown', 'blocked', 'ready')
    ),
    ADD COLUMN sync_state TEXT NOT NULL DEFAULT 'stale' CHECK (
        sync_state IN ('stale', 'fresh', 'failed')
    ),
    ADD COLUMN requested_reviewers JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(requested_reviewers) = 'array'
        AND jsonb_array_length(requested_reviewers) <= 50
        AND octet_length(requested_reviewers::text) <= 8192
    );

CREATE TABLE github_pull_request_check_runs (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    github_check_run_id BIGINT NOT NULL CHECK (github_check_run_id > 0),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 256),
    status TEXT NOT NULL CHECK (status IN ('queued', 'in_progress', 'completed')),
    conclusion TEXT CHECK (
        conclusion IS NULL OR conclusion IN (
            'action_required', 'cancelled', 'failure', 'neutral', 'skipped',
            'stale', 'startup_failure', 'success', 'timed_out'
        )
    ),
    details_url TEXT CHECK (
        details_url IS NULL OR (
            char_length(details_url) BETWEEN 1 AND 2048
            AND details_url ~ '^https://'
        )
    ),
    external_started_at TIMESTAMPTZ,
    external_completed_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (github_pull_request_id, github_check_run_id),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    CHECK (
        (status = 'completed' AND conclusion IS NOT NULL)
        OR (status <> 'completed' AND conclusion IS NULL)
    )
);

CREATE TABLE github_pull_request_reviews (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_pull_request_id UUID NOT NULL,
    github_review_id BIGINT NOT NULL CHECK (github_review_id > 0),
    author_login TEXT NOT NULL CHECK (char_length(author_login) BETWEEN 1 AND 100),
    state TEXT NOT NULL CHECK (
        state IN ('approved', 'changes_requested', 'commented', 'dismissed')
    ),
    external_submitted_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (github_pull_request_id, github_review_id),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id)
);

ALTER TABLE github_jobs
    ADD COLUMN github_pull_request_id UUID,
    ADD CONSTRAINT github_jobs_pull_request_fk
        FOREIGN KEY (project_id, github_pull_request_id)
        REFERENCES github_pull_requests(project_id, id),
    ADD CONSTRAINT github_jobs_manual_sync_shape_check CHECK (
        github_pull_request_id IS NULL OR (
            kind IN ('pull_request_sync', 'check_review_sync')
            AND repository_id IS NOT NULL
            AND requesting_actor_id IS NOT NULL
            AND webhook_delivery_id IS NULL
        )
    );

CREATE UNIQUE INDEX github_jobs_one_live_pull_request_sync
    ON github_jobs (github_pull_request_id)
    WHERE github_pull_request_id IS NOT NULL
      AND kind IN ('pull_request_sync', 'check_review_sync')
      AND state IN ('queued', 'running', 'failed');

CREATE INDEX github_pull_request_check_runs_projection
    ON github_pull_request_check_runs (github_pull_request_id, status, conclusion);

CREATE INDEX github_pull_request_reviews_projection
    ON github_pull_request_reviews (github_pull_request_id, author_login, external_submitted_at DESC);

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
       OR NEW.pull_request_request_id IS DISTINCT FROM OLD.pull_request_request_id
       OR NEW.github_pull_request_id IS DISTINCT FROM OLD.github_pull_request_id THEN
        RAISE EXCEPTION 'GitHub job identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
