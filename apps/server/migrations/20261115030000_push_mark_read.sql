-- C2.2: a mention's or message's push also offers Mark read, which reads
-- the conversation through the same read path as the web and agents.
ALTER TABLE notification_action_tokens
    DROP CONSTRAINT notification_action_tokens_actions_check,
    DROP CONSTRAINT notification_action_tokens_used_action_check;
ALTER TABLE notification_action_tokens
    ADD CONSTRAINT notification_action_tokens_actions_check CHECK (
        cardinality(actions) BETWEEN 1 AND 3
        AND actions <@ ARRAY['approve', 'deny', 'reply', 'read', 'done', 'snooze']::TEXT[]
    ),
    ADD CONSTRAINT notification_action_tokens_used_action_check CHECK (
        used_action IN ('approve', 'deny', 'reply', 'read', 'done', 'snooze')
    );
