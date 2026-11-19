-- T1.36: fields on pages, and page lists through views. Each space gets a
-- built-in Pages collection (`builtin = 'pages'`) whose records are its
-- pages: every page has a record there with the same ID, kept in step by
-- the triggers below, so a page's fields are that record's values and any
-- view of the collection (filters, sorts, groups, boards) lists pages.
--
-- * The page stays the one that owns its title and archiving: the record
--   copies them, and a record of the Pages collection can't be added,
--   removed, retitled or archived any other way (page_record_guard; the
--   app refuses it first). Only its field values change on the record.
-- * The Pages collection is made with the space's first page, or when
--   someone opens the collections list (ensure_pages_collection, which
--   crate::collections::pages also calls). It takes the first free name of
--   "Pages", "Pages (built in)", "Pages (built in 2)" and so on.
-- * A record names its maker and last editor among the space's members:
--   the page's maker or editor when they are one, else whoever made the
--   collection.
-- * Records aren't things of their own: a record shares its page's ID, and
--   the thing store's source view leaves them out, so search and links
--   show the page once.

ALTER TABLE collections DROP CONSTRAINT collections_builtin_check;
ALTER TABLE collections
    ADD CONSTRAINT collections_builtin_check CHECK (builtin IN ('tasks', 'pages'));
ALTER TABLE collections DROP CONSTRAINT collections_builtin_acts_like_task;
ALTER TABLE collections
    ADD CONSTRAINT collections_builtin_acts_like_task
    CHECK (builtin IS DISTINCT FROM 'tasks' OR acts_like_task);
ALTER TABLE collections
    ADD CONSTRAINT collections_pages_not_tasks
    CHECK (builtin IS DISTINCT FROM 'pages' OR NOT acts_like_task);

-- The first of `candidates` who is a member of the space, or NULL.
CREATE FUNCTION page_record_actor(target_project UUID, candidates UUID[]) RETURNS UUID
LANGUAGE sql STABLE AS $$
    SELECT candidate
    FROM unnest(candidates) WITH ORDINALITY AS listed(candidate, place)
    WHERE candidate IS NOT NULL AND EXISTS (
        SELECT 1 FROM project_memberships member
        WHERE member.project_id = target_project AND member.actor_id = listed.candidate
    )
    ORDER BY place
    LIMIT 1
$$;

-- The space's Pages collection, made (with an "All pages" table view) if
-- it doesn't exist yet, as `actor` (a member of the space). NULL when it
-- doesn't exist and `actor` isn't a member.
CREATE FUNCTION ensure_pages_collection(target_project UUID, actor UUID) RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    found_id UUID;
    candidate TEXT := 'Pages';
    tries INTEGER := 1;
BEGIN
    SELECT id INTO found_id FROM collections
    WHERE project_id = target_project AND builtin = 'pages';
    IF found_id IS NOT NULL THEN
        RETURN found_id;
    END IF;
    IF page_record_actor(target_project, ARRAY[actor]) IS NULL THEN
        RETURN NULL;
    END IF;
    WHILE EXISTS (
        SELECT 1 FROM collections
        WHERE project_id = target_project AND archived_at IS NULL
          AND lower(name) = lower(candidate)
    ) LOOP
        tries := tries + 1;
        candidate := CASE WHEN tries = 2 THEN 'Pages (built in)'
                          ELSE 'Pages (built in ' || (tries - 1)::text || ')' END;
    END LOOP;
    found_id := gen_random_uuid();
    INSERT INTO collections (id, project_id, name, title_name, created_by_actor_id, builtin)
    VALUES (found_id, target_project, candidate, 'Title', actor, 'pages')
    ON CONFLICT (project_id, builtin) WHERE builtin IS NOT NULL DO NOTHING;
    IF NOT FOUND THEN
        -- Someone else made it first.
        SELECT id INTO found_id FROM collections
        WHERE project_id = target_project AND builtin = 'pages';
        RETURN found_id;
    END IF;
    INSERT INTO collection_views
        (id, project_id, collection_id, name, view_type, created_by_actor_id)
    VALUES (gen_random_uuid(), target_project, found_id, 'All pages', 'table', actor);
    RETURN found_id;
END
$$;

