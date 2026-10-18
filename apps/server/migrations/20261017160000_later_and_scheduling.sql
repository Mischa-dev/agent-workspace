-- W21: Later, reminders and scheduling.
--
-- Everything here acts later on someone's behalf or is read back by one
-- person, so each row names its actor, and every read and every job that
-- fires rechecks that actor's current access (principle 9): a saved message
-- from a conversation they've left stops showing, and a reminder or a
-- scheduled send for one does nothing.

-- Saved for later: a message, a task, or a reminder with only a note. Each
-- may carry a time to be reminded at; a reminder is a notification through
-- the inbox, push and email paths.
CREATE TABLE saved_items (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Where its notification is filed: the message's or task's project, or
    -- the workspace's direct space for a bare reminder.
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'work_item', 'reminder')),
    message_id UUID REFERENCES messages(id) ON DELETE CASCADE,
    work_item_id UUID REFERENCES work_items(id) ON DELETE CASCADE,
    note TEXT NOT NULL DEFAULT '' CHECK (char_length(note) <= 1000),
    remind_at TIMESTAMPTZ,
    -- The remind_at that was last delivered, so a retried job reminds once.
    reminded_for TIMESTAMPTZ,
    done_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((kind = 'message') = (message_id IS NOT NULL)),
    CHECK ((kind = 'work_item') = (work_item_id IS NOT NULL)),
    CHECK (kind <> 'reminder' OR char_length(btrim(note)) >= 1)
);

CREATE UNIQUE INDEX saved_items_one_per_message
    ON saved_items (actor_id, message_id) WHERE message_id IS NOT NULL;
CREATE UNIQUE INDEX saved_items_one_per_work_item
    ON saved_items (actor_id, work_item_id) WHERE work_item_id IS NOT NULL;
CREATE INDEX saved_items_by_actor ON saved_items (actor_id, created_at DESC);

-- Messages to send later, as their author, through the job queue. The
-- content is checked when scheduled and resolved again when it sends, so
-- mentions and reply rules apply as if it were sent then.
CREATE TABLE scheduled_messages (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    author_actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- An agent's: the connection it scheduled through. When it sends, a
    -- current credential of that connection must still allow it.
    external_agent_connection_id UUID REFERENCES external_agent_connections(id) ON DELETE CASCADE,
    -- A person's connected app it was scheduled through, shown as "via".
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) ON DELETE SET NULL,
    content JSONB NOT NULL,
    plain_text TEXT NOT NULL,
    -- Who and what it mentioned when last saved, to edit it with; sending
    -- resolves the content again.
    mentioned_actors JSONB NOT NULL DEFAULT '[]',
    referenced_work_items JSONB NOT NULL DEFAULT '[]',
    send_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'scheduled'
        CHECK (status IN ('scheduled', 'sent', 'failed', 'canceled')),
    message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    failure TEXT CHECK (char_length(failure) <= 300),
    sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((status = 'sent') = (sent_at IS NOT NULL))
);

CREATE INDEX scheduled_messages_by_author
    ON scheduled_messages (author_actor_id, status, send_at);

-- How much one person hears from one conversation: every message, only
-- mentions (the default), or nothing. A thread without its own level
-- follows its channel's. Project mutes still apply on top.
CREATE TABLE conversation_notification_levels (
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    level TEXT NOT NULL CHECK (level IN ('all', 'mentions', 'nothing')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_id, conversation_id)
);

CREATE INDEX conversation_notification_levels_following
    ON conversation_notification_levels (conversation_id) WHERE level = 'all';

-- The level that applies: the conversation's own, else (for a thread) its
-- channel's, else mentions.
CREATE FUNCTION conversation_notification_level(p_actor_id UUID, p_conversation_id UUID)
RETURNS TEXT
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(
        (SELECT level FROM conversation_notification_levels
         WHERE actor_id = p_actor_id AND conversation_id = p_conversation_id),
        (SELECT level.level
         FROM conversations conversation
         JOIN conversation_notification_levels level
           ON level.conversation_id = conversation.parent_channel_id
          AND level.actor_id = p_actor_id
         WHERE conversation.id = p_conversation_id),
        'mentions'
    )
$$;

-- A forwarded message points at the one it quotes. The quote is read live
-- and shown only to people who can see the original.
ALTER TABLE messages
    ADD COLUMN forwarded_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;

-- A reminder's push offers Done and Snooze, like W18's Approve and Reply.
ALTER TABLE notification_action_tokens
    DROP CONSTRAINT notification_action_tokens_actions_check,
    DROP CONSTRAINT notification_action_tokens_used_action_check;
ALTER TABLE notification_action_tokens
    ADD CONSTRAINT notification_action_tokens_actions_check CHECK (
        cardinality(actions) BETWEEN 1 AND 3
        AND actions <@ ARRAY['approve', 'deny', 'reply', 'done', 'snooze']::TEXT[]
    ),
    ADD CONSTRAINT notification_action_tokens_used_action_check CHECK (
        used_action IN ('approve', 'deny', 'reply', 'done', 'snooze')
    );
