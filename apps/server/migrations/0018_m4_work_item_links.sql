CREATE TABLE work_item_conversation_links (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (work_item_id, conversation_id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id)
);

CREATE INDEX work_item_conversation_links_by_conversation
    ON work_item_conversation_links (conversation_id, work_item_id);

CREATE OR REPLACE FUNCTION validate_work_item_thread_link()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_kind TEXT;
BEGIN
    SELECT kind INTO conversation_kind
      FROM conversations
     WHERE project_id = NEW.project_id AND id = NEW.conversation_id;
    IF conversation_kind IS DISTINCT FROM 'thread' THEN
        RAISE EXCEPTION 'Work Items may link only to Threads';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_conversation_links_validate_thread
BEFORE INSERT OR UPDATE ON work_item_conversation_links
FOR EACH ROW EXECUTE FUNCTION validate_work_item_thread_link();

CREATE TABLE work_item_repository_links (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('primary', 'related')),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (work_item_id, repository_id),
    FOREIGN KEY (project_id, work_item_id)
        REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id)
);

CREATE UNIQUE INDEX work_item_repository_links_one_primary
    ON work_item_repository_links (work_item_id)
    WHERE role = 'primary';

ALTER TABLE work_item_activity
    DROP CONSTRAINT work_item_activity_kind_check;

ALTER TABLE work_item_activity
    ADD CONSTRAINT work_item_activity_kind_check CHECK (kind IN (
        'work_item_created',
        'work_item_updated',
        'work_item_archived',
        'work_item_restored',
        'criterion_created',
        'criterion_updated',
        'criterion_completed',
        'criterion_reopened',
        'criterion_removed',
        'comment_created',
        'thread_linked',
        'thread_unlinked',
        'repository_linked',
        'repository_unlinked',
        'repository_primary_changed',
        'worker_invoked'
    ));
