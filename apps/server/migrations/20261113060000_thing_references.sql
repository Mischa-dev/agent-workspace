-- AWD-58: "Referenced in" backlinks.
--
-- A link to a task, page, whiteboard, thread, channel or conversation
-- written in a message, a page, a task's description or a task comment is
-- kept here, so the thing it points at can list where it's mentioned
-- (`modules::things::links`, read as "Referenced in"). Triggers keep it
-- current on every write path (the app, MCP tools, imports, automations):
-- each write of a source replaces that source's rows.
--
-- `thing_kind`/`thing_id` is what a reader opens: the conversation a
-- message is in, the page, or the task (for a comment, its task). Readers
-- still pass each one through the things resolver, so a reference from a
-- private conversation or a project the reader isn't in never shows.

CREATE TABLE thing_references (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL
        CHECK (source_kind IN ('message', 'page', 'task', 'task_comment')),
    source_id UUID NOT NULL,
    thing_kind TEXT NOT NULL CHECK (thing_kind IN ('conversation', 'page', 'task')),
    thing_id UUID NOT NULL,
    target_kind TEXT NOT NULL CHECK (
        target_kind IN ('task', 'page', 'whiteboard', 'thread', 'channel', 'conversation')
    ),
    target_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (source_kind, source_id, target_kind, target_id)
);

CREATE INDEX thing_references_target ON thing_references (target_id, target_kind);
CREATE INDEX thing_references_thing ON thing_references (thing_id, thing_kind);
CREATE INDEX thing_references_project ON thing_references (project_id);

-- The things a text links to, as (kind, id): app paths such as
-- /projects/{project}/work-items/{id} or /whiteboards/{id}, absolute or
-- relative. A thread in a DM or group opens at /messages/{parent}?thread={id}
-- and counts as that thread. At most 100 per text; the same thing once.
CREATE FUNCTION thing_reference_targets(body TEXT)
RETURNS TABLE (target_kind TEXT, target_id UUID)
LANGUAGE sql IMMUTABLE AS $$
    SELECT DISTINCT kind, id
    FROM (
        SELECT CASE
                 WHEN m[4] IS NOT NULL THEN 'thread'
                 ELSE CASE COALESCE(m[1], m[2])
                   WHEN 'work-items' THEN 'task'
                   WHEN 'pages' THEN 'page'
                   WHEN 'threads' THEN 'thread'
                   WHEN 'channels' THEN 'channel'
                   WHEN 'whiteboards' THEN 'whiteboard'
                   WHEN 'messages' THEN 'conversation'
                 END
               END AS kind,
               lower(COALESCE(m[4], m[3]))::uuid AS id
        FROM regexp_matches(
            COALESCE(body, ''),
            '(?:/projects/[0-9a-fA-F-]{36}/(work-items|pages|threads|channels)|/(whiteboards|messages))/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})(?:\?(?:[^\s#&?]*&)*thread=([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}))?',
            'g'
        ) AS m
        LIMIT 400
    ) found
    LIMIT 100
$$;

