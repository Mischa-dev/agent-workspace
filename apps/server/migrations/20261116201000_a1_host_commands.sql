-- A1.12: the host command channel (A1 section 3.9).
--
-- host_commands: what Workspace asks a host to do (start, cancel, steer,
-- answer, inspect, action). The host reads them by long-poll, a stream, or
-- after a signed push, fetches each in full, and acknowledges it. A start's
-- start_key never changes, so a re-offer after a lost acknowledgement is
-- the same command, and UNIQUE (host_id, start_key) makes a second start
-- for one run impossible. The body never holds a secret: the run's
-- credential is minted when the host fetches the command.
--
-- run_credentials: the interim run credential (until K1.24's
-- issue_scoped): a short-lived token for the run's agent in the run's
-- space, minted at fetch, kept only as a digest, revoked when the run ends.
--
-- runs.host_missed_beats: heartbeats in a row that left out an active run
-- on its host; three make it interrupted.

CREATE TABLE host_commands (
    id UUID PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY UNIQUE,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    host_id UUID NOT NULL REFERENCES hosts(id),
    run_id UUID REFERENCES runs(id),
    request_id UUID REFERENCES requests(id),
    kind TEXT NOT NULL CHECK (kind IN ('start', 'cancel', 'steer', 'answer', 'inspect', 'action')),
    start_key TEXT CHECK (start_key IS NULL OR char_length(start_key) BETWEEN 1 AND 200),
    CHECK ((kind = 'start') = (start_key IS NOT NULL)),
    body JSONB NOT NULL CHECK (jsonb_typeof(body) = 'object' AND octet_length(body::text) <= 131072),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN (
        'pending', 'fetched', 'acked', 'refused', 'uncertain', 'expired', 'superseded')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_offer_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ack_due_at TIMESTAMPTZ,
    ack JSONB CHECK (ack IS NULL OR (jsonb_typeof(ack) = 'object' AND octet_length(ack::text) <= 70000)),
    acked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (host_id, start_key)
);

CREATE INDEX host_commands_due ON host_commands (host_id, seq)
    WHERE state IN ('pending', 'fetched', 'uncertain');
CREATE INDEX host_commands_run ON host_commands (run_id) WHERE run_id IS NOT NULL;
CREATE INDEX host_commands_ack_due ON host_commands (ack_due_at)
    WHERE state IN ('pending', 'fetched') AND ack_due_at IS NOT NULL;
CREATE INDEX host_commands_workspace ON host_commands (workspace_id);
SELECT tenancy_protect('host_commands', 'workspace_id');

CREATE TABLE run_credentials (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    run_id UUID NOT NULL REFERENCES runs(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    host_id UUID NOT NULL REFERENCES hosts(id),
    command_id UUID NOT NULL REFERENCES host_commands(id),
    token_prefix TEXT NOT NULL UNIQUE CHECK (char_length(token_prefix) = 12),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

CREATE INDEX run_credentials_run ON run_credentials (run_id) WHERE revoked_at IS NULL;
CREATE INDEX run_credentials_workspace ON run_credentials (workspace_id);
SELECT tenancy_protect('run_credentials', 'workspace_id');

ALTER TABLE runs
    ADD COLUMN host_missed_beats SMALLINT NOT NULL DEFAULT 0 CHECK (host_missed_beats >= 0);
