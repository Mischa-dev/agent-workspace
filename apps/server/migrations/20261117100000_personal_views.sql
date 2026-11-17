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
    space_id UUID NOT NULL,
    view_id UUID NOT NULL,
    actor_id UUID NOT NULL REFERENCES actors(id),
    pinned BOOLEAN NOT NULL,
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (view_id, actor_id),
    FOREIGN KEY (space_id, view_id)
        REFERENCES collection_views(project_id, id) ON DELETE CASCADE
);
CREATE INDEX collection_view_pins_actor ON collection_view_pins (actor_id, space_id);
SELECT tenancy_protect('collection_view_pins', 'workspace_id');

CREATE TABLE collection_view_defaults (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL,
    collection_id UUID NOT NULL,
    -- Exactly one of the two: a role's default, or one account's own.
    role_id UUID REFERENCES workspace_roles(id) ON DELETE CASCADE,
    actor_id UUID REFERENCES actors(id),
    view_id UUID NOT NULL,
    set_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((role_id IS NULL) <> (actor_id IS NULL)),
    FOREIGN KEY (space_id, collection_id) REFERENCES collections(project_id, id),
    FOREIGN KEY (space_id, view_id)
        REFERENCES collection_views(project_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collection_view_defaults_role
    ON collection_view_defaults (collection_id, role_id) WHERE role_id IS NOT NULL;
CREATE UNIQUE INDEX collection_view_defaults_actor
    ON collection_view_defaults (collection_id, actor_id) WHERE actor_id IS NOT NULL;
SELECT tenancy_protect('collection_view_defaults', 'workspace_id');

-- T1.8: the task list's saved views move into this store, as views of each
-- space's Tasks collection, and `saved_views` goes. A saved view kept the
-- list page's address (`statusId=...&includeDone=false&sort=due`); this
-- reads it the way `collections::task_query::from_params` does, each list
-- filter a built-in field of the Tasks collection. A parameter it doesn't
-- know is left out.
CREATE FUNCTION task_view_config(query JSONB, layout TEXT) RETURNS JSONB
LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE
    filters JSONB := '[]'::jsonb;
    config JSONB := '{}'::jsonb;
    key TEXT;
    raw TEXT;
    parts TEXT[];
    has_none BOOLEAN;
    field UUID;
    either JSONB;
BEGIN
    FOR key, raw IN SELECT entry.key, entry.value FROM jsonb_each_text(query) entry LOOP
        parts := ARRAY(
            SELECT btrim(part) FROM unnest(string_to_array(raw, ',')) part
            WHERE btrim(part) NOT IN ('', 'none', 'no_priority')
        );
        has_none := EXISTS (
            SELECT 1 FROM unnest(string_to_array(raw, ',')) part
            WHERE btrim(part) IN ('none', 'no_priority')
        );
        field := CASE
            WHEN key IN ('statusId', 'notStatusId', 'semanticCategory', 'notSemanticCategory', 'includeDone')
                THEN '0192f5c0-0000-7000-8000-000000000001'
            WHEN key IN ('assigneeActorId', 'notAssigneeActorId', 'unassigned')
                THEN '0192f5c0-0000-7000-8000-000000000002'
            WHEN key IN ('priority', 'notPriority') THEN '0192f5c0-0000-7000-8000-000000000003'
            WHEN key IN ('dueBefore', 'dueAfter', 'overdue') THEN '0192f5c0-0000-7000-8000-000000000005'
            WHEN key IN ('labelIds', 'anyLabelIds', 'notLabelIds') THEN '0192f5c0-0000-7000-8000-000000000006'
            WHEN key IN ('typeId', 'notTypeId') THEN '0192f5c0-0000-7000-8000-000000000007'
            WHEN key IN ('milestoneId', 'notMilestoneId') THEN '0192f5c0-0000-7000-8000-000000000008'
            WHEN key = 'parentId' THEN '0192f5c0-0000-7000-8000-000000000009'
            WHEN key = 'blocked' THEN '0192f5c0-0000-7000-8000-00000000000a'
            WHEN key = 'ready' THEN '0192f5c0-0000-7000-8000-00000000000b'
            WHEN key = 'claimed' THEN '0192f5c0-0000-7000-8000-00000000000c'
        END;
        IF key IN ('priority', 'notPriority') THEN
            parts := ARRAY(
                SELECT CASE part
                    WHEN 'urgent' THEN '0192f5c0-0000-7000-8000-0000000000a1'
                    WHEN 'high' THEN '0192f5c0-0000-7000-8000-0000000000a2'
                    WHEN 'medium' THEN '0192f5c0-0000-7000-8000-0000000000a3'
                    WHEN 'low' THEN '0192f5c0-0000-7000-8000-0000000000a4'
                END
                FROM unnest(parts) part
                WHERE part IN ('urgent', 'high', 'medium', 'low')
            );
        END IF;
        IF key IN ('statusId', 'typeId') AND cardinality(parts) > 0 THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'is', 'values', to_jsonb(parts)));
        ELSIF key IN ('notStatusId', 'notTypeId') AND cardinality(parts) > 0 THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'isNot', 'values', to_jsonb(parts)));
        ELSIF key = 'semanticCategory' AND cardinality(parts) > 0 THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'categoryIs', 'values', to_jsonb(parts)));
        ELSIF key = 'notSemanticCategory' AND cardinality(parts) > 0 THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'categoryIsNot', 'values', to_jsonb(parts)));
        ELSIF key = 'includeDone' AND raw IN ('false', '0') THEN
            filters := filters || jsonb_build_array(jsonb_build_object(
                'field', field, 'op', 'categoryIsNot', 'values', jsonb_build_array('completed', 'canceled')));
        ELSIF key IN ('priority', 'assigneeActorId', 'anyLabelIds', 'milestoneId', 'parentId') THEN
            either := '[]'::jsonb;
            IF cardinality(parts) > 0 THEN
                either := either || jsonb_build_array(jsonb_build_object('field', field, 'op', 'is', 'values', to_jsonb(parts)));
            END IF;
            IF has_none THEN
                either := either || jsonb_build_array(jsonb_build_object('field', field, 'op', 'empty'));
            END IF;
            IF jsonb_array_length(either) = 1 THEN
                filters := filters || either;
            ELSIF jsonb_array_length(either) = 2 THEN
                filters := filters || jsonb_build_array(jsonb_build_object('or', either));
            END IF;
        ELSIF key IN ('notPriority', 'notAssigneeActorId', 'notLabelIds', 'notMilestoneId') THEN
            IF cardinality(parts) > 0 THEN
                filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'isNot', 'values', to_jsonb(parts)));
            END IF;
            IF has_none THEN
                filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'notEmpty'));
            END IF;
        ELSIF key = 'unassigned' AND raw IN ('true', '1') THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'empty'));
        ELSIF key = 'labelIds' THEN
            filters := filters || COALESCE((
                SELECT jsonb_agg(jsonb_build_object('field', field, 'op', 'is', 'values', jsonb_build_array(part)))
                FROM unnest(parts) part
            ), '[]'::jsonb);
            IF has_none THEN
                filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'empty'));
            END IF;
        ELSIF key IN ('blocked', 'ready') THEN
            filters := filters || jsonb_build_array(CASE WHEN raw IN ('true', '1')
                THEN jsonb_build_object('field', field, 'op', 'is', 'values', jsonb_build_array('true'))
                ELSE jsonb_build_object('field', field, 'op', 'empty') END);
        ELSIF key = 'claimed' THEN
            filters := filters || jsonb_build_array(jsonb_build_object(
                'field', field, 'op', CASE WHEN raw IN ('true', '1') THEN 'notEmpty' ELSE 'empty' END));
        ELSIF key = 'dueBefore' THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'lt', 'values', jsonb_build_array(raw)));
        ELSIF key = 'dueAfter' THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'gt', 'values', jsonb_build_array(raw)));
        ELSIF key = 'overdue' AND raw IN ('true', '1') THEN
            filters := filters || jsonb_build_array(jsonb_build_object('field', field, 'op', 'within', 'values', jsonb_build_array('overdue')));
        ELSIF key = 'fieldFilters' AND jsonb_typeof(raw::jsonb) = 'array' THEN
            filters := filters || raw::jsonb;
        ELSIF key = 'sort' THEN
            config := config || jsonb_build_object('sort', CASE
                WHEN raw = 'newest' THEN jsonb_build_array(jsonb_build_object('field', 'number', 'direction', 'desc'))
                WHEN raw = 'oldest' THEN jsonb_build_array(jsonb_build_object('field', 'number', 'direction', 'asc'))
                WHEN raw = 'updated' THEN jsonb_build_array(jsonb_build_object('field', 'updated', 'direction', 'desc'))
                WHEN raw = 'priority' THEN jsonb_build_array(
                    jsonb_build_object('field', '0192f5c0-0000-7000-8000-000000000003', 'direction', 'asc'),
                    jsonb_build_object('field', 'number', 'direction', 'desc'))
                WHEN raw = 'due' THEN jsonb_build_array(
                    jsonb_build_object('field', '0192f5c0-0000-7000-8000-000000000005', 'direction', 'asc'),
                    jsonb_build_object('field', '0192f5c0-0000-7000-8000-000000000003', 'direction', 'asc'))
                WHEN raw = 'title' THEN jsonb_build_array(jsonb_build_object('field', 'title', 'direction', 'asc'))
                WHEN raw = 'manual' THEN jsonb_build_array(jsonb_build_object('field', 'rank', 'direction', 'asc'))
                WHEN raw LIKE 'field:%' THEN jsonb_build_array(
                    jsonb_build_object('field', substr(raw, 7), 'direction', 'asc'),
                    jsonb_build_object('field', 'number', 'direction', 'desc'))
                ELSE '[]'::jsonb END);
        ELSIF key IN ('groupBy', 'columnsBy') AND (key = 'columnsBy') = (layout = 'board') THEN
            config := config || COALESCE(jsonb_build_object('groupBy', CASE
                WHEN raw = 'status' THEN '0192f5c0-0000-7000-8000-000000000001'
                WHEN raw = 'assignee' THEN '0192f5c0-0000-7000-8000-000000000002'
                WHEN raw = 'priority' THEN '0192f5c0-0000-7000-8000-000000000003'
                WHEN raw = 'label' THEN '0192f5c0-0000-7000-8000-000000000006'
                WHEN raw = 'type' THEN '0192f5c0-0000-7000-8000-000000000007'
                WHEN raw = 'milestone' THEN '0192f5c0-0000-7000-8000-000000000008'
                WHEN raw = 'parent' THEN '0192f5c0-0000-7000-8000-000000000009'
                WHEN raw LIKE 'field:%' THEN substr(raw, 7)
            END), '{}'::jsonb);
        END IF;
    END LOOP;
    IF layout = 'board' AND NOT config ? 'groupBy' THEN
        config := config || jsonb_build_object('groupBy', '0192f5c0-0000-7000-8000-000000000001');
    END IF;
    IF jsonb_array_length(filters) > 0 THEN
        config := config || jsonb_build_object('filters', filters);
    END IF;
    IF config -> 'sort' = '[]'::jsonb THEN
        config := config - 'sort';
    END IF;
    RETURN config;
