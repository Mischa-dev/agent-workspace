-- K3.12: the link store, copied and mirrored (K3 3.2, 4.2 steps 1 and 2).
--
-- One table for every link between two things. Today's stores keep working
-- unchanged: a trigger on each mirrors its inserts and deletes into `links`,
-- and this migration copies what they already hold. Nothing reads `links`
-- yet (K3.14 switches the reader; K3.15 the writers).
--
-- A link needs both ends in `things`. Rows whose ends aren't there yet (the
-- registry backfill hasn't reached them) are skipped here and copied by the
-- `links.backfill` job, which runs once the registry backfill is done.
--
-- What each store becomes (`from` is the store's own row, as written):
--   work_item_dependencies          blocking -blocks-> blocked          explicit
--   work_item_relations             task -relates_to|duplicates-> other explicit
--   work_item_links                 task -about-> page/decision/conversation,
--                                   task -made_from-> message           explicit
--   work_item_conversation_links    task -about-> thread                explicit
--   whiteboard_links                board -about-> project/task/page    explicit
--   message_work_item_references    message -mentions-> task            text
--   work_item_comment_references    task -mentions-> task (the comment) text
--   thing_references                holder -mentions-> target           text
--   github_pull_request_*_links     pull request -about-> task/conversation/run
--                                                                       derived
--   made-from columns               thread, decision, run, board -made_from->
--                                   message, conversation or board      derived
--   link fields (records, tasks)    record/task -relates_to-> value     field
-- Embeds (`[label](/path "embed")` in a page) arrive through
-- thing_references as `mentions` until K3.13 reads them as `embeds`.

CREATE TABLE links (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    from_thing_id UUID NOT NULL REFERENCES things(id) ON DELETE CASCADE,
    to_thing_id UUID NOT NULL REFERENCES things(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN (
        'blocks', 'about', 'made_from', 'relates_to', 'duplicates', 'mentions', 'embeds'
    )),
    -- 'explicit': someone linked them. 'text': found in the text of
    -- source_*. 'field': a link field's value. 'derived': mirrored from a
    -- kind's own column (made from, pull request links).
    origin TEXT NOT NULL CHECK (origin IN ('explicit', 'text', 'field', 'derived')),
    -- For 'text': the message, page, task description or task comment
    -- holding the link. Not a foreign key: task comments aren't things.
    source_kind TEXT CHECK (source_kind IN ('message', 'page', 'task', 'task_comment')),
    source_id UUID,
    -- For 'field': the field definition.
    field_id UUID,
    created_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    -- K1's account_credentials(id) once it lands; none of today's stores
    -- records it.
    via_credential_id UUID,
    requested_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (from_thing_id <> to_thing_id),
    CHECK ((origin = 'text') = (source_id IS NOT NULL)),
    CHECK ((origin = 'text') = (source_kind IS NOT NULL)),
    CHECK ((origin = 'field') = (field_id IS NOT NULL))
);
-- One explicit or derived link of a kind between two things.
CREATE UNIQUE INDEX links_explicit_unique ON links (from_thing_id, kind, to_thing_id)
    WHERE origin IN ('explicit', 'derived');
-- Symmetric kinds once per pair, whichever way it was made.
CREATE UNIQUE INDEX links_symmetric_unique
    ON links (kind, LEAST(from_thing_id, to_thing_id), GREATEST(from_thing_id, to_thing_id))
    WHERE kind = 'relates_to' AND origin = 'explicit';
CREATE UNIQUE INDEX links_one_original ON links (from_thing_id) WHERE kind = 'duplicates';
-- One row per text source and target, one per field value.
CREATE UNIQUE INDEX links_text_unique ON links (source_kind, source_id, kind, to_thing_id)
    WHERE origin = 'text';
CREATE UNIQUE INDEX links_field_unique ON links (from_thing_id, field_id, to_thing_id)
    WHERE origin = 'field';
CREATE INDEX links_from ON links (from_thing_id, kind);
CREATE INDEX links_to ON links (to_thing_id, kind);
CREATE INDEX links_source ON links (source_kind, source_id) WHERE source_id IS NOT NULL;
CREATE INDEX links_workspace ON links (workspace_id);

-- Add one link if both ends are things in one workspace; otherwise nothing
-- (the backfill job brings it once the ends exist).
CREATE FUNCTION links_put(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT,
    p_source_kind TEXT, p_source_id UUID, p_field_id UUID, p_by UUID
) RETURNS void LANGUAGE sql AS $$
    INSERT INTO links (workspace_id, from_thing_id, to_thing_id, kind, origin,
                       source_kind, source_id, field_id, created_by_account_id)
    SELECT source.workspace_id, source.id, target.id, p_kind, p_origin,
           p_source_kind, p_source_id, p_field_id,
           (SELECT id FROM actors WHERE id = p_by)
    FROM things source
    JOIN things target ON target.id = p_to AND target.workspace_id = source.workspace_id
    WHERE source.id = p_from AND p_from <> p_to
    ON CONFLICT DO NOTHING
$$;

-- Whether another of today's stores still holds the same link: two stores
-- can say one thing (a task chip and its URL in one message; a task's
-- thread in work_item_links and work_item_conversation_links).
CREATE FUNCTION links_still_held(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT, p_source_kind TEXT, p_source_id UUID
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN p_origin = 'text' THEN
            EXISTS (SELECT 1 FROM thing_references reference
                    WHERE reference.source_kind = p_source_kind
                      AND reference.source_id = p_source_id
                      AND reference.target_id = p_to)
            OR (p_source_kind = 'message' AND EXISTS (
                    SELECT 1 FROM message_work_item_references reference
                    WHERE reference.message_id = p_source_id AND reference.work_item_id = p_to))
            OR (p_source_kind = 'task_comment' AND EXISTS (
                    SELECT 1 FROM work_item_comment_references reference
                    WHERE reference.comment_id = p_source_id
                      AND reference.referenced_work_item_id = p_to))
        WHEN p_origin = 'explicit' AND p_kind = 'about' THEN
            EXISTS (SELECT 1 FROM work_item_links link
                    WHERE link.work_item_id = p_from AND link.target_id = p_to
                      AND link.target_kind IN ('page', 'decision', 'conversation'))
            OR EXISTS (SELECT 1 FROM work_item_conversation_links link
                       WHERE link.work_item_id = p_from AND link.conversation_id = p_to)
        ELSE false
    END
$$;

-- Take one link away, unless another store still holds it.
CREATE FUNCTION links_drop(
    p_from UUID, p_to UUID, p_kind TEXT, p_origin TEXT, p_source_kind TEXT, p_source_id UUID
) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF links_still_held(p_from, p_to, p_kind, p_origin, p_source_kind, p_source_id) THEN
        RETURN;
    END IF;
    DELETE FROM links
    WHERE from_thing_id = p_from AND to_thing_id = p_to AND kind = p_kind
      AND origin = p_origin
      AND source_kind IS NOT DISTINCT FROM p_source_kind
      AND source_id IS NOT DISTINCT FROM p_source_id;
END $$;

-- The link one row of a store stands for. One function per store, used by
-- both the mirror trigger and the copy, so the two can't disagree.
CREATE TYPE link_of_row AS (
    from_id UUID, to_id UUID, kind TEXT, origin TEXT,
    source_kind TEXT, source_id UUID, created_by UUID
);

CREATE FUNCTION link_of(r work_item_dependencies) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.blocking_work_item_id, r.blocked_work_item_id, 'blocks', 'explicit',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r work_item_relations) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.work_item_id, r.other_work_item_id, r.kind, 'explicit',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r work_item_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.work_item_id, r.target_id,
               CASE WHEN r.target_kind = 'message' THEN 'made_from' ELSE 'about' END,
               'explicit', NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r work_item_conversation_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.work_item_id, r.conversation_id, 'about', 'explicit',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r whiteboard_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.whiteboard_id, r.target_id, 'about', 'explicit',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r message_work_item_references) RETURNS link_of_row
