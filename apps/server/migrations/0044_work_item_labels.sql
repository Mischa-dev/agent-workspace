-- Project labels (docs/plans/mcp-tools.md, Phase 2).
--
-- A label belongs to one project. Names are unique among a project's active
-- labels, ignoring case. Archiving a label hides it from every task until it's
-- restored; the rows that put it on tasks stay, so nothing is lost. Nothing
-- is ever deleted from either table.
--
-- Every write to these tables runs after the writer has locked the project
-- row (work_item_commands::lock_membership), the same order every task write
-- takes. That keeps the count limits and name uniqueness exact.

CREATE TABLE project_labels (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 50
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
        AND name !~ '\s\s'
    ),
    color TEXT NOT NULL CHECK (color ~ '^#[0-9a-f]{6}$'),
    description TEXT NOT NULL DEFAULT '' CHECK (
        char_length(description) <= 200 AND description !~ '[[:cntrl:]]'
    ),
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

CREATE UNIQUE INDEX project_labels_active_name_unique
    ON project_labels (project_id, lower(name))
    WHERE archived_at IS NULL;

CREATE TABLE work_item_labels (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    label_id UUID NOT NULL,
    added_by_actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, work_item_id, label_id),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, label_id) REFERENCES project_labels(project_id, id),
    FOREIGN KEY (project_id, added_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX work_item_labels_by_label
    ON work_item_labels (project_id, label_id, work_item_id);

-- Every read of a task's labels goes through this view, so an archived label
-- never shows up by accident.
CREATE VIEW work_item_active_labels AS
SELECT applied.project_id, applied.work_item_id, label.id AS label_id,
       label.name, label.color
FROM work_item_labels applied
JOIN project_labels label
  ON label.project_id = applied.project_id AND label.id = applied.label_id
WHERE label.archived_at IS NULL;

-- Backstop for every code path: only an active label can be put on a task.
CREATE FUNCTION validate_work_item_label_active()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM 1 FROM project_labels
     WHERE project_id = NEW.project_id AND id = NEW.label_id AND archived_at IS NULL
     FOR SHARE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'an archived label cannot be added to a Work Item'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_labels_validate_active
BEFORE INSERT ON work_item_labels
FOR EACH ROW EXECUTE FUNCTION validate_work_item_label_active();
