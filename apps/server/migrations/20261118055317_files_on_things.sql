-- C1.33 (lane D2, documents): one attachment model. A file belongs to a
-- conversation (a message's file), a task (its own files, kept as
-- work_item_id), or now any other thing, such as a picture pasted into a
-- page. A thing's file is attached at once and is read by whoever can see
-- the thing (`subject_visible_to`), checked on every read.
--
-- `purpose` says what the file is for the thing (an attachment, a picture
-- shown inline, a render); `external_key` is the thing's own name for the
-- file, such as a board's file id, unique per thing. `preview_text` holds
-- the start of a text or code file, to show before it is opened.

ALTER TABLE files
    ADD COLUMN thing_kind TEXT CHECK (thing_kind ~ '^[a-z][a-z_]{1,31}$'),
    ADD COLUMN thing_id UUID,
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'attachment'
        CHECK (purpose IN ('attachment', 'inline', 'cover', 'render', 'import', 'export')),
    ADD COLUMN external_key TEXT
        CHECK (external_key IS NULL OR external_key ~ '^[A-Za-z0-9_-]{1,100}$'),
    ADD COLUMN preview_text TEXT CHECK (char_length(preview_text) <= 2000),
    ADD CONSTRAINT files_thing_pair CHECK ((thing_kind IS NULL) = (thing_id IS NULL)),
    -- A thing's file is attached when it is uploaded.
    ADD CONSTRAINT files_thing_attached CHECK (thing_id IS NULL OR attached_at IS NOT NULL);

-- Exactly one owner: a conversation, a task or another thing.
ALTER TABLE files DROP CONSTRAINT files_check;
ALTER TABLE files ADD CONSTRAINT files_one_owner
    CHECK (num_nonnulls(conversation_id, work_item_id, thing_id) = 1);

CREATE INDEX files_thing ON files (thing_kind, thing_id, created_at)
    WHERE thing_id IS NOT NULL;
CREATE UNIQUE INDEX files_thing_external ON files (thing_kind, thing_id, external_key)
    WHERE external_key IS NOT NULL AND removed_at IS NULL;

-- A thing's file sits under its thing in the things mirror, and its events
-- name the thing.
CREATE OR REPLACE VIEW thing_source_files AS
SELECT file.id, 'file'::text AS kind, file.organization_id AS workspace_id,
       file.project_id AS space_id,
       thing_if_registered(COALESCE(file.conversation_id, file.work_item_id, file.thing_id)) AS parent_id,
       thing_if_registered(COALESCE(file.conversation_id, file.work_item_id, file.thing_id)) AS access_parent_id,
       left(file.name, 300) AS title, NULL::text AS status,
       file.uploaded_by_actor_id AS owner_account_id,
       file.uploaded_by_actor_id AS created_by_account_id,
       file.removed_by_actor_id AS updated_by_account_id, 1::bigint AS version,
       file.created_at, COALESCE(file.removed_at, file.attached_at, file.created_at) AS updated_at,
       NULL::timestamptz AS archived_at, file.removed_at AS deleted_at
FROM files file;

CREATE OR REPLACE FUNCTION events_from_files()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (OLD.removed_at IS NOT NULL OR NEW.removed_at IS NULL) THEN
        RETURN NULL;
    END IF;
    PERFORM stage_event(
        CASE WHEN TG_OP = 'INSERT' THEN 'file.attached' ELSE 'file.removed' END,
        'fact', NEW.project_id, 'file', NEW.id,
        CASE WHEN NEW.conversation_id IS NULL THEN 'space' ELSE 'conversation' END,
        COALESCE(NEW.removed_by_actor_id, NEW.uploaded_by_actor_id), NEW.conversation_id,
        NULL, COALESCE(NEW.conversation_id, NEW.work_item_id, NEW.thing_id), NULL, '{}',
        jsonb_strip_nulls(jsonb_build_object('workItemId', NEW.work_item_id,
                                             'messageId', NEW.message_id,
                                             'thingKind', NEW.thing_kind,
                                             'thingId', NEW.thing_id)));
    RETURN NULL;
END $$;
