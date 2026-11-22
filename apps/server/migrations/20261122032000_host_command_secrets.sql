-- Lane E, A1.18 follow-up: repository work on the built-in host's disk is
-- asked of the host as commands. A git token the host needs for one (a
-- clone, a refresh, a push) is sealed here beside its command when the host
-- runs elsewhere, and taken (deleted) when the host fetches the command, so
-- no row keeps it. It goes with its command.
CREATE TABLE host_command_secrets (
    command_id UUID PRIMARY KEY REFERENCES host_commands(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    ciphertext BYTEA NOT NULL,
    nonce BYTEA NOT NULL,
    wrapped_key BYTEA NOT NULL,
    wrap_nonce BYTEA,
    master_key_id TEXT NOT NULL CHECK (char_length(master_key_id) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

SELECT tenancy_protect('host_command_secrets', 'workspace_id');
