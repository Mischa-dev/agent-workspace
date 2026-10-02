-- Editing and removing messages (docs/plans/mcp-tools.md, decision 3).
-- Phase 2 owns 0044, 0045 and 0046; Phase 3 uses 0047 (this), 0048 and 0049.
--
-- Messages stay immutable except through two commands. Each sets
-- aw.message_command to the one message it changes, for its transaction only.
-- An edit records the old and new versions in message_revisions first; a
-- removal redacts every revision first. Message and revision rows are never
-- deleted.

ALTER TABLE messages
    ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision BETWEEN 1 AND 1000),
    ADD COLUMN edited_at TIMESTAMPTZ,
    ADD COLUMN removed_at TIMESTAMPTZ,
    ADD COLUMN removed_by_actor_id UUID,
    ADD COLUMN removed_via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    ADD CONSTRAINT messages_removed_by_membership_fk
        FOREIGN KEY (project_id, removed_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT messages_edit_shape_check
        CHECK ((revision = 1) = (edited_at IS NULL)),
    ADD CONSTRAINT messages_removal_shape_check CHECK (
        (removed_at IS NULL AND removed_by_actor_id IS NULL
         AND removed_via_oauth_grant_id IS NULL)
        OR
        (removed_at IS NOT NULL AND removed_by_actor_id IS NOT NULL
         AND plain_text = ''
         AND content = '{"version": 1, "nodes": []}'::jsonb)
    );

CREATE TABLE message_revisions (
    project_id UUID NOT NULL,
    message_id UUID NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 1000),
    content JSONB CHECK (
        content IS NULL OR (
            jsonb_typeof(content) = 'object'
            AND content->>'version' = '1'
            AND jsonb_typeof(content->'nodes') = 'array'
        )
    ),
    plain_text TEXT CHECK (plain_text IS NULL OR char_length(plain_text) <= 200000),
    written_by_actor_id UUID NOT NULL,
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    written_at TIMESTAMPTZ NOT NULL,
    redacted_at TIMESTAMPTZ,
    redacted_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, revision),
    FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id),
    FOREIGN KEY (project_id, written_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (
        (redacted_at IS NULL AND redacted_by_actor_id IS NULL
         AND content IS NOT NULL AND plain_text IS NOT NULL)
        OR
        (redacted_at IS NOT NULL AND redacted_by_actor_id IS NOT NULL
         AND content IS NULL AND plain_text IS NULL)
    )
);

-- A revision is a snapshot of the current version (the first edit records
-- the original) or the next version, written only by the edit command.
CREATE FUNCTION validate_message_revision()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    current_revision INTEGER;
    current_content JSONB;
    current_plain_text TEXT;
    current_removed TIMESTAMPTZ;
BEGIN
    IF current_setting('aw.message_command', true) IS DISTINCT FROM NEW.message_id::text THEN
        RAISE EXCEPTION 'Message revisions are written only by the edit command'
            USING ERRCODE = '23514';
    END IF;
    SELECT revision, content, plain_text, removed_at
      INTO current_revision, current_content, current_plain_text, current_removed
      FROM messages
     WHERE id = NEW.message_id AND project_id = NEW.project_id
       FOR KEY SHARE;
    IF current_revision IS NULL OR current_removed IS NOT NULL THEN
        RAISE EXCEPTION 'Removed or missing Messages take no revisions'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.redacted_at IS NOT NULL THEN
        RAISE EXCEPTION 'New revisions cannot start redacted' USING ERRCODE = '23514';
    END IF;
    IF NOT ((NEW.revision = current_revision
             AND NEW.content = current_content
             AND NEW.plain_text = current_plain_text)
            OR NEW.revision = current_revision + 1) THEN
        RAISE EXCEPTION 'A revision is the current version or the next one'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER message_revisions_validate
BEFORE INSERT ON message_revisions
FOR EACH ROW EXECUTE FUNCTION validate_message_revision();

