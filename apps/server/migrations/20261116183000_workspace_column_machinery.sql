-- H1.4 (build 3R): the machinery that gives every workspace-owned table its
-- workspace column (H1 3.3), used by H1.4 to H1.7's batches.
--
-- Per table, in two migrations:
-- 1. Expand (`tenancy_expand`): `workspace_id UUID` is added nullable, with
--    its foreign key to organizations(id) added NOT VALID (instant), and a
--    BEFORE INSERT trigger `<table>_fill_workspace` that fills it, when the
--    writer didn't, from the row's project or its parent row. The trigger
--    writes only the new row's own column, never another table (R16). A
--    `tenancy.backfill` job is queued to copy the column into existing rows a
--    few pages at a time, resuming from its cursor after a restart.
-- 2. Switch (`tenancy_switch`): the rest of the backfill runs inline (so it
--    doesn't matter whether the job ran), then the column becomes NOT NULL
--    through a validated CHECK, the foreign key is validated, and an index
--    leading with the column is added where the table has none.
--
-- Where the column comes from is written once, in the fill trigger's
-- arguments: none for a table with `project_id` (the project's workspace),
-- otherwise groups of three: the parent table, this table's column naming the
-- parent's `id`, and the parent's tenant column. The backfill reads the same
-- arguments, so the trigger and the backfill can't disagree.

-- A row with a project takes the project's workspace.
CREATE FUNCTION fill_workspace_from_project() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.workspace_id IS NULL AND NEW.project_id IS NOT NULL THEN
        SELECT organization_id INTO NEW.workspace_id FROM projects WHERE id = NEW.project_id;
    END IF;
    RETURN NEW;
END;
$$;

-- A row without a project takes its parent row's workspace: the trigger's
-- arguments name one or more parents in groups of (table, column, tenant
-- column); the first that is found wins.
CREATE FUNCTION fill_workspace_from_parent() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    at INTEGER := 0;
    found UUID;
BEGIN
    IF NEW.workspace_id IS NOT NULL THEN
        RETURN NEW;
    END IF;
    WHILE at + 2 < TG_NARGS LOOP
        EXECUTE format(
            'SELECT parent.%I FROM %I parent WHERE parent.id = ($1).%I',
            TG_ARGV[at + 2], TG_ARGV[at], TG_ARGV[at + 1]
        ) INTO found USING NEW;
        IF found IS NOT NULL THEN
            NEW.workspace_id := found;
            RETURN NEW;
        END IF;
        at := at + 3;
    END LOOP;
    RETURN NEW;
END;
$$;

-- The fill trigger's arguments for `target`: empty for a project-filled
-- table, the parent groups otherwise; NULL when the table has no fill
-- trigger.
CREATE FUNCTION tenancy_fill_sources(target REGCLASS) RETURNS TEXT[]
LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN trigger_row.tgnargs = 0 THEN ARRAY[]::TEXT[]
        ELSE (string_to_array(
            rtrim(encode(trigger_row.tgargs, 'escape'), '\000'), '\000'
        ))[1:trigger_row.tgnargs]
    END
    FROM pg_trigger trigger_row
    JOIN pg_class class ON class.oid = trigger_row.tgrelid
    WHERE trigger_row.tgrelid = target
      AND trigger_row.tgname = class.relname || '_fill_workspace';
$$;

-- Expand: the column, its foreign key (NOT VALID, so instant), the fill
-- trigger. `sources` as for the trigger: empty for a table with a project.
-- Safe to run again, and on a table that already has the column (a sibling
-- branch may have added it).
CREATE FUNCTION tenancy_expand(target REGCLASS, VARIADIC sources TEXT[] DEFAULT ARRAY[]::TEXT[])
RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    label TEXT := (SELECT relname FROM pg_class WHERE oid = target);
BEGIN
    IF cardinality(sources) % 3 <> 0 THEN
        RAISE EXCEPTION 'tenancy_expand(%): sources come in threes', label;
    END IF;
    EXECUTE format('ALTER TABLE %s ADD COLUMN IF NOT EXISTS workspace_id UUID', target);
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = target AND conname = label || '_workspace_id_fkey'
    ) THEN
        EXECUTE format(
            'ALTER TABLE %s ADD CONSTRAINT %I FOREIGN KEY (workspace_id) '
            'REFERENCES organizations(id) NOT VALID',
            target, label || '_workspace_id_fkey'
        );
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgrelid = target AND tgname = label || '_fill_workspace'
    ) THEN
        EXECUTE format('DROP TRIGGER %I ON %s', label || '_fill_workspace', target);
    END IF;
    IF cardinality(sources) = 0 THEN
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE INSERT ON %s FOR EACH ROW '
            'EXECUTE FUNCTION fill_workspace_from_project()',
            label || '_fill_workspace', target
        );
    ELSE
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE INSERT ON %s FOR EACH ROW '
            'EXECUTE FUNCTION fill_workspace_from_parent(%s)',
            label || '_fill_workspace', target,
            (SELECT string_agg(quote_literal(source), ', ') FROM unnest(sources) source)
        );
    END IF;
    -- The job that copies the column into the rows there are now, if any.
    EXECUTE format(
        'INSERT INTO background_jobs (id, kind, payload, unique_key, max_attempts) '
        'SELECT gen_random_uuid(), ''tenancy.backfill'', $1, $2, 50 '
        'WHERE EXISTS (SELECT 1 FROM %s WHERE workspace_id IS NULL) '
        'ON CONFLICT DO NOTHING',
        target
    ) USING jsonb_build_object('table', label, 'afterPage', 0),
            'tenancy.backfill:' || label || ':0';
