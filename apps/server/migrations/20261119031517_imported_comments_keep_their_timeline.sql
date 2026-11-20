-- An imported task comment already has its timeline entry, and its task
-- its next sequence, in the archive: the trigger that numbers a new
-- comment's entry steps aside inside an import (`importing()`), as the
-- checks do.

CREATE OR REPLACE FUNCTION task_timeline_from_comments()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    task_id uuid;
    next_sequence bigint;
BEGIN
    IF importing() THEN
        RETURN NULL;
    END IF;
    SELECT conversation.subject_id INTO task_id
      FROM conversations conversation
     WHERE conversation.id = NEW.conversation_id
       AND conversation.kind = 'comments' AND conversation.subject_kind = 'task';
    IF task_id IS NULL THEN
        RETURN NULL;
    END IF;
    UPDATE work_items
    SET next_timeline_sequence = next_timeline_sequence + 1
    WHERE project_id = NEW.project_id AND id = task_id
    RETURNING next_timeline_sequence - 1 INTO next_sequence;
    IF next_sequence IS NOT NULL THEN
        INSERT INTO work_item_timeline_entries (project_id, work_item_id, sequence, kind, message_id)
        VALUES (NEW.project_id, task_id, next_sequence, 'comment', NEW.id);
    END IF;
    RETURN NULL;
END;
$$;
