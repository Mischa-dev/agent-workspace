-- C2.3: unread state that is exactly right (C2 plan 3.3, rules R1 and R4).
--
-- Only messages make a conversation unread: not system entries, worker
-- events or run events. `last_message_sequence` is the sequence of the
-- newest message that isn't removed; a conversation is unread for someone
-- when it is past their read mark. Two triggers keep it, so every path that
-- adds a message entry (the send pipeline, an agent's reply, a prompt) or
-- removes a message keeps it right without remembering to.

ALTER TABLE conversations
    ADD COLUMN last_message_sequence BIGINT NOT NULL DEFAULT 0
        CHECK (last_message_sequence >= 0);

-- Backfill: the newest message that isn't removed, per conversation. Bounded
-- by the (conversation_id, sequence) index; conversations with no messages
-- keep 0.
UPDATE conversations conversation
SET last_message_sequence = newest.sequence
FROM (
    SELECT entry.conversation_id, max(entry.sequence) AS sequence
    FROM conversation_entries entry
    JOIN messages message ON message.id = entry.message_id
    WHERE entry.kind = 'message' AND message.removed_at IS NULL
    GROUP BY entry.conversation_id
) newest
WHERE newest.conversation_id = conversation.id;

-- R1: a message entry moves the conversation's newest message forward.
CREATE FUNCTION conversation_entry_moves_last_message() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE conversations
    SET last_message_sequence = GREATEST(last_message_sequence, NEW.sequence)
    WHERE id = NEW.conversation_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER conversation_entries_last_message
    AFTER INSERT ON conversation_entries
    FOR EACH ROW
    WHEN (NEW.kind = 'message')
    EXECUTE FUNCTION conversation_entry_moves_last_message();

-- R4: removing a message takes it out; if it was the newest, the newest one
-- left is the last message (so removing the only unread one reads it).
CREATE FUNCTION message_removal_moves_last_message() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE conversations conversation
    SET last_message_sequence = COALESCE((
        SELECT max(entry.sequence)
        FROM conversation_entries entry
        JOIN messages message ON message.id = entry.message_id
        WHERE entry.conversation_id = NEW.conversation_id
          AND entry.kind = 'message'
          AND message.removed_at IS NULL
    ), 0)
    WHERE conversation.id = NEW.conversation_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER messages_removal_last_message
    AFTER UPDATE OF removed_at ON messages
    FOR EACH ROW
    WHEN (OLD.removed_at IS NULL AND NEW.removed_at IS NOT NULL)
    EXECUTE FUNCTION message_removal_moves_last_message();
