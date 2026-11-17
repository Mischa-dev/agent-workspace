-- H1.9 and H1.10: a workspace leaves as one archive and comes back from
-- one. Each export or import is a transfer, driven by a job: an export
-- writes its archive to the workspace's store and keeps it 7 days; an
-- import walks an archive in batches, its cursor saved with each batch so
-- a stopped import resumes where it was. `workspace_id` is the workspace
-- exported, or the new workspace an import fills.
CREATE TABLE workspace_transfers (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('export', 'import')),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'expired')),
    requested_by_actor_id UUID NOT NULL REFERENCES actors(id),
    store_id TEXT CHECK (store_id IS NULL OR store_id ~ '^[a-z][a-z0-9_-]{0,47}$'),
    file_key TEXT CHECK (file_key IS NULL OR file_key ~ '^[0-9a-z/.-]{1,200}$'),
    byte_size BIGINT CHECK (byte_size IS NULL OR byte_size >= 0),
    id_mode TEXT CHECK (id_mode IS NULL OR id_mode IN ('preserve', 'copy')),
    cursor JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(cursor::text) <= 4096),
    counts JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(counts::text) <= 65536),
    error TEXT CHECK (error IS NULL OR char_length(error) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ
);
CREATE INDEX workspace_transfers_workspace ON workspace_transfers (workspace_id, created_at DESC);
CREATE INDEX workspace_transfers_expiring ON workspace_transfers (expires_at)
    WHERE state = 'succeeded' AND expires_at IS NOT NULL;
SELECT tenancy_protect('workspace_transfers', 'workspace_id');

-- Copy mode: every id an import gave a fresh one, so references (in
-- columns and in text) are rewritten through it.
CREATE TABLE workspace_import_ids (
    transfer_id UUID NOT NULL REFERENCES workspace_transfers(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    source_id UUID NOT NULL,
    target_id UUID NOT NULL,
    PRIMARY KEY (transfer_id, source_id)
);
SELECT tenancy_protect('workspace_import_ids', 'workspace_id');

-- "Export the workspace" (`workspace.export`) in the SQL permission
-- catalogue, beside `policy::PERMISSIONS`. Owners hold it by default.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('workspace.export', 'settings', 'workspace', false, ARRAY['owner']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;
