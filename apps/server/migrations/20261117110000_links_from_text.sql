-- K3.13: links from text go straight into the link store.
--
-- Until now a link written in text went to `thing_references` through a
-- regular expression per route (`/projects/<id>/work-items/<id>`,
-- `/whiteboards/<id>`, ...), and a mirror copied it into `links`. Now the
-- text is read without knowing any route: each app path in it counts as
-- the last id in the path that names a thing in the registry, in the same
-- workspace as the text. So `/w/<slug>/...` addresses, sheets, collections
-- and any kind a module registers later count without a change here, and a
-- path to something in another workspace never does.
--
-- A Markdown link whose title is "embed" (`[label](/path "embed")`) is an
-- `embeds` link; every other one is `mentions`.
--
-- New sources: page comments and suggestions (the page holds them),
-- whiteboard text and links, sheet cells, and records' titles and text
-- fields. `thing_references` and its triggers go; the task chips kept by
-- `message_work_item_references` and `work_item_comment_references` still
-- mirror in (they come from the message's or comment's structure, not its
-- text), and a link held by both stays until neither holds it.

ALTER TABLE links DROP CONSTRAINT links_source_kind_check;
ALTER TABLE links ADD CONSTRAINT links_source_kind_check CHECK (source_kind IN (
    'message', 'page', 'task', 'task_comment', 'page_comment', 'page_suggestion',
    'whiteboard', 'sheet', 'record'
));

-- Every app path in a text, and each id in it in order: (path, place, id,
-- whether the path is an embed). At most 400 paths.
CREATE FUNCTION text_link_candidates(body TEXT)
RETURNS TABLE (path_no BIGINT, place BIGINT, id UUID, embed BOOLEAN)
LANGUAGE sql IMMUTABLE AS $$
    SELECT path.n, found.n, lower(found.m[1])::uuid, path.m[2] IS NOT NULL
    FROM (
        SELECT m, row_number() OVER () AS n
        FROM regexp_matches(
            COALESCE(body, ''),
            '(/[^\s()<>"''\[\]]*[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}[^\s()<>"''\[\]]*)(\s+"embed"\))?',
            'g'
        ) AS m
        LIMIT 400
    ) path
    CROSS JOIN LATERAL (
        SELECT m, row_number() OVER () AS n
        FROM regexp_matches(
            path.m[1],
            '([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})',
            'g'
        ) AS m
    ) found
$$;

-- The things a text links to: for each path, the last id in it that is a
-- registered thing; deleted things don't count. Each thing once, an embed
-- before a mention. At most 100.
CREATE FUNCTION text_link_targets(body TEXT)
RETURNS TABLE (target_id UUID, kind TEXT)
LANGUAGE sql STABLE AS $$
    SELECT target_id, kind FROM (
        SELECT DISTINCT ON (picked.id) picked.id AS target_id,
               CASE WHEN picked.embed THEN 'embeds' ELSE 'mentions' END AS kind
        FROM (
            SELECT DISTINCT ON (candidate.path_no) thing.id, candidate.embed, thing.deleted_at
            FROM text_link_candidates(body) candidate
            JOIN things thing ON thing.id = candidate.id
            ORDER BY candidate.path_no, candidate.place DESC
        ) picked
        WHERE picked.deleted_at IS NULL
        ORDER BY picked.id, picked.embed DESC
    ) targets
    LIMIT 100
$$;

