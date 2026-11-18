-- Lane E (A1.16): a run's agent may say several things in its thread.
-- Any host reports what its agent says there (`text_message` with
-- `inThread`), and each message links to its run; the old rule of one
-- worker message per built-in run is gone with the old turn driver.
DROP INDEX IF EXISTS messages_one_worker_message_per_agent_run;
CREATE INDEX messages_agent_run ON messages (agent_run_id) WHERE agent_run_id IS NOT NULL;
-- A message links to any host's run (`messages_run_conversation_fk`), not
-- only a built-in one.
ALTER TABLE messages DROP CONSTRAINT IF EXISTS messages_agent_run_id_fkey;
