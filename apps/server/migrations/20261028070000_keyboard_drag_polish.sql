-- Keyboard, dragging and the polish leftovers (brief 2026-10-28-keyboard-drag-polish):
-- the project rail in each person's own order, more places a project
-- remembers per person, threads named by their first message until someone
-- names them, editing and deleting task comments, hiding agent profiles and
-- run accounts that past runs still point at, and deleting trashed
-- whiteboards for good.

-- The rail: pinned projects in the order each person dragged them into.
-- Null sorts after every placed project, by when it was pinned.
ALTER TABLE project_navigation_states
    ADD COLUMN rail_position INTEGER CHECK (rail_position BETWEEN 0 AND 9999),
    ADD COLUMN page_id UUID,
    ADD CONSTRAINT project_navigation_states_page_fk
        FOREIGN KEY (project_id, page_id) REFERENCES project_pages(project_id, id),
    DROP CONSTRAINT project_navigation_states_location_kind_check,
    DROP CONSTRAINT project_navigation_states_location_shape_check;

-- More places to come back to: each task tab, the pages (or one page), the
-- decisions and the runs.
ALTER TABLE project_navigation_states
    ADD CONSTRAINT project_navigation_states_location_kind_check CHECK (
        location_kind IN (
            'project_overview', 'channel', 'thread', 'work_items', 'settings',
            'milestones', 'timeline', 'planning', 'pages', 'page', 'decisions', 'runs'
        )
    ),
    ADD CONSTRAINT project_navigation_states_location_shape_check CHECK (
        (
            location_kind IN ('channel', 'thread')
            AND conversation_id IS NOT NULL
            AND page_id IS NULL
        )
        OR
        (
            location_kind = 'page'
            AND page_id IS NOT NULL
            AND conversation_id IS NULL
        )
        OR
        (
            location_kind IN (
                'project_overview', 'work_items', 'settings', 'milestones',
                'timeline', 'planning', 'pages', 'decisions', 'runs'
            )
            AND conversation_id IS NULL
            AND page_id IS NULL
        )
    );

-- A thread started without a name goes by its first message (or the message
-- it was started from) until someone names it. Those names needn't be unique.
ALTER TABLE conversations
    ADD COLUMN auto_named BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT conversations_auto_named_thread_check
        CHECK (NOT auto_named OR kind = 'thread');

DROP INDEX conversations_active_thread_name_unique;
CREATE UNIQUE INDEX conversations_active_thread_name_unique
    ON conversations (parent_channel_id, lower(name))
    WHERE kind = 'thread' AND archived_at IS NULL AND NOT auto_named;

-- Task comments can be edited and deleted, the way messages are (0047).
-- Comments stay immutable except through two commands, each of which sets
-- aw.comment_command to the one comment it changes, for its transaction only.
-- An edit keeps the old and new versions in work_item_comment_revisions; a
-- deletion blanks the comment and every revision's text but keeps its place
-- in the task's history, with who deleted it and when.
ALTER TABLE work_item_comments
    DROP CONSTRAINT work_item_comments_plain_text_check,
    ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision BETWEEN 1 AND 1000),
    ADD COLUMN edited_at TIMESTAMPTZ,
    ADD COLUMN removed_at TIMESTAMPTZ,
    ADD COLUMN removed_by_actor_id UUID,
    ADD CONSTRAINT work_item_comments_removed_by_membership_fk
        FOREIGN KEY (project_id, removed_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT work_item_comments_plain_text_check CHECK (
        char_length(plain_text) <= 20000
        AND (removed_at IS NOT NULL OR char_length(plain_text) >= 1)
    ),
    ADD CONSTRAINT work_item_comments_edit_shape_check
        CHECK ((revision = 1) = (edited_at IS NULL)),
    ADD CONSTRAINT work_item_comments_removal_shape_check CHECK (
        (removed_at IS NULL AND removed_by_actor_id IS NULL)
        OR
        (removed_at IS NOT NULL AND removed_by_actor_id IS NOT NULL
         AND plain_text = ''
         AND content = '{"version": 1, "nodes": []}'::jsonb)
    );

CREATE TABLE work_item_comment_revisions (
    project_id UUID NOT NULL,
    work_item_id UUID NOT NULL,
    comment_id UUID NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 1000),
    content JSONB CHECK (
        content IS NULL OR (
            jsonb_typeof(content) = 'object'
            AND content->>'version' = '1'
            AND jsonb_typeof(content->'nodes') = 'array'
        )
    ),
    plain_text TEXT CHECK (plain_text IS NULL OR char_length(plain_text) <= 20000),
    written_by_actor_id UUID NOT NULL,
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    written_at TIMESTAMPTZ NOT NULL,
    redacted_at TIMESTAMPTZ,
    redacted_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (comment_id, revision),
    FOREIGN KEY (project_id, work_item_id, comment_id)
        REFERENCES work_item_comments(project_id, work_item_id, id),
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
-- the original) or the next one, written only by the edit command.
CREATE FUNCTION validate_work_item_comment_revision()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    current_revision INTEGER;
    current_content JSONB;
    current_plain_text TEXT;
    current_removed TIMESTAMPTZ;
