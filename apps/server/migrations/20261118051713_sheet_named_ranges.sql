-- C1.20 (lane D2, documents): named ranges on sheets.
--
-- A named range gives a block of one tab a name, such as "Budget" for
-- Summary!A1:B12, so a dashboard tile, a page embed or an agent can read
-- it by name. It moves with the rows and columns inserted or deleted
-- around it and goes with its tab, or when every cell of it is deleted.
-- Rows and columns count from 1, both ends inclusive.

CREATE TABLE sheet_named_ranges (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    sheet_id UUID NOT NULL,
    tab_id UUID NOT NULL REFERENCES sheet_tabs(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60),
    first_row INTEGER NOT NULL CHECK (first_row >= 1),
    last_row INTEGER NOT NULL CHECK (last_row >= first_row),
    first_column INTEGER NOT NULL CHECK (first_column >= 1),
    last_column INTEGER NOT NULL CHECK (last_column >= first_column),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, sheet_id) REFERENCES sheets(project_id, id) ON UPDATE CASCADE
);
SELECT tenancy_protect('sheet_named_ranges', 'workspace_id');

CREATE UNIQUE INDEX sheet_named_ranges_name_unique ON sheet_named_ranges (sheet_id, lower(name));
CREATE INDEX sheet_named_ranges_tab ON sheet_named_ranges (tab_id);