-- Revisions are kept forever; removal may only redact their text.
CREATE FUNCTION guard_message_revision_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Message revisions are kept' USING ERRCODE = '23514';
    END IF;
    IF current_setting('aw.message_command', true) IS DISTINCT FROM OLD.message_id::text
       OR OLD.redacted_at IS NOT NULL
       OR NEW.redacted_at IS NULL
       OR NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.message_id IS DISTINCT FROM OLD.message_id
       OR NEW.revision IS DISTINCT FROM OLD.revision
       OR NEW.written_by_actor_id IS DISTINCT FROM OLD.written_by_actor_id
       OR NEW.via_oauth_grant_id IS DISTINCT FROM OLD.via_oauth_grant_id
       OR NEW.written_at IS DISTINCT FROM OLD.written_at
       OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'A Message revision only changes by having its text removed'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER message_revisions_guard
BEFORE UPDATE OR DELETE ON message_revisions
FOR EACH ROW EXECUTE FUNCTION guard_message_revision_mutation();

-- Replaces 0005's blanket reject_message_mutation.
CREATE FUNCTION guard_message_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Messages are never deleted; remove them instead'
            USING ERRCODE = '23514';
    END IF;
    IF current_setting('aw.message_command', true) IS DISTINCT FROM OLD.id::text THEN
        RAISE EXCEPTION 'Sent Messages are immutable' USING ERRCODE = '23514';
    END IF;
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.author_actor_id IS DISTINCT FROM OLD.author_actor_id
       OR NEW.content_version IS DISTINCT FROM OLD.content_version
       OR NEW.agent_run_id IS DISTINCT FROM OLD.agent_run_id
       OR NEW.via_oauth_grant_id IS DISTINCT FROM OLD.via_oauth_grant_id
       OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'Only a Message''s text can change' USING ERRCODE = '23514';
    END IF;
    IF OLD.removed_at IS NOT NULL THEN
        RAISE EXCEPTION 'Removed Messages cannot change' USING ERRCODE = '23514';
    END IF;
    IF NEW.removed_at IS NOT NULL THEN
        IF NEW.revision IS DISTINCT FROM OLD.revision
           OR NEW.edited_at IS DISTINCT FROM OLD.edited_at THEN
            RAISE EXCEPTION 'Removing a Message does not edit it' USING ERRCODE = '23514';
        END IF;
        IF EXISTS (SELECT 1 FROM message_revisions
                   WHERE message_id = OLD.id AND redacted_at IS NULL) THEN
            RAISE EXCEPTION 'Redact every revision before removing a Message'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.revision IS DISTINCT FROM OLD.revision + 1 OR NEW.edited_at IS NULL THEN
        RAISE EXCEPTION 'An edit adds exactly one revision' USING ERRCODE = '23514';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM message_revisions r
                   WHERE r.message_id = OLD.id AND r.revision = OLD.revision
                     AND r.content = OLD.content AND r.plain_text = OLD.plain_text)
       OR NOT EXISTS (SELECT 1 FROM message_revisions r
                      WHERE r.message_id = NEW.id AND r.revision = NEW.revision
                        AND r.content = NEW.content AND r.plain_text = NEW.plain_text) THEN
        RAISE EXCEPTION 'An edit must keep both versions in message_revisions'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER messages_reject_mutation ON messages;
DROP FUNCTION reject_message_mutation();
CREATE TRIGGER messages_guard_mutation
BEFORE UPDATE OR DELETE ON messages
FOR EACH ROW EXECUTE FUNCTION guard_message_mutation();

-- Mentions and task references follow their Message's current content: they
-- are deleted only by that Message's own edit or removal, and never updated.
CREATE FUNCTION guard_message_projection_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('aw.message_command', true) = OLD.message_id::text THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'Message projections change only with their Message'
        USING ERRCODE = '23514';
END;
$$;

DROP TRIGGER message_mentions_reject_mutation ON message_mentions;
DROP FUNCTION reject_message_mention_mutation();
CREATE TRIGGER message_mentions_guard_mutation
BEFORE UPDATE OR DELETE ON message_mentions
FOR EACH ROW EXECUTE FUNCTION guard_message_projection_mutation();

-- reject_work_item_projection_mutation stays in use for the two comment tables.
DROP TRIGGER message_work_item_references_reject_mutation ON message_work_item_references;
CREATE TRIGGER message_work_item_references_guard_mutation
BEFORE UPDATE OR DELETE ON message_work_item_references
FOR EACH ROW EXECUTE FUNCTION guard_message_projection_mutation();
