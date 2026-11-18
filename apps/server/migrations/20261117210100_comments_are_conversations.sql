-- C1.4, C1.5 and C1.6 (Lane D2): task and page comments become messages in
-- their thing's comments conversation (C1.3), with the same ids, and the old
-- comment tables go in the same step (one user, one deploy: replace, don't
-- run side by side).
--
-- A task's comments are the stream of its comments conversation; a reply
-- keeps the comment it answers (`reply_to_message_id`). A page's comment
-- threads become threads under the page's comments conversation, each with
-- an id made from its first comment's and its anchor (`{"kind": "text", "blockId",
-- "quote"}`, or none for the whole page), Resolved or Open.
--
-- Revisions, mentions, task references, files and notifications move to
-- the message tables. The task timeline keeps its entries, which now name
-- the message; a trigger on messages adds the entry for each new task
-- comment and stages `task.commented` / `page.commented` as the old comment
-- triggers did.

-- ---------------------------------------------------------------------------
-- What a comment's references belong to: its thing.

ALTER TABLE thing_references DROP CONSTRAINT thing_references_thing_kind_check;
ALTER TABLE thing_references ADD CONSTRAINT thing_references_thing_kind_check
    CHECK (thing_kind IN ('conversation', 'page', 'task', 'sheet', 'whiteboard', 'record'));

-- A message in a thing's comments (or a thread under them) lists its
-- references as its thing's; any other message as its conversation's. The
-- message stays the source, so the link store's row is the message's.
CREATE OR REPLACE FUNCTION thing_references_from_message()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    holder_kind text := 'conversation';
    holder_id uuid := NEW.conversation_id;
    found_kind text;
    found_id uuid;
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM thing_references WHERE source_kind = 'message' AND source_id = OLD.id;
        RETURN OLD;
    END IF;
    SELECT COALESCE(parent.subject_kind, conversation.subject_kind),
           COALESCE(parent.subject_id, conversation.subject_id)
      INTO found_kind, found_id
      FROM conversations conversation
      LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
     WHERE conversation.id = NEW.conversation_id
       AND COALESCE(parent.kind, conversation.kind) = 'comments';
    IF found_kind IS NOT NULL THEN
        holder_kind := found_kind;
        holder_id := found_id;
    END IF;
    PERFORM thing_references_replace(
        NEW.project_id, 'message', NEW.id, holder_kind, holder_id,
        CASE WHEN NEW.removed_at IS NULL THEN NEW.plain_text END
    );
    RETURN NEW;
END;
$$;

-- ---------------------------------------------------------------------------
-- Task comments (C1.4).

INSERT INTO conversations (
    id, project_id, kind, name, visibility, subject_kind, subject_id, created_at, updated_at
)
SELECT md5('comments:task:' || item.id::text)::uuid, item.project_id, 'comments', 'Comments',
       'subject', 'task', item.id, min(comment.created_at), now()
FROM work_items item
JOIN work_item_comments comment ON comment.work_item_id = item.id
GROUP BY item.id, item.project_id
ON CONFLICT (subject_kind, subject_id) WHERE kind = 'comments' DO NOTHING;

CREATE TEMPORARY TABLE moved_comment (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL,
    created_at timestamptz NOT NULL
) ON COMMIT DROP;

INSERT INTO moved_comment (id, conversation_id, created_at)
SELECT comment.id, place.id, comment.created_at
FROM work_item_comments comment
JOIN conversations place
  ON place.kind = 'comments' AND place.subject_kind = 'task'
 AND place.subject_id = comment.work_item_id;

INSERT INTO messages (
    id, project_id, conversation_id, author_actor_id, content_version, content, plain_text,
    created_at, via_oauth_grant_id, revision, edited_at, removed_at, removed_by_actor_id,
    reply_to_message_id
)
SELECT comment.id, place.project_id, moved.conversation_id, comment.author_actor_id, 1,
       comment.content, comment.plain_text, comment.created_at, comment.via_oauth_grant_id,
       comment.revision, comment.edited_at, comment.removed_at, comment.removed_by_actor_id,
       comment.reply_to_comment_id
FROM work_item_comments comment
JOIN moved_comment moved ON moved.id = comment.id
JOIN conversations place ON place.id = moved.conversation_id
ORDER BY comment.created_at, comment.id;

-- The validators check what a new projection says against the message's
-- current text and the people's current memberships; history copied as it
-- was skips them.
ALTER TABLE message_revisions DISABLE TRIGGER message_revisions_validate;
ALTER TABLE message_mentions DISABLE TRIGGER message_mentions_validate;
ALTER TABLE message_work_item_references DISABLE TRIGGER message_work_item_references_validate;

INSERT INTO message_revisions (
    project_id, message_id, revision, content, plain_text, written_by_actor_id,
    via_oauth_grant_id, written_at, redacted_at, redacted_by_actor_id, created_at
)
SELECT message.project_id, revision.comment_id, revision.revision, revision.content,
       revision.plain_text, revision.written_by_actor_id, revision.via_oauth_grant_id,
       revision.written_at, revision.redacted_at, revision.redacted_by_actor_id,
       revision.created_at
FROM work_item_comment_revisions revision
JOIN messages message ON message.id = revision.comment_id;

INSERT INTO message_mentions (project_id, message_id, actor_id, created_at)
SELECT DISTINCT ON (mention.comment_id, mention.actor_id)
       message.project_id, mention.comment_id, mention.actor_id, mention.created_at
FROM work_item_comment_mentions mention
JOIN messages message ON message.id = mention.comment_id
ORDER BY mention.comment_id, mention.actor_id, mention.created_at;

INSERT INTO message_work_item_references (
    project_id, message_id, work_item_id, node_index, created_at
)
SELECT message.project_id, reference.comment_id, reference.referenced_work_item_id,
       reference.node_index, reference.created_at
FROM work_item_comment_references reference
JOIN messages message ON message.id = reference.comment_id
ON CONFLICT DO NOTHING;

-- A comment's files are its message's.
UPDATE files
SET conversation_id = moved.conversation_id, message_id = files.comment_id,
    work_item_id = NULL, comment_id = NULL
FROM moved_comment moved
WHERE files.comment_id = moved.id;

-- Notifications about a comment point at its message.
UPDATE notifications
SET message_id = notifications.comment_id, conversation_id = moved.conversation_id
FROM moved_comment moved
WHERE notifications.comment_id = moved.id;

-- The timeline's comment entries name the message (the same id).
ALTER TABLE work_item_timeline_entries
    DROP CONSTRAINT work_item_timeline_entries_project_id_work_item_id_comment_fkey;
ALTER TABLE work_item_timeline_entries RENAME COLUMN comment_id TO message_id;
ALTER TABLE work_item_timeline_entries
    ADD CONSTRAINT work_item_timeline_entries_message_fkey
    FOREIGN KEY (message_id) REFERENCES messages(id);
CREATE INDEX work_item_timeline_entries_message
    ON work_item_timeline_entries (message_id) WHERE message_id IS NOT NULL;

-- The old comments' own references and links go; the messages wrote theirs.
DELETE FROM thing_references WHERE source_kind = 'task_comment';
DELETE FROM links WHERE source_kind = 'task_comment';

-- ---------------------------------------------------------------------------
-- Page comments (C1.5).

INSERT INTO conversations (
    id, project_id, kind, name, visibility, subject_kind, subject_id, created_at, updated_at
)
SELECT md5('comments:page:' || page.id::text)::uuid, page.project_id, 'comments', 'Comments',
       'subject', 'page', page.id, min(comment.created_at), now()
FROM project_pages page
JOIN project_page_comments comment ON comment.page_id = page.id
GROUP BY page.id, page.project_id
ON CONFLICT (subject_kind, subject_id) WHERE kind = 'comments' DO NOTHING;

-- Each thread, named from its first comment's words as an auto-named
-- thread is. Its id comes from its first comment's (a message's id can't
-- also be its conversation's).
INSERT INTO conversations (
    id, project_id, kind, parent_channel_id, name, auto_named, visibility, anchor,
    thread_status, thread_status_changed_at, thread_status_changed_by_actor_id,
    created_at, updated_at
)
SELECT md5('comment-thread:' || root.id::text)::uuid, place.project_id, 'thread', place.id,
       COALESCE(NULLIF(left(btrim(regexp_replace(
           CASE WHEN root.deleted_at IS NULL THEN root.body ELSE '' END,
           '[[:space:][:cntrl:]]+', ' ', 'g')), 80), ''), 'New thread'),
       true, 'project',
       CASE WHEN root.block_id IS NULL THEN NULL
            ELSE jsonb_strip_nulls(jsonb_build_object(
                'kind', 'text', 'blockId', root.block_id, 'quote', root.quote))
       END,
       CASE WHEN root.resolved_at IS NULL THEN 'Open' ELSE 'Resolved' END,
       root.resolved_at, root.resolved_by_actor_id, root.created_at, now()
FROM project_page_comments root
JOIN conversations place
  ON place.kind = 'comments' AND place.subject_kind = 'page' AND place.subject_id = root.page_id
WHERE root.id = root.thread_id;

INSERT INTO moved_comment (id, conversation_id, created_at)
SELECT comment.id, md5('comment-thread:' || comment.thread_id::text)::uuid, comment.created_at
FROM project_page_comments comment;

INSERT INTO messages (
    id, project_id, conversation_id, author_actor_id, content_version, content, plain_text,
    created_at, via_oauth_grant_id, revision, edited_at, removed_at, removed_by_actor_id
)
SELECT comment.id, thread.project_id, thread.id, comment.author_actor_id, 1,
       CASE WHEN comment.deleted_at IS NULL
            THEN jsonb_build_object('version', 1, 'nodes',
                     jsonb_build_array(jsonb_build_object('type', 'text', 'text', comment.body)))
            ELSE '{"nodes": [], "version": 1}'::jsonb END,
       CASE WHEN comment.deleted_at IS NULL THEN comment.body ELSE '' END,
       comment.created_at, comment.via_oauth_grant_id,
       -- No earlier versions were kept; an edited comment says so.
       CASE WHEN comment.edited_at IS NULL THEN 1 ELSE 2 END,
       comment.edited_at, comment.deleted_at,
       CASE WHEN comment.deleted_at IS NOT NULL THEN comment.author_actor_id END
FROM project_page_comments comment
JOIN conversations thread
  ON thread.id = md5('comment-thread:' || comment.thread_id::text)::uuid
ORDER BY comment.created_at, comment.id;

-- Page comment notifications point at the comment's message.
UPDATE notifications
SET message_id = moved.id, conversation_id = moved.conversation_id
FROM moved_comment moved
WHERE notifications.kind = 'page_comment'
  AND notifications.detail->>'pageCommentId' = moved.id::text;

-- ---------------------------------------------------------------------------
-- Every moved comment takes its place in its conversation, oldest first,
-- after any entries the conversation already holds (comments posted to it
-- before this ran), and nobody finds old comments unread.

INSERT INTO conversation_entries (id, conversation_id, sequence, kind, message_id, created_at)
SELECT gen_random_uuid(), moved.conversation_id,
       COALESCE((SELECT max(held.sequence) FROM conversation_entries held
                 WHERE held.conversation_id = moved.conversation_id), 0)
       + row_number() OVER (PARTITION BY moved.conversation_id ORDER BY moved.created_at, moved.id),
       'message', moved.id, moved.created_at
FROM moved_comment moved;

UPDATE conversations
SET next_entry_sequence = placed.last + 1
FROM (
    SELECT conversation_id, max(sequence) AS last
    FROM conversation_entries
    WHERE conversation_id IN (SELECT conversation_id FROM moved_comment)
    GROUP BY conversation_id
) placed
WHERE conversations.id = placed.conversation_id;

INSERT INTO conversation_read_states (
    project_id, conversation_id, actor_id, highest_seen_sequence
)
SELECT place.project_id, place.id, membership.actor_id, place.next_entry_sequence - 1
FROM conversations place
JOIN project_memberships membership
  ON membership.project_id = place.project_id AND membership.enabled
WHERE place.id IN (SELECT conversation_id FROM moved_comment)
ON CONFLICT DO NOTHING;

ALTER TABLE message_revisions ENABLE TRIGGER message_revisions_validate;
ALTER TABLE message_mentions ENABLE TRIGGER message_mentions_validate;
ALTER TABLE message_work_item_references ENABLE TRIGGER message_work_item_references_validate;

-- ---------------------------------------------------------------------------
-- New comments: the task timeline's entry, and the facts the old comment
-- triggers staged. Ids only (rule 10).

CREATE FUNCTION task_timeline_from_comments()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    task_id uuid;
    next_sequence bigint;
BEGIN
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

CREATE TRIGGER task_timeline_from_comments
AFTER INSERT ON messages
FOR EACH ROW EXECUTE FUNCTION task_timeline_from_comments();

CREATE FUNCTION events_from_thing_comments()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    thing_kind text;
    thing_id uuid;
    thread_id uuid;
BEGIN
    SELECT COALESCE(parent.subject_kind, conversation.subject_kind),
           COALESCE(parent.subject_id, conversation.subject_id),
           CASE WHEN conversation.kind = 'thread' THEN conversation.id END
      INTO thing_kind, thing_id, thread_id
      FROM conversations conversation
      LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
     WHERE conversation.id = NEW.conversation_id
       AND COALESCE(parent.kind, conversation.kind) = 'comments';
    IF thing_kind IS NULL OR thing_kind NOT IN ('task', 'page') THEN
        RETURN NULL;
    END IF;
    PERFORM stage_event(
        thing_kind || CASE WHEN TG_OP = 'INSERT' THEN '.commented' ELSE '.comment_changed' END,
        'fact', NEW.project_id, thing_kind, thing_id, 'space',
        COALESCE(NEW.removed_by_actor_id, NEW.author_actor_id), NULL, NULL, NULL, NULL,
        CASE WHEN TG_OP = 'INSERT' THEN '{}'::text[]
             WHEN NEW.removed_at IS NOT NULL THEN '{removed}'::text[]
             ELSE '{content}'::text[] END,
        jsonb_strip_nulls(jsonb_build_object('commentId', NEW.id, 'threadId', thread_id)));
    RETURN NULL;
END;
$$;

CREATE TRIGGER events_from_thing_comments
AFTER INSERT OR UPDATE OF content, removed_at ON messages
FOR EACH ROW EXECUTE FUNCTION events_from_thing_comments();

-- Each inbox occurrence about a comment names its message, so removing the
-- comment takes it out of the inbox as removing any message does.
UPDATE notification_occurrences occurrence
SET message_id = message.id
FROM messages message
WHERE occurrence.message_id IS NULL
  AND occurrence.dedupe_key ~ '^(comment-mention|watch|page-comment):[0-9a-f-]{36}$'
  AND message.id = substring(occurrence.dedupe_key FROM ':(.*)$')::uuid;

-- ---------------------------------------------------------------------------
-- C1.6: the old comment tables, columns and functions go.

-- A file's events name its message, which a comment's file now has.
CREATE OR REPLACE FUNCTION events_from_files()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (OLD.removed_at IS NOT NULL OR NEW.removed_at IS NULL) THEN
        RETURN NULL;
    END IF;
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'file.attached' ELSE 'file.removed' END,
        'fact', NEW.project_id, 'file', NEW.id,
        CASE WHEN NEW.conversation_id IS NULL THEN 'space' ELSE 'conversation' END,
        COALESCE(NEW.removed_by_actor_id, NEW.uploaded_by_actor_id), NEW.conversation_id,
        NULL, COALESCE(NEW.conversation_id, NEW.work_item_id), NULL, '{}',
        jsonb_strip_nulls(jsonb_build_object('workItemId', NEW.work_item_id,
                                             'messageId', NEW.message_id)));
    RETURN NULL;
END $$;

ALTER TABLE files DROP COLUMN comment_id;
ALTER TABLE notifications DROP COLUMN comment_id;

DROP FUNCTION link_of(work_item_comment_references);

-- A message's text links from the message, but a comment's from its thing
-- (the task or page it's on), as the old comment tables did.
CREATE OR REPLACE FUNCTION link_of(r thing_references)
RETURNS link_of_row
LANGUAGE sql
STABLE
AS $$
    SELECT ROW(CASE WHEN r.source_kind = 'message' AND r.thing_kind = 'conversation'
                    THEN r.source_id ELSE r.thing_id END,
               r.target_id, 'mentions', 'text', r.source_kind, r.source_id,
               CASE r.source_kind
                   WHEN 'message' THEN (SELECT author_actor_id FROM messages WHERE id = r.source_id)
                   WHEN 'page' THEN
                       (SELECT updated_by_actor_id FROM project_pages WHERE id = r.source_id)
               END)::link_of_row
$$;

CREATE OR REPLACE FUNCTION link_of(r message_work_item_references)
RETURNS link_of_row
LANGUAGE sql
STABLE
AS $$
    SELECT ROW(COALESCE(
                   (SELECT COALESCE(parent.subject_id, place.subject_id)
                    FROM messages message
                    JOIN conversations place ON place.id = message.conversation_id
                    LEFT JOIN conversations parent ON parent.id = place.parent_channel_id
                    WHERE message.id = r.message_id
                      AND COALESCE(parent.kind, place.kind) = 'comments'),
                   r.message_id),
               r.work_item_id, 'mentions', 'text', 'message', r.message_id,
               (SELECT author_actor_id FROM messages WHERE id = r.message_id))::link_of_row
$$;

CREATE OR REPLACE FUNCTION links_mirror()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    old_link link_of_row;
    new_link link_of_row;
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        EXECUTE 'SELECT (link_of($1)).*' INTO old_link USING OLD;
    END IF;
    IF TG_OP IN ('UPDATE', 'INSERT') THEN
        EXECUTE 'SELECT (link_of($1)).*' INTO new_link USING NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND old_link IS NOT DISTINCT FROM new_link THEN
        RETURN NULL;
    END IF;
    -- (A row value with any NULL field is not "IS NOT NULL", so test the
    -- operation, not the value.)
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        PERFORM links_drop_row(old_link);
    END IF;
    IF TG_OP IN ('UPDATE', 'INSERT') THEN
        -- A message's, page's or task's references are written by a trigger
        -- that fires before the holder's own `things_sync` (names sort
        -- `thing_references_*` first), so bring the holder's row now.
        IF TG_TABLE_NAME = 'thing_references' THEN
            PERFORM things_upsert_from(
                CASE new_link.source_kind
                    WHEN 'message' THEN 'thing_source_messages'
                    WHEN 'page' THEN 'thing_source_project_pages'
                    ELSE 'thing_source_work_items'
                END,
                ARRAY[new_link.source_id]);
        END IF;
        PERFORM links_put_row(new_link);
    END IF;
    RETURN NULL;
END $$;

CREATE OR REPLACE FUNCTION links_still_held(
    p_from uuid, p_to uuid, p_kind text, p_origin text, p_source_kind text, p_source_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN p_origin = 'text' THEN
            EXISTS (SELECT 1 FROM thing_references reference
                    WHERE reference.source_kind = p_source_kind
                      AND reference.source_id = p_source_id
                      AND reference.target_id = p_to)
            OR (p_source_kind = 'message' AND EXISTS (
                    SELECT 1 FROM message_work_item_references reference
                    WHERE reference.message_id = p_source_id AND reference.work_item_id = p_to))
        WHEN p_origin = 'explicit' AND p_kind = 'about' THEN
            EXISTS (SELECT 1 FROM work_item_links link
                    WHERE link.work_item_id = p_from AND link.target_id = p_to
                      AND link.target_kind IN ('page', 'decision', 'conversation'))
            OR EXISTS (SELECT 1 FROM work_item_conversation_links link
                       WHERE link.work_item_id = p_from AND link.conversation_id = p_to)
        ELSE false
    END
$$;

CREATE OR REPLACE FUNCTION links_copy_all()
RETURNS bigint
LANGUAGE plpgsql
AS $$
DECLARE
    before bigint;
BEGIN
    SELECT count(*) INTO before FROM links;
    PERFORM links_put_row(link_of(r)) FROM work_item_dependencies r;
    PERFORM links_put_row(link_of(r)) FROM work_item_relations r;
    PERFORM links_put_row(link_of(r)) FROM work_item_links r;
    PERFORM links_put_row(link_of(r)) FROM work_item_conversation_links r;
    PERFORM links_put_row(link_of(r)) FROM whiteboard_links r;
    PERFORM links_put_row(link_of(r)) FROM message_work_item_references r;
    PERFORM links_put_row(link_of(r)) FROM thing_references r;
    PERFORM links_put_row(link_of(r)) FROM github_pull_request_work_item_links r;
    PERFORM links_put_row(link_of(r)) FROM github_pull_request_conversation_links r;
    PERFORM links_put_row(link_of(r)) FROM github_pull_request_workspace_links r;
    PERFORM links_put_row(link_of(r)) FROM launches r WHERE r.source_message_id IS NOT NULL;
    PERFORM links_put_row(link_of(r)) FROM conversations r WHERE r.origin_message_id IS NOT NULL;
    PERFORM links_put_row(link_of(r)) FROM project_decisions r
        WHERE r.source_message_id IS NOT NULL OR r.source_conversation_id IS NOT NULL;
    PERFORM links_put_row(link_of(r)) FROM whiteboards r WHERE r.duplicated_from_id IS NOT NULL;
    PERFORM links_sync_record_fields(r) FROM collection_records r
        WHERE r.field_values <> '{}'::jsonb
          AND cardinality(links_record_field_ids(r)) > 0;
    RETURN (SELECT count(*) FROM links) - before;
END $$;

CREATE OR REPLACE FUNCTION validate_work_item_reference_projection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    node JSONB;
BEGIN
    SELECT message.content->'nodes'->NEW.node_index
      INTO node
      FROM messages message
     WHERE message.id = NEW.message_id AND message.project_id = NEW.project_id
     FOR KEY SHARE;
    IF node IS NULL
       OR node->>'type' IS DISTINCT FROM 'work_item_ref'
       OR COALESCE(node->>'work_item_id', node->>'workItemId')
          IS DISTINCT FROM NEW.work_item_id::text THEN
        RAISE EXCEPTION 'Work Item reference projection must match its AST node'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

-- Comments' references and links again, now that both belong to the thing
-- the comment is on (the moved ones, and those posted since C1.3).
CREATE TEMP TABLE comment_message ON COMMIT DROP AS
SELECT message.id, message.project_id, message.plain_text, message.removed_at,
       COALESCE(parent.subject_kind, place.subject_kind) AS thing_kind,
       COALESCE(parent.subject_id, place.subject_id) AS thing_id
FROM messages message
JOIN conversations place ON place.id = message.conversation_id
LEFT JOIN conversations parent ON parent.id = place.parent_channel_id
WHERE COALESCE(parent.kind, place.kind) = 'comments';
DELETE FROM links
WHERE source_kind = 'message' AND source_id IN (SELECT id FROM comment_message);
SELECT thing_references_replace(project_id, 'message', id, thing_kind, thing_id,
                                CASE WHEN removed_at IS NULL THEN plain_text END)
FROM comment_message;
SELECT links_put_row(link_of(reference))
FROM message_work_item_references reference
WHERE reference.message_id IN (SELECT id FROM comment_message);

DROP TABLE work_item_comment_revisions;
DROP TABLE work_item_comment_mentions;
DROP TABLE work_item_comment_references;
DROP TABLE work_item_comments;
DROP TABLE project_page_comments;

DROP FUNCTION events_from_task_comments();
DROP FUNCTION events_from_page_comments();
DROP FUNCTION thing_references_from_task_comment();
DROP FUNCTION validate_work_item_comment_mention();
DROP FUNCTION validate_work_item_comment_revision();
DROP FUNCTION guard_work_item_comment_revision_mutation();
DROP FUNCTION guard_work_item_comment_mutation();
DROP FUNCTION guard_work_item_comment_projection_mutation();
DROP FUNCTION guard_work_item_comment_reply_to();

ALTER TABLE thing_references DROP CONSTRAINT thing_references_source_kind_check;
ALTER TABLE thing_references ADD CONSTRAINT thing_references_source_kind_check
    CHECK (source_kind IN ('message', 'page', 'task'));
ALTER TABLE links DROP CONSTRAINT links_source_kind_check;
ALTER TABLE links ADD CONSTRAINT links_source_kind_check
    CHECK (source_kind IN ('message', 'page', 'task'));
