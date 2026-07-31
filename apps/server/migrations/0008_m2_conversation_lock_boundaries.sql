-- Serialize Thread creation and remembered-navigation writes with archive
-- updates. FOR KEY SHARE protects identity keys but does not conflict with an
-- archived_at update; FOR SHARE makes the active-parent decision stable until
-- the writing transaction commits.

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

CREATE OR REPLACE FUNCTION validate_project_navigation_location()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_kind TEXT;
    conversation_archived_at TIMESTAMPTZ;
    navigation_parent_channel_id UUID;
    parent_archived_at TIMESTAMPTZ;
BEGIN
    IF NEW.location_kind NOT IN ('channel', 'thread') THEN
        RETURN NEW;
    END IF;

    SELECT conversation.kind,
           conversation.archived_at,
           conversation.parent_channel_id
      INTO conversation_kind,
           conversation_archived_at,
           navigation_parent_channel_id
      FROM conversations conversation
     WHERE conversation.project_id = NEW.project_id
       AND conversation.id = NEW.conversation_id
     FOR SHARE;

    IF conversation_kind IS DISTINCT FROM NEW.location_kind
       OR conversation_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Project navigation must reference an active same-kind Conversation'
            USING ERRCODE = '23514';
    END IF;

    IF conversation_kind = 'thread' THEN
        SELECT parent.archived_at
          INTO parent_archived_at
          FROM conversations parent
         WHERE parent.project_id = NEW.project_id
           AND parent.id = navigation_parent_channel_id
           AND parent.kind = 'channel'
         FOR SHARE;

        IF NOT FOUND OR parent_archived_at IS NOT NULL THEN
            RAISE EXCEPTION 'Project navigation cannot reference a Thread under an archived Channel'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;
