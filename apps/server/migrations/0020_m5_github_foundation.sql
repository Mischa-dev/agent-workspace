-- M5 GitHub App installation, verified webhook receipt, and durable job
-- foundation. Credential material is deliberately absent from this schema.

CREATE TABLE github_installations (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    integration_actor_id UUID NOT NULL,
    app_id BIGINT NOT NULL CHECK (app_id > 0),
    installation_id BIGINT NOT NULL CHECK (installation_id > 0),
    account_id BIGINT NOT NULL CHECK (account_id > 0),
    account_login TEXT NOT NULL CHECK (
        char_length(account_login) BETWEEN 1 AND 100
        AND account_login !~ '[[:cntrl:]]'
    ),
    target_type TEXT NOT NULL CHECK (target_type IN ('User', 'Organization')),
    repository_selection TEXT NOT NULL CHECK (repository_selection = 'selected'),
    metadata_permission TEXT NOT NULL CHECK (metadata_permission = 'read'),
    contents_permission TEXT NOT NULL CHECK (contents_permission = 'write'),
    pull_requests_permission TEXT NOT NULL CHECK (pull_requests_permission = 'write'),
    checks_permission TEXT NOT NULL CHECK (checks_permission = 'read'),
    actions_permission TEXT CHECK (actions_permission IS NULL OR actions_permission = 'read'),
    subscribed_events TEXT[] NOT NULL CHECK (
        cardinality(subscribed_events) BETWEEN 7 AND 8
        AND subscribed_events <@ ARRAY[
            'installation', 'installation_repositories', 'installation_target',
            'push', 'pull_request', 'pull_request_review', 'check_suite', 'check_run'
        ]::TEXT[]
    ),
    state TEXT NOT NULL CHECK (
        state IN ('available', 'unavailable', 'revoked', 'misconfigured')
    ),
    enabled BOOLEAN NOT NULL DEFAULT true,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_success_at TIMESTAMPTZ,
    failure_category TEXT CHECK (
        failure_category IS NULL
        OR (
            char_length(failure_category) BETWEEN 1 AND 64
            AND failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, installation_id),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, integration_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        (state = 'available' AND enabled AND last_success_at IS NOT NULL AND failure_category IS NULL)
        OR (state <> 'available' AND (NOT enabled OR failure_category IS NOT NULL))
    )
);

CREATE UNIQUE INDEX github_installations_one_enabled_project
    ON github_installations (project_id)
    WHERE enabled;

CREATE TABLE github_installation_repositories (
    project_id UUID NOT NULL,
    github_installation_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    github_repository_id BIGINT NOT NULL CHECK (github_repository_id > 0),
    access_state TEXT NOT NULL CHECK (access_state IN ('selected', 'missing')),
    last_verified_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (github_installation_id, repository_id),
    UNIQUE (project_id, github_repository_id),
    FOREIGN KEY (project_id, github_installation_id)
        REFERENCES github_installations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    CHECK (
        (access_state = 'selected' AND last_verified_at IS NOT NULL)
        OR access_state = 'missing'
    )
);

CREATE TABLE github_webhook_deliveries (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_installation_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    delivery_id TEXT NOT NULL CHECK (
        char_length(delivery_id) BETWEEN 1 AND 128
        AND delivery_id !~ '[[:cntrl:]]'
    ),
    event TEXT NOT NULL CHECK (
        event IN (
            'installation', 'installation_repositories', 'push',
            'pull_request', 'pull_request_review', 'check_suite', 'check_run'
        )
    ),
    action TEXT CHECK (
        action IS NULL OR (
            char_length(action) BETWEEN 1 AND 64
            AND action ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    payload_sha256 BYTEA NOT NULL CHECK (octet_length(payload_sha256) = 32),
    state TEXT NOT NULL DEFAULT 'received' CHECK (
        state IN ('received', 'processing', 'processed', 'failed', 'exhausted')
    ),
    failure_category TEXT CHECK (
        failure_category IS NULL OR (
            char_length(failure_category) BETWEEN 1 AND 64
            AND failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (github_installation_id, delivery_id),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, github_installation_id)
        REFERENCES github_installations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    CHECK (
        (state = 'processed' AND processed_at IS NOT NULL AND failure_category IS NULL)
        OR (state <> 'processed' AND processed_at IS NULL)
    )
);

CREATE TABLE github_jobs (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    github_installation_id UUID NOT NULL,
    repository_id UUID,
    webhook_delivery_id UUID,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'installation_sync', 'repository_access_sync', 'push_reconcile',
            'pull_request_sync', 'check_review_sync', 'workspace_publish',
            'pull_request_create', 'cleanup_reconcile'
        )
    ),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version = 1),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object'
        AND octet_length(payload::text) <= 16384
    ),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (
        state IN ('queued', 'running', 'completed', 'failed', 'exhausted')
    ),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 20),
    max_attempts INTEGER NOT NULL DEFAULT 8 CHECK (max_attempts BETWEEN 1 AND 20),
    lease_owner UUID,
    lease_expires_at TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    failure_category TEXT CHECK (
        failure_category IS NULL OR (
            char_length(failure_category) BETWEEN 1 AND 64
            AND failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE (webhook_delivery_id, kind),
    FOREIGN KEY (project_id, github_installation_id)
        REFERENCES github_installations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    FOREIGN KEY (project_id, webhook_delivery_id)
        REFERENCES github_webhook_deliveries(project_id, id),
    CHECK (
        (lease_owner IS NULL AND lease_expires_at IS NULL)
        OR (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
    ),
    CHECK (
        (state = 'completed' AND completed_at IS NOT NULL AND failure_category IS NULL)
        OR (state <> 'completed' AND completed_at IS NULL)
    )
);

CREATE INDEX github_jobs_claimable
    ON github_jobs (next_attempt_at, created_at, id)
    WHERE state IN ('queued', 'failed');

CREATE INDEX github_jobs_lease
    ON github_jobs (lease_expires_at)
    WHERE state = 'running';

CREATE OR REPLACE FUNCTION enforce_github_installation_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.integration_actor_id IS DISTINCT FROM OLD.integration_actor_id
       OR NEW.app_id IS DISTINCT FROM OLD.app_id
       OR NEW.installation_id IS DISTINCT FROM OLD.installation_id
       OR NEW.account_id IS DISTINCT FROM OLD.account_id THEN
        RAISE EXCEPTION 'GitHub installation identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER github_installations_immutable_identity
BEFORE UPDATE ON github_installations
FOR EACH ROW
EXECUTE FUNCTION enforce_github_installation_identity();
