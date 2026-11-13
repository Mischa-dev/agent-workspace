-- AWD-112: "Referenced in" for collection records.
--
-- A record opens at /projects/{project}/collections/{parent}?record={id}
-- (AWD-110). A link like that in a message, page, task description or task
-- comment now counts as a reference to the record, so the record can list
-- where it's mentioned. Readers still pass each reference through the
-- things resolver, so one from a private conversation never shows to
-- someone outside it.

ALTER TABLE thing_references DROP CONSTRAINT thing_references_target_kind_check;
ALTER TABLE thing_references ADD CONSTRAINT thing_references_target_kind_check CHECK (
    target_kind IN ('task', 'page', 'whiteboard', 'thread', 'channel', 'conversation', 'record')
);

-- As before, plus a collection link's `?record=<id>`, which counts as that
-- record. At most 100 per text; the same thing once.
CREATE OR REPLACE FUNCTION thing_reference_targets(body TEXT)
RETURNS TABLE (target_kind TEXT, target_id UUID)
LANGUAGE sql IMMUTABLE AS $$
    SELECT DISTINCT kind, id
    FROM (
        (
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
        )
        UNION ALL
        (
            SELECT 'record' AS kind, lower(m[1])::uuid AS id
            FROM regexp_matches(
                COALESCE(body, ''),
                '/projects/[0-9a-fA-F-]{36}/collections/[0-9a-fA-F-]{36}\?(?:[^\s#&?]*&)*record=([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})',
                'g'
            ) AS m
            LIMIT 400
        )
    ) found
    LIMIT 100
$$;

-- Record links already written.
INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT message.project_id, 'message', message.id, 'conversation', message.conversation_id,
       found.target_kind, found.target_id
FROM messages message
CROSS JOIN LATERAL thing_reference_targets(message.plain_text) found
WHERE message.removed_at IS NULL
  AND message.plain_text ~ '/collections/[^\s]*record='
  AND found.target_kind = 'record'
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT page.project_id, 'page', page.id, 'page', page.id, found.target_kind, found.target_id
FROM project_pages page
CROSS JOIN LATERAL thing_reference_targets(page.body) found
WHERE page.archived_at IS NULL
  AND page.body ~ '/collections/[^\s]*record='
  AND found.target_kind = 'record'
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT item.project_id, 'task', item.id, 'task', item.id, found.target_kind, found.target_id
FROM work_items item
CROSS JOIN LATERAL thing_reference_targets(item.description) found
WHERE item.archived_at IS NULL
  AND item.description ~ '/collections/[^\s]*record='
  AND found.target_kind = 'record'
ON CONFLICT DO NOTHING;

INSERT INTO thing_references
    (project_id, source_kind, source_id, thing_kind, thing_id, target_kind, target_id)
SELECT comment.project_id, 'task_comment', comment.id, 'task', comment.work_item_id,
       found.target_kind, found.target_id
FROM work_item_comments comment
CROSS JOIN LATERAL thing_reference_targets(comment.plain_text) found
WHERE comment.removed_at IS NULL
  AND comment.plain_text ~ '/collections/[^\s]*record='
  AND found.target_kind = 'record'
ON CONFLICT DO NOTHING;
