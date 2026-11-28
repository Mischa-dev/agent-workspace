-- AWD-220: working notes.
--
-- A message is a record and, until now, always a tap on the shoulder too:
-- it makes its conversation unread, notifies, and asks for replies. A
-- working note is the record without the tap. It is a message in every
-- other way (same table, same send path, same search, same access rule),
-- so anyone who can read the conversation can read its notes.
--
-- 1. `messages.register`: 'message' (as before) or 'note'.
-- 2. `conversations.default_register`: what a message sent there is when
--    its sender doesn't say. A channel made for working defaults to notes.
-- 3. The two triggers that keep `last_message_sequence` (C2.3) leave notes
--    out, so a note never makes a conversation unread.

ALTER TABLE messages
    ADD COLUMN register TEXT NOT NULL DEFAULT 'message'
        CHECK (register IN ('message', 'note'));

ALTER TABLE conversations
    ADD COLUMN default_register TEXT NOT NULL DEFAULT 'message'
        CHECK (default_register IN ('message', 'note'));

-- A conversation's notes after a point, for the folded count and the
-- roll-up. Most messages aren't notes, so the index is small.
CREATE INDEX messages_notes ON messages (conversation_id, created_at)
    WHERE register = 'note' AND removed_at IS NULL;

-- R1, with notes left out: only a message that isn't a note moves the
-- conversation's newest message forward. The message row is written before
-- its entry in every send path.
CREATE OR REPLACE FUNCTION conversation_entry_moves_last_message() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE conversations
    SET last_message_sequence = GREATEST(last_message_sequence, NEW.sequence)
    WHERE id = NEW.conversation_id
      AND NOT EXISTS (
          SELECT 1 FROM messages message
          WHERE message.id = NEW.message_id AND message.register = 'note'
      );
    RETURN NULL;
END;
$$;

-- R4, with notes left out.
CREATE OR REPLACE FUNCTION message_removal_moves_last_message() RETURNS trigger
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
          AND message.register = 'message'
    ), 0)
    WHERE conversation.id = NEW.conversation_id;
    RETURN NULL;
END;
$$;
