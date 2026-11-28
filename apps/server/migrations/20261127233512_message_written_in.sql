-- AWD-220: a position for the catch-up read (`message_list`) that a late
-- commit can't slip behind.
--
-- `created_at` is when a message's transaction began, so a send that waits
-- (on its conversation's lock, say) commits with a time older than messages
-- that were already read, and a cursor by time passes it for good.
--
-- `messages.written_in` is the id of the transaction that wrote the row. A
-- read takes only rows whose transaction id is below the oldest transaction
-- still running: every transaction below that has finished, so none of them
-- can add a row behind the cursor. Rows from before this migration all carry
-- its transaction id and keep their order by time within it.

ALTER TABLE messages
    ADD COLUMN written_in BIGINT NOT NULL DEFAULT txid_current();

CREATE INDEX messages_written_in ON messages (written_in, created_at, id)
    WHERE removed_at IS NULL;
