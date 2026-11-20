-- A conversation's memory page (A2.16) is as private as its conversation,
-- so it has no record in its space's Pages collection (T1.36): record
-- lists and views read a collection by space, and would show the page's
-- title and fields to space members outside the conversation.
--
-- * page_record_make skips such a page.
-- * A page becomes a conversation's memory right after it is made; its
--   record goes then, with any history it had (it has none: nobody could
--   have reached it yet).
-- * Records already made for such pages go too.

CREATE OR REPLACE FUNCTION page_record_make(page project_pages) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    maker UUID := page_record_actor(
        page.project_id, ARRAY[page.created_by_actor_id, page.updated_by_actor_id]
    );
    collection_id UUID;
    taken BIGINT;
BEGIN
    IF page.memory_for_kind = 'conversation' THEN
        RETURN;
    END IF;
    SELECT id INTO collection_id FROM collections
    WHERE project_id = page.project_id AND builtin = 'pages';
    IF collection_id IS NULL THEN
        IF maker IS NULL THEN
            RETURN;
        END IF;
        collection_id := ensure_pages_collection(page.project_id, maker);
    END IF;
    IF maker IS NULL THEN
        SELECT created_by_actor_id INTO maker FROM collections WHERE id = collection_id;
    END IF;
    UPDATE collections SET next_number = next_number + 1
    WHERE id = collection_id
    RETURNING next_number - 1 INTO taken;
    INSERT INTO collection_records (
        id, project_id, collection_id, number, title, created_by_actor_id,
        updated_by_actor_id, archived_at, archived_by_actor_id, created_at, updated_at
    ) VALUES (
        page.id, page.project_id, collection_id, taken, page_record_title(page.title),
        maker, maker, page.archived_at,
        CASE WHEN page.archived_at IS NULL THEN NULL ELSE maker END,
        page.created_at, page.updated_at
    )
    ON CONFLICT (id) DO NOTHING;
END
$$;

CREATE FUNCTION page_record_leave() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM collection_record_history WHERE record_id = NEW.id;
    DELETE FROM collection_records WHERE id = NEW.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER project_pages_record_leave
    AFTER UPDATE OF memory_for_kind ON project_pages
    FOR EACH ROW
    WHEN (NEW.memory_for_kind = 'conversation'
          AND OLD.memory_for_kind IS DISTINCT FROM NEW.memory_for_kind
          AND NOT importing())
    EXECUTE FUNCTION page_record_leave();

-- The guard lets only the page's own triggers remove a Pages record.
ALTER TABLE collection_records DISABLE TRIGGER collection_records_page_guard;
DELETE FROM collection_record_history history
USING project_pages page
WHERE history.record_id = page.id AND page.memory_for_kind = 'conversation';
DELETE FROM collection_records record
USING project_pages page
WHERE record.id = page.id AND page.memory_for_kind = 'conversation';
ALTER TABLE collection_records ENABLE TRIGGER collection_records_page_guard;
