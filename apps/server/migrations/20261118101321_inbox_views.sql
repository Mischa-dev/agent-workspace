-- C2.11: inbox views a person saves, each account's own in each workspace.
-- A view holds a filter (which part of the inbox, which kinds, which
-- spaces) and how densely to show it; the rows it shows are read live with
-- the reader's access every time.

CREATE TABLE inbox_views (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 40
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    filter JSONB NOT NULL CHECK (
        jsonb_typeof(filter) = 'object' AND octet_length(filter::text) <= 4096
    ),
    density TEXT NOT NULL DEFAULT 'detailed' CHECK (density IN ('detailed', 'compact')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX inbox_views_name_unique
    ON inbox_views (account_id, workspace_id, lower(name));
CREATE INDEX inbox_views_by_workspace ON inbox_views (workspace_id);
SELECT tenancy_protect('inbox_views', 'workspace_id');
