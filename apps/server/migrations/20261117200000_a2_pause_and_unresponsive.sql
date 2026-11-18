-- A2.10 (Lane E): picking up a request in time, and pausing an account.
--
-- A required reply request is due to be picked up by `claim_due_at` (the
-- space's `replies.claim_window_seconds` after it was made). The reply sweep
-- marks one nobody claimed by then `unresponsive_at`: the conversation says
-- so, with Ask again, the account shows as not responding in that workspace
-- (derived from these rows, not stored), and the people who manage it hear
-- about it at most once an hour.
--
-- Pausing uses `account_statuses`' own paused columns (C2.15 made them for
-- this): no table of its own.

ALTER TABLE reply_requests
    ADD COLUMN claim_due_at TIMESTAMPTZ,
    ADD COLUMN unresponsive_at TIMESTAMPTZ;

CREATE INDEX reply_requests_claim_due
    ON reply_requests (claim_due_at)
    WHERE status = 'pending' AND unresponsive_at IS NULL AND claim_due_at IS NOT NULL;
