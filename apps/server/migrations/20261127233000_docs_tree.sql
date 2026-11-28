-- AWD-223: the Docs tree. A space's pages, sheets, whiteboards, collections
-- and dashboards sit in one tree of folders, and a document or a folder can
-- be restricted to named people.
--
-- 1. `doc_folders`: a folder of a space.
-- 2. `doc_places`: where a document or folder sits (its parent: a folder,
--    or for a page inside a page that page) and whether it is restricted.
--    No row means the top of the tree, open to the space.
-- 3. The rule, `doc_open_to`: a thing is open to everyone in its space
--    unless it, or something above it, is restricted; then only to the
--    people named on it or on anything between it and the nearest
--    restricted thing above. The named people are `thing_grants`, which a
--    page's shares already are.
-- 4. Row rules that hold the restriction wherever the rows are read: a
--    restricted document doesn't exist for a signed-in account it isn't
--    open to, in any list, search or lookup.
-- 5. The visibility functions say the same for work that runs for nobody in
--    particular (delivery, jobs).

-- 1. Folders -----------------------------------------------------------------

CREATE TABLE doc_folders (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    title TEXT NOT NULL CHECK (
        char_length(title) BETWEEN 1 AND 200 AND title = btrim(title)
        AND title !~ '[[:cntrl:]]'),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    updated_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);

CREATE INDEX doc_folders_space ON doc_folders (space_id) WHERE archived_at IS NULL;

SELECT tenancy_protect('doc_folders', 'workspace_id');

-- 2. Places ------------------------------------------------------------------

