-- C2.5: DMs and threads reach you (C2 plan 3.4).
--
-- A conversation level is either chosen by the person or set for them when
-- they start, post in, are mentioned in, or have a thread opened from their
-- message ("auto"). Automatic following never overrides a choice: it is
-- written with ON CONFLICT DO NOTHING, and choosing (including "mentions",
-- to unfollow) writes "chosen".
ALTER TABLE conversation_notification_levels
    ADD COLUMN source TEXT NOT NULL DEFAULT 'chosen'
        CHECK (source IN ('chosen', 'auto'));