END
$$;

-- The views every Tasks collection starts with, after its own (the same
-- configs `task_view_config` gives their old queries).
CREATE FUNCTION add_builtin_task_views(target_collection UUID) RETURNS VOID
LANGUAGE sql AS $$
    INSERT INTO collection_views
        (id, project_id, collection_id, name, view_type, config, position, created_by_actor_id, builtin)
    SELECT gen_random_uuid(), collection.project_id, collection.id, builtin.name, 'table',
           builtin.config,
           (SELECT COALESCE(max(view.position) + 1, 0) FROM collection_views view
            WHERE view.collection_id = collection.id AND view.owner_actor_id IS NULL) + builtin.place,
           collection.created_by_actor_id, builtin.key
    FROM collections collection
    CROSS JOIN (VALUES
        ('my_tasks', 'My tasks', '{"filters": [{"field": "0192f5c0-0000-7000-8000-000000000002", "op": "is", "values": ["me"]}, {"field": "0192f5c0-0000-7000-8000-000000000001", "op": "categoryIsNot", "values": ["completed", "canceled"]}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000005", "direction": "asc"}, {"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}]}'::jsonb, 0),
        ('ready_for_agents', 'Ready for agents', '{"filters": [{"field": "0192f5c0-0000-7000-8000-00000000000b", "op": "is", "values": ["true"]}, {"field": "0192f5c0-0000-7000-8000-00000000000c", "op": "empty"}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}, {"field": "number", "direction": "desc"}]}'::jsonb, 1),
        ('blocked', 'Blocked', '{"filters": [{"field": "0192f5c0-0000-7000-8000-00000000000a", "op": "is", "values": ["true"]}, {"field": "0192f5c0-0000-7000-8000-000000000001", "op": "categoryIsNot", "values": ["completed", "canceled"]}], "sort": [{"field": "0192f5c0-0000-7000-8000-000000000003", "direction": "asc"}, {"field": "number", "direction": "desc"}]}'::jsonb, 2),
        ('recently_changed', 'Recently changed', '{"sort": [{"field": "updated", "direction": "desc"}]}'::jsonb, 3)
    ) AS builtin(key, name, config, place)
    WHERE collection.id = target_collection
    ON CONFLICT DO NOTHING