LANGUAGE sql STABLE AS $$
    SELECT ROW(r.message_id, r.work_item_id, 'mentions', 'text', 'message', r.message_id,
               (SELECT author_actor_id FROM messages WHERE id = r.message_id))::link_of_row
$$;

CREATE FUNCTION link_of(r work_item_comment_references) RETURNS link_of_row
LANGUAGE sql STABLE AS $$
    SELECT ROW(r.source_work_item_id, r.referenced_work_item_id, 'mentions', 'text',
               'task_comment', r.comment_id,
               (SELECT author_actor_id FROM work_item_comments WHERE id = r.comment_id))::link_of_row
$$;

-- The holder of the text: the message itself (a thing since K3.11), the
-- page, or the task (for a task comment, its task).
CREATE FUNCTION link_of(r thing_references) RETURNS link_of_row
LANGUAGE sql STABLE AS $$
    SELECT ROW(CASE WHEN r.source_kind = 'message' THEN r.source_id ELSE r.thing_id END,
               r.target_id, 'mentions', 'text', r.source_kind, r.source_id,
               CASE r.source_kind
                   WHEN 'message' THEN (SELECT author_actor_id FROM messages WHERE id = r.source_id)
                   WHEN 'task_comment' THEN
                       (SELECT author_actor_id FROM work_item_comments WHERE id = r.source_id)
                   WHEN 'page' THEN
                       (SELECT updated_by_actor_id FROM project_pages WHERE id = r.source_id)
               END)::link_of_row
