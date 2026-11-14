-- A1.11 (build 1G): hosts and enrolment (A1 section 3.9). A host is a system
-- that runs agents; it signs in as its own `integration` account.
--
-- * hosts: global (R19), owned by a person or by a workspace. Until K1.22
--   and K1.23 issue app credentials, a host holds an interim bearer token
--   kept only as a SHA-256 digest (interim_token_digest); K1 migrates it.
-- * host_enrolments: one-time codes, kept only as a digest, 15 minutes.
-- * host_agents: which agent accounts a host may act for, under its own
--   reference. Bound by people (host_agent_bind), never chosen by the host.
-- * host_notices: the three notices a host sends, kept. A reference change
--   with a matching lineage moves the binding now; K1.7's outside
--   references replay the stored notices when they arrive.
-- * Every workspace has a built-in host row (the server's own runner), made
--   here for existing workspaces and by a trigger for new ones.
-- * runs.host_id, run_usage.host_id, usage_reports.host_id and
--   requests.raised_via_host_id get their foreign keys.

CREATE TABLE hosts (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL UNIQUE REFERENCES actors(id),
    owner_account_id UUID REFERENCES actors(id),
    owner_workspace_id UUID REFERENCES organizations(id),
    CHECK (num_nonnulls(owner_account_id, owner_workspace_id) = 1),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'),
    software TEXT CHECK (software IS NULL OR char_length(software) <= 80),
    software_version TEXT CHECK (software_version IS NULL OR char_length(software_version) <= 40),
    protocol_version TEXT NOT NULL CHECK (protocol_version ~ '^[0-9]{4}-[0-9]{2}$'),
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(capabilities) = 'object' AND octet_length(capabilities::text) <= 32768
    ),
    offers JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(offers) = 'array' AND octet_length(offers::text) <= 32768
    ),
    actions JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(actions) = 'array' AND octet_length(actions::text) <= 32768
    ),
    -- The agents the host last described, bound or not: what a person picks
    -- from when binding one.
    described_agents JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(described_agents) = 'array' AND octet_length(described_agents::text) <= 131072
    ),
    -- PostgreSQL caps a regex repetition count at 255, so the length is its
    -- own check.
    issuer TEXT CHECK (
        issuer IS NULL
        OR (issuer ~ '^https://[^[:space:][:cntrl:]]+$' AND char_length(issuer) <= 1008)
    ),
    module_manifest_url TEXT CHECK (
        module_manifest_url IS NULL
        OR (module_manifest_url ~ '^https://[^[:space:][:cntrl:]]+$'
            AND char_length(module_manifest_url) <= 1008)
    ),
    connection TEXT NOT NULL DEFAULT 'dial_out' CHECK (connection IN ('dial_out', 'push')),
    push_url TEXT CHECK (push_url IS NULL OR push_url ~ '^https://[^[:space:][:cntrl:]]+$'),
    push_secret_id UUID,
    host_token_secret_id UUID,
    -- Interim credential (until K1.22/K1.23): the token's first characters,
    -- to find the row, and the SHA-256 of the whole token.
    interim_token_prefix TEXT UNIQUE CHECK (
        interim_token_prefix IS NULL OR char_length(interim_token_prefix) = 12
    ),
    interim_token_digest BYTEA CHECK (
        interim_token_digest IS NULL OR octet_length(interim_token_digest) = 32
    ),
    CHECK ((interim_token_prefix IS NULL) = (interim_token_digest IS NULL)),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'offline', 'revoked')),
    builtin BOOLEAN NOT NULL DEFAULT false,
    report_interval_seconds INTEGER NOT NULL DEFAULT 120 CHECK (
        report_interval_seconds BETWEEN 15 AND 3600
    ),
    capacity JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(capacity) = 'object' AND octet_length(capacity::text) <= 4096
    ),
    last_seen_at TIMESTAMPTZ,
    last_command_seq_fetched BIGINT NOT NULL DEFAULT 0,
    enrolled_by_account_id UUID NOT NULL REFERENCES actors(id),
    revoked_at TIMESTAMPTZ,
    revoked_by_account_id UUID REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    CHECK (NOT builtin OR owner_workspace_id IS NOT NULL)
);

CREATE INDEX hosts_owner_workspace ON hosts (owner_workspace_id) WHERE owner_workspace_id IS NOT NULL;
CREATE INDEX hosts_owner_account ON hosts (owner_account_id) WHERE owner_account_id IS NOT NULL;
CREATE UNIQUE INDEX hosts_builtin_per_workspace ON hosts (owner_workspace_id) WHERE builtin;

