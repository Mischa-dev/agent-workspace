-- K3.23: the old link stores go.
--
-- A task's links to pages, decisions, conversations and messages
-- (`work_item_links`), its threads (`work_item_conversation_links`) and a
-- board's links (`whiteboard_links`) were mirrored into `links` (K3.12) and
-- copied once (#291's conversion). Their writers now write the link store
-- (`links::put_explicit`, `drop_explicit`), so the three tables and their
-- mirror triggers go. Readers keep their shape through three views over
-- `links` and the registry:
--   task_links    task -about-> page, decision or conversation;
--                 task -made_from-> message
--   task_threads  the task_links to a thread in the task's own space
--   board_links   board -about-> project, task or page
-- Like the tables, the views keep a link whose other end is archived or
-- removed; readers join the other end as they did.
--
-- What stays mirrored, as the plan has it: the pull request link tables
-- (they carry where the link came from), a message's task chips
-- (`message_work_item_references`, in the order the message shows them),
-- a conversation's tabs (their order), and the "made from" columns, each a
-- kind's own facts.

-- Anything a mirror missed comes over first.
SELECT links_put_row(link_of(r)) FROM work_item_links r;
SELECT links_put_row(link_of(r)) FROM work_item_conversation_links r;
SELECT links_put_row(link_of(r)) FROM whiteboard_links r;

-- And each keeps when it was first made: the mirrors and the copies stamped
-- the time they ran, and a launch reuses the thread linked to its task
-- most recently.
UPDATE links link SET created_at = made.created_at
FROM (
    SELECT (old.link).from_id, (old.link).to_id, (old.link).kind,
           min(old.created_at) AS created_at
    FROM (
        SELECT link_of(r) AS link, r.created_at FROM work_item_links r
        UNION ALL
        SELECT link_of(r), r.created_at FROM work_item_conversation_links r
        UNION ALL
        SELECT link_of(r), r.created_at FROM whiteboard_links r
    ) old
    GROUP BY 1, 2, 3
) made
WHERE link.origin = 'explicit'
  AND link.from_thing_id = made.from_id
  AND link.to_thing_id = made.to_id
  AND link.kind = made.kind
  AND link.created_at <> made.created_at;

-- An explicit link has one store now, so nothing else can still hold it.
CREATE OR REPLACE FUNCTION links_still_held(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT, p_source_kind TEXT, p_source_id UUID
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN p_origin = 'text' THEN
            links_chip_held(p_source_kind, p_source_id, p_to)
            OR links_text_holds(p_source_kind, p_source_id, p_to, p_kind)
        ELSE false
    END
$$;

-- The copy no longer reads the three tables.
CREATE OR REPLACE FUNCTION links_copy_all() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    before bigint;
BEGIN
    SELECT count(*) INTO before FROM links;
    PERFORM links_put_row(link_of(r)) FROM message_work_item_references r;
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
    PERFORM links_text_all();
    RETURN (SELECT count(*) FROM links) - before;
END $$;

-- The tables, their mirror triggers and their `link_of` functions.
DROP TABLE work_item_links CASCADE;
DROP TABLE work_item_conversation_links CASCADE;
DROP TABLE whiteboard_links CASCADE;

CREATE VIEW task_links WITH (security_invoker = true) AS
SELECT link.id,
       link.from_thing_id AS work_item_id,
       task.space_id AS project_id,
       CASE WHEN other.kind IN ('channel', 'conversation', 'thread') THEN 'conversation'
            ELSE other.kind::text END AS target_kind,
       link.to_thing_id AS target_id,
       other.space_id AS target_project_id,
       link.created_by_account_id AS created_by_actor_id,
       link.created_at
FROM links link
JOIN things task ON task.id = link.from_thing_id AND task.kind = 'task'
JOIN things other ON other.id = link.to_thing_id
WHERE link.origin = 'explicit'
  AND ((link.kind = 'about'
        AND other.kind IN ('page', 'decision', 'channel', 'conversation', 'thread'))
       OR (link.kind = 'made_from' AND other.kind = 'message'));

CREATE VIEW task_threads WITH (security_invoker = true) AS
SELECT link.id,
       link.work_item_id,
       link.project_id,
       link.target_id AS conversation_id,
       link.created_by_actor_id,
       link.created_at
FROM task_links link
JOIN things thread ON thread.id = link.target_id AND thread.kind = 'thread'
WHERE link.target_project_id = link.project_id;

CREATE VIEW board_links WITH (security_invoker = true) AS
SELECT link.id,
       link.from_thing_id AS whiteboard_id,
       CASE other.kind WHEN 'task' THEN 'work_item' ELSE other.kind::text END AS target_kind,
       link.to_thing_id AS target_id,
       other.space_id AS target_project_id,
       link.created_by_account_id AS created_by_actor_id,
       link.created_at
FROM links link
JOIN things board ON board.id = link.from_thing_id AND board.kind = 'whiteboard'
JOIN things other ON other.id = link.to_thing_id AND other.kind IN ('project', 'task', 'page')
WHERE link.kind = 'about' AND link.origin = 'explicit';
