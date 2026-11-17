-- C1.3: the discussion foundation. Expand only: nothing moves, and nothing
-- writes a comments conversation yet except comment_create for sheets,
-- whiteboards and records.
--
-- A thing's comments are a conversation of the new kind `comments`, one per
-- thing (its subject), made by the first comment. Anchored discussions are
-- threads under it, carrying an anchor (a quote in a page, a cell or range
-- in a sheet, elements on a board, a field of a record or task).

-- 1. The comments kind, its subject and anchors. Kind names follow R54's
-- grammar (K3's ThingKind).
ALTER TABLE conversations
    DROP CONSTRAINT conversations_kind_check,
    DROP CONSTRAINT conversations_kind_shape_check,
    DROP CONSTRAINT conversations_visibility_check;

ALTER TABLE conversations
    ADD COLUMN subject_kind TEXT
        CHECK (subject_kind ~ '^[a-z][a-z0-9_]{1,39}(\.[a-z][a-z0-9_]{1,39})?$'),
    ADD COLUMN subject_id UUID,
    -- Only on threads under a comments conversation; its shape is per kind
    -- (discussion/anchors.rs).
    ADD COLUMN anchor JSONB
        CHECK (anchor IS NULL OR (jsonb_typeof(anchor) = 'object'
                                  AND octet_length(anchor::text) <= 4096)),
    ADD CONSTRAINT conversations_kind_check
        CHECK (kind IN ('channel', 'thread', 'dm', 'group', 'comments')),
    ADD CONSTRAINT conversations_visibility_check
        CHECK (visibility IN ('project', 'members', 'subject')),
    ADD CONSTRAINT conversations_kind_shape_check CHECK (
        (
            kind = 'channel'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND dm_key IS NULL
            AND visibility <> 'subject'
            AND subject_kind IS NULL AND subject_id IS NULL AND anchor IS NULL
        )
        OR
        (
            kind = 'thread'
            AND parent_channel_id IS NOT NULL
            AND dm_key IS NULL
            AND visibility <> 'subject'
            AND subject_kind IS NULL AND subject_id IS NULL
        )
        OR
        (
            kind = 'dm'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND visibility = 'members'
            AND dm_key IS NOT NULL
            AND subject_kind IS NULL AND subject_id IS NULL AND anchor IS NULL
        )
        OR
        (
            kind = 'group'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND visibility = 'members'
            AND dm_key IS NULL
            AND subject_kind IS NULL AND subject_id IS NULL AND anchor IS NULL
        )
        OR
        (
            kind = 'comments'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND dm_key IS NULL
            AND visibility = 'subject'
            AND subject_kind IS NOT NULL AND subject_id IS NOT NULL AND anchor IS NULL
        )
    );

CREATE UNIQUE INDEX conversations_one_comments_per_subject
    ON conversations (subject_kind, subject_id)
    WHERE kind = 'comments';

CREATE INDEX conversations_anchored_threads
    ON conversations (parent_channel_id, created_at)
    WHERE anchor IS NOT NULL;

-- 2. Ruling R3: conversation_visible_to is split into named helpers, each
-- owned by one workstream, without changing behaviour. Its body is K1's;
-- conversation_member_visible is K4's (today's rule, moved unchanged);
-- subject_visible_to is C1's. A later migration replaces only the helper it
-- owns, never conversation_visible_to itself.

-- K4's helper: today's rule for project-wide and members-only conversations.
-- `target` is the conversation whose member list decides (a thread's parent).
CREATE FUNCTION conversation_member_visible(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversation_members member
        WHERE member.conversation_id = target
          AND member.actor_id = viewer
    )
$$;

-- C1's helper: may `viewer` see the thing a comments conversation is about?
-- It checks membership itself (unlike the 'project' arm, which callers pair
-- with a membership check), so a comment is never wider than its thing. K1
-- replaces this body with its generated "may see" filter; K3's thing
-- registry absorbs the kind list (the static table in discussion/subjects.rs
-- names the same kinds).
-- A member of the space a thing lives in, whatever kind of account: the
-- membership and the workspace membership enabled, the account active.
-- (actor_can_access_project without a connection also wants a person's
-- sign-in, which an agent hasn't got.)
CREATE FUNCTION subject_space_member(viewer UUID, space UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = viewer
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = viewer
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = viewer AND actor.status = 'active'
        WHERE project.id = space AND project.archived_at IS NULL
    )
$$;

-- Tasks, and records of collections that act like tasks, also want the
-- viewer's role there to allow seeing tasks (the same catalogue the access
-- module reads).
CREATE FUNCTION subject_sees_tasks(viewer UUID, space UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM project_memberships membership
        WHERE membership.project_id = space
          AND membership.actor_id = viewer
          AND membership.enabled
          AND role_allows(membership.role, membership.role_id, 'tasks.view')
    )
$$;

CREATE FUNCTION subject_visible_to(target_kind TEXT, target_id UUID, viewer UUID)
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
              AND subject_space_member(viewer, page.project_id))
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

