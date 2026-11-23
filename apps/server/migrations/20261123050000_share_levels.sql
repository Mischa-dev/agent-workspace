-- C1.27: a page shared with one person can let them comment on it or edit
-- it too ('comment' includes reading, 'edit' includes commenting). A link
-- only ever opens the page to read.
ALTER TABLE thing_grants DROP CONSTRAINT thing_grants_level_check;
ALTER TABLE thing_grants
    ADD CONSTRAINT thing_grants_level_check CHECK (level IN ('see', 'comment', 'edit')),
    ADD CONSTRAINT thing_grants_links_only_see CHECK (link_digest IS NULL OR level = 'see');

-- Whether a page was shared with `viewer` on its own for commenting (or
-- editing), while they are still an active member of its workspace.
CREATE FUNCTION page_shared_for_comment(target_page UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM thing_grants grant_row
        JOIN organization_memberships membership
          ON membership.organization_id = grant_row.workspace_id
         AND membership.actor_id = grant_row.grantee_account_id
         AND membership.enabled
        JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
        WHERE grant_row.thing_kind = 'page' AND grant_row.thing_id = target_page
          AND grant_row.grantee_account_id = viewer
          AND grant_row.level IN ('comment', 'edit')
          AND grant_row.revoked_at IS NULL
          AND (grant_row.expires_at IS NULL OR grant_row.expires_at > now()))
$$;

-- A page's comment threads are conversations about it: someone it was
-- shared with for commenting reads and writes them too. A conversation's
-- memory page follows its conversation and is never shared.
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
                       AND page_shared_for_comment(page.id, viewer))))
        WHEN 'sheet' THEN EXISTS (
            SELECT 1 FROM sheets sheet
            WHERE sheet.id = target_id
              AND subject_space_member(viewer, sheet.project_id))
        WHEN 'record' THEN EXISTS (
            SELECT 1 FROM collection_records record
            JOIN collections collection ON collection.id = record.collection_id
            WHERE record.id = target_id
              AND subject_space_member(viewer, record.project_id)
              AND (NOT collection.acts_like_task
                   OR subject_sees_tasks(viewer, record.project_id)))
        ELSE false
    END
$$;

-- Whether a conversation is a page's comments (or a thread in them) whose
-- page was shared with `viewer` for commenting: they post there though they
-- aren't in its space.
CREATE FUNCTION page_discussion_shared(target_conversation UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = target_conversation
          AND ((conversation.kind = 'comments' AND conversation.subject_kind = 'page'
                AND page_shared_for_comment(conversation.subject_id, viewer))
               OR (parent.kind = 'comments' AND parent.subject_kind = 'page'
                   AND page_shared_for_comment(parent.subject_id, viewer))))
$$;

-- Someone a page was shared with to comment keeps a read state in its
-- comments like anyone who reads them.
CREATE OR REPLACE FUNCTION validate_conversation_read_state()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    next_sequence BIGINT;
    reader_enabled BOOLEAN;
    conversation_project UUID;
BEGIN
    SELECT conversation.next_entry_sequence, conversation.project_id
      INTO next_sequence, conversation_project
      FROM conversations conversation
     WHERE conversation.id = NEW.conversation_id
       AND conversation.project_id IS NOT DISTINCT FROM NEW.project_id
       FOR KEY SHARE OF conversation;

    IF conversation_project IS NULL THEN
        -- A DM, a group or a thread under one: its participants, active.
        SELECT actor.status = 'active'
               AND conversation_member_visible(NEW.conversation_id, NEW.actor_id)
          INTO reader_enabled
          FROM actors actor
         WHERE actor.id = NEW.actor_id
           FOR KEY SHARE OF actor;
    ELSE
        SELECT membership.enabled AND actor.status = 'active'
          INTO reader_enabled
          FROM project_memberships membership
          JOIN actors actor ON actor.id = membership.actor_id
         WHERE membership.project_id = conversation_project
           AND membership.actor_id = NEW.actor_id
           FOR KEY SHARE OF membership, actor;
        -- C1.27: a page's comments, read by someone the page was shared
        -- with to comment, who isn't in its space.
        IF reader_enabled IS NULL THEN
            SELECT actor.status = 'active'
                   AND page_discussion_shared(NEW.conversation_id, NEW.actor_id)
              INTO reader_enabled
              FROM actors actor
             WHERE actor.id = NEW.actor_id
               FOR KEY SHARE OF actor;
        END IF;
    END IF;

    IF next_sequence IS NULL OR reader_enabled IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Read state requires an enabled same-Project membership'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.highest_seen_sequence >= next_sequence THEN
        RAISE EXCEPTION 'Read state cannot advance beyond durable Conversation entries'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE' THEN
        IF NEW.highest_seen_sequence < OLD.highest_seen_sequence THEN
            IF current_setting('aw.read_rewind', true) IS DISTINCT FROM
               (NEW.conversation_id::text || ':' || NEW.actor_id::text)
               OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
               OR NEW.actor_id IS DISTINCT FROM OLD.actor_id THEN
                RAISE EXCEPTION 'Conversation read state cannot move backwards'
                    USING ERRCODE = '23514';
            END IF;
            IF NEW.version <= OLD.version THEN
                RAISE EXCEPTION 'Rewinding read state must advance its version'
                    USING ERRCODE = '23514';
            END IF;
        END IF;
        IF NEW.highest_seen_sequence > OLD.highest_seen_sequence
           AND NEW.version <= OLD.version THEN
            RAISE EXCEPTION 'Advancing read state must advance its version'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.highest_seen_sequence = OLD.highest_seen_sequence
           AND NEW.version IS DISTINCT FROM OLD.version THEN
            RAISE EXCEPTION 'Idempotent read state cannot churn its version'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

