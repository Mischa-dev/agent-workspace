-- A1.3: run operations for any agent.
--
-- * run_visible_to(run, viewer): who can see a run. In a conversation, who
--   can see the conversation (conversation_visible_to); otherwise in a
--   project, its members; otherwise the account that does it and who asked
--   for it. The run service, the MCP scope lookup and the guards read it.
-- * The tables that point at a run point at `runs` now (A1 section 4, step
--   3): notifications, launches, agent_sessions and messages. Every
--   agent_runs row has a runs row since 20261114072611_a1_runs, so each
--   constraint is added NOT VALID and then validated. agent_runs.id points
--   at runs too, checked at commit, since the mirror trigger writes the
--   runs row after the agent_runs row.
-- * launches.run_id: the run a launch started, for every kind of agent
--   (agent_run_id stays the built-in detail row).
-- * Custom roles gain runs.report and requests.raise, which every built-in
--   role now holds, so agents with a custom role can keep reporting.

CREATE FUNCTION run_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM runs run
        WHERE run.id = target
          AND CASE
                WHEN run.conversation_id IS NOT NULL
                  THEN conversation_visible_to(run.conversation_id, viewer)
                WHEN run.space_id IS NOT NULL
                  THEN EXISTS (
                      SELECT 1 FROM project_memberships membership
                      WHERE membership.project_id = run.space_id
                        AND membership.actor_id = viewer
                        AND membership.enabled)
                ELSE viewer IN (run.account_id, run.requested_by_account_id)
              END
    )
$$;

-- Drop the foreign key on `columns` of `source` that points at agent_runs.
CREATE FUNCTION pg_temp.drop_agent_runs_reference(source REGCLASS, columns TEXT[])
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    found TEXT;
BEGIN
    SELECT constraint_row.conname INTO found
    FROM pg_constraint constraint_row
    WHERE constraint_row.conrelid = source
      AND constraint_row.contype = 'f'
      AND constraint_row.confrelid = 'agent_runs'::regclass
      AND ARRAY(
            SELECT attribute.attname::text
            FROM unnest(constraint_row.conkey) WITH ORDINALITY AS key(number, position)
            JOIN pg_attribute attribute
              ON attribute.attrelid = constraint_row.conrelid AND attribute.attnum = key.number
            ORDER BY key.position
          ) = columns;
    IF found IS NULL THEN
        RAISE EXCEPTION 'no agent_runs reference on % (%)', source, columns;
    END IF;
    EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', source, found);
END;
$$;

SELECT pg_temp.drop_agent_runs_reference('notifications', ARRAY['agent_run_id']);
ALTER TABLE notifications
    ADD CONSTRAINT notifications_run_fk
        FOREIGN KEY (agent_run_id) REFERENCES runs(id) ON DELETE CASCADE NOT VALID;
ALTER TABLE notifications VALIDATE CONSTRAINT notifications_run_fk;

SELECT pg_temp.drop_agent_runs_reference('launches', ARRAY['project_id', 'agent_run_id']);
ALTER TABLE launches
    ADD CONSTRAINT launches_agent_run_fk
        FOREIGN KEY (project_id, agent_run_id) REFERENCES runs(space_id, id) NOT VALID;
ALTER TABLE launches VALIDATE CONSTRAINT launches_agent_run_fk;
ALTER TABLE launches ADD COLUMN run_id UUID;
UPDATE launches SET run_id = agent_run_id WHERE agent_run_id IS NOT NULL;
ALTER TABLE launches
    ADD CONSTRAINT launches_run_fk
        FOREIGN KEY (project_id, run_id) REFERENCES runs(space_id, id) NOT VALID;
ALTER TABLE launches VALIDATE CONSTRAINT launches_run_fk;
CREATE INDEX launches_run ON launches (run_id) WHERE run_id IS NOT NULL;

SELECT pg_temp.drop_agent_runs_reference('agent_sessions', ARRAY['project_id', 'agent_run_id']);
ALTER TABLE agent_sessions
    ADD CONSTRAINT agent_sessions_run_fk
        FOREIGN KEY (project_id, agent_run_id) REFERENCES runs(space_id, id) NOT VALID;
ALTER TABLE agent_sessions VALIDATE CONSTRAINT agent_sessions_run_fk;

SELECT pg_temp.drop_agent_runs_reference('messages', ARRAY['conversation_id', 'agent_run_id']);
ALTER TABLE messages
    ADD CONSTRAINT messages_run_conversation_fk
        FOREIGN KEY (conversation_id, agent_run_id) REFERENCES runs(conversation_id, id) NOT VALID;
ALTER TABLE messages VALIDATE CONSTRAINT messages_run_conversation_fk;

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_run_fk
        FOREIGN KEY (id) REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED NOT VALID;
ALTER TABLE agent_runs VALIDATE CONSTRAINT agent_runs_run_fk;

UPDATE workspace_roles role
   SET permissions = role.permissions
           || ARRAY(SELECT added FROM unnest(ARRAY['runs.report', 'requests.raise']) AS added
                    WHERE NOT added = ANY(role.permissions)),
       version = role.version + 1,
       updated_at = now()
 WHERE NOT role.built_in
   AND NOT (role.permissions @> ARRAY['runs.report', 'requests.raise']);