-- Where a thing opens in the web app, for links in notifications and agent
-- events (ids only, never text). C1's until K3's links module builds
-- addresses; the web builds the same paths (projects/locations.ts).
CREATE FUNCTION subject_href(target_kind TEXT, target_id UUID)
RETURNS TEXT
LANGUAGE sql
STABLE
AS $$
    SELECT CASE target_kind
        WHEN 'whiteboard' THEN '/whiteboards/' || target_id::text
        WHEN 'task' THEN (
            SELECT '/projects/' || item.project_id::text || '/work-items/' || item.id::text
            FROM work_items item WHERE item.id = target_id)
        WHEN 'page' THEN (
            SELECT '/projects/' || page.project_id::text || '/pages/' || page.id::text
            FROM project_pages page WHERE page.id = target_id)
        WHEN 'sheet' THEN (
            SELECT '/projects/' || sheet.project_id::text || '/sheets/' || sheet.id::text
            FROM sheets sheet WHERE sheet.id = target_id)
        WHEN 'record' THEN (
            SELECT '/projects/' || record.project_id::text || '/collections/'
                   || record.collection_id::text || '?record=' || record.id::text
            FROM collection_records record WHERE record.id = target_id)
    END
$$;

-- K1's body: the composition. The CASE keeps the planner from evaluating
-- the subject arm for ordinary conversations.
CREATE OR REPLACE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = target
          AND CASE COALESCE(parent.visibility, conversation.visibility)
                WHEN 'project' THEN true
                WHEN 'members' THEN conversation_member_visible(
                    COALESCE(conversation.parent_channel_id, conversation.id), viewer)
                WHEN 'subject' THEN subject_visible_to(
                    COALESCE(parent.subject_kind, conversation.subject_kind),
                    COALESCE(parent.subject_id, conversation.subject_id),
                    viewer)
                ELSE false
              END
    )
$$;

-- 3. Same rules as 0042, except a thread's parent may also be a comments
-- conversation, and only a thread under one carries an anchor. The subject
-- and anchor are immutable once set, like the parent.
CREATE OR REPLACE FUNCTION validate_conversation_parent()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    parent_kind TEXT;
    parent_project_id UUID;
    parent_archived_at TIMESTAMPTZ;
    origin_conversation_id UUID;
    origin_project_id UUID;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.kind IS DISTINCT FROM OLD.kind THEN
            RAISE EXCEPTION 'Conversation kind is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.project_id IS DISTINCT FROM OLD.project_id THEN
            RAISE EXCEPTION 'Conversation Project is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.parent_channel_id IS DISTINCT FROM OLD.parent_channel_id THEN
            RAISE EXCEPTION 'Thread parent Channel is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.origin_message_id IS DISTINCT FROM OLD.origin_message_id THEN
            RAISE EXCEPTION 'Thread origin Message is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.subject_kind IS DISTINCT FROM OLD.subject_kind
           OR NEW.subject_id IS DISTINCT FROM OLD.subject_id THEN
            RAISE EXCEPTION 'Comments subject is immutable'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    IF NEW.kind <> 'thread' THEN
        RETURN NEW;
    END IF;

    SELECT parent.kind, parent.project_id, parent.archived_at
      INTO parent_kind, parent_project_id, parent_archived_at
      FROM conversations parent
     WHERE parent.id = NEW.parent_channel_id
       FOR SHARE;

    IF parent_kind IS NULL
       OR parent_kind NOT IN ('channel', 'dm', 'group', 'comments')
       OR parent_project_id IS DISTINCT FROM NEW.project_id THEN
        RAISE EXCEPTION 'Thread parent must be a Channel, DM, group, or comments in the same Project'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.anchor IS NOT NULL AND parent_kind <> 'comments' THEN
        RAISE EXCEPTION 'Only a thread under comments carries an anchor'
            USING ERRCODE = '23514';
    END IF;
    IF parent_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Thread parent Conversation is archived'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.origin_message_id IS NOT NULL THEN
        SELECT message.conversation_id, message.project_id
          INTO origin_conversation_id, origin_project_id
          FROM messages message
         WHERE message.id = NEW.origin_message_id
           FOR SHARE;

        IF origin_conversation_id IS DISTINCT FROM NEW.parent_channel_id
           OR origin_project_id IS DISTINCT FROM NEW.project_id THEN
            RAISE EXCEPTION 'Thread origin Message must belong to its parent Channel'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;