BEGIN
    IF current_setting('aw.comment_command', true) IS DISTINCT FROM NEW.comment_id::text THEN
        RAISE EXCEPTION 'Task comment revisions are written only by the edit command'
            USING ERRCODE = '23514';
    END IF;
    SELECT revision, content, plain_text, removed_at
      INTO current_revision, current_content, current_plain_text, current_removed
      FROM work_item_comments
     WHERE id = NEW.comment_id AND project_id = NEW.project_id
       AND work_item_id = NEW.work_item_id
       FOR KEY SHARE;
    IF current_revision IS NULL OR current_removed IS NOT NULL THEN
        RAISE EXCEPTION 'Deleted or missing task comments take no revisions'
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

CREATE TRIGGER work_item_comment_revisions_validate
BEFORE INSERT ON work_item_comment_revisions
FOR EACH ROW EXECUTE FUNCTION validate_work_item_comment_revision();

-- Revisions are kept; deleting the comment may only blank their text.
CREATE FUNCTION guard_work_item_comment_revision_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Task comment revisions are kept' USING ERRCODE = '23514';
    END IF;
    IF current_setting('aw.comment_command', true) IS DISTINCT FROM OLD.comment_id::text
       OR OLD.redacted_at IS NOT NULL
       OR NEW.redacted_at IS NULL
       OR NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.work_item_id IS DISTINCT FROM OLD.work_item_id
       OR NEW.comment_id IS DISTINCT FROM OLD.comment_id
       OR NEW.revision IS DISTINCT FROM OLD.revision
       OR NEW.written_by_actor_id IS DISTINCT FROM OLD.written_by_actor_id
       OR NEW.via_oauth_grant_id IS DISTINCT FROM OLD.via_oauth_grant_id
       OR NEW.written_at IS DISTINCT FROM OLD.written_at
       OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'A task comment revision only changes by having its text removed'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_item_comment_revisions_guard
BEFORE UPDATE OR DELETE ON work_item_comment_revisions
FOR EACH ROW EXECUTE FUNCTION guard_work_item_comment_revision_mutation();