-- A Pages record is its page in the thing store: it never becomes a thing
-- of its own (thing_registry's view, less those records).
CREATE OR REPLACE VIEW thing_source_collection_records AS
SELECT record.id, 'record'::text AS kind, project.organization_id AS workspace_id,
       record.project_id AS space_id, thing_if_registered(record.collection_id) AS parent_id,
       NULL::uuid AS access_parent_id, left(record.title, 300) AS title, NULL::text AS status,
       record.created_by_actor_id AS owner_account_id,
       record.created_by_actor_id AS created_by_account_id,
       record.updated_by_actor_id AS updated_by_account_id, record.version,
       record.created_at, record.updated_at, record.archived_at, NULL::timestamptz AS deleted_at
FROM collection_records record
JOIN projects project ON project.id = record.project_id
WHERE NOT EXISTS (
    SELECT 1 FROM collections collection
    WHERE collection.id = record.collection_id AND collection.builtin = 'pages'
);

-- What a page's title becomes on its record (as task_record_title).
CREATE FUNCTION page_record_title(title TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT btrim(left(btrim(regexp_replace(title, '[[:cntrl:]]', ' ', 'g')), 300))
$$;

-- A page's record, made in its space's Pages collection (made first when
-- needed). Nothing when nobody named on the page is a member yet.
CREATE FUNCTION page_record_make(page project_pages) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    maker UUID := page_record_actor(
        page.project_id, ARRAY[page.created_by_actor_id, page.updated_by_actor_id]
    );
    collection_id UUID;
    taken BIGINT;
BEGIN
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

CREATE FUNCTION page_record_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM page_record_make(NEW);
    RETURN NULL;
END
$$;

-- A workspace import brings each page's record with it (`importing()`).
CREATE TRIGGER project_pages_record_insert
    AFTER INSERT ON project_pages
    FOR EACH ROW
    WHEN (NOT importing())
    EXECUTE FUNCTION page_record_insert();

CREATE FUNCTION page_record_update() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    editor UUID;
BEGIN
    SELECT page_record_actor(
               NEW.project_id,
               ARRAY[NEW.updated_by_actor_id, record.updated_by_actor_id]
           )
    INTO editor
    FROM collection_records record WHERE record.id = NEW.id;
    IF NOT FOUND THEN
        PERFORM page_record_make(NEW);
        RETURN NULL;
    END IF;
    UPDATE collection_records
    SET title = page_record_title(NEW.title),
        archived_at = NEW.archived_at,
        archived_by_actor_id = CASE WHEN NEW.archived_at IS NULL THEN NULL
                                    ELSE coalesce(archived_by_actor_id, editor) END,
        updated_by_actor_id = editor,
        updated_at = greatest(updated_at, NEW.updated_at),
        version = version + 1
    WHERE id = NEW.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER project_pages_record_update
    AFTER UPDATE OF title, archived_at ON project_pages
    FOR EACH ROW
    WHEN (
        (OLD.title IS DISTINCT FROM NEW.title
         OR OLD.archived_at IS DISTINCT FROM NEW.archived_at)
        AND NOT importing()
    )
    EXECUTE FUNCTION page_record_update();

-- Every page there is gets its record (and its space its Pages
-- collection), oldest first so numbers follow the order pages were made.
SELECT page_record_make(page)
FROM project_pages page
ORDER BY page.created_at, page.id;

-- A Pages record is added, removed, retitled and archived only through its
-- page: a write that doesn't come from the triggers above is refused.
CREATE FUNCTION page_record_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() = 1 AND (
        (TG_OP = 'INSERT' AND EXISTS (
            SELECT 1 FROM collections WHERE id = NEW.collection_id AND builtin = 'pages'
        ))
        OR (TG_OP = 'DELETE' AND EXISTS (
            SELECT 1 FROM collections WHERE id = OLD.collection_id AND builtin = 'pages'
        ))
        OR (TG_OP = 'UPDATE'
            AND (OLD.collection_id IS DISTINCT FROM NEW.collection_id
                 OR OLD.project_id IS DISTINCT FROM NEW.project_id
                 OR OLD.number IS DISTINCT FROM NEW.number
                 OR OLD.title IS DISTINCT FROM NEW.title
                 OR OLD.archived_at IS DISTINCT FROM NEW.archived_at)
            AND EXISTS (
                SELECT 1 FROM collections
                WHERE id IN (OLD.collection_id, NEW.collection_id) AND builtin = 'pages'
            ))
    ) THEN
        RAISE EXCEPTION 'a Pages record changes only through its page'
            USING ERRCODE = 'check_violation';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER collection_records_page_guard
    BEFORE INSERT OR UPDATE OR DELETE ON collection_records
    FOR EACH ROW
    WHEN (NOT workspace_deletion_in_progress() AND NOT importing())
    EXECUTE FUNCTION page_record_guard();

