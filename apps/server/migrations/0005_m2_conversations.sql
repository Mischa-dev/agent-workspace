-- M2 durable Conversations, immutable Messages, per-Actor read state, and
-- transactional outbox schema.
--
-- This migration evolves the accepted M0/M1 tables in place. Derived Project
-- and Conversation columns are backfilled from their canonical parents, and
-- insert triggers keep the M0 execution path compatible while M2 command
-- handlers begin supplying the columns explicitly.

ALTER TABLE repositories
    ADD CONSTRAINT repositories_project_id_id_unique UNIQUE (project_id, id);

ALTER TABLE conversations
    ADD COLUMN origin_message_id UUID,
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD COLUMN archived_by_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT conversations_archive_shape_check CHECK (
        archived_at IS NOT NULL OR archived_by_actor_id IS NULL
    );

ALTER TABLE conversations
    DROP CONSTRAINT conversations_check;

ALTER TABLE conversations
    ADD CONSTRAINT conversations_kind_shape_check CHECK (
        (
            kind = 'channel'
            AND parent_channel_id IS NULL
            AND origin_message_id IS NULL
            AND repository_id IS NULL
        )
        OR
        (
            kind = 'thread'
            AND parent_channel_id IS NOT NULL
        )
    ),
    ADD CONSTRAINT conversations_name_length_check CHECK (
        char_length(name) BETWEEN 1 AND 120
    ),
    ADD CONSTRAINT conversations_parent_project_fk
        FOREIGN KEY (project_id, parent_channel_id)
        REFERENCES conversations(project_id, id),
    ADD CONSTRAINT conversations_repository_project_fk
        FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    ADD CONSTRAINT conversations_archive_actor_membership_fk
        FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

ALTER TABLE messages
    ADD COLUMN project_id UUID;

UPDATE messages message
SET project_id = conversation.project_id
FROM conversations conversation
WHERE conversation.id = message.conversation_id;

