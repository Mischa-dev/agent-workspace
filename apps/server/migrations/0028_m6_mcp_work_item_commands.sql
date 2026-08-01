-- M6 bounded Work Item command state used by local Codex.

ALTER TABLE work_items
    ADD COLUMN parent_work_item_id UUID,
    ADD COLUMN blocked_reason TEXT CHECK (
        blocked_reason IS NULL OR char_length(blocked_reason) BETWEEN 1 AND 2000
    ),
    ADD COLUMN blocked_at TIMESTAMPTZ,
    ADD COLUMN blocked_by_actor_id UUID,
    ADD CONSTRAINT work_items_parent_project_fk
        FOREIGN KEY (project_id, parent_work_item_id)
        REFERENCES work_items(project_id, id),
    ADD CONSTRAINT work_items_blocked_actor_fk
        FOREIGN KEY (project_id, blocked_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT work_items_not_own_parent CHECK (
        parent_work_item_id IS NULL OR parent_work_item_id <> id
    ),
    ADD CONSTRAINT work_items_blocked_shape_check CHECK (
        (blocked_reason IS NULL AND blocked_at IS NULL AND blocked_by_actor_id IS NULL)
        OR
        (blocked_reason IS NOT NULL AND blocked_at IS NOT NULL AND blocked_by_actor_id IS NOT NULL)
    );

CREATE INDEX work_items_parent
    ON work_items (project_id, parent_work_item_id, number)
    WHERE parent_work_item_id IS NOT NULL;
