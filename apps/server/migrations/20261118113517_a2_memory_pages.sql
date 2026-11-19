-- A2.16: memory pages. A space and a conversation (a channel or thread)
-- can each have one memory page: an ordinary page in the space, with every
-- version kept, that agents read before acting there. A conversation's
-- memory is as private as the conversation: only those who may read the
-- conversation may read or change its memory page.

ALTER TABLE project_pages
    ADD COLUMN memory_for_kind TEXT CHECK (memory_for_kind IN ('space', 'conversation')),
    ADD COLUMN memory_for_id UUID,
    ADD CONSTRAINT project_pages_memory_for_whole
        CHECK ((memory_for_kind IS NULL) = (memory_for_id IS NULL));

CREATE UNIQUE INDEX project_pages_one_memory
    ON project_pages (memory_for_kind, memory_for_id)
    WHERE memory_for_kind IS NOT NULL AND archived_at IS NULL;

-- Whether `viewer` may see a page: a member of its space, and for a
-- conversation's memory page, a reader of that conversation.
CREATE FUNCTION page_visible_to(page_id UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT subject_visible_to('page', page_id, viewer)
       AND COALESCE((
           SELECT page.memory_for_kind IS DISTINCT FROM 'conversation'
               OR COALESCE(conversation_visible_to(page.memory_for_id, viewer), false)
           FROM project_pages page
           WHERE page.id = page_id
       ), false)
$$;

-- Delivery checks a page by the same rule.
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
        ELSE true
    END
$$;

-- In the thing registry a conversation's memory page has the conversation
-- as its access parent, so finding it as a thing follows the same rule.
CREATE OR REPLACE VIEW thing_source_project_pages AS
SELECT page.id, 'page'::text AS kind, project.organization_id AS workspace_id,
       page.project_id AS space_id, NULL::uuid AS parent_id,
       CASE WHEN page.memory_for_kind = 'conversation' THEN page.memory_for_id END
           AS access_parent_id,
       left(page.title, 300) AS title, NULL::text AS status,
       page.created_by_actor_id AS owner_account_id, page.created_by_actor_id AS created_by_account_id,
       page.updated_by_actor_id AS updated_by_account_id, page.version,
       page.created_at, page.updated_at, page.archived_at, NULL::timestamptz AS deleted_at
FROM project_pages page
JOIN projects project ON project.id = page.project_id;
