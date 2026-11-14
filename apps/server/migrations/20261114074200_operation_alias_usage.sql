-- K2.3 (operation layer): how often each old tool name or v1 path is still
-- called in a workspace, a row per alias per day, so an alias is retired
-- only once nobody uses it (K2.23). Written by K2.6's alias adapters with
-- INSERT ... ON CONFLICT DO UPDATE SET calls = calls + 1.
-- `workspace_id` references today's `organizations` until K4's rename.
CREATE TABLE operation_alias_usage (
    workspace_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    alias        text NOT NULL CHECK (char_length(alias) BETWEEN 1 AND 80 AND alias !~ '[[:cntrl:]]'),
    day          date NOT NULL,
    calls        bigint NOT NULL DEFAULT 0 CHECK (calls >= 0),
    PRIMARY KEY (workspace_id, alias, day)
);
