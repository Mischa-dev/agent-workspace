-- K3.10 (build 1E): the thing registry (docs/plans/master/plans/
-- K3-things-links-events.md 3.1, 4.1). One row in `things` for every task,
-- page, decision, board, sheet, collection, record, conversation and project,
-- with the same id as its own row, kept current by a trigger on each kind's
-- table. Links, events and K1's access filter point at it.
--
-- * thing_kinds: the registered kinds (global: a kind is the server's, not a
--   workspace's). The Rust registry (`things::registry`) lists the same set;
--   a test keeps them equal.
-- * things: the envelope. Titles of kinds whose text removal must scrub
--   (messages, comments) stay NULL; nothing here is body text (rule 10).
-- * thing_source_<table>: one view per kind table giving its rows as
--   `things` rows. The trigger upserts one row from it, and the backfill job
--   copies it in id order, so the two never disagree about a column.
-- * things_backfill: how far the `things.backfill` job got per source, and
--   when it finished (global: the job is the server's).
--
-- A parent or access parent is set only once its own row is registered, so
-- a write during the backfill never fails on the deferred foreign key; the
-- backfill sets the rest when a source finishes.
--
-- Tasks as records (T1.3's stopgap, 20261114010000): a task's record has the
-- task's id. The work_items source claims the row as kind `task` whichever
-- is written first; the collection_records source never overwrites a row of
-- another kind.

CREATE TABLE thing_kinds (
    kind TEXT PRIMARY KEY CHECK (kind ~ '^[a-z][a-z0-9_]{1,39}$'),
    module TEXT NOT NULL CHECK (module ~ '^[a-z][a-z0-9_-]{0,39}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO thing_kinds (kind, module) VALUES
    ('project', 'projects'),
    ('channel', 'channels'),
    ('thread', 'channels'),
    ('conversation', 'messages'),
    ('task', 'tasks'),
    ('page', 'pages'),
    ('decision', 'pages'),
    ('whiteboard', 'whiteboards'),
    ('sheet', 'sheets'),
    ('collection', 'collections'),
    ('record', 'collections');

CREATE TABLE things (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL REFERENCES thing_kinds(kind),
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Today's project id; NULL for a thing across a workspace (a workspace
    -- board). A project's own row names itself.
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    -- Containment: thread -> channel, record -> collection, subtask -> task.
    parent_id UUID REFERENCES things(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    -- Whose visibility governs this one: a decision -> its source conversation.
    access_parent_id UUID REFERENCES things(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    title TEXT CHECK (title IS NULL OR char_length(title) <= 300),
    -- A stable key, not a display name.
    status TEXT CHECK (status IS NULL OR status ~ '^[a-z][a-z0-9_]{0,39}$'),
    owner_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    created_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    updated_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ,
    -- A tombstone: the row stays so old events and links can still be
    -- checked against it.
    deleted_at TIMESTAMPTZ,
    CHECK (id <> parent_id),
    CHECK (id <> access_parent_id)
);
CREATE INDEX things_space_kind ON things (space_id, kind, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX things_workspace_kind ON things (workspace_id, kind, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX things_parent ON things (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX things_access_parent ON things (access_parent_id) WHERE access_parent_id IS NOT NULL;
CREATE INDEX things_owner ON things (owner_account_id) WHERE deleted_at IS NULL;

-- A free-text status as a key, or NULL when it can't be one.
CREATE FUNCTION thing_status_key(value TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN key ~ '^[a-z][a-z0-9_]{0,39}$' THEN key END
    FROM (SELECT trim(both '_' FROM lower(regexp_replace(value, '[^A-Za-z0-9]+', '_', 'g'))) AS key) normalised
$$;

-- `target` when its own row is registered, else NULL (see the header).
CREATE FUNCTION thing_if_registered(target UUID) RETURNS UUID
LANGUAGE sql STABLE AS $$
    SELECT id FROM things WHERE id = target
$$;

-- The sources. Every view has the columns of `things`, in its order.
CREATE VIEW thing_source_projects AS
SELECT project.id, 'project'::text AS kind, project.organization_id AS workspace_id,
       project.id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(project.name, 300) AS title, NULL::text AS status,
       NULL::uuid AS owner_account_id, NULL::uuid AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, project.version,
       project.created_at, project.updated_at, project.archived_at, NULL::timestamptz AS deleted_at
FROM projects project;

CREATE VIEW thing_source_conversations AS
SELECT conversation.id,
       CASE conversation.kind WHEN 'thread' THEN 'thread' WHEN 'channel' THEN 'channel'
            ELSE 'conversation' END AS kind,
       project.organization_id AS workspace_id, conversation.project_id AS space_id,
       thing_if_registered(conversation.parent_channel_id) AS parent_id,
       NULL::uuid AS access_parent_id,
       left(conversation.name, 300) AS title,
       CASE WHEN conversation.kind = 'thread' THEN thing_status_key(conversation.thread_status) END AS status,
       NULL::uuid AS owner_account_id, NULL::uuid AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, conversation.version,
       conversation.created_at, conversation.updated_at, conversation.archived_at,
       NULL::timestamptz AS deleted_at
FROM conversations conversation
JOIN projects project ON project.id = conversation.project_id;

CREATE VIEW thing_source_work_items AS
SELECT item.id, 'task'::text AS kind, project.organization_id AS workspace_id,
       item.project_id AS space_id, thing_if_registered(item.parent_work_item_id) AS parent_id,
       NULL::uuid AS access_parent_id, left(item.title, 300) AS title,
       status.semantic_category AS status,
       item.assignee_actor_id AS owner_account_id, item.created_by_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, item.version,
       item.created_at, item.updated_at, item.archived_at, NULL::timestamptz AS deleted_at
FROM work_items item
JOIN projects project ON project.id = item.project_id
JOIN work_item_statuses status ON status.id = item.status_id;

CREATE VIEW thing_source_project_pages AS
SELECT page.id, 'page'::text AS kind, project.organization_id AS workspace_id,
       page.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(page.title, 300) AS title, NULL::text AS status,
       page.created_by_actor_id AS owner_account_id, page.created_by_actor_id AS created_by_account_id,
       page.updated_by_actor_id AS updated_by_account_id, page.version,
       page.created_at, page.updated_at, page.archived_at, NULL::timestamptz AS deleted_at
FROM project_pages page
JOIN projects project ON project.id = page.project_id;

CREATE VIEW thing_source_project_decisions AS
SELECT decision.id, 'decision'::text AS kind, project.organization_id AS workspace_id,
       decision.project_id AS space_id, NULL::uuid AS parent_id,
       thing_if_registered(decision.source_conversation_id) AS access_parent_id,
       left(decision.title, 300) AS title, decision.status,
       decision.decided_by_actor_id AS owner_account_id,
       decision.decided_by_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, 1::bigint AS version,
       decision.created_at, decision.updated_at, NULL::timestamptz AS archived_at,
       NULL::timestamptz AS deleted_at
FROM project_decisions decision
JOIN projects project ON project.id = decision.project_id;

CREATE VIEW thing_source_whiteboards AS
SELECT board.id, 'whiteboard'::text AS kind, board.organization_id AS workspace_id,
       board.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(board.title, 300) AS title, NULL::text AS status,
       board.created_by_actor_id AS owner_account_id, board.created_by_actor_id AS created_by_account_id,
       board.updated_by_actor_id AS updated_by_account_id, board.version,
       board.created_at, board.updated_at, NULL::timestamptz AS archived_at, board.deleted_at
FROM whiteboards board;

CREATE VIEW thing_source_sheets AS
SELECT sheet.id, 'sheet'::text AS kind, project.organization_id AS workspace_id,
       sheet.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(sheet.name, 300) AS title, NULL::text AS status,
       sheet.created_by_actor_id AS owner_account_id, sheet.created_by_actor_id AS created_by_account_id,
       sheet.updated_by_actor_id AS updated_by_account_id, sheet.revision AS version,
       sheet.created_at, sheet.updated_at, sheet.archived_at, NULL::timestamptz AS deleted_at
FROM sheets sheet
JOIN projects project ON project.id = sheet.project_id;

CREATE VIEW thing_source_collections AS
SELECT collection.id, 'collection'::text AS kind, project.organization_id AS workspace_id,
       collection.project_id AS space_id, NULL::uuid AS parent_id, NULL::uuid AS access_parent_id,
       left(collection.name, 300) AS title, NULL::text AS status,
       collection.created_by_actor_id AS owner_account_id,
       collection.created_by_actor_id AS created_by_account_id,
       NULL::uuid AS updated_by_account_id, collection.version,
       collection.created_at, collection.updated_at, collection.archived_at,
       NULL::timestamptz AS deleted_at
FROM collections collection
JOIN projects project ON project.id = collection.project_id;

CREATE VIEW thing_source_collection_records AS
SELECT record.id, 'record'::text AS kind, project.organization_id AS workspace_id,
       record.project_id AS space_id, thing_if_registered(record.collection_id) AS parent_id,
       NULL::uuid AS access_parent_id, left(record.title, 300) AS title, NULL::text AS status,
       record.created_by_actor_id AS owner_account_id,
       record.created_by_actor_id AS created_by_account_id,
       record.updated_by_actor_id AS updated_by_account_id, record.version,
       record.created_at, record.updated_at, record.archived_at, NULL::timestamptz AS deleted_at
FROM collection_records record
JOIN projects project ON project.id = record.project_id;

-- Upsert `ids` from one source. A row of another kind is left alone, except
-- that a task claims its record's row.
CREATE FUNCTION things_upsert_from(source TEXT, ids UUID[]) RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE format($sql$
        INSERT INTO things (id, kind, workspace_id, space_id, parent_id, access_parent_id,
                            title, status, owner_account_id, created_by_account_id,
                            updated_by_account_id, version, created_at, updated_at,
                            archived_at, deleted_at)
        SELECT id, kind, workspace_id, space_id, parent_id, access_parent_id,
               title, status, owner_account_id, created_by_account_id,
               updated_by_account_id, version, created_at, updated_at,
               archived_at, deleted_at
        FROM %I WHERE id = ANY($1)
        ON CONFLICT (id) DO UPDATE SET
            kind = EXCLUDED.kind,
            workspace_id = EXCLUDED.workspace_id,
            space_id = EXCLUDED.space_id,
            parent_id = EXCLUDED.parent_id,
            access_parent_id = EXCLUDED.access_parent_id,
            title = EXCLUDED.title,
            status = EXCLUDED.status,
            owner_account_id = EXCLUDED.owner_account_id,
            created_by_account_id = EXCLUDED.created_by_account_id,
            updated_by_account_id = EXCLUDED.updated_by_account_id,
            version = EXCLUDED.version,
            updated_at = EXCLUDED.updated_at,
            archived_at = EXCLUDED.archived_at,
            deleted_at = EXCLUDED.deleted_at
        WHERE things.kind = EXCLUDED.kind
           OR EXCLUDED.kind = 'task'
           OR (things.kind IN ('channel', 'thread', 'conversation')
               AND EXCLUDED.kind IN ('channel', 'thread', 'conversation'))
    $sql$, source) USING ids;
END $$;

-- The trigger: TG_ARGV[0] is the source view, TG_ARGV[1] the kinds a hard
-- delete of the row tombstones.
CREATE FUNCTION things_sync() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE things SET deleted_at = COALESCE(deleted_at, now()), updated_at = now()
        WHERE id = OLD.id AND kind = ANY(TG_ARGV[1]::text[]);
        RETURN NULL;
    END IF;
    PERFORM things_upsert_from(TG_ARGV[0], ARRAY[NEW.id]);
    RETURN NULL;
END $$;

CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    organization_id, name, version, created_at, updated_at, archived_at
    ON projects FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_projects', '{project}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, kind, parent_channel_id, name, thread_status, version, updated_at, archived_at
    ON conversations FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_conversations', '{channel,thread,conversation}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, parent_work_item_id, title, status_id, assignee_actor_id, version, updated_at,
    archived_at
    ON work_items FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_work_items', '{task}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, title, updated_by_actor_id, version, updated_at, archived_at
    ON project_pages FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_project_pages', '{page}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, source_conversation_id, title, status, decided_by_actor_id, updated_at
    ON project_decisions FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_project_decisions', '{decision}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    organization_id, project_id, title, updated_by_actor_id, version, updated_at, deleted_at
    ON whiteboards FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_whiteboards', '{whiteboard}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, name, updated_by_actor_id, revision, updated_at, archived_at
    ON sheets FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_sheets', '{sheet}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, name, version, updated_at, archived_at
    ON collections FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_collections', '{collection}');
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, collection_id, title, updated_by_actor_id, version, updated_at, archived_at
    ON collection_records FOR EACH ROW
    EXECUTE FUNCTION things_sync('thing_source_collection_records', '{record}');

CREATE TABLE things_backfill (
    source TEXT PRIMARY KEY CHECK (source ~ '^thing_source_[a-z_]+$'),
    -- The last id copied: the next batch starts after it.
    last_id UUID,
    done_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
