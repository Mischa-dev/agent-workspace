-- K5.20 follow-up (Lane B chunk 3): a workspace's own bucket is read with
-- keys the workspace's managers hold, so it never keeps a DM's or group's
-- files. Each file says whether only its conversation's members read it
-- (a DM, a group, or a thread in one), set from the conversation whatever
-- wrote the row, and such a file can't name a workspace's bucket (`ws-`).

ALTER TABLE files ADD COLUMN members_only BOOLEAN NOT NULL DEFAULT false;

-- A conversation that can't be read here counts as members-only: the safe
-- side is the server's store.
CREATE FUNCTION files_members_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.conversation_id IS NULL THEN
        NEW.members_only := false;
    ELSE
        NEW.members_only := COALESCE((
            SELECT COALESCE(CASE WHEN conversation.kind = 'thread' THEN parent.kind END,
                            conversation.kind) IN ('dm', 'group')
            FROM conversations conversation
            LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
            WHERE conversation.id = NEW.conversation_id
        ), true);
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER files_members_only
BEFORE INSERT OR UPDATE OF conversation_id, members_only ON files
FOR EACH ROW EXECUTE FUNCTION files_members_only();

UPDATE files SET conversation_id = conversation_id WHERE conversation_id IS NOT NULL;

-- Nothing is deployed with a workspace bucket yet; a DM's file found in one
-- would stop this migration rather than stay there.
ALTER TABLE files ADD CONSTRAINT files_members_only_off_workspace_buckets
    CHECK (NOT (members_only AND store_id LIKE 'ws-%'));
