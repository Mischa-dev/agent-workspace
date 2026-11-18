-- C1.16 (lane D2, documents): filter views on sheets.
--
-- A view is a saved way to look at one tab: which rows show (filters by
-- column), in what order (sort), and which columns are hidden. It changes
-- no data. A view with no owner is shared with everyone who can see the
-- sheet (made and changed by people who can edit it); one with an owner is
-- that person's alone. A view goes with its tab.

CREATE TABLE sheet_views (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    sheet_id UUID NOT NULL,
    tab_id UUID NOT NULL REFERENCES sheet_tabs(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    owner_actor_id UUID REFERENCES actors(id),
    filters JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(filters) = 'array' AND octet_length(filters::text) <= 16384
    ),
    sort JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(sort) = 'array' AND octet_length(sort::text) <= 1024
    ),
    hidden_columns JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(hidden_columns) = 'array' AND octet_length(hidden_columns::text) <= 2048
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, sheet_id) REFERENCES sheets(project_id, id) ON UPDATE CASCADE
);
SELECT tenancy_protect('sheet_views', 'workspace_id');

CREATE INDEX sheet_views_sheet ON sheet_views (sheet_id, tab_id);
CREATE UNIQUE INDEX sheet_views_name_unique ON sheet_views (
    tab_id, COALESCE(owner_actor_id, '00000000-0000-0000-0000-000000000000'::uuid), lower(name)
);
