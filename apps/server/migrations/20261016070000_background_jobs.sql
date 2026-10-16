-- F9: one durable job queue for background work.
--
-- Workers claim due jobs with FOR UPDATE SKIP LOCKED, take a lease, commit,
-- and only then do the work, so HTTP calls and other slow work never run
-- while a row lock is held. A job whose lease runs out (the worker died) is
-- claimed again. Finishing a job checks the lease token, so a worker that
-- lost its lease can't overwrite the newer attempt.
--
-- Jobs belong to a workspace (organization) and optionally a project.
-- System-wide maintenance, such as retention, has no workspace.
CREATE TABLE background_jobs (
    id UUID PRIMARY KEY,
    organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (
        char_length(kind) BETWEEN 1 AND 120
        AND kind ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'
    ),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object'
        AND octet_length(payload::text) <= 65536
    ),
    -- At most one queued or running job holds a given key.
    unique_key TEXT CHECK (
        unique_key IS NULL OR char_length(unique_key) BETWEEN 1 AND 300
    ),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (
        state IN ('queued', 'running', 'succeeded', 'dead', 'canceled')
    ),
    priority SMALLINT NOT NULL DEFAULT 0 CHECK (priority BETWEEN -100 AND 100),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 8 CHECK (max_attempts BETWEEN 1 AND 100),
    run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_token UUID,
    lease_owner TEXT CHECK (lease_owner IS NULL OR char_length(lease_owner) <= 200),
    lease_expires_at TIMESTAMPTZ,
    -- A short, bounded reason, never a response body or secret.
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    CHECK (project_id IS NULL OR organization_id IS NOT NULL),
    CHECK ((state = 'running') = (lease_token IS NOT NULL)),
    CHECK ((state = 'running') = (lease_expires_at IS NOT NULL)),
    CHECK ((state IN ('succeeded', 'dead', 'canceled')) = (finished_at IS NOT NULL))
);

CREATE UNIQUE INDEX background_jobs_active_unique_key
    ON background_jobs (unique_key)
    WHERE unique_key IS NOT NULL AND state IN ('queued', 'running');

CREATE INDEX background_jobs_due
    ON background_jobs (priority DESC, run_at, id)
    WHERE state = 'queued';

CREATE INDEX background_jobs_expired_leases
    ON background_jobs (lease_expires_at)
    WHERE state = 'running';

CREATE INDEX background_jobs_finished
    ON background_jobs (finished_at)
    WHERE finished_at IS NOT NULL;

CREATE INDEX background_jobs_organization
    ON background_jobs (organization_id, created_at DESC)
    WHERE organization_id IS NOT NULL;

CREATE INDEX background_jobs_kind_recent
    ON background_jobs (kind, finished_at DESC);
