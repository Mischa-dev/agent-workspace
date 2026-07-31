-- Treat a Thread beneath an archived Channel as archived history for
-- remembered Project navigation. Direct history routes remain available with
-- includeArchived=true, but they must never become the normal restored route.

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
     FOR KEY SHARE;

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
         FOR KEY SHARE;

        IF NOT FOUND OR parent_archived_at IS NOT NULL THEN
            RAISE EXCEPTION 'Project navigation cannot reference a Thread under an archived Channel'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;