CREATE TABLE host_enrolments (
    id UUID PRIMARY KEY,
    owner_account_id UUID REFERENCES actors(id),
    owner_workspace_id UUID REFERENCES organizations(id),
    CHECK (num_nonnulls(owner_account_id, owner_workspace_id) = 1),
    -- SHA-256 of the code; the code itself is shown once.
    code_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    label TEXT CHECK (label IS NULL OR (char_length(label) <= 80 AND label !~ '[[:cntrl:]]')),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    host_id UUID REFERENCES hosts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((used_at IS NULL) = (host_id IS NULL))
);

CREATE INDEX host_enrolments_open ON host_enrolments (created_by_account_id, created_at)
    WHERE used_at IS NULL;

CREATE TABLE host_agents (
    host_id UUID NOT NULL REFERENCES hosts(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    host_agent_ref TEXT NOT NULL CHECK (
        char_length(host_agent_ref) BETWEEN 1 AND 200 AND host_agent_ref !~ '[[:cntrl:]]'
    ),
    -- The outside reference's lineage, as the host described it: a
    -- reference change keeps the account only when this matches.
    lineage TEXT CHECK (lineage IS NULL OR char_length(lineage) BETWEEN 1 AND 200),
    backends JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(backends) = 'array' AND octet_length(backends::text) <= 16384
    ),
    actions JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(actions) = 'array' AND octet_length(actions::text) <= 16384
    ),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'gone')),
    bound_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (host_id, account_id),
    UNIQUE (host_id, host_agent_ref)
);

CREATE INDEX host_agents_account ON host_agents (account_id);

CREATE TABLE host_notices (
    id UUID PRIMARY KEY,
    host_id UUID NOT NULL REFERENCES hosts(id),
    account_id UUID REFERENCES actors(id),
    kind TEXT NOT NULL CHECK (kind IN (
        'agent_profile_changed', 'agent_ref_changed', 'agent_retired'
    )),
    host_agent_ref TEXT NOT NULL CHECK (char_length(host_agent_ref) BETWEEN 1 AND 200),
    body JSONB NOT NULL CHECK (jsonb_typeof(body) = 'object' AND octet_length(body::text) <= 16384),
    -- applied: done here; waiting: for K1.7 (outside references) to replay;
    -- request_opened: the account's managers decide.
    outcome TEXT NOT NULL CHECK (outcome IN ('applied', 'waiting', 'request_opened')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX host_notices_host ON host_notices (host_id, created_at DESC);
CREATE INDEX host_notices_waiting ON host_notices (created_at) WHERE outcome = 'waiting';

-- The built-in host of one workspace: the server's own runner, under an
-- integration account of its own.
CREATE FUNCTION hosts_ensure_builtin(workspace UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    account UUID;
BEGIN
    IF EXISTS (SELECT 1 FROM hosts WHERE owner_workspace_id = workspace AND builtin) THEN
        RETURN;
    END IF;
    account := gen_random_uuid();
    INSERT INTO actors (id, kind, status) VALUES (account, 'integration', 'active');
    INSERT INTO hosts (
        id, account_id, owner_workspace_id, name, software, protocol_version,
        capabilities, builtin, enrolled_by_account_id
    )
    VALUES (
        gen_random_uuid(), account, workspace, 'This server', 'agent-workspace', '2026-11',
        '{"start": true, "cancel": true, "steer": true, "answer": true, "inspect": true}'::jsonb,
        true, account
    );
END
$$;

SELECT hosts_ensure_builtin(id) FROM organizations ORDER BY id;

CREATE FUNCTION hosts_builtin_for_new_workspace()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM hosts_ensure_builtin(NEW.id);
    RETURN NEW;
END
$$;

CREATE TRIGGER organizations_builtin_host
    AFTER INSERT ON organizations
    FOR EACH ROW EXECUTE FUNCTION hosts_builtin_for_new_workspace();

ALTER TABLE runs
    ADD CONSTRAINT runs_host_fk FOREIGN KEY (host_id) REFERENCES hosts(id) NOT VALID;
ALTER TABLE runs VALIDATE CONSTRAINT runs_host_fk;
ALTER TABLE run_usage
    ADD CONSTRAINT run_usage_host_fk FOREIGN KEY (host_id) REFERENCES hosts(id) NOT VALID;
ALTER TABLE run_usage VALIDATE CONSTRAINT run_usage_host_fk;
ALTER TABLE usage_reports
    ADD CONSTRAINT usage_reports_host_fk FOREIGN KEY (host_id) REFERENCES hosts(id) NOT VALID;
ALTER TABLE usage_reports VALIDATE CONSTRAINT usage_reports_host_fk;
ALTER TABLE requests
    ADD CONSTRAINT requests_raised_via_host_fk
        FOREIGN KEY (raised_via_host_id) REFERENCES hosts(id) NOT VALID;
ALTER TABLE requests VALIDATE CONSTRAINT requests_raised_via_host_fk;
