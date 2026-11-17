-- T1.32: dashboards. A dashboard is a page of tiles in a project: each tile
-- shows one of a collection's views (a chart, a number, a short list) or a
-- note. Shared dashboards are everyone's in the project; a personal one
-- (owner set) is its owner's alone. A tile only names a view: its figures
-- are read live, as whoever looks, so sharing a dashboard shares its layout
-- and never data the viewer can't already see.

CREATE TABLE dashboards (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    -- NULL: shared in the project; else its owner's alone.
    owner_actor_id UUID REFERENCES actors(id) ON DELETE CASCADE,
    -- [{"id", "kind": "view"|"text", "title", "view", "collection", "text",
    --   "width": 1..12, "height": 1..4}]: see crate::dashboards.
    tiles JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(tiles) = 'array' AND octet_length(tiles::text) <= 65536
    ),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX dashboards_name_unique
    ON dashboards (
        project_id,
        COALESCE(owner_actor_id, '00000000-0000-0000-0000-000000000000'::uuid),
        lower(name)
    );

CREATE INDEX dashboards_by_project ON dashboards (project_id, position);
CREATE INDEX dashboards_by_workspace ON dashboards (workspace_id);
