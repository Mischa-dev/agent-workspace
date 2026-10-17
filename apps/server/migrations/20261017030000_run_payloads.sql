-- H5: what a run did. Run events keep a redacted, size-capped payload (the
-- command, its exit code and the end of its output, the files a change
-- touched), and two kinds that only show on the run page: the agent's own
-- narration between steps and the other tools it used.
ALTER TABLE agent_run_events
    DROP CONSTRAINT IF EXISTS agent_run_events_kind_check,
    ADD CONSTRAINT agent_run_events_kind_check CHECK (
        kind IN (
            'run_state_changed',
            'command_started', 'command_completed', 'file_change',
            'approval_requested', 'approval_resolved', 'question_requested',
            'question_resolved', 'test_result', 'run_completed', 'run_failed',
            'run_interrupted', 'recovery',
            'agent_note', 'tool_used'
        )
    ),
    ADD CONSTRAINT agent_run_events_payload_size_check CHECK (
        jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 65536
    ) NOT VALID;

CREATE INDEX agent_run_events_run_order
    ON agent_run_events (agent_run_id, created_at, id);
