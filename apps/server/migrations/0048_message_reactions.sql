-- Emoji reactions on messages (docs/plans/mcp-tools.md, Phase 3).
-- Phase 2 owns 0044, 0045 and 0046; Phase 3 uses 0047, 0048 (this) and 0049.
--
-- One row per person or agent, message and emoji, from a fixed list
-- (src/reactions.rs). Adding the same reaction twice changes nothing; taking
-- it away deletes the row. Rows never change in place and hold no text. Only
-- someone who can see the conversation can react, and never on a removed
-- message; the trigger checks both again.

CREATE TABLE message_reactions (
    project_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    message_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    -- Must match REACTIONS in src/reactions.rs exactly. ❤️ is U+2764 U+FE0F;
    -- every other entry is a single code point.
    emoji TEXT NOT NULL CONSTRAINT message_reactions_emoji_allowed CHECK (
        emoji IN ('👍', '👎', '❤️', '😄', '🎉', '😕', '🚀', '👀', '✅', '❌', '🙏', '🤔')
    ),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id)
        DEFAULT current_via_grant(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, actor_id, emoji),
    FOREIGN KEY (conversation_id, message_id)
        REFERENCES messages(conversation_id, id),
    FOREIGN KEY (project_id, message_id)
        REFERENCES messages(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE FUNCTION validate_message_reaction()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    reactor_enabled BOOLEAN;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'Message reactions are added or removed, never changed'
            USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active'
      INTO reactor_enabled
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.actor_id
       FOR KEY SHARE OF membership, actor;

    IF reactor_enabled IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Reacting Actor must have an enabled Project membership'
            USING ERRCODE = '23514';
    END IF;

    IF NOT conversation_visible_to(NEW.conversation_id, NEW.actor_id) THEN
        RAISE EXCEPTION 'Reacting Actor must be able to see the Conversation'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1 FROM messages
         WHERE id = NEW.message_id AND removed_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'Removed Messages cannot get reactions'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER message_reactions_validate
BEFORE INSERT OR UPDATE
ON message_reactions
FOR EACH ROW
EXECUTE FUNCTION validate_message_reaction();