$$;

CREATE FUNCTION link_of(r github_pull_request_work_item_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.github_pull_request_id, r.work_item_id, 'about', 'derived',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r github_pull_request_conversation_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.github_pull_request_id, r.conversation_id, 'about', 'derived',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

-- The run that worked on the pull request (a run's id is its agent_runs id).
CREATE FUNCTION link_of(r github_pull_request_workspace_links) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.github_pull_request_id, r.agent_run_id, 'about', 'derived',
               NULL, NULL, NULL)::link_of_row
$$;

-- Made from: a thread from a message, a decision from a message (or, with
-- none, its conversation), a run from a message, a board from a board.
CREATE FUNCTION link_of(r conversations) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.id, r.origin_message_id, 'made_from', 'derived', NULL, NULL, NULL)::link_of_row
$$;

CREATE FUNCTION link_of(r project_decisions) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.id, COALESCE(r.source_message_id, r.source_conversation_id), 'made_from',
               'derived', NULL, NULL, r.decided_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r launches) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(COALESCE(r.run_id, r.agent_run_id), r.source_message_id, 'made_from',
               'derived', NULL, NULL, r.requested_by_actor_id)::link_of_row
$$;

CREATE FUNCTION link_of(r whiteboards) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.id, r.duplicated_from_id, 'made_from', 'derived',
               NULL, NULL, r.created_by_actor_id)::link_of_row
$$;

CREATE FUNCTION links_put_row(link link_of_row) RETURNS void LANGUAGE sql AS $$
    SELECT links_put(link.from_id, link.to_id, link.kind, link.origin,
                     link.source_kind, link.source_id, NULL, link.created_by)
    WHERE link.from_id IS NOT NULL AND link.to_id IS NOT NULL
$$;

CREATE FUNCTION links_drop_row(link link_of_row) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF link.from_id IS NOT NULL AND link.to_id IS NOT NULL THEN
        PERFORM links_drop(link.from_id, link.to_id, link.kind, link.origin,
                           link.source_kind, link.source_id);
    END IF;
END $$;

-- The mirror: the old row's link goes, the new row's comes. An update that
-- doesn't change the link leaves it alone.
CREATE FUNCTION links_mirror() RETURNS trigger LANGUAGE plpgsql AS $$
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
            IF new_link.source_kind <> 'task_comment' THEN
                PERFORM things_upsert_from(
                    CASE new_link.source_kind
                        WHEN 'message' THEN 'thing_source_messages'
                        WHEN 'page' THEN 'thing_source_project_pages'
                        ELSE 'thing_source_work_items'
                    END,
                    ARRAY[new_link.source_id]);
            END IF;
        END IF;
        PERFORM links_put_row(new_link);
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON work_item_dependencies
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON work_item_relations
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON work_item_links
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON work_item_conversation_links
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON whiteboard_links
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON message_work_item_references
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON work_item_comment_references
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE ON thing_references
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE
    ON github_pull_request_work_item_links FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE
    ON github_pull_request_conversation_links FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OR DELETE
    ON github_pull_request_workspace_links FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OF run_id, agent_run_id, source_message_id
    OR DELETE ON launches FOR EACH ROW EXECUTE FUNCTION links_mirror();
