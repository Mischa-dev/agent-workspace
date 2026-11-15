-- K1.4: `account.managers.changed`, an outbox event about an account (who
-- manages an agent or app changed), delivered to the manager named or
-- un-named. Ids only.

ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification', 'whiteboard', 'page', 'collection',
            'sheet', 'account'
        )
    );