-- Replace one source's references.
CREATE FUNCTION thing_references_replace(
    p_project_id UUID,
    p_source_kind TEXT,
    p_source_id UUID,
    p_thing_kind TEXT,
    p_thing_id UUID,
    p_body TEXT
) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM thing_references
    WHERE source_kind = p_source_kind AND source_id = p_source_id;
    IF p_body IS NULL OR p_body = '' THEN
        RETURN;
    END IF;
    INSERT INTO thing_references
        (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
    SELECT p_project_id, p_source_kind, p_source_id, p_thing_kind, p_thing_id,
           found.target_kind, found.target_id
    FROM thing_reference_targets(p_body) found
    -- A thing never lists itself (a page linking to its own heading, a
    -- message in a thread linking to that thread).
    WHERE found.target_id <> p_thing_id
    ON CONFLICT DO NOTHING;
END;
$$;

-- Messages: the reader opens the conversation. A removed message keeps none.
CREATE FUNCTION thing_references_from_message() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references WHERE source_kind = 'message' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'message', NEW.id, 'conversation', NEW.conversation_id,
        CASE WHEN NEW.removed_at IS NULL THEN NEW.plain_text END
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER thing_references_from_message
AFTER INSERT OR UPDATE OF plain_text, removed_at ON messages
FOR EACH ROW EXECUTE FUNCTION thing_references_from_message();

CREATE TRIGGER thing_references_from_message_delete
AFTER DELETE ON messages
FOR EACH ROW EXECUTE FUNCTION thing_references_from_message();

-- Pages: an archived page keeps none.
CREATE FUNCTION thing_references_from_page() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references WHERE source_kind = 'page' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'page', NEW.id, 'page', NEW.id,
        CASE WHEN NEW.archived_at IS NULL THEN NEW.body END
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER thing_references_from_page
AFTER INSERT OR UPDATE OF body, archived_at ON project_pages
FOR EACH ROW EXECUTE FUNCTION thing_references_from_page();

CREATE TRIGGER thing_references_from_page_delete
AFTER DELETE ON project_pages
FOR EACH ROW EXECUTE FUNCTION thing_references_from_page();

-- Task descriptions: an archived task keeps none.
CREATE FUNCTION thing_references_from_task() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references WHERE source_kind = 'task' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'task', NEW.id, 'task', NEW.id,
        CASE WHEN NEW.archived_at IS NULL THEN NEW.description END
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER thing_references_from_task
AFTER INSERT OR UPDATE OF description, archived_at ON work_items
FOR EACH ROW EXECUTE FUNCTION thing_references_from_task();

CREATE TRIGGER thing_references_from_task_delete
AFTER DELETE ON work_items
FOR EACH ROW EXECUTE FUNCTION thing_references_from_task();

-- Task comments: the reader opens the task. A removed comment keeps none.
CREATE FUNCTION thing_references_from_task_comment() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references
        WHERE source_kind = 'task_comment' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'task_comment', NEW.id, 'task', NEW.work_item_id,
        CASE WHEN NEW.removed_at IS NULL THEN NEW.plain_text END
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER thing_references_from_task_comment
AFTER INSERT OR UPDATE OF plain_text, removed_at ON work_item_comments
FOR EACH ROW EXECUTE FUNCTION thing_references_from_task_comment();

CREATE TRIGGER thing_references_from_task_comment_delete
AFTER DELETE ON work_item_comments
FOR EACH ROW EXECUTE FUNCTION thing_references_from_task_comment();

-- What's already written.
INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT message.project_id, 'message', message.id, 'conversation', message.conversation_id,
       found.target_kind, found.target_id
FROM messages message
CROSS JOIN LATERAL thing_reference_targets(message.plain_text) found
WHERE message.removed_at IS NULL
  AND message.plain_text ~ '/(work-items|pages|threads|channels|whiteboards|messages)/'
  AND found.target_id <> message.conversation_id
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT page.project_id, 'page', page.id, 'page', page.id, found.target_kind, found.target_id
FROM project_pages page
CROSS JOIN LATERAL thing_reference_targets(page.body) found
WHERE page.archived_at IS NULL
  AND page.body ~ '/(work-items|pages|threads|channels|whiteboards|messages)/'
  AND found.target_id <> page.id
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT item.project_id, 'task', item.id, 'task', item.id, found.target_kind, found.target_id
FROM work_items item
CROSS JOIN LATERAL thing_reference_targets(item.description) found
WHERE item.archived_at IS NULL
  AND item.description ~ '/(work-items|pages|threads|channels|whiteboards|messages)/'
  AND found.target_id <> item.id
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT comment.project_id, 'task_comment', comment.id, 'task', comment.work_item_id,
       found.target_kind, found.target_id
FROM work_item_comments comment
CROSS JOIN LATERAL thing_reference_targets(comment.plain_text) found
WHERE comment.removed_at IS NULL
  AND comment.plain_text ~ '/(work-items|pages|threads|channels|whiteboards|messages)/'
  AND found.target_id <> comment.work_item_id
ON CONFLICT DO NOTHING;
