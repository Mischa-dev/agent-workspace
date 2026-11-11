-- AWD-90: collections, the record types a project makes for itself, such as
-- Playtest feedback or Builds (M4 of the AWD-81 plan; docs/plans/collections.md).
--
-- * collections: a project's record types. Each has a name, a description
--   and the name of its title column (Name, Version, Tester). Archiving hides
--   it and its records; nothing is deleted.
-- * field_definitions gains collection_id: a collection's fields are rows
--   with record_kind 'record' and the collection's ID, so tasks and
--   collections share one set of field types, checks, filters and sorts
--   (crate::fields), as migration 20261105041500 planned. Task fields keep
--   collection_id NULL. Names stay unique among a project's task fields, and
--   among one collection's fields.
-- * collection_views: the ways to look at a collection. A view has a type
--   (table first) and its filters, sort, grouping and columns as JSON, which
--   crate::collections checks on every write.
-- * collection_records: the records. Each has a number in its collection
--   (#12), a title, and values keyed by field ID, as work_items.field_values
--   keeps a task's.
-- * collection_record_history: every change to a record, with who made it
--   and the connected app they acted through (current_via_grant(), migration
--   0042).
--
-- Every write takes the project row lock first
-- (work_item_commands::lock_membership), then the collection's row, so
-- record numbers, limits and names are exact.

CREATE TABLE collections (
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
    -- What the title column is called.
    title_name TEXT NOT NULL DEFAULT 'Name' CHECK (
        char_length(title_name) BETWEEN 1 AND 60
        AND title_name = btrim(title_name)
        AND title_name !~ '[[:cntrl:]]'
    ),
    -- The number the next record gets.
    next_number BIGINT NOT NULL DEFAULT 1 CHECK (next_number > 0),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_by_actor_id UUID NOT NULL,
    archived_at TIMESTAMPTZ,
    archived_by_actor_id UUID,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK ((archived_at IS NULL) = (archived_by_actor_id IS NULL))
);

-- Agents name a collection instead of passing its ID.
CREATE UNIQUE INDEX collections_active_name_unique
    ON collections (project_id, lower(name))
    WHERE archived_at IS NULL;

CREATE INDEX collections_project_order ON collections (project_id, position, id);

ALTER TABLE field_definitions ADD COLUMN collection_id UUID;

ALTER TABLE field_definitions
    DROP CONSTRAINT field_definitions_record_kind_check,
    ADD CONSTRAINT field_definitions_record_kind_check CHECK (
        record_kind IN ('task', 'record')
    ),
    ADD CONSTRAINT field_definitions_collection_check CHECK (
        (record_kind = 'record') = (collection_id IS NOT NULL)
    ),
    ADD CONSTRAINT field_definitions_collection_fkey
        FOREIGN KEY (project_id, collection_id) REFERENCES collections(project_id, id);

-- Task fields' names are unique in their project, a collection's within it.
DROP INDEX field_definitions_active_name_unique;

CREATE UNIQUE INDEX field_definitions_active_name_unique
    ON field_definitions (project_id, record_kind, lower(name))
    WHERE archived_at IS NULL AND collection_id IS NULL;

CREATE UNIQUE INDEX field_definitions_collection_name_unique
    ON field_definitions (collection_id, lower(name))
    WHERE archived_at IS NULL AND collection_id IS NOT NULL;

CREATE INDEX field_definitions_collection_order
    ON field_definitions (collection_id, position, id)
    WHERE collection_id IS NOT NULL;

CREATE TABLE collection_views (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    collection_id UUID NOT NULL,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    view_type TEXT NOT NULL CHECK (
        view_type IN ('table', 'board', 'list', 'gallery', 'calendar')
    ),
    -- {"filters", "sort", "groupBy", "columns"}: see crate::collections::views.
    config JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(config) = 'object' AND octet_length(config::text) <= 16384
    ),
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    created_by_actor_id UUID NOT NULL,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, collection_id) REFERENCES collections(project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE UNIQUE INDEX collection_views_name_unique
    ON collection_views (collection_id, lower(name));

CREATE INDEX collection_views_order ON collection_views (collection_id, position, id);

CREATE TABLE collection_records (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    collection_id UUID NOT NULL,
    number BIGINT NOT NULL CHECK (number > 0),
    title TEXT NOT NULL DEFAULT '' CHECK (
        char_length(title) <= 300
        AND title = btrim(title)
        AND title !~ '[[:cntrl:]]'
    ),
    -- Values keyed by field ID; a field with no value has no key.
    field_values JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(field_values) = 'object'
        AND octet_length(field_values::text) <= 65536
    ),
    created_by_actor_id UUID NOT NULL,
    updated_by_actor_id UUID NOT NULL,
    archived_at TIMESTAMPTZ,
    archived_by_actor_id UUID,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    UNIQUE (collection_id, number),
    FOREIGN KEY (project_id, collection_id) REFERENCES collections(project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, updated_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK ((archived_at IS NULL) = (archived_by_actor_id IS NULL))
);

CREATE INDEX collection_records_updated
    ON collection_records (collection_id, updated_at DESC, id DESC);

CREATE TABLE collection_record_history (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    record_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    action TEXT NOT NULL CHECK (action IN ('created', 'updated', 'archived', 'restored')),
    -- [{"field": a field ID or "title", "name", "before", "after"}], each
    -- value as it read then: option names, a person's name, a task's key, a
    -- page's title. Long text is cut short.
    changes JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(changes) = 'array' AND octet_length(changes::text) <= 262144
    ),
    version BIGINT NOT NULL CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, record_id) REFERENCES collection_records(project_id, id),
    FOREIGN KEY (project_id, actor_id) REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX collection_record_history_record
    ON collection_record_history (record_id, created_at DESC, id DESC);

-- collection.updated: a collection, its fields or views, or its records
-- changed. Ids only; open screens fetch what they show over HTTP.
ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification', 'whiteboard', 'page', 'collection'
        )
    );
