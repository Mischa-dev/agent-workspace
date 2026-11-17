-- T1.6 (lane D, records): personal, shared, pinned and default views, in
-- the one view store every collection (Tasks included) reads.
--
-- * `collection_views.owner_actor_id`: NULL for a view shared with everyone
--   who can see the collection, else the one person (or agent) it belongs
--   to. Names are unique among the shared views, and among each owner's
--   own.
-- * `collection_views.builtin`: the views every Tasks collection starts
--   with, made once.
-- * Two more kinds a view can be: a timeline and a chart.
-- * `collection_view_pins`: who keeps which view in their sidebar, in what
--   order. A row overrides the default (built-in My tasks and Ready for
--   agents start pinned; the rest start unpinned).
-- * `collection_view_defaults`: the view a collection opens on, for one
--   person (theirs wins) or for everyone holding a role.

ALTER TABLE collection_views
    ADD COLUMN owner_actor_id UUID,
    ADD COLUMN builtin TEXT CHECK (
        builtin IS NULL
        OR builtin IN ('my_tasks', 'ready_for_agents', 'blocked', 'recently_changed')
    ),
    ADD CONSTRAINT collection_views_builtin_shared CHECK (builtin IS NULL OR owner_actor_id IS NULL),
    ADD CONSTRAINT collection_views_owner_fkey FOREIGN KEY (project_id, owner_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

ALTER TABLE collection_views DROP CONSTRAINT collection_views_view_type_check;
ALTER TABLE collection_views ADD CONSTRAINT collection_views_view_type_check CHECK (
    view_type IN ('table', 'board', 'list', 'gallery', 'calendar', 'timeline', 'chart')
);

DROP INDEX collection_views_name_unique;
CREATE UNIQUE INDEX collection_views_name_unique
    ON collection_views (
        collection_id,
        COALESCE(owner_actor_id, '00000000-0000-0000-0000-000000000000'::uuid),
        lower(name)
    );
CREATE UNIQUE INDEX collection_views_one_builtin
    ON collection_views (collection_id, builtin)
    WHERE builtin IS NOT NULL;
CREATE INDEX collection_views_owner
    ON collection_views (collection_id, owner_actor_id, position, id)
    WHERE owner_actor_id IS NOT NULL;

CREATE TABLE collection_view_pins (
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL,
    view_id UUID NOT NULL,
    actor_id UUID NOT NULL REFERENCES actors(id),
    pinned BOOLEAN NOT NULL,
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (view_id, actor_id),
    FOREIGN KEY (project_id, view_id)
        REFERENCES collection_views(project_id, id) ON DELETE CASCADE
);
CREATE INDEX collection_view_pins_actor ON collection_view_pins (actor_id, project_id);
SELECT tenancy_protect('collection_view_pins', 'workspace_id');

CREATE TABLE collection_view_defaults (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL,
    collection_id UUID NOT NULL,
    -- Exactly one of the two: a role's default, or one account's own.
    role_id UUID REFERENCES workspace_roles(id) ON DELETE CASCADE,
    actor_id UUID REFERENCES actors(id),
    view_id UUID NOT NULL,
    set_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((role_id IS NULL) <> (actor_id IS NULL)),
    FOREIGN KEY (project_id, collection_id) REFERENCES collections(project_id, id),
    FOREIGN KEY (project_id, view_id)
        REFERENCES collection_views(project_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collection_view_defaults_role
    ON collection_view_defaults (collection_id, role_id) WHERE role_id IS NOT NULL;
CREATE UNIQUE INDEX collection_view_defaults_actor
    ON collection_view_defaults (collection_id, actor_id) WHERE actor_id IS NOT NULL;
SELECT tenancy_protect('collection_view_defaults', 'workspace_id');
