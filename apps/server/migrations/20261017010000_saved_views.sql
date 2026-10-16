-- Saved views (docs/plans/roadmap.md, workstream W11, V5).
--
-- A view is a named task list: the same query the list page keeps in its URL
-- (filters, sort, grouping, layout) plus which fields it shows. A personal
-- view belongs to the person who saved it; a project view is shared with
-- everyone in the project. Every project gets four built-in project views
-- (My tasks, Ready for agents, Blocked, Recently changed), created the first
-- time its views are read; `builtin` names them so they're made once.
CREATE TABLE saved_views (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    -- NULL for a view shared with the project.
    owner_actor_id UUID REFERENCES actors(id),
    created_by_actor_id UUID REFERENCES actors(id),
    builtin TEXT CHECK (
        builtin IS NULL
        OR builtin IN ('my_tasks', 'ready_for_agents', 'blocked', 'recently_changed')
    ),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name = btrim(name)
    ),
    -- List query parameters, each a string, as the list page's URL has them.
    -- "me" stands for whoever reads the view.
    query JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(query) = 'object' AND octet_length(query::text) <= 4096
    ),
    layout TEXT NOT NULL DEFAULT 'list' CHECK (layout IN ('list', 'board')),
    -- The list columns it shows, in order; NULL shows the default columns.
    fields TEXT[] CHECK (fields IS NULL OR cardinality(fields) <= 20),
    position INTEGER NOT NULL DEFAULT 0,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    CHECK (builtin IS NULL OR owner_actor_id IS NULL),
    FOREIGN KEY (project_id, owner_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE UNIQUE INDEX saved_views_one_builtin
    ON saved_views (project_id, builtin)
    WHERE builtin IS NOT NULL;

CREATE INDEX saved_views_project_owner
    ON saved_views (project_id, owner_actor_id, position, id);

-- Who pinned which view to their sidebar. A row overrides the default: the
-- built-in My tasks and Ready for agents start pinned for everyone, others
-- start unpinned.
CREATE TABLE saved_view_pins (
    project_id UUID NOT NULL,
    view_id UUID NOT NULL,
    actor_id UUID NOT NULL REFERENCES actors(id),
    pinned BOOLEAN NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (view_id, actor_id),
    FOREIGN KEY (project_id, view_id)
        REFERENCES saved_views(project_id, id) ON DELETE CASCADE
);

CREATE INDEX saved_view_pins_actor ON saved_view_pins (actor_id, project_id);