-- Whether a task chip (not the text) still holds a text link.
CREATE FUNCTION links_chip_held(p_source_kind TEXT, p_source_id UUID, p_to UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT (p_source_kind = 'message' AND EXISTS (
                SELECT 1 FROM message_work_item_references reference
                WHERE reference.message_id = p_source_id AND reference.work_item_id = p_to))
        OR (p_source_kind = 'task_comment' AND EXISTS (
                SELECT 1 FROM work_item_comment_references reference
                WHERE reference.comment_id = p_source_id
                  AND reference.referenced_work_item_id = p_to))
$$;

-- Whether a message's or comment's text (not a chip) still links there.
CREATE FUNCTION links_text_holds(p_source_kind TEXT, p_source_id UUID, p_to UUID, p_kind TEXT)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE p_source_kind
        WHEN 'message' THEN EXISTS (
            SELECT 1 FROM messages message
            CROSS JOIN LATERAL text_link_targets(message.plain_text) found
            WHERE message.id = p_source_id AND message.removed_at IS NULL
              AND found.target_id = p_to AND found.kind = p_kind)
        WHEN 'task_comment' THEN EXISTS (
            SELECT 1 FROM work_item_comments comment
            CROSS JOIN LATERAL text_link_targets(comment.plain_text) found
            WHERE comment.id = p_source_id AND comment.removed_at IS NULL
              AND found.target_id = p_to AND found.kind = p_kind)
        ELSE false
    END
$$;

CREATE OR REPLACE FUNCTION links_still_held(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT, p_source_kind TEXT, p_source_id UUID
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN p_origin = 'text' THEN
            links_chip_held(p_source_kind, p_source_id, p_to)
            OR links_text_holds(p_source_kind, p_source_id, p_to, p_kind)
        WHEN p_origin = 'explicit' AND p_kind = 'about' THEN
            EXISTS (SELECT 1 FROM work_item_links link
                    WHERE link.work_item_id = p_from AND link.target_id = p_to
                      AND link.target_kind IN ('page', 'decision', 'conversation'))
            OR EXISTS (SELECT 1 FROM work_item_conversation_links link
                       WHERE link.work_item_id = p_from AND link.conversation_id = p_to)
        ELSE false
    END
$$;

-- Replace one source's text links: those the text no longer holds go
-- (unless a chip holds them), those it holds come. `p_body` NULL (a
-- removed message, an archived page) keeps none.
CREATE FUNCTION links_from_text(
    p_source_kind TEXT, p_source_id UUID, p_holder UUID, p_body TEXT, p_by UUID
) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM links link
    WHERE link.origin = 'text' AND link.source_kind = p_source_kind
      AND link.source_id = p_source_id
      AND NOT (link.kind = 'mentions'
               AND links_chip_held(p_source_kind, p_source_id, link.to_thing_id));
    IF p_holder IS NULL OR p_body IS NULL
       OR p_body !~ '/[^\s]*[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-' THEN
        RETURN;
    END IF;
    PERFORM links_put(p_holder, found.target_id, found.kind, 'text',
                      p_source_kind, p_source_id, NULL, p_by)
    FROM text_link_targets(p_body) found;
END $$;

-- A source row that goes takes its text links with it (one whose holder
-- is itself loses them with its thing).
CREATE FUNCTION links_text_forget() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM links
    WHERE origin = 'text' AND source_kind = TG_ARGV[0] AND source_id = OLD.id;
    RETURN NULL;
END $$;

-- The old route-by-route store goes.
DROP FUNCTION thing_references_from_message() CASCADE;
DROP FUNCTION thing_references_from_page() CASCADE;
DROP FUNCTION thing_references_from_task() CASCADE;
DROP FUNCTION thing_references_from_task_record() CASCADE;
DROP FUNCTION thing_references_from_task_comment() CASCADE;
DROP FUNCTION thing_references_replace(UUID, TEXT, UUID, TEXT, UUID, TEXT);
DROP FUNCTION thing_reference_targets(TEXT);
DROP TABLE thing_references CASCADE;

-- Messages: the message holds its links (the reader shows its
-- conversation). Trigger names sort after `things_sync`, so the holder's
-- own row is there first.
CREATE FUNCTION links_text_from_message() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('message', NEW.id, NEW.id,
        CASE WHEN NEW.removed_at IS NULL THEN NEW.plain_text END, NEW.author_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF plain_text, removed_at
    ON messages FOR EACH ROW EXECUTE FUNCTION links_text_from_message();

-- Pages: an archived page keeps none.
CREATE FUNCTION links_text_from_page() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('page', NEW.id, NEW.id,
        CASE WHEN NEW.archived_at IS NULL THEN NEW.body END, NEW.updated_by_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF body, archived_at
    ON project_pages FOR EACH ROW EXECUTE FUNCTION links_text_from_page();

-- Task descriptions: the description is the core's, the archive state the
-- record's, so both writes refresh them.
CREATE FUNCTION links_text_from_task(p_task UUID) RETURNS void LANGUAGE sql AS $$
    SELECT links_from_text('task', task.id, task.id,
        CASE WHEN task.archived_at IS NULL THEN task.description END, NULL)
    FROM task_rows task WHERE task.id = p_task
$$;
CREATE FUNCTION links_text_from_work_item() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_text_from_task(NEW.id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF description
    ON work_items FOR EACH ROW EXECUTE FUNCTION links_text_from_work_item();

-- Records: a task's record refreshes its task's description links when it
-- is archived or restored; any other record's title and text fields are
-- its own text. An archived record keeps none.
CREATE FUNCTION links_text_from_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM work_items task WHERE task.id = NEW.id) THEN
        IF TG_OP = 'UPDATE' AND OLD.archived_at IS DISTINCT FROM NEW.archived_at THEN
            PERFORM links_text_from_task(NEW.id);
        END IF;
        RETURN NULL;
    END IF;
    PERFORM links_from_text('record', NEW.id, NEW.id,
        CASE WHEN NEW.archived_at IS NULL
             THEN concat_ws(' ', NEW.title, NEW.field_values::text) END,
        NEW.updated_by_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF title, field_values, archived_at
    ON collection_records FOR EACH ROW EXECUTE FUNCTION links_text_from_record();

-- Task comments: the task holds them. A removed comment keeps none.
CREATE FUNCTION links_text_from_task_comment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('task_comment', NEW.id, NEW.work_item_id,
        CASE WHEN NEW.removed_at IS NULL THEN NEW.plain_text END, NEW.author_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF plain_text, removed_at
    ON work_item_comments FOR EACH ROW EXECUTE FUNCTION links_text_from_task_comment();
CREATE TRIGGER things_sync_text_links_delete AFTER DELETE ON work_item_comments
    FOR EACH ROW EXECUTE FUNCTION links_text_forget('task_comment');

-- Page comments: the page holds them. A deleted comment keeps none.
CREATE FUNCTION links_text_from_page_comment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('page_comment', NEW.id, NEW.page_id,
        CASE WHEN NEW.deleted_at IS NULL THEN NEW.body END, NEW.author_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF body, deleted_at
    ON project_page_comments FOR EACH ROW EXECUTE FUNCTION links_text_from_page_comment();
CREATE TRIGGER things_sync_text_links_delete AFTER DELETE ON project_page_comments
    FOR EACH ROW EXECUTE FUNCTION links_text_forget('page_comment');

-- Page suggestions: the page holds them while they wait for a decision
-- (an accepted one's text is the page's own by then).
CREATE FUNCTION links_text_from_page_suggestion() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('page_suggestion', NEW.id, NEW.page_id,
        CASE WHEN NEW.status = 'pending' THEN NEW.edits::text END, NEW.author_actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links AFTER INSERT OR UPDATE OF edits, status
    ON project_page_suggestions FOR EACH ROW EXECUTE FUNCTION links_text_from_page_suggestion();
CREATE TRIGGER things_sync_text_links_delete AFTER DELETE ON project_page_suggestions
    FOR EACH ROW EXECUTE FUNCTION links_text_forget('page_suggestion');

-- Whiteboards: the text and link of every element on the board, read
-- again for the boards whose changed elements carry an id either side.
CREATE FUNCTION links_text_from_boards(p_boards UUID[]) RETURNS void LANGUAGE sql AS $$
    SELECT links_from_text('whiteboard', board.id, board.id,
        (SELECT string_agg(concat_ws(' ', element.data->>'text', element.data->>'link'), ' ')
         FROM whiteboard_elements element
         WHERE element.whiteboard_id = board.id AND NOT element.is_deleted
           AND concat_ws(' ', element.data->>'text', element.data->>'link') ~ '[0-9a-fA-F]{8}-'),
        NULL)
    FROM whiteboards board WHERE board.id = ANY(p_boards)
$$;
CREATE FUNCTION links_text_board_inserted() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_text_from_boards(ARRAY(
        SELECT DISTINCT whiteboard_id FROM changed
        WHERE concat_ws(' ', data->>'text', data->>'link') ~ '[0-9a-fA-F]{8}-'));
    RETURN NULL;
END $$;
CREATE FUNCTION links_text_board_updated() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_text_from_boards(ARRAY(
        SELECT whiteboard_id FROM changed
        WHERE concat_ws(' ', data->>'text', data->>'link') ~ '[0-9a-fA-F]{8}-'
        UNION
        SELECT whiteboard_id FROM was
        WHERE concat_ws(' ', data->>'text', data->>'link') ~ '[0-9a-fA-F]{8}-'));
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links_insert AFTER INSERT ON whiteboard_elements
    REFERENCING NEW TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_board_inserted();
CREATE TRIGGER things_sync_text_links_update AFTER UPDATE ON whiteboard_elements
    REFERENCING OLD TABLE AS was NEW TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_board_updated();
CREATE TRIGGER things_sync_text_links_delete AFTER DELETE ON whiteboard_elements
    REFERENCING OLD TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_board_inserted();

-- Sheets: every cell's input, read again for the sheets whose changed
-- cells carry an id either side.
CREATE FUNCTION links_text_from_sheets(p_sheets UUID[]) RETURNS void LANGUAGE sql AS $$
    SELECT links_from_text('sheet', sheet.id, sheet.id,
        (SELECT string_agg(cell.input, ' ') FROM sheet_cells cell
         WHERE cell.sheet_id = sheet.id AND cell.input ~ '[0-9a-fA-F]{8}-'),
        NULL)
    FROM sheets sheet WHERE sheet.id = ANY(p_sheets)
$$;
CREATE FUNCTION links_text_cells_inserted() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_text_from_sheets(ARRAY(
        SELECT DISTINCT sheet_id FROM changed WHERE input ~ '[0-9a-fA-F]{8}-'));
    RETURN NULL;
END $$;
CREATE FUNCTION links_text_cells_updated() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_text_from_sheets(ARRAY(
        SELECT sheet_id FROM changed WHERE input ~ '[0-9a-fA-F]{8}-'
        UNION
        SELECT sheet_id FROM was WHERE input ~ '[0-9a-fA-F]{8}-'));
    RETURN NULL;
END $$;
CREATE TRIGGER things_sync_text_links_insert AFTER INSERT ON sheet_cells
    REFERENCING NEW TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_cells_inserted();
CREATE TRIGGER things_sync_text_links_update AFTER UPDATE ON sheet_cells
    REFERENCING OLD TABLE AS was NEW TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_cells_updated();
CREATE TRIGGER things_sync_text_links_delete AFTER DELETE ON sheet_cells
    REFERENCING OLD TABLE AS changed
    FOR EACH STATEMENT EXECUTE FUNCTION links_text_cells_inserted();

-- The copy no longer reads `thing_references`; text is read again.
CREATE OR REPLACE FUNCTION links_copy_all() RETURNS bigint LANGUAGE plpgsql AS $$
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
    PERFORM links_put_row(link_of(r)) FROM work_item_comment_references r;
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

-- Read every text again (the one-time conversion below, and the copy).
CREATE FUNCTION links_text_all() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_from_text('message', message.id, message.id, message.plain_text,
                            message.author_actor_id)
    FROM messages message
    WHERE message.removed_at IS NULL AND message.plain_text ~ '[0-9a-fA-F]{8}-';
    PERFORM links_from_text('page', page.id, page.id, page.body, page.updated_by_actor_id)
    FROM project_pages page
    WHERE page.archived_at IS NULL AND page.body ~ '[0-9a-fA-F]{8}-';
    PERFORM links_text_from_task(task.id)
    FROM task_rows task
    WHERE task.archived_at IS NULL AND task.description ~ '[0-9a-fA-F]{8}-';
    PERFORM links_from_text('record', record.id, record.id,
                            concat_ws(' ', record.title, record.field_values::text),
                            record.updated_by_actor_id)
    FROM collection_records record
    WHERE record.archived_at IS NULL
      AND NOT EXISTS (SELECT 1 FROM work_items task WHERE task.id = record.id)
      AND concat_ws(' ', record.title, record.field_values::text) ~ '[0-9a-fA-F]{8}-';
    PERFORM links_from_text('task_comment', comment.id, comment.work_item_id,
                            comment.plain_text, comment.author_actor_id)
    FROM work_item_comments comment
    WHERE comment.removed_at IS NULL AND comment.plain_text ~ '[0-9a-fA-F]{8}-';
    PERFORM links_from_text('page_comment', comment.id, comment.page_id, comment.body,
                            comment.author_actor_id)
    FROM project_page_comments comment
    WHERE comment.deleted_at IS NULL AND comment.body ~ '[0-9a-fA-F]{8}-';
    PERFORM links_from_text('page_suggestion', suggestion.id, suggestion.page_id,
                            suggestion.edits::text, suggestion.author_actor_id)
    FROM project_page_suggestions suggestion
    WHERE suggestion.status = 'pending' AND suggestion.edits::text ~ '[0-9a-fA-F]{8}-';
    PERFORM links_text_from_boards(ARRAY(
        SELECT DISTINCT element.whiteboard_id FROM whiteboard_elements element
        WHERE concat_ws(' ', element.data->>'text', element.data->>'link') ~ '[0-9a-fA-F]{8}-'));
    PERFORM links_text_from_sheets(ARRAY(
        SELECT DISTINCT cell.sheet_id FROM sheet_cells cell
        WHERE cell.input ~ '[0-9a-fA-F]{8}-'));
END $$;

-- Converted once: every text link is read again by the new rules. Links
-- the old store held under a route the registry doesn't know go.
DELETE FROM links link
WHERE link.origin = 'text'
  AND NOT (link.kind = 'mentions'
           AND links_chip_held(link.source_kind, link.source_id, link.to_thing_id));
SELECT links_text_all();
