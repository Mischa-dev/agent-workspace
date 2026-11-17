-- Rule 3 (one deploy, replace rather than run side by side): rows that
-- existed before the thing registry and the link store are converted once,
-- here, instead of by the `things.backfill` job (which queued
-- `links.backfill`). Writes since then reach both through their triggers.
--
-- `things_convert_all()` is the conversion: every source table's rows into
-- `things`, then each thing's parent. Tests call it to check it rebuilds
-- the registry from the sources.

CREATE FUNCTION things_convert_all() RETURNS BIGINT
LANGUAGE plpgsql AS $$
DECLARE
    source TEXT;
    converted BIGINT := 0;
    batch UUID[];
BEGIN
    FOREACH source IN ARRAY ARRAY[
        'projects', 'repositories', 'conversations', 'collections', 'project_pages',
        'project_decisions', 'whiteboards', 'sheets', 'project_milestones', 'work_items',
        'collection_records', 'messages', 'files', 'github_pull_requests',
        'automation_rules', 'runs', 'requests'
    ] LOOP
        EXECUTE format('SELECT ARRAY(SELECT id FROM %I ORDER BY id)', source) INTO batch;
        PERFORM things_upsert_from('thing_source_' || source, batch);
        converted := converted + cardinality(batch);
    END LOOP;
    -- Parents after every kind is in, so a thing's parent row exists.
    FOREACH source IN ARRAY ARRAY[
        'conversations', 'project_decisions', 'work_items', 'collection_records', 'messages',
        'files', 'github_pull_requests', 'runs', 'requests'
    ] LOOP
        EXECUTE format($sql$
            UPDATE things thing
            SET parent_id = source.parent_id, access_parent_id = source.access_parent_id
            FROM %I source
            WHERE source.id = thing.id AND source.kind = thing.kind
              AND (thing.parent_id IS DISTINCT FROM source.parent_id
                   OR thing.access_parent_id IS DISTINCT FROM source.access_parent_id)
        $sql$, 'thing_source_' || source);
    END LOOP;
    RETURN converted;
END;
$$;

SELECT things_convert_all();
SELECT links_copy_all();

-- The jobs that did this a batch at a time are gone, with their progress.
DELETE FROM background_jobs WHERE kind IN ('things.backfill', 'links.backfill');
DROP TABLE things_backfill;
