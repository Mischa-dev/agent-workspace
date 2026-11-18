-- A1.14: the built-in host writes its steps and usage straight into the
-- run service's own records, `run_activities` and `run_usage`, as any host
-- reports them. `agent_run_events` and `agent_run_usage`, the triggers that
-- copied them across and the batch copy job go.
--
-- Every step already has its activity (the copy trigger, with the step's id
-- as the activity's id); the few the batch job might not have reached yet
-- are copied first. A step's own name moves from `payload.legacyKind` to
-- `payload.step`, and its key becomes the activity's once-only id. A thread
-- entry that showed a step points at the activity, which has the same id.

SELECT runs_copy_legacy_event(event)
  FROM agent_run_events event
 WHERE NOT EXISTS (
       SELECT 1 FROM run_activities activity WHERE activity.legacy_event_id = event.id)
 ORDER BY event.id;

UPDATE run_activities activity
   SET client_id = event.event_key
  FROM agent_run_events event
 WHERE activity.legacy_event_id = event.id
   AND activity.client_id IS NULL
   AND char_length(event.event_key) <= 200;

UPDATE run_activities
   SET payload = (payload - 'legacyKind') || jsonb_build_object('step', payload->'legacyKind')
 WHERE payload ? 'legacyKind';

UPDATE run_activities activity
   SET in_thread = true
  FROM conversation_entries entry
 WHERE entry.agent_run_event_id = activity.id;

ALTER TABLE conversation_entries
    DROP CONSTRAINT conversation_entries_event_conversation_fk,
    DROP CONSTRAINT conversation_entries_agent_run_event_id_fkey;
ALTER TABLE conversation_entries RENAME COLUMN agent_run_event_id TO run_activity_id;
ALTER TABLE conversation_entries
    ADD CONSTRAINT conversation_entries_run_activity_fk
        FOREIGN KEY (run_activity_id) REFERENCES run_activities(id);
CREATE INDEX conversation_entries_run_activity
    ON conversation_entries (run_activity_id) WHERE run_activity_id IS NOT NULL;

-- A failure someone dismissed names the activity it showed.
ALTER TABLE agent_run_acknowledgements
    DROP CONSTRAINT agent_run_acknowledgements_acknowledged_event_id_fkey,
    ADD CONSTRAINT agent_run_acknowledgements_activity_fk
        FOREIGN KEY (acknowledged_event_id) REFERENCES run_activities(id);

CREATE OR REPLACE FUNCTION enforce_agent_run_acknowledgement_scope()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM run_activities step
         WHERE step.id = NEW.acknowledged_event_id
           AND step.run_id = NEW.agent_run_id
           AND step.payload->>'step' = 'run_failed'
    ) THEN
        RAISE EXCEPTION 'Acknowledgement requires this Agent Run failure event'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER agent_run_events_mirror ON agent_run_events;
DROP TRIGGER agent_run_usage_mirror ON agent_run_usage;
DROP FUNCTION runs_backfill_activities(UUID, INTEGER);
DROP FUNCTION runs_mirror_agent_run_event();
DROP FUNCTION runs_copy_legacy_event(agent_run_events);
DROP FUNCTION runs_activity_kind_from_legacy(TEXT, JSONB);
DROP FUNCTION runs_activity_payload_from_legacy(TEXT, JSONB);
DROP FUNCTION runs_mirror_agent_run_usage();
DELETE FROM background_jobs WHERE kind = 'runs.backfill_activities';

ALTER TABLE run_activities DROP COLUMN legacy_event_id;
DROP TABLE agent_run_events;
DROP TABLE agent_run_usage;
