-- C1.18 (lane D2, documents): charts on sheets.
--
-- A chart draws a block of a sheet: labels from its first column, numbers
-- from its second (a header row names the series), as bars, a line, a
-- donut or one number. `source` is what it reads, a named range such as
-- "Budget" (which moves with its cells) or a block such as Summary!A1:B12;
-- the figures are worked out when the chart is read, so a chart always
-- shows the cells as they are. A page embeds a chart by its id.

CREATE TABLE sheet_charts (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    sheet_id UUID NOT NULL,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    style TEXT NOT NULL CHECK (style IN ('bar', 'line', 'donut', 'number')),
    source TEXT NOT NULL CHECK (char_length(source) BETWEEN 1 AND 200),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, sheet_id) REFERENCES sheets(project_id, id) ON UPDATE CASCADE ON DELETE CASCADE
);
SELECT tenancy_protect('sheet_charts', 'workspace_id');

CREATE INDEX sheet_charts_sheet ON sheet_charts (sheet_id, position);
