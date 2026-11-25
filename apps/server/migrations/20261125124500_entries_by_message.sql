-- Usability pass (AWD-218): find a message's entry by the message.
--
-- The notifier asks, for every notification about a message, whether its
-- reader has already seen it, and reading a conversation asks which of the
-- reader's notifications it covers. Both look an entry up by its message;
-- with no index on it, the first read every entry there is.
CREATE INDEX conversation_entries_message
    ON conversation_entries (message_id)
    WHERE message_id IS NOT NULL;
