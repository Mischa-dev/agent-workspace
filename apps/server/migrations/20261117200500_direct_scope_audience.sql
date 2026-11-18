-- K3.24 (rulings R19, R20): DMs and groups belong to the people in them,
-- under the direct scope's fixed id (K4.23 adds its reserved
-- `organizations` row). An event there is heard only by a conversation's
-- participants or by one account, never by a workspace or a space, so no
-- owner or admin of anyone's workspace reads it.
ALTER TABLE events ADD CONSTRAINT events_direct_scope_audience
    CHECK (workspace_id <> '00000000-0000-7000-8000-00000000d1ec'
           OR audience = 'account'
           OR (audience = 'conversation' AND conversation_restricted));
ALTER TABLE event_staging ADD CONSTRAINT event_staging_direct_scope_audience
    CHECK (workspace_id <> '00000000-0000-7000-8000-00000000d1ec'
           OR audience = 'account'
           OR (audience = 'conversation' AND conversation_restricted));
