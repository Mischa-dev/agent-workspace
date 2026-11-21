-- A1.17: a run's changes, as the host doing it reports them. The server no
-- longer reads a run's worktree or the repository mirror to show what a
-- run changed: the host, which holds them and may be another machine,
-- reports the files and the patch, and the run page shows the latest
-- report. Any host may report them, not only the built-in one.
CREATE TABLE run_changes (
    run_id UUID PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    -- [{path, status, added, removed}], at most 2,000.
    files JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(files) = 'array' AND jsonb_array_length(files) <= 2000),
    -- The unified diff, empty when it was too big to send.
    patch TEXT NOT NULL DEFAULT '' CHECK (octet_length(patch) <= 786432),
    truncated BOOLEAN NOT NULL DEFAULT false,
    reported_by_account_id UUID NOT NULL REFERENCES actors(id),
    reported_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX run_changes_workspace ON run_changes (workspace_id);
SELECT tenancy_protect('run_changes', 'workspace_id');