$$;

SELECT add_builtin_task_views(id) FROM collections WHERE builtin = 'tasks';

CREATE OR REPLACE FUNCTION ensure_tasks_collection(target_project UUID, actor UUID) RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    found_id UUID;
    candidate TEXT := 'Tasks';
    tries INTEGER := 1;
BEGIN
    SELECT id INTO found_id FROM collections
    WHERE project_id = target_project AND builtin = 'tasks';
    IF found_id IS NOT NULL THEN
        RETURN found_id;
    END IF;
    WHILE EXISTS (
        SELECT 1 FROM collections
        WHERE project_id = target_project AND archived_at IS NULL
          AND lower(name) = lower(candidate)
    ) LOOP
        tries := tries + 1;
        candidate := CASE WHEN tries = 2 THEN 'Tasks (built in)'
                          ELSE 'Tasks (built in ' || (tries - 1)::text || ')' END;
    END LOOP;
    found_id := gen_random_uuid();
    INSERT INTO collections
        (id, project_id, name, title_name, created_by_actor_id, builtin, acts_like_task)
    VALUES (found_id, target_project, candidate, 'Title', actor, 'tasks', true)
    ON CONFLICT (project_id, builtin) WHERE builtin IS NOT NULL DO NOTHING;
    IF NOT FOUND THEN
        -- Someone else made it first.
        SELECT id INTO found_id FROM collections
        WHERE project_id = target_project AND builtin = 'tasks';
        RETURN found_id;
    END IF;
    INSERT INTO collection_views
        (id, project_id, collection_id, name, view_type, created_by_actor_id)
    VALUES (gen_random_uuid(), target_project, found_id, 'All tasks', 'table', actor);
    PERFORM add_builtin_task_views(found_id);
    RETURN found_id;