-- Replaces 0017's blanket reject_work_item_comment_mutation.
CREATE FUNCTION guard_work_item_comment_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Task comments are never deleted from the table; delete them with the command'
            USING ERRCODE = '23514';
    END IF;
    IF current_setting('aw.comment_command', true) IS DISTINCT FROM OLD.id::text THEN
        RAISE EXCEPTION 'Work Item comments are immutable' USING ERRCODE = '23514';
    END IF;
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.work_item_id IS DISTINCT FROM OLD.work_item_id
       OR NEW.author_actor_id IS DISTINCT FROM OLD.author_actor_id
       OR NEW.content_version IS DISTINCT FROM OLD.content_version
       OR NEW.via_oauth_grant_id IS DISTINCT FROM OLD.via_oauth_grant_id
       OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'Only a task comment''s text can change' USING ERRCODE = '23514';
    END IF;
    IF OLD.removed_at IS NOT NULL THEN
        RAISE EXCEPTION 'Deleted task comments cannot change' USING ERRCODE = '23514';
    END IF;
    IF NEW.removed_at IS NOT NULL THEN
        IF NEW.revision IS DISTINCT FROM OLD.revision
           OR NEW.edited_at IS DISTINCT FROM OLD.edited_at THEN
            RAISE EXCEPTION 'Deleting a task comment does not edit it' USING ERRCODE = '23514';
        END IF;
        IF EXISTS (SELECT 1 FROM work_item_comment_revisions
                   WHERE comment_id = OLD.id AND redacted_at IS NULL) THEN
            RAISE EXCEPTION 'Blank every revision before deleting a task comment'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.revision IS DISTINCT FROM OLD.revision + 1 OR NEW.edited_at IS NULL THEN
        RAISE EXCEPTION 'An edit adds exactly one revision' USING ERRCODE = '23514';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM work_item_comment_revisions r
                   WHERE r.comment_id = OLD.id AND r.revision = OLD.revision
                     AND r.content = OLD.content AND r.plain_text = OLD.plain_text)
       OR NOT EXISTS (SELECT 1 FROM work_item_comment_revisions r
                      WHERE r.comment_id = NEW.id AND r.revision = NEW.revision
                        AND r.content = NEW.content AND r.plain_text = NEW.plain_text) THEN
        RAISE EXCEPTION 'An edit must keep both versions in work_item_comment_revisions'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER work_item_comments_reject_mutation ON work_item_comments;
DROP FUNCTION reject_work_item_comment_mutation();
CREATE TRIGGER work_item_comments_guard_mutation
BEFORE UPDATE OR DELETE ON work_item_comments
FOR EACH ROW EXECUTE FUNCTION guard_work_item_comment_mutation();

-- A comment's mentions and task references follow its current text: they are
-- deleted only by that comment's own edit or deletion, and never updated.
CREATE FUNCTION guard_work_item_comment_projection_mutation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('aw.comment_command', true) = OLD.comment_id::text THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'Task comment projections change only with their comment'
        USING ERRCODE = '23514';
END;
$$;

DROP TRIGGER work_item_comment_mentions_reject_mutation ON work_item_comment_mentions;
CREATE TRIGGER work_item_comment_mentions_guard_mutation
BEFORE UPDATE OR DELETE ON work_item_comment_mentions
FOR EACH ROW EXECUTE FUNCTION guard_work_item_comment_projection_mutation();

DROP TRIGGER work_item_comment_references_reject_mutation ON work_item_comment_references;
CREATE TRIGGER work_item_comment_references_guard_mutation
BEFORE UPDATE OR DELETE ON work_item_comment_references
FOR EACH ROW EXECUTE FUNCTION guard_work_item_comment_projection_mutation();

-- An agent profile or a run account that past runs point at is hidden
-- instead of deleted, so those runs still say what ran them.
ALTER TABLE agent_profiles ADD COLUMN hidden_at TIMESTAMPTZ;
ALTER TABLE provider_accounts ADD COLUMN hidden_at TIMESTAMPTZ;

-- A trashed whiteboard can be deleted for good; copies made from it keep
-- their content and lose only the link back.
ALTER TABLE whiteboards
    DROP CONSTRAINT whiteboards_duplicated_from_id_fkey,
    ADD CONSTRAINT whiteboards_duplicated_from_id_fkey
        FOREIGN KEY (duplicated_from_id) REFERENCES whiteboards(id) ON DELETE SET NULL;
