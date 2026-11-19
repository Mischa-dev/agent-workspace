-- K4.17 with A1.25: a DM or group is in no space, yet a host's agent
-- answers one with a run. The launch and the run are anchored in the space
-- the DM's decisions are kept in (`memory::direct_home_space`: its
-- workspace's own space) while they name the DM. A link from a launch or a
-- run to its conversation (and from a launch to its source message) used
-- to require both in the same space; now the conversation is in the row's
-- space or, for a DM or group, in none.

ALTER TABLE launches
    DROP CONSTRAINT launches_project_id_conversation_id_fkey,
    DROP CONSTRAINT launches_project_id_source_message_id_fkey,
    ADD CONSTRAINT launches_conversation_fkey
        FOREIGN KEY (conversation_id) REFERENCES conversations(id),
    ADD CONSTRAINT launches_source_message_fkey
        FOREIGN KEY (source_message_id) REFERENCES messages(id);

ALTER TABLE runs
    DROP CONSTRAINT runs_space_id_conversation_id_fkey,
    ADD CONSTRAINT runs_conversation_fkey
        FOREIGN KEY (conversation_id) REFERENCES conversations(id);

-- The conversation (and message) a launch or run names is in its space, or
-- is a DM or group, in none. Checked as the row is written: a discussion
-- that later follows its thing to another space leaves its earlier launches
-- and runs where they ran.
CREATE OR REPLACE FUNCTION conversation_in_space_or_none(
    space UUID, conversation UUID, message UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    found_space UUID;
BEGIN
    IF conversation IS NOT NULL THEN
        SELECT project_id INTO found_space FROM conversations WHERE id = conversation;
        IF found_space IS NOT NULL AND found_space IS DISTINCT FROM space THEN
            RAISE EXCEPTION 'the conversation is in another space'
                USING ERRCODE = '23503';
        END IF;
    END IF;
    IF message IS NOT NULL THEN
        SELECT project_id INTO found_space FROM messages WHERE id = message;
        IF found_space IS NOT NULL AND found_space IS DISTINCT FROM space THEN
            RAISE EXCEPTION 'the message is in another space'
                USING ERRCODE = '23503';
        END IF;
    END IF;
END;
$$;

CREATE OR REPLACE FUNCTION launch_conversation_in_space()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM conversation_in_space_or_none(NEW.project_id, NEW.conversation_id, NEW.source_message_id);
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION run_conversation_in_space()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM conversation_in_space_or_none(NEW.space_id, NEW.conversation_id, NULL);
    RETURN NEW;
END;
$$;

CREATE TRIGGER launches_conversation_in_space
    BEFORE INSERT OR UPDATE OF project_id, conversation_id, source_message_id ON launches
    FOR EACH ROW EXECUTE FUNCTION launch_conversation_in_space();

CREATE TRIGGER runs_conversation_in_space
    BEFORE INSERT OR UPDATE OF space_id, conversation_id ON runs
    FOR EACH ROW EXECUTE FUNCTION run_conversation_in_space();
