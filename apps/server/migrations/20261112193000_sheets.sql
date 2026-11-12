-- AWD-54: sheets, a project's spreadsheets (docs/plans/sheets.md).
--
-- * sheets: a project's workbooks. Each has a name, a description and a
--   revision that goes up by one with every change to its cells or tabs.
--   Archiving hides it; nothing is deleted.
-- * sheet_tabs: a sheet's tabs, in order. structure_revision is the
--   revision that last inserted or deleted its rows or columns, or renamed
--   it: a cell edit made against an older revision is refused, since the
--   cell it named may have moved.
-- * sheet_cells: the cells that hold something. input is what was typed
--   (12, Due, TRUE, =SUM(B2:B9)); kind and value are what it computes to,
--   worked out on the server by crate::sheets::engine. revision is the one
--   that last changed the input, so two people (or agents) editing different
--   cells never conflict, and the same cell only when one of them read it
--   before the other's change.
-- * sheet_cleared_cells: where cells were cleared, and when, for the same
--   conflict check.
-- * sheet_revisions: every change, with who made it and the connected app
--   they acted through (current_via_grant(), migration 0042).
--
-- Every write takes the project row lock first
-- (work_item_commands::lock_membership), then the sheet's row.

CREATE TABLE sheets (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 500 AND description !~ '[[:cntrl:]]'
    ),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_by_actor_id UUID NOT NULL,
    updated_by_actor_id UUID NOT NULL,
    archived_at TIMESTAMPTZ,
    archived_by_actor_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, updated_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK ((archived_at IS NULL) = (archived_by_actor_id IS NULL))
);

-- Agents name a sheet instead of passing its ID.
CREATE UNIQUE INDEX sheets_active_name_unique
    ON sheets (project_id, lower(name))
    WHERE archived_at IS NULL;

CREATE INDEX sheets_project_updated ON sheets (project_id, updated_at DESC, id);

CREATE TABLE sheet_tabs (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    sheet_id UUID NOT NULL,
    -- Spreadsheet tab names: up to 31 characters, none of []:*?/\ and not
    -- starting or ending with an apostrophe.
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 31
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
        AND translate(name, '[]:*?/\', '') = name
        AND name !~ '^''|''$'
    ),
    position INTEGER NOT NULL CHECK (position >= 0),
    structure_revision BIGINT NOT NULL DEFAULT 1 CHECK (structure_revision > 0),
    -- Rows at the top that stay put while the rest scrolls (the header).
    frozen_rows INTEGER NOT NULL DEFAULT 1 CHECK (frozen_rows BETWEEN 0 AND 20),
    -- {"<column number>": width in pixels} for columns not at the default.
    column_widths JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(column_widths) = 'object'
        AND octet_length(column_widths::text) <= 8192
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, sheet_id) REFERENCES sheets(project_id, id)
);

CREATE UNIQUE INDEX sheet_tabs_name_unique ON sheet_tabs (sheet_id, lower(name));
CREATE INDEX sheet_tabs_order ON sheet_tabs (sheet_id, position, id);

CREATE TABLE sheet_cells (
    project_id UUID NOT NULL,
    sheet_id UUID NOT NULL,
    tab_id UUID NOT NULL,
    row_number INTEGER NOT NULL CHECK (row_number BETWEEN 1 AND 10000),
    column_number INTEGER NOT NULL CHECK (column_number BETWEEN 1 AND 200),
    input TEXT NOT NULL CHECK (char_length(input) BETWEEN 1 AND 10000),
    kind TEXT NOT NULL CHECK (kind IN ('empty', 'number', 'text', 'boolean', 'error')),
    value JSONB NOT NULL DEFAULT 'null'::jsonb CHECK (octet_length(value::text) <= 65536),
    revision BIGINT NOT NULL CHECK (revision > 0),
    updated_by_actor_id UUID NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tab_id, row_number, column_number),
    FOREIGN KEY (project_id, sheet_id) REFERENCES sheets(project_id, id),
    FOREIGN KEY (project_id, tab_id) REFERENCES sheet_tabs(project_id, id),
    FOREIGN KEY (project_id, updated_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX sheet_cells_sheet ON sheet_cells (sheet_id);

-- Cells cleared since their tab's rows or columns last moved, with the
-- revision that cleared them: an edit read before the clear is refused, as
-- it would be for a cell that changed. A tab whose rows or columns move
-- forgets its clears, since its structure_revision then refuses older edits.
CREATE TABLE sheet_cleared_cells (
    tab_id UUID NOT NULL REFERENCES sheet_tabs(id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL CHECK (row_number BETWEEN 1 AND 10000),
    column_number INTEGER NOT NULL CHECK (column_number BETWEEN 1 AND 200),
    revision BIGINT NOT NULL CHECK (revision > 0),
    PRIMARY KEY (tab_id, row_number, column_number)
);

CREATE TABLE sheet_revisions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    sheet_id UUID NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    actor_id UUID NOT NULL,
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    -- {"cells": changed inputs, "operations": [what was asked, in short]}.
    summary JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(summary) = 'object' AND octet_length(summary::text) <= 65536
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (sheet_id, revision),
    FOREIGN KEY (project_id, sheet_id) REFERENCES sheets(project_id, id),
    FOREIGN KEY (project_id, actor_id) REFERENCES project_memberships(project_id, actor_id)
);

-- sheet.updated: a sheet's cells, tabs, name or archive state changed. Ids
-- and the new revision only; open screens fetch what they show over HTTP.
ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification', 'whiteboard', 'page', 'collection',
            'sheet'
        )
    );
