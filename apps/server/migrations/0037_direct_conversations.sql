-- Direct messages and group conversations.
--
-- Each Organization gets one hidden "direct" Project that holds its DMs and
-- groups. Reusing a Project row keeps every existing project-scoped
-- constraint, outbox, realtime, and policy path working unchanged. The web
-- client never lists this Project with ordinary Projects.
--
-- Conversations gain a visibility. "project" keeps today's behavior (every
-- Project member can see it). "members" limits it to rows in
-- conversation_members. Threads follow their parent's visibility.

ALTER TABLE projects
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'standard'
        CHECK (kind IN ('standard', 'direct'));

CREATE UNIQUE INDEX projects_one_direct_space_per_organization
    ON projects (organization_id)
    WHERE kind = 'direct';

ALTER TABLE conversations
    DROP CONSTRAINT conversations_kind_check,
    DROP CONSTRAINT conversations_kind_shape_check;

ALTER TABLE conversations
    ADD COLUMN visibility TEXT NOT NULL DEFAULT 'project'
        CHECK (visibility IN ('project', 'members')),
    -- Sorted "actorA:actorB" pair so one DM exists per pair of Actors.
    ADD COLUMN dm_key TEXT,
    ADD CONSTRAINT conversations_kind_check
        CHECK (kind IN ('channel', 'thread', 'dm', 'group')),
    ADD CONSTRAINT conversations_kind_shape_check CHECK (
        (
            kind = 'channel'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND dm_key IS NULL
        )
        OR
        (
            kind = 'thread'
            AND parent_channel_id IS NOT NULL
            AND dm_key IS NULL
        )
        OR
        (
            kind = 'dm'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND visibility = 'members'
            AND dm_key IS NOT NULL
        )
        OR
        (
            kind = 'group'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
            AND visibility = 'members'
            AND dm_key IS NULL
        )
    );

-- Same rules as 0008, except only Threads need a parent. The previous version
-- returned early only for Channels, so DMs and groups would have failed the
-- parent check.
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
    END IF;

    IF NEW.kind <> 'thread' THEN
        RETURN NEW;
    END IF;

    SELECT parent.kind, parent.project_id, parent.archived_at
      INTO parent_kind, parent_project_id, parent_archived_at
      FROM conversations parent
     WHERE parent.id = NEW.parent_channel_id
       FOR SHARE;

    IF parent_kind IS DISTINCT FROM 'channel'
       OR parent_project_id IS DISTINCT FROM NEW.project_id THEN
        RAISE EXCEPTION 'Thread parent must be a Channel in the same Project'
            USING ERRCODE = '23514';
    END IF;
    IF parent_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Thread parent Channel is archived'
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

CREATE UNIQUE INDEX conversations_dm_pair_unique
    ON conversations (project_id, dm_key)
    WHERE kind = 'dm';

CREATE TABLE conversation_members (
    project_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    added_by_actor_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, actor_id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, added_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX conversation_members_actor_idx
    ON conversation_members (actor_id, conversation_id);

-- The one visibility rule. Every conversation access query calls this so the
-- rule cannot drift between HTTP, MCP, and realtime paths.
CREATE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = target
          AND (
              COALESCE(parent.visibility, conversation.visibility) = 'project'
              OR EXISTS (
                  SELECT 1
                  FROM conversation_members member
                  WHERE member.conversation_id =
                        COALESCE(conversation.parent_channel_id, conversation.id)
                    AND member.actor_id = viewer
              )
          )
    )
$$;

-- Agent connections were limited to one pinned Hermes release. Any runtime
-- that speaks the MCP contract may now connect; the label is informational.
ALTER TABLE external_agent_connections
    DROP CONSTRAINT external_agent_connections_runtime_kind_check,
    DROP CONSTRAINT external_agent_connections_runtime_version_check,
    ADD CONSTRAINT external_agent_connections_runtime_kind_check
        CHECK (runtime_kind ~ '^[a-z][a-z0-9_-]{0,31}$'),
    ADD CONSTRAINT external_agent_connections_runtime_version_check
        CHECK (char_length(runtime_version) BETWEEN 1 AND 40);
