-- K3.15, K3.16: task dependencies and relations live in the link store.
--
-- `work_item_dependencies` and `work_item_relations` were mirrored into
-- `links` (K3.12) and copied once (#291's conversion). Now the link store
-- is where they are written, and the two tables go. Readers keep their
-- shape through two views over `links` and the registry, named for what
-- they read: `task_dependencies` (blocking -blocks-> blocked) and
-- `task_relations` (task -relates_to|duplicates-> other). Each end's
-- project is the registry's space, so a task moved to another project
-- takes its links with it, and a deleted task's links stop counting.

-- Anything a mirror missed comes over first.
SELECT links_put_row(link_of(r)) FROM work_item_dependencies r;
SELECT links_put_row(link_of(r)) FROM work_item_relations r;

DROP TABLE work_item_dependencies CASCADE;
DROP TABLE work_item_relations CASCADE;
DROP FUNCTION IF EXISTS validate_cross_project_task_link();

-- As the old table had it: a pair is duplicated one way only.
CREATE UNIQUE INDEX links_duplicates_pair
    ON links (LEAST(from_thing_id, to_thing_id), GREATEST(from_thing_id, to_thing_id))
    WHERE kind = 'duplicates' AND origin = 'explicit';

CREATE VIEW task_dependencies WITH (security_invoker = true) AS
SELECT link.id,
       blocked.space_id AS project_id,
       link.from_thing_id AS blocking_work_item_id,
       blocking.space_id AS blocking_project_id,
       link.to_thing_id AS blocked_work_item_id,
       link.created_by_account_id AS created_by_actor_id,
       link.created_at
FROM links link
JOIN things blocking
  ON blocking.id = link.from_thing_id AND blocking.kind = 'task' AND blocking.deleted_at IS NULL
JOIN things blocked
  ON blocked.id = link.to_thing_id AND blocked.kind = 'task' AND blocked.deleted_at IS NULL
WHERE link.kind = 'blocks' AND link.origin = 'explicit';

CREATE VIEW task_relations WITH (security_invoker = true) AS
SELECT link.id,
       link.kind,
       task.space_id AS project_id,
       link.from_thing_id AS work_item_id,
       other.space_id AS other_project_id,
       link.to_thing_id AS other_work_item_id,
       link.created_by_account_id AS created_by_actor_id,
       link.created_at
FROM links link
JOIN things task
  ON task.id = link.from_thing_id AND task.kind = 'task' AND task.deleted_at IS NULL
JOIN things other
  ON other.id = link.to_thing_id AND other.kind = 'task' AND other.deleted_at IS NULL
WHERE link.kind IN ('relates_to', 'duplicates') AND link.origin = 'explicit';

-- The copy no longer reads the two tables.
CREATE OR REPLACE FUNCTION links_copy_all() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    before bigint;
BEGIN
    SELECT count(*) INTO before FROM links;
    PERFORM links_put_row(link_of(r)) FROM work_item_links r;
    PERFORM links_put_row(link_of(r)) FROM work_item_conversation_links r;
    PERFORM links_put_row(link_of(r)) FROM whiteboard_links r;
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
