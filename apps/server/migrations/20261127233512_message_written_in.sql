-- AWD-220: a position for the catch-up read (`message_list`) that a late
-- commit can't slip behind.
--
-- `created_at` is when a message's transaction began, so a send that waits
-- (on its conversation's lock, say) commits with a time older than messages
-- that were already read, and a cursor by time passes it for good.
--
-- 1. `messages.written_in` is the id of the transaction that wrote the row.
--    A read takes only rows whose transaction id is below the oldest
--    transaction still running: every transaction below that has finished,
--    so none of them can add a row behind the cursor. Rows from before this
--    migration all carry its transaction id and keep their order by time
--    within it.
-- 2. A transaction id means something only in the database server that
--    gave it. A trigger stamps every inserted row with this server's, so an
--    imported row never keeps the id it had elsewhere.
-- 3. A restored database carries the ids of the server it was dumped from
--    (a restore loads rows before triggers exist). `message_positions_rebase`
--    puts every row whose id this server hasn't reached yet at position 0,
--    the oldest, where rows keep their order by time. The server calls it
--    when it starts. A row this server wrote and a reader can see is always
--    below the next id to give, so it is never touched.

ALTER TABLE messages
    ADD COLUMN written_in BIGINT NOT NULL DEFAULT txid_current();

CREATE INDEX messages_written_in ON messages (written_in, created_at, id)
    WHERE removed_at IS NULL;

CREATE FUNCTION message_written_in_is_local() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.written_in := txid_current();
    RETURN NEW;
END
$$;

CREATE TRIGGER message_written_in_is_local
    BEFORE INSERT ON messages
    FOR EACH ROW EXECUTE FUNCTION message_written_in_is_local();

-- Sent messages don't change (`guard_message_mutation`). A position isn't
-- part of what was sent, so that guard steps aside for the rebase, as it
-- does for a workspace's deletion and a discussion's move.
CREATE FUNCTION message_rebase_in_progress()
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('aw.message_rebase', true), '') = 'on'
$$;

DROP TRIGGER messages_guard_mutation ON messages;
CREATE TRIGGER messages_guard_mutation BEFORE DELETE OR UPDATE ON messages
    FOR EACH ROW WHEN (
        NOT workspace_deletion_in_progress()
        AND NOT discussion_move_in_progress()
        AND NOT message_rebase_in_progress()
    )
    EXECUTE FUNCTION guard_message_mutation();

CREATE FUNCTION message_positions_rebase() RETURNS BIGINT
LANGUAGE plpgsql
AS $$
DECLARE
    moved BIGINT;
BEGIN
    PERFORM set_config('aw.message_rebase', 'on', true);
    UPDATE messages
    SET written_in = 0
    -- Removed messages are never read by position, and leaving them out
    -- lets the position index answer this without reading the table.
    WHERE removed_at IS NULL
      AND written_in >= txid_snapshot_xmax(txid_current_snapshot());
    GET DIAGNOSTICS moved = ROW_COUNT;
    PERFORM set_config('aw.message_rebase', 'off', true);
    RETURN moved;
END
$$;