ALTER TABLE messages
    ALTER COLUMN project_id SET NOT NULL,
    ADD CONSTRAINT messages_project_id_id_unique UNIQUE (project_id, id),
    ADD CONSTRAINT messages_conversation_id_id_unique UNIQUE (conversation_id, id),
    ADD CONSTRAINT messages_project_conversation_fk
        FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    ADD CONSTRAINT messages_author_membership_fk
        FOREIGN KEY (project_id, author_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT messages_content_shape_check CHECK (
        jsonb_typeof(content) = 'object'
        AND content->>'version' = content_version::text
        AND jsonb_typeof(content->'nodes') = 'array'
    );

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_conversation_id_id_unique
        UNIQUE (conversation_id, id);

ALTER TABLE messages
    ADD CONSTRAINT messages_agent_run_conversation_fk
        FOREIGN KEY (conversation_id, agent_run_id)
        REFERENCES agent_runs(conversation_id, id);

ALTER TABLE agent_run_events
    ADD COLUMN conversation_id UUID;

UPDATE agent_run_events event
SET conversation_id = run.conversation_id
FROM agent_runs run
WHERE run.id = event.agent_run_id;

ALTER TABLE agent_run_events
    ALTER COLUMN conversation_id SET NOT NULL,
    ADD CONSTRAINT agent_run_events_conversation_id_id_unique
        UNIQUE (conversation_id, id),
    ADD CONSTRAINT agent_run_events_run_conversation_fk
        FOREIGN KEY (conversation_id, agent_run_id)
        REFERENCES agent_runs(conversation_id, id);

ALTER TABLE conversation_entries
    ADD CONSTRAINT conversation_entries_message_conversation_fk
        FOREIGN KEY (conversation_id, message_id)
        REFERENCES messages(conversation_id, id),
    ADD CONSTRAINT conversation_entries_event_conversation_fk
        FOREIGN KEY (conversation_id, agent_run_event_id)
        REFERENCES agent_run_events(conversation_id, id);

ALTER TABLE conversations
    ADD CONSTRAINT conversations_origin_message_project_fk
        FOREIGN KEY (project_id, origin_message_id)
        REFERENCES messages(project_id, id);

CREATE OR REPLACE FUNCTION derive_message_project()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_project_id UUID;
BEGIN
    SELECT project_id
      INTO conversation_project_id
      FROM conversations
     WHERE id = NEW.conversation_id
       FOR KEY SHARE;

    IF conversation_project_id IS NULL THEN
        RAISE EXCEPTION 'Message Conversation does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Message and Conversation must belong to the same Project'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER messages_derive_project
BEFORE INSERT OR UPDATE OF project_id, conversation_id
ON messages
FOR EACH ROW
EXECUTE FUNCTION derive_message_project();

CREATE OR REPLACE FUNCTION derive_agent_run_event_conversation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    run_conversation_id UUID;
BEGIN
    SELECT conversation_id
      INTO run_conversation_id
      FROM agent_runs
     WHERE id = NEW.agent_run_id
       FOR KEY SHARE;

    IF run_conversation_id IS NULL THEN
        RAISE EXCEPTION 'Agent Run does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.conversation_id IS NULL THEN
        NEW.conversation_id := run_conversation_id;
    ELSIF NEW.conversation_id IS DISTINCT FROM run_conversation_id THEN
        RAISE EXCEPTION 'Agent Run event and Agent Run must share a Conversation'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_run_events_derive_conversation
BEFORE INSERT OR UPDATE OF conversation_id, agent_run_id
ON agent_run_events
FOR EACH ROW
EXECUTE FUNCTION derive_agent_run_event_conversation();

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

    IF NEW.kind = 'channel' THEN
        RETURN NEW;
    END IF;

    SELECT kind, project_id, archived_at
      INTO parent_kind, parent_project_id, parent_archived_at
      FROM conversations
     WHERE id = NEW.parent_channel_id
       FOR KEY SHARE;

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
        SELECT conversation_id, project_id
          INTO origin_conversation_id, origin_project_id
          FROM messages
         WHERE id = NEW.origin_message_id
           FOR KEY SHARE;

        IF origin_conversation_id IS DISTINCT FROM NEW.parent_channel_id
           OR origin_project_id IS DISTINCT FROM NEW.project_id THEN
            RAISE EXCEPTION 'Thread origin Message must belong to its parent Channel'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER conversations_validate_parent ON conversations;

CREATE TRIGGER conversations_validate_parent
BEFORE INSERT OR UPDATE OF kind, parent_channel_id, project_id, origin_message_id
ON conversations
FOR EACH ROW
EXECUTE FUNCTION validate_conversation_parent();

CREATE OR REPLACE FUNCTION enforce_conversation_archive()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.archived_at IS NULL AND NEW.archived_by_actor_id IS NOT NULL THEN
        RAISE EXCEPTION 'Active Conversation cannot have archive attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'INSERT'
       AND NEW.archived_at IS NOT NULL
       AND NEW.archived_by_actor_id IS NULL THEN
        RAISE EXCEPTION 'New archived Conversation requires Actor attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NULL
       AND NEW.archived_at IS NOT NULL THEN
        IF NEW.archived_by_actor_id IS NULL THEN
            RAISE EXCEPTION 'Archiving a Conversation requires Actor attribution'
                USING ERRCODE = '23514';
        END IF;

        IF EXISTS (
            SELECT 1
              FROM agent_runs run
              JOIN conversations owned
                ON owned.id = run.conversation_id
             WHERE run.state IN (
                    'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
             )
               AND (
                    owned.id = OLD.id
                    OR (
                        OLD.kind = 'channel'
                        AND owned.parent_channel_id = OLD.id
                    )
               )
        ) THEN
            RAISE EXCEPTION 'Conversation with a non-terminal Agent Run cannot be archived'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NOT NULL
       AND NEW.archived_at IS NULL
       AND NEW.archived_by_actor_id IS NOT NULL THEN
        RAISE EXCEPTION 'Restored Conversation must clear archive attribution'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER conversations_enforce_archive
BEFORE INSERT OR UPDATE OF archived_at, archived_by_actor_id
ON conversations
FOR EACH ROW
EXECUTE FUNCTION enforce_conversation_archive();

CREATE OR REPLACE FUNCTION enforce_thread_repository_immutability()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.repository_id IS DISTINCT FROM OLD.repository_id
       AND EXISTS (
           SELECT 1
             FROM agent_runs
            WHERE conversation_id = OLD.id
       ) THEN
        RAISE EXCEPTION 'Thread repository is immutable after its first Agent Run'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER conversations_enforce_repository_immutability
BEFORE UPDATE OF repository_id
ON conversations
FOR EACH ROW
EXECUTE FUNCTION enforce_thread_repository_immutability();

CREATE OR REPLACE FUNCTION reject_message_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'Sent Messages are immutable'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER messages_reject_mutation
BEFORE UPDATE OR DELETE
ON messages
FOR EACH ROW
EXECUTE FUNCTION reject_message_mutation();

CREATE TABLE message_mentions (
    project_id UUID NOT NULL,
    message_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    node_index INTEGER NOT NULL CHECK (node_index >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, node_index),
    FOREIGN KEY (project_id, message_id)
        REFERENCES messages(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX message_mentions_actor
    ON message_mentions (project_id, actor_id, message_id);

CREATE OR REPLACE FUNCTION validate_message_mention()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    node JSONB;
    node_actor_id TEXT;
    mentionable BOOLEAN;
BEGIN
    SELECT message.content->'nodes'->NEW.node_index
      INTO node
      FROM messages message
     WHERE message.id = NEW.message_id
       AND message.project_id = NEW.project_id
       FOR KEY SHARE;

    node_actor_id := COALESCE(node->>'actor_id', node->>'actorId');
    IF node IS NULL
       OR node->>'type' IS DISTINCT FROM 'mention'
       OR node_actor_id IS DISTINCT FROM NEW.actor_id::text THEN
        RAISE EXCEPTION 'Message mention projection must match its AST node'
            USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active'
      INTO mentionable
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.actor_id
       FOR KEY SHARE OF membership, actor;

    IF mentionable IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Mentioned Actor must have an enabled Project membership'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER message_mentions_validate
BEFORE INSERT OR UPDATE
ON message_mentions
FOR EACH ROW
EXECUTE FUNCTION validate_message_mention();

CREATE OR REPLACE FUNCTION reject_message_mention_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'Sent Message mentions are immutable'
        USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER message_mentions_reject_mutation
BEFORE UPDATE OR DELETE
ON message_mentions
FOR EACH ROW
EXECUTE FUNCTION reject_message_mention_mutation();

CREATE TABLE conversation_read_states (
    project_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    highest_seen_sequence BIGINT NOT NULL DEFAULT 0 CHECK (
        highest_seen_sequence >= 0
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, conversation_id, actor_id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX conversation_read_states_actor
    ON conversation_read_states (actor_id, project_id, updated_at DESC);

CREATE OR REPLACE FUNCTION validate_conversation_read_state()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    next_sequence BIGINT;
    reader_enabled BOOLEAN;
BEGIN
    SELECT conversation.next_entry_sequence,
           membership.enabled AND actor.status = 'active'
      INTO next_sequence, reader_enabled
      FROM conversations conversation
      JOIN project_memberships membership
        ON membership.project_id = conversation.project_id
       AND membership.actor_id = NEW.actor_id
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE conversation.project_id = NEW.project_id
       AND conversation.id = NEW.conversation_id
       FOR KEY SHARE OF conversation, membership, actor;

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
            RAISE EXCEPTION 'Conversation read state cannot move backwards'
                USING ERRCODE = '23514';
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

CREATE TRIGGER conversation_read_states_validate
BEFORE INSERT OR UPDATE
ON conversation_read_states
FOR EACH ROW
EXECUTE FUNCTION validate_conversation_read_state();

CREATE TABLE outbox_events (
    event_id UUID PRIMARY KEY CHECK (
        substring(event_id::text FROM 15 FOR 1) = '7'
    ),
    event_cursor BIGINT GENERATED ALWAYS AS IDENTITY UNIQUE,
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    project_id UUID NOT NULL REFERENCES projects(id),
    conversation_id UUID,
    resource_kind TEXT NOT NULL CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run'
        )
    ),
    resource_id UUID NOT NULL,
    event_type TEXT NOT NULL CHECK (
        char_length(event_type) BETWEEN 3 AND 120
        AND event_type ~ '^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$'
    ),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object'
        AND octet_length(payload::text) <= 65536
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts BETWEEN 1 AND 100),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_owner TEXT CHECK (
        lease_owner IS NULL OR char_length(lease_owner) BETWEEN 1 AND 128
    ),
    lease_expires_at TIMESTAMPTZ,
    last_error_summary TEXT CHECK (
        last_error_summary IS NULL OR char_length(last_error_summary) <= 2000
    ),
    published_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    CHECK (
        (lease_owner IS NULL AND lease_expires_at IS NULL)
        OR
        (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
    ),
    CHECK (published_at IS NULL OR failed_at IS NULL),
    CHECK (attempt_count <= max_attempts)
);

CREATE INDEX outbox_events_ready
    ON outbox_events (event_cursor)
    WHERE published_at IS NULL AND failed_at IS NULL;

CREATE INDEX outbox_events_lease
    ON outbox_events (lease_expires_at, event_cursor)
    WHERE published_at IS NULL AND failed_at IS NULL;

ALTER TABLE project_navigation_states
    DROP CONSTRAINT project_navigation_states_location_kind_check,
    DROP CONSTRAINT project_navigation_states_check;

ALTER TABLE project_navigation_states
    ADD CONSTRAINT project_navigation_states_location_kind_check CHECK (
        location_kind IN ('project_overview', 'channel', 'thread', 'settings')
    ),
    ADD CONSTRAINT project_navigation_states_location_shape_check CHECK (
        (
            location_kind IN ('channel', 'thread')
            AND conversation_id IS NOT NULL
        )
        OR
        (
            location_kind IN ('project_overview', 'settings')
            AND conversation_id IS NULL
        )
    );

CREATE OR REPLACE FUNCTION validate_project_navigation_location()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_kind TEXT;
    conversation_archived_at TIMESTAMPTZ;
BEGIN
    IF NEW.location_kind NOT IN ('channel', 'thread') THEN
        RETURN NEW;
    END IF;

    SELECT kind, archived_at
      INTO conversation_kind, conversation_archived_at
      FROM conversations
     WHERE project_id = NEW.project_id
       AND id = NEW.conversation_id
       FOR KEY SHARE;

    IF conversation_kind IS DISTINCT FROM NEW.location_kind
       OR conversation_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Project navigation must reference an active same-kind Conversation'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;