END
$$;

-- Saved views people made keep their ids, owners and places; the built-in
-- ones are the Tasks collection's own now. A view whose name a shared view
-- already has is left out (none in the one live workspace).
INSERT INTO collection_views
    (id, project_id, collection_id, name, view_type, config, position, created_by_actor_id,
     owner_actor_id, created_at, updated_at)
SELECT saved.id, saved.project_id, collection.id, saved.name,
       CASE saved.layout WHEN 'board' THEN 'board' ELSE 'table' END,
       task_view_config(saved.query, saved.layout),
       1000 + saved.position, COALESCE(member.actor_id, collection.created_by_actor_id),
       saved.owner_actor_id, saved.created_at, saved.updated_at
FROM saved_views saved
JOIN collections collection ON collection.project_id = saved.project_id AND collection.builtin = 'tasks'
LEFT JOIN project_memberships member
  ON member.project_id = saved.project_id
 AND member.actor_id = COALESCE(saved.created_by_actor_id, saved.owner_actor_id)
WHERE saved.builtin IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO collection_view_pins (workspace_id, space_id, view_id, actor_id, pinned, position, updated_at)
SELECT project.organization_id, pin.project_id, COALESCE(builtin_view.id, pin.view_id),
       pin.actor_id, pin.pinned, pin.position, pin.updated_at
FROM saved_view_pins pin
JOIN saved_views saved ON saved.project_id = pin.project_id AND saved.id = pin.view_id
JOIN projects project ON project.id = pin.project_id
LEFT JOIN collections collection
  ON collection.project_id = saved.project_id AND collection.builtin = 'tasks'
LEFT JOIN collection_views builtin_view
  ON builtin_view.collection_id = collection.id AND builtin_view.builtin = saved.builtin
WHERE EXISTS (
    SELECT 1 FROM collection_views view
    WHERE view.project_id = pin.project_id AND view.id = COALESCE(builtin_view.id, pin.view_id)
)
ON CONFLICT DO NOTHING;

-- A rule whose conditions used a built-in saved view uses the Tasks
-- collection's own built-in view now; any other kept its id above.
UPDATE automation_rules rule
SET view_id = builtin_view.id
FROM saved_views saved
JOIN collections collection
  ON collection.project_id = saved.project_id AND collection.builtin = 'tasks'
JOIN collection_views builtin_view
  ON builtin_view.collection_id = collection.id AND builtin_view.builtin = saved.builtin
WHERE saved.project_id = rule.project_id AND saved.id = rule.view_id
  AND saved.builtin IS NOT NULL;

DROP TABLE saved_view_pins;
DROP TABLE saved_views;
DROP FUNCTION task_view_config(JSONB, TEXT);
DROP FUNCTION events_from_saved_views();

-- A view changed: a shared one is heard by its space, a personal one by
-- its owner only.
CREATE FUNCTION events_from_collection_views() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    changed collection_views%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN changed := OLD; ELSE changed := NEW; END IF;
    PERFORM stage_event(
        'view.changed', 'fact', changed.project_id, 'view', changed.id,
        CASE WHEN changed.owner_actor_id IS NULL THEN 'space' ELSE 'account' END,
        changed.created_by_actor_id, NULL, changed.owner_actor_id, NULL, changed.version, '{}',
        jsonb_build_object(
            'collectionId', changed.collection_id,
            'removed', TG_OP = 'DELETE'));
    RETURN NULL;
END $$;
CREATE TRIGGER events_from_collection_views
    AFTER INSERT OR DELETE OR UPDATE OF name, view_type, config, position, owner_actor_id
    ON collection_views FOR EACH ROW EXECUTE FUNCTION events_from_collection_views();
