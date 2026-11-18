-- A1.13 (Lane E): one launch path.
--
-- A launch resolves the agent account that does the work and routes to it:
-- the agent's host (a start command), a connected runner or an agent that
-- connects by itself (the hand-off message, and a `run.start` event), or
-- the built-in host. Every route opens a run, so a started launch names it
-- in `run_id`, the one column for it; `agent_run_id` (the built-in row)
-- goes. A hand-off that opened a run is `started` too, and keeps its
-- message; `handed_off` stays for older rows and a hand-off whose run
-- couldn't be opened.
--
-- * agent_account_id: the agent a launch goes to when the launch names one
--   (a tag of an agent bound to a host, or `agentAccountId`); otherwise the
--   agent profile's account, the harness's runner or the built-in host.

UPDATE launches SET run_id = agent_run_id WHERE run_id IS NULL AND agent_run_id IS NOT NULL;

-- The link mirror reads the run from `run_id` alone.
DROP TRIGGER links_mirror ON launches;
CREATE OR REPLACE FUNCTION link_of(r launches) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.run_id, r.source_message_id, 'made_from',
               'derived', NULL, NULL, r.requested_by_actor_id)::link_of_row
$$;

-- Takes the started-has-a-built-in-run check and its foreign key with it.
ALTER TABLE launches DROP COLUMN agent_run_id;

CREATE TRIGGER links_mirror AFTER INSERT OR UPDATE OF run_id, source_message_id
    OR DELETE ON launches FOR EACH ROW EXECUTE FUNCTION links_mirror();

ALTER TABLE launches
    ADD CONSTRAINT launches_started_has_run CHECK (state <> 'started' OR run_id IS NOT NULL);

ALTER TABLE launches DROP CONSTRAINT launches_handoff_has_message;
ALTER TABLE launches
    ADD CONSTRAINT launches_handoff_has_message
        CHECK (state <> 'handed_off' OR handoff_message_id IS NOT NULL);

ALTER TABLE launches ADD COLUMN agent_account_id UUID REFERENCES actors(id);