CREATE TABLE doc_places (
    thing_id UUID PRIMARY KEY,
    thing_kind TEXT NOT NULL CHECK (
        thing_kind IN ('folder', 'page', 'sheet', 'whiteboard', 'collection', 'dashboard')),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    -- A folder, or for a page inside a page, that page. NULL is the top.
    parent_id UUID CHECK (parent_id <> thing_id),
    restricted BOOLEAN NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX doc_places_parent ON doc_places (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX doc_places_restricted ON doc_places (space_id) WHERE restricted;
CREATE INDEX doc_places_workspace ON doc_places (workspace_id);

SELECT tenancy_protect('doc_places', 'workspace_id');

-- A page inside a page sits under it in the tree too, so what restricts the
-- outer page covers the ones inside. Kept by the page's own row: put inside
-- a page, taken out (to the top), or moved to another space (where its old
-- folder isn't).
INSERT INTO doc_places (thing_id, thing_kind, workspace_id, space_id, parent_id)
SELECT page.id, 'page', page.workspace_id, page.project_id, page.parent_page_id
FROM project_pages page
WHERE page.parent_page_id IS NOT NULL;

CREATE FUNCTION doc_place_follows_page() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.parent_page_id IS NOT DISTINCT FROM OLD.parent_page_id
       AND NEW.project_id IS NOT DISTINCT FROM OLD.project_id THEN
        RETURN NULL;
    END IF;
    IF NEW.parent_page_id IS NOT NULL THEN
        INSERT INTO doc_places (thing_id, thing_kind, workspace_id, space_id, parent_id)
        VALUES (NEW.id, 'page', NEW.workspace_id, NEW.project_id, NEW.parent_page_id)
        ON CONFLICT (thing_id) DO UPDATE
            SET parent_id = EXCLUDED.parent_id, space_id = EXCLUDED.space_id,
                workspace_id = EXCLUDED.workspace_id, updated_at = now();
    ELSIF TG_OP = 'UPDATE' THEN
        UPDATE doc_places
           SET parent_id = NULL, space_id = NEW.project_id, workspace_id = NEW.workspace_id,
               updated_at = now()
         WHERE thing_id = NEW.id;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER doc_place_follows AFTER INSERT OR UPDATE OF parent_page_id, project_id
    ON project_pages FOR EACH ROW EXECUTE FUNCTION doc_place_follows_page();

-- 3. The rule ----------------------------------------------------------------

-- Folders, sheets, whiteboards, collections and dashboards can name people
-- too; sharing with someone outside the space stays a page's alone for now.
ALTER TABLE thing_grants DROP CONSTRAINT thing_grants_thing_kind_check;
ALTER TABLE thing_grants ADD CONSTRAINT thing_grants_thing_kind_check CHECK (
    thing_kind IN ('page', 'folder', 'sheet', 'whiteboard', 'collection', 'dashboard'));

CREATE INDEX thing_grants_named ON thing_grants (thing_id, grantee_account_id)
    WHERE revoked_at IS NULL AND grantee_account_id IS NOT NULL;

-- Whether no restriction hides `target` from `viewer`. Who may open it at
-- all (the space's members, a share) is still the caller's question; this
-- only ever takes away. Reads `doc_places` and `thing_grants` alone, so the
-- row rules below can ask it about the rows they guard.
CREATE FUNCTION doc_open_to(target UUID, viewer UUID) RETURNS BOOLEAN
LANGUAGE plpgsql STABLE AS $$
DECLARE
    current_id UUID := target;
    passed UUID[] := ARRAY[]::UUID[];
    place RECORD;
BEGIN
    FOR step IN 1..64 LOOP
        SELECT parent_id, restricted INTO place FROM doc_places WHERE thing_id = current_id;
        IF NOT FOUND THEN
            RETURN true;
        END IF;
        passed := passed || current_id;
        IF place.restricted THEN
            RETURN viewer IS NOT NULL AND EXISTS (
                SELECT 1 FROM thing_grants named
                WHERE named.thing_id = ANY(passed)
                  AND named.grantee_account_id = viewer
                  AND named.revoked_at IS NULL
                  AND (named.expires_at IS NULL OR named.expires_at > now()));
        END IF;
        IF place.parent_id IS NULL THEN
            RETURN true;
        END IF;
        current_id := place.parent_id;
    END LOOP;
    -- Deeper than any tree the app makes: closed.
    RETURN false;
END $$;

-- The account a row rule asks about: the signed-in account the work in
-- hand is for. Work for nobody in particular (a job, delivery, an export,
-- a public link) has none and is checked by its own code.
CREATE FUNCTION doc_viewer() RETURNS UUID
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN split_part(coalesce(current_setting('app.tenancy', true), ''), ':', 1) IN ('m', 'w')
        THEN NULLIF(
            split_part(split_part(current_setting('app.tenancy', true), ':', 2), '|', 1), '')::UUID
    END
$$;

-- 4. Row rules ---------------------------------------------------------------

-- Everything in the tree that is hidden from `viewer`: the same rule as
-- `doc_open_to`, worked out for the whole tree at once, from the top down.
-- A row rule asks it once a statement and then looks each row up in the
-- answer, so a list of ten thousand rows costs one walk, not ten thousand.
-- Its tables are named with this schema, since pg_dump reads the guarded
-- tables with an empty search_path.
DO $migration$
BEGIN
    EXECUTE format($function$
        CREATE FUNCTION doc_hidden(viewer UUID) RETURNS SETOF UUID
        LANGUAGE sql STABLE AS $$
            WITH RECURSIVE named AS (
                SELECT DISTINCT granted.thing_id
                FROM %1$I.thing_grants granted
                WHERE granted.grantee_account_id = viewer
                  AND granted.revoked_at IS NULL
                  AND (granted.expires_at IS NULL OR granted.expires_at > now())
            ),
            walk(thing_id, open, depth) AS (
                SELECT place.thing_id, NOT place.restricted OR named.thing_id IS NOT NULL, 1
                FROM %1$I.doc_places place
                LEFT JOIN named ON named.thing_id = place.thing_id
                WHERE place.parent_id IS NULL
                   OR NOT EXISTS (
                       SELECT 1 FROM %1$I.doc_places above WHERE above.thing_id = place.parent_id)
                UNION ALL
                SELECT place.thing_id,
                       CASE WHEN place.restricted THEN named.thing_id IS NOT NULL
                            ELSE named.thing_id IS NOT NULL OR walk.open END,
                       walk.depth + 1
                FROM walk
                JOIN %1$I.doc_places place ON place.parent_id = walk.thing_id
                LEFT JOIN named ON named.thing_id = place.thing_id
                WHERE walk.depth < 64
            )
            -- Whatever the walk didn't reach and find open is hidden, so a
            -- loop or a tree deeper than the app makes stays closed.
            SELECT place.thing_id
            FROM %1$I.doc_places place
            WHERE NOT EXISTS (
                SELECT 1 FROM walk WHERE walk.thing_id = place.thing_id AND walk.open)
        $$
    $function$, current_schema());
END
$migration$;

-- A row exists for the account the work is for unless the tree hides it.
-- The first arm is one lookup a statement (work for nobody, or a workspace
-- with nothing restricted, pays nothing a row); the second is one walk a
-- statement and a hash lookup a row.
CREATE POLICY doc_restricted ON doc_folders AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
CREATE POLICY doc_restricted ON project_pages AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
CREATE POLICY doc_restricted ON sheets AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
CREATE POLICY doc_restricted ON whiteboards AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
CREATE POLICY doc_restricted ON collections AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
CREATE POLICY doc_restricted ON dashboards AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR id NOT IN (SELECT doc_hidden((SELECT doc_viewer()))));
-- A record follows its collection; a page's own record (the Pages
-- collection keeps one for each page, under the page's id) follows the page.
CREATE POLICY doc_restricted ON collection_records AS RESTRICTIVE FOR SELECT USING (
    (SELECT doc_viewer() IS NULL OR NOT EXISTS (SELECT 1 FROM doc_places WHERE restricted))
    OR (collection_id NOT IN (SELECT doc_hidden((SELECT doc_viewer())))
        AND id NOT IN (SELECT doc_hidden((SELECT doc_viewer())))));

-- 5. The same rule where a viewer is named -------------------------------------

CREATE OR REPLACE FUNCTION whiteboard_visible_to(target_whiteboard_id UUID, viewer_actor_id UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM whiteboards board
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = board.organization_id
         AND organization_membership.actor_id = viewer_actor_id
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = viewer_actor_id AND actor.status = 'active'
        WHERE board.id = target_whiteboard_id
          AND (
              board.project_id IS NULL
              OR EXISTS (
                  SELECT 1
                  FROM project_memberships membership
                  JOIN projects project ON project.id = membership.project_id
                  WHERE membership.project_id = board.project_id
                    AND membership.actor_id = viewer_actor_id
                    AND membership.enabled
                    AND project.archived_at IS NULL
              )
          )
          AND doc_open_to(board.id, viewer_actor_id)
    )
$$;

CREATE OR REPLACE FUNCTION subject_visible_to(target_kind TEXT, target_id UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE target_kind
        WHEN 'whiteboard' THEN whiteboard_visible_to(target_id, viewer)
        WHEN 'task' THEN EXISTS (
            SELECT 1 FROM work_items item
            WHERE item.id = target_id
              AND subject_space_member(viewer, item.project_id)
              AND subject_sees_tasks(viewer, item.project_id))
        WHEN 'page' THEN EXISTS (
            SELECT 1 FROM project_pages page
            WHERE page.id = target_id
              AND ((subject_space_member(viewer, page.project_id)
                    AND (page.memory_for_kind IS DISTINCT FROM 'conversation'
                         OR COALESCE(conversation_visible_to(page.memory_for_id, viewer), false)))
                   OR (page.memory_for_kind IS DISTINCT FROM 'conversation'
                       AND page_shared_for_comment(page.id, viewer)))
              AND doc_open_to(page.id, viewer))
        WHEN 'sheet' THEN EXISTS (
            SELECT 1 FROM sheets sheet
            WHERE sheet.id = target_id
              AND subject_space_member(viewer, sheet.project_id)
              AND doc_open_to(sheet.id, viewer))
        WHEN 'record' THEN EXISTS (
            SELECT 1 FROM collection_records record
            JOIN collections collection ON collection.id = record.collection_id
            WHERE record.id = target_id
              AND subject_space_member(viewer, record.project_id)
              AND (NOT collection.acts_like_task
                   OR subject_sees_tasks(viewer, record.project_id))
              AND doc_open_to(collection.id, viewer)
              AND doc_open_to(record.id, viewer))
        ELSE false
    END
$$;

-- Delivery hears about collections and dashboards too.
CREATE OR REPLACE FUNCTION delivery_thing_visible(thing UUID, recipient UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN EXISTS (SELECT 1 FROM conversations c WHERE c.id = thing)
            THEN conversation_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM requests r WHERE r.id = thing)
            THEN request_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM runs r WHERE r.id = thing)
            THEN run_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM whiteboards w WHERE w.id = thing)
            THEN whiteboard_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM work_items w WHERE w.id = thing)
            THEN subject_visible_to('task', thing, recipient)
        WHEN EXISTS (SELECT 1 FROM project_pages p WHERE p.id = thing)
            THEN page_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM sheets s WHERE s.id = thing)
            THEN subject_visible_to('sheet', thing, recipient)
        WHEN EXISTS (SELECT 1 FROM collection_records r WHERE r.id = thing)
            THEN subject_visible_to('record', thing, recipient)
        ELSE doc_open_to(thing, recipient)
    END
$$;
