-- C1.13 (lane D2, documents): a sheet's history reaches every revision.
--
-- * `sheet_revisions.cells`: the inputs a revision changed, by tab id and
--   position ([[tabId, row, column, input or null], ...]); NULL when the
--   revision changed tabs or too many cells, which then has a snapshot.
-- * `sheet_snapshots`: every tab and input of a sheet at a revision, kept
--   when the sheet is made, when its tabs change, after a write too large
--   to keep cell by cell, and every 25 revisions. A revision is the latest
--   snapshot at or before it with the later revisions' inputs laid over.
-- * Every sheet gets a snapshot of its revision now, so its history from
--   here on can be shown and restored.

ALTER TABLE sheet_revisions
    ADD COLUMN cells JSONB CHECK (
        cells IS NULL OR (jsonb_typeof(cells) = 'array' AND octet_length(cells::text) <= 4194304)
    );

CREATE TABLE sheet_snapshots (
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    sheet_id UUID NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    state JSONB NOT NULL CHECK (
        jsonb_typeof(state) = 'object' AND octet_length(state::text) <= 67108864
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (sheet_id, revision),
    FOREIGN KEY (space_id, sheet_id) REFERENCES sheets(project_id, id) ON UPDATE CASCADE
);
SELECT tenancy_protect('sheet_snapshots', 'workspace_id');

INSERT INTO sheet_snapshots (workspace_id, space_id, sheet_id, revision, state)
SELECT project.organization_id, sheet.project_id, sheet.id, sheet.revision,
       jsonb_build_object('tabs', COALESCE((
           SELECT jsonb_agg(jsonb_build_object(
               'id', tab.id, 'name', tab.name, 'frozenRows', tab.frozen_rows,
               'columnWidths', tab.column_widths,
               'cells', COALESCE((
                   SELECT jsonb_agg(jsonb_build_array(cell.row_number, cell.column_number, cell.input)
                                    ORDER BY cell.row_number, cell.column_number)
                   FROM sheet_cells cell WHERE cell.tab_id = tab.id), '[]'::jsonb)
           ) ORDER BY tab.position, tab.id)
           FROM sheet_tabs tab WHERE tab.sheet_id = sheet.id), '[]'::jsonb))
FROM sheets sheet JOIN projects project ON project.id = sheet.project_id;
