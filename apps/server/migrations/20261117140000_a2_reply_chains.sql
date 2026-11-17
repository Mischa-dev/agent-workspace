-- A2.8 (Lane E): mentions by anyone; loops end by chain depth and turns.
--
-- Who is asked to reply no longer depends on the kind of account. An account
-- takes reply requests when `actors.accepts_reply_requests` says so: agents
-- do when they are made (the default below, which an account's settings can
-- change), people don't. Anyone's message asks, and each request records how
-- deep in a chain of asks it is (`reply_requests.chain_depth`): a message
-- answering an ask is one deeper, and the reply rules stop asking past the
-- `replies.max_chain_depth` setting. `conversations.agent_turns_remaining`
-- now counts in every kind of conversation.

ALTER TABLE actors ADD COLUMN accepts_reply_requests BOOLEAN;
UPDATE actors SET accepts_reply_requests = (kind = 'external_agent');
ALTER TABLE actors ALTER COLUMN accepts_reply_requests SET NOT NULL;

-- A new account takes reply requests when it is an agent, unless whoever
-- made it said otherwise.
CREATE FUNCTION actors_default_accepts_reply_requests() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.accepts_reply_requests IS NULL THEN
        NEW.accepts_reply_requests := NEW.kind = 'external_agent';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER actors_default_accepts_reply_requests
BEFORE INSERT ON actors
FOR EACH ROW EXECUTE FUNCTION actors_default_accepts_reply_requests();

ALTER TABLE reply_requests
    ADD COLUMN chain_depth SMALLINT NOT NULL DEFAULT 0 CHECK (chain_depth BETWEEN 0 AND 100);
