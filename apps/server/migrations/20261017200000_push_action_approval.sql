-- Bug sweep: a push's Approve, Deny or Answer acts only on the approval the
-- push showed. Approval notifications are grouped per run, so the same
-- notification row later points at the run's next approval; the token now
-- remembers which one it was minted for. Tokens minted before this have no
-- approval and can't approve or answer anything.

ALTER TABLE notification_action_tokens
    ADD COLUMN approval_id UUID REFERENCES agent_approvals(id) ON DELETE CASCADE;