-- On kinds' own tables the name sorts after `things_sync`, so the thing's
-- own row is there first (PostgreSQL fires triggers in name order).
CREATE TRIGGER things_sync_links AFTER INSERT OR UPDATE OF origin_message_id OR DELETE
    ON conversations FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER things_sync_links
    AFTER INSERT OR UPDATE OF source_message_id, source_conversation_id OR DELETE
    ON project_decisions FOR EACH ROW EXECUTE FUNCTION links_mirror();
CREATE TRIGGER things_sync_links AFTER INSERT OR UPDATE OF duplicated_from_id OR DELETE
    ON whiteboards FOR EACH ROW EXECUTE FUNCTION links_mirror();

-- Link fields: a value is one id. The links of one thing's fields in one
-- scope (a collection's fields, or a project's task fields) are replaced
-- whenever its values change. The kind is relates_to until a field's
-- config names another (K3.14).
CREATE FUNCTION links_sync_fields(
    p_from UUID, p_values JSONB, p_by UUID, p_definitions UUID[]
) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM links
    WHERE from_thing_id = p_from AND origin = 'field' AND field_id = ANY(p_definitions);
    PERFORM links_put(p_from, value.target, 'relates_to', 'field', NULL, NULL,
                      value.field_id, p_by)
    FROM (
        SELECT definition.id AS field_id,
               (p_values->>definition.id::text)::uuid AS target
        FROM field_definitions definition
        WHERE definition.id = ANY(p_definitions)
          AND definition.field_type = 'link' AND definition.archived_at IS NULL
          AND p_values->>definition.id::text
              ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
    ) value;
END $$;

-- A task's field values are its record's (T1.3), so a record of a
-- collection that acts like a task also has the project's task fields.
CREATE FUNCTION links_record_field_ids(r collection_records) RETURNS UUID[]
LANGUAGE sql STABLE AS $$
    SELECT ARRAY(
        SELECT definition.id FROM field_definitions definition
        WHERE definition.project_id = r.project_id AND definition.field_type = 'link'
          AND (definition.collection_id = r.collection_id
               OR (definition.record_kind = 'task' AND definition.collection_id IS NULL
                   AND EXISTS (SELECT 1 FROM collections collection
                               WHERE collection.id = r.collection_id
                                 AND collection.acts_like_task))))
$$;

CREATE FUNCTION links_sync_record_fields(r collection_records) RETURNS void
LANGUAGE sql AS $$
    SELECT links_sync_fields(r.id, r.field_values, r.updated_by_actor_id,
                             links_record_field_ids(r))
$$;

CREATE FUNCTION links_from_fields() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM links_sync_record_fields(NEW);
    RETURN NULL;
END $$;

CREATE TRIGGER things_sync_links AFTER INSERT ON collection_records
    FOR EACH ROW WHEN (NEW.field_values IS NOT NULL AND NEW.field_values <> '{}'::jsonb)
    EXECUTE FUNCTION links_from_fields();
CREATE TRIGGER things_sync_links_update AFTER UPDATE OF field_values ON collection_records
    FOR EACH ROW WHEN (OLD.field_values IS DISTINCT FROM NEW.field_values)
    EXECUTE FUNCTION links_from_fields();

-- Copy every store into `links`; rows whose ends aren't things yet are
-- skipped. Safe to run again: each insert ignores what is there. The
-- `links.backfill` job runs it once the registry backfill is done.
CREATE FUNCTION links_copy_all() RETURNS bigint LANGUAGE plpgsql AS $$
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

SELECT links_copy_all();
