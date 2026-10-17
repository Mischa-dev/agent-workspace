-- X2: a launch to a connected runner hands the task to the runner agent as
-- a tag instead of starting a run here; the launch records the message.
ALTER TABLE launches DROP CONSTRAINT IF EXISTS launches_state_check;
ALTER TABLE launches ADD CONSTRAINT launches_state_check CHECK (
    state IN ('queued', 'started', 'handed_off', 'cancelled', 'failed')
);
ALTER TABLE launches ADD COLUMN handoff_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE launches ADD CONSTRAINT launches_handoff_has_message CHECK (
    (state = 'handed_off') = (handoff_message_id IS NOT NULL)
);
