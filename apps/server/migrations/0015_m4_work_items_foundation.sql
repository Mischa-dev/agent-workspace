-- M4 canonical Work Item, Project workflow, and activity foundation.

CREATE TABLE work_item_types (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    position INTEGER NOT NULL CHECK (position BETWEEN 1 AND 1000),
    is_default BOOLEAN NOT NULL DEFAULT false,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);

CREATE UNIQUE INDEX work_item_types_active_name_unique
    ON work_item_types (project_id, lower(name))
    WHERE enabled;
CREATE UNIQUE INDEX work_item_types_active_position_unique
    ON work_item_types (project_id, position)
    WHERE enabled;
CREATE UNIQUE INDEX work_item_types_one_default
    ON work_item_types (project_id)
    WHERE enabled AND is_default;

CREATE TABLE work_item_statuses (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    semantic_category TEXT NOT NULL CHECK (
        semantic_category IN (
            'backlog', 'unstarted', 'started', 'review', 'verification',
            'completed', 'canceled'
        )
    ),
    position INTEGER NOT NULL CHECK (position BETWEEN 1 AND 1000),
    is_default BOOLEAN NOT NULL DEFAULT false,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);

CREATE UNIQUE INDEX work_item_statuses_active_name_unique
    ON work_item_statuses (project_id, lower(name))
    WHERE enabled;
CREATE UNIQUE INDEX work_item_statuses_active_position_unique
    ON work_item_statuses (project_id, position)
    WHERE enabled;
CREATE UNIQUE INDEX work_item_statuses_one_default
    ON work_item_statuses (project_id)
    WHERE enabled AND is_default;

CREATE TABLE work_item_status_transitions (
    project_id UUID NOT NULL REFERENCES projects(id),
    from_status_id UUID NOT NULL,
    to_status_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, from_status_id, to_status_id),
    FOREIGN KEY (project_id, from_status_id)
        REFERENCES work_item_statuses(project_id, id),
    FOREIGN KEY (project_id, to_status_id)
        REFERENCES work_item_statuses(project_id, id),
    CHECK (from_status_id <> to_status_id)
);

CREATE TABLE work_items (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    number BIGINT NOT NULL CHECK (number > 0),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 240),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 100000),
    type_id UUID NOT NULL,
    status_id UUID NOT NULL,
    priority TEXT NOT NULL DEFAULT 'no_priority' CHECK (
        priority IN ('no_priority', 'low', 'medium', 'high', 'urgent')
    ),
    assignee_actor_id UUID,
    created_by_actor_id UUID NOT NULL,
    archived_at TIMESTAMPTZ,
    archived_by_actor_id UUID,
    board_rank BIGINT NOT NULL CHECK (board_rank > 0),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    UNIQUE (project_id, number),
    FOREIGN KEY (project_id, type_id)
        REFERENCES work_item_types(project_id, id),
    FOREIGN KEY (project_id, status_id)
        REFERENCES work_item_statuses(project_id, id),
    FOREIGN KEY (project_id, created_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, assignee_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        (archived_at IS NULL AND archived_by_actor_id IS NULL)
        OR
        (archived_at IS NOT NULL AND archived_by_actor_id IS NOT NULL)
    )
);

CREATE INDEX work_items_project_updated
    ON work_items (project_id, updated_at DESC, id DESC);
CREATE INDEX work_items_project_status_rank
    ON work_items (project_id, status_id, board_rank, id)
    WHERE archived_at IS NULL;
CREATE INDEX work_items_project_assignee
    ON work_items (project_id, assignee_actor_id, updated_at DESC)
    WHERE archived_at IS NULL AND assignee_actor_id IS NOT NULL;

CREATE TABLE work_item_activity (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN ('work_item_created', 'work_item_updated', 'work_item_archived',
                 'work_item_restored')
    ),
    summary TEXT NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 500),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object'
        AND octet_length(payload::text) <= 8192
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX work_item_activity_timeline
    ON work_item_activity (project_id, work_item_id, created_at, id);

CREATE OR REPLACE FUNCTION validate_work_item_definition_state()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    selected_type_enabled BOOLEAN;
    selected_status_enabled BOOLEAN;
BEGIN
    SELECT enabled INTO selected_type_enabled
      FROM work_item_types
     WHERE project_id = NEW.project_id AND id = NEW.type_id
     FOR KEY SHARE;
    SELECT enabled INTO selected_status_enabled
      FROM work_item_statuses
     WHERE project_id = NEW.project_id AND id = NEW.status_id
     FOR KEY SHARE;

    IF selected_type_enabled IS DISTINCT FROM true
       OR selected_status_enabled IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Work Item type and status must be active in its Project'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_items_validate_definitions
BEFORE INSERT OR UPDATE OF project_id, type_id, status_id
ON work_items
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_definition_state();

CREATE OR REPLACE FUNCTION validate_work_item_assignee()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    assignee_kind TEXT;
    assignee_active BOOLEAN;
BEGIN
    IF NEW.assignee_actor_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT a.kind, a.status = 'active' AND pm.enabled
      INTO assignee_kind, assignee_active
      FROM project_memberships pm
      JOIN actors a ON a.id = pm.actor_id
     WHERE pm.project_id = NEW.project_id
       AND pm.actor_id = NEW.assignee_actor_id
     FOR KEY SHARE OF pm, a;

    IF assignee_kind NOT IN ('human', 'external_agent')
       OR assignee_active IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Work Item assignee must be an active human or external-agent Project member'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_items_validate_assignee
BEFORE INSERT OR UPDATE OF project_id, assignee_actor_id
ON work_items
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_assignee();

CREATE OR REPLACE FUNCTION prevent_disabling_referenced_work_item_definition()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.enabled AND NOT NEW.enabled AND (
        (TG_TABLE_NAME = 'work_item_types' AND EXISTS (
            SELECT 1
              FROM work_items
             WHERE project_id = OLD.project_id
               AND type_id = OLD.id
               AND archived_at IS NULL
        ))
        OR
        (TG_TABLE_NAME = 'work_item_statuses' AND EXISTS (
            SELECT 1
              FROM work_items
             WHERE project_id = OLD.project_id
               AND status_id = OLD.id
               AND archived_at IS NULL
        ))
    ) THEN
        RAISE EXCEPTION 'an active Work Item definition cannot be disabled while referenced'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_types_prevent_referenced_disable
BEFORE UPDATE OF enabled
ON work_item_types
FOR EACH ROW
EXECUTE FUNCTION prevent_disabling_referenced_work_item_definition();

CREATE TRIGGER work_item_statuses_prevent_referenced_disable
BEFORE UPDATE OF enabled
ON work_item_statuses
FOR EACH ROW
EXECUTE FUNCTION prevent_disabling_referenced_work_item_definition();

ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item'
        )
    );
