-- AWD-70: reply to a specific message.
--
-- A reply is an ordinary message in the same conversation that points at the
-- one it answers, the way a forward (W21) points at what it forwards. The
-- quote is read live for each viewer, through conversation_visible_to, and
-- never copied. Starting a thread from a message stays what it was; this is
-- the quoted reply that sits beside it.
--
-- The foreign key on (conversation_id, reply_to_message_id) means a reply can
-- only point inside its own conversation, so it can never carry a quote from
-- somewhere its readers can't see. Messages are never deleted (0047), so the
-- key needs no ON DELETE.
ALTER TABLE messages
    ADD COLUMN reply_to_message_id UUID,
    ADD CONSTRAINT messages_reply_to_same_conversation_fk
        FOREIGN KEY (conversation_id, reply_to_message_id)
        REFERENCES messages (conversation_id, id),
    ADD CONSTRAINT messages_reply_to_other_message_check
        CHECK (reply_to_message_id IS DISTINCT FROM id);

-- What a message replies to is fixed once it's sent; an edit changes only the
-- text (0047's guard covers the other columns).
CREATE FUNCTION guard_message_reply_to()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.reply_to_message_id IS DISTINCT FROM OLD.reply_to_message_id THEN
        RAISE EXCEPTION 'What a Message replies to cannot change'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER messages_guard_reply_to
BEFORE UPDATE OF reply_to_message_id ON messages
FOR EACH ROW EXECUTE FUNCTION guard_message_reply_to();

-- Replies to a message, for "n replies" later and for the key's checks.
CREATE INDEX messages_reply_to ON messages (reply_to_message_id)
    WHERE reply_to_message_id IS NOT NULL;