END;
$$;

-- One batch of the backfill: rows without the column on pages
-- [from_page, from_page + pages) get it from their project or parent. Returns
-- the page to start the next batch at, or NULL when the table is done.
-- Batching by page (a TID range scan) needs no sort and no index, and a row
-- whose parent can't be found is passed over rather than looked at forever.
CREATE FUNCTION tenancy_backfill(target REGCLASS, from_page BIGINT, pages INTEGER)
RETURNS BIGINT
LANGUAGE plpgsql AS $$
DECLARE
    sources TEXT[] := tenancy_fill_sources(target);
    last_page BIGINT := pg_relation_size(target) / current_setting('block_size')::BIGINT;
    to_page BIGINT := from_page + pages;
    page_range TEXT := format(
        'target.ctid >= ''(%s,0)''::tid AND target.ctid < ''(%s,0)''::tid', from_page, to_page
    );
    parents TEXT[] := ARRAY[]::TEXT[];
    at INTEGER := 1;
    -- The table's own triggers that fire on UPDATE (guards that refuse
    -- changes to sent messages or answered questions, sync and timestamp
    -- triggers) are about its content; filling the workspace column is
    -- none of theirs, so they are off for the batch and on again after it.
    quiet TEXT[] := ARRAY(
        SELECT tgname FROM pg_trigger
        WHERE tgrelid = target AND NOT tgisinternal AND tgenabled = 'O'
          AND tgtype & 16 <> 0
        ORDER BY tgname
    );
    trigger_name TEXT;
BEGIN
    IF sources IS NULL THEN
        RAISE EXCEPTION 'tenancy_backfill(%): the table has no fill trigger', target;
    END IF;
    FOREACH trigger_name IN ARRAY quiet LOOP
        EXECUTE format('ALTER TABLE %s DISABLE TRIGGER %I', target, trigger_name);
    END LOOP;
    IF cardinality(sources) = 0 THEN
        EXECUTE format(
            'UPDATE %s target SET workspace_id = project.organization_id FROM projects project '
            'WHERE project.id = target.project_id AND target.workspace_id IS NULL AND %s',
            target, page_range
        );
    ELSE
        WHILE at + 2 <= cardinality(sources) LOOP
            parents := parents || format(
                '(SELECT parent.%I FROM %I parent WHERE parent.id = target.%I)',
                sources[at + 2], sources[at], sources[at + 1]
            );
            at := at + 3;
        END LOOP;
        EXECUTE format(
            'UPDATE %s target SET workspace_id = COALESCE(%s) '
            'WHERE target.workspace_id IS NULL AND %s',
            target, array_to_string(parents, ', '), page_range
        );
    END IF;
    FOREACH trigger_name IN ARRAY quiet LOOP
        EXECUTE format('ALTER TABLE %s ENABLE TRIGGER %I', target, trigger_name);
    END LOOP;
    IF to_page >= last_page THEN
        RETURN NULL;
    END IF;
    RETURN to_page;
END;
$$;

-- Switch: finish the backfill inline, then make the column required, check
-- its foreign key and index it. Rows still without a workspace after the
-- backfill have no project or parent to take one from: the migration fails
-- and names the table rather than guessing.
CREATE FUNCTION tenancy_switch(target REGCLASS) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    label TEXT := (SELECT relname FROM pg_class WHERE oid = target);
    next_page BIGINT := 0;
BEGIN
    IF (SELECT attnotnull FROM pg_attribute
        WHERE attrelid = target AND attname = 'workspace_id') THEN
        RETURN;
    END IF;
    LOOP
        next_page := tenancy_backfill(target, next_page, 1000);
        EXIT WHEN next_page IS NULL;
    END LOOP;
    -- Deferred checks the backfill queued run now, so the table can be
    -- altered in this transaction.
    SET CONSTRAINTS ALL IMMEDIATE;
    EXECUTE format(
        'ALTER TABLE %s ADD CONSTRAINT %I CHECK (workspace_id IS NOT NULL) NOT VALID',
        target, label || '_workspace_id_set'
    );
    BEGIN
        EXECUTE format('ALTER TABLE %s VALIDATE CONSTRAINT %I', target, label || '_workspace_id_set');
    EXCEPTION WHEN check_violation THEN
        RAISE EXCEPTION 'tenancy_switch(%): rows with no project or parent to take a workspace from', label;
    END;
    -- The validated CHECK lets SET NOT NULL skip its scan.
    EXECUTE format('ALTER TABLE %s ALTER COLUMN workspace_id SET NOT NULL', target);
    EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', target, label || '_workspace_id_set');
    EXECUTE format('ALTER TABLE %s VALIDATE CONSTRAINT %I', target, label || '_workspace_id_fkey');
    IF NOT EXISTS (
        SELECT 1 FROM pg_index index_row
        JOIN pg_attribute attribute
          ON attribute.attrelid = index_row.indrelid
         AND attribute.attnum = index_row.indkey[0]
        WHERE index_row.indrelid = target AND attribute.attname = 'workspace_id'
    ) THEN
        EXECUTE format('CREATE INDEX %I ON %s (workspace_id)', label || '_workspace', target);
    END IF;
END;
$$;
