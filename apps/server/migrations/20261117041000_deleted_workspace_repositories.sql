-- H1.24: the repositories a deleted workspace had, so the deletion job can
-- remove their mirrors and worktrees from disk once its rows are gone for
-- good, and again on a retry after that removal failed.
ALTER TABLE deleted_workspaces
    ADD COLUMN repository_ids UUID[] NOT NULL DEFAULT '{}';
