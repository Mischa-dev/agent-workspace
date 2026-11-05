-- AWD-89: custom fields, starting with fields on tasks.
--
-- * field_definitions: the fields a project defines for one kind of record.
--   The field types (text, number, date, single and multiple choice, person,
--   URL, checkbox, and a link to a task or a page) are shared: the code lives
--   in `crate::fields`, and collections (AWD-90) add their own record_kind
--   rather than a table of their own. Owners and admins add, rename,
--   reorder and archive fields; nothing is deleted, so a restored field gets
--   its values back. A field's type never changes after it's made.
-- * work_items.field_values: a task's values, keyed by field ID. A field with
--   no value has no key (an unchecked checkbox, a cleared date). Values that
--   name something (a choice option, a person, a task or a page) hold its ID;
--   the code checks them on every write, as it does for assignees.
--
-- Writes to field_definitions take the project row lock first
-- (work_item_commands::lock_membership), as label writes do, so the limits
-- and name checks are exact.

CREATE TABLE field_definitions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    -- Which records the field belongs to. Collections (AWD-90) add theirs.
    record_kind TEXT NOT NULL CHECK (record_kind IN ('task')),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 60
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 300 AND description !~ '[[:cntrl:]]'
    ),
    field_type TEXT NOT NULL CHECK (
        field_type IN (
            'text', 'number', 'date', 'single_choice', 'multiple_choice',
            'person', 'url', 'checkbox', 'link'
        )
    ),
    -- Choice options ({"options": [{"id", "name", "color", "archived"}]}) or
    -- a link's target ({"target": "task" | "page"}); see crate::fields.
    config JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(config) = 'object' AND octet_length(config::text) <= 16384
    ),
    position INTEGER NOT NULL CHECK (position >= 0),
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

-- Names are unique among a project's active fields for one kind of record,
-- ignoring case, so agents can name a field instead of passing its ID.
CREATE UNIQUE INDEX field_definitions_active_name_unique
    ON field_definitions (project_id, record_kind, lower(name))
    WHERE archived_at IS NULL;

CREATE INDEX field_definitions_project_order
    ON field_definitions (project_id, record_kind, position, id);

ALTER TABLE work_items
    ADD COLUMN field_values JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(field_values) = 'object'
        AND octet_length(field_values::text) <= 65536
    );
