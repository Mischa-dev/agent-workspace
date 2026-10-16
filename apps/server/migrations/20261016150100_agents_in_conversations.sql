-- Tagging an agent to hand it work (docs/plans/agent-tag.md, pieces 2, 4, 5
-- and 6).
--
-- Channel tags open a thread. A person's @mention in a project channel is
-- answered in a thread started from that message, so the work and its
-- follow-ups stay together. The request keeps its channel message;
-- `thread_id` says where the answer goes once the agent claims it. The reply
-- can therefore live in a different conversation than the request, so the
-- reply message is tied to the project rather than to the request's
-- conversation.
ALTER TABLE reply_requests
    ADD COLUMN thread_id UUID,
    ADD CONSTRAINT reply_requests_thread_fkey
        FOREIGN KEY (project_id, thread_id) REFERENCES conversations(project_id, id),
    DROP CONSTRAINT reply_requests_conversation_id_reply_message_id_fkey,
    ADD CONSTRAINT reply_requests_reply_message_fkey
        FOREIGN KEY (project_id, reply_message_id) REFERENCES messages(project_id, id);

CREATE INDEX reply_requests_by_thread
    ON reply_requests (thread_id, created_at)
    WHERE thread_id IS NOT NULL;

-- Follow-ups. A person's next message in a thread where an agent is working
-- asks that agent again without a new mention. When the agent still holds a
-- claim there, the new request queues behind it (never replacing it) and
-- remembers which claim it follows, so the runner can feed it into the
-- session that is already running.
ALTER TABLE reply_requests
    ADD COLUMN follows_request_id UUID REFERENCES reply_requests(id);

-- Stop. A person can stop an agent's pending or claimed request. The agent's
-- next call on it is refused with the reason, and a wakeup says so at once.
ALTER TABLE reply_requests
    DROP CONSTRAINT reply_requests_status_check,
    ADD CONSTRAINT reply_requests_status_check CHECK (
        status IN (
            'pending', 'claimed', 'replied', 'passed', 'released',
            'expired', 'superseded', 'cancelled'
        )
    );

-- Typed wakeups. Each delivery says what happened: a new request, a
-- follow-up, or a stop. One request can need a "requested" and later a
-- "cancelled" delivery to the same connection.
ALTER TABLE agent_webhook_deliveries
    ADD COLUMN event TEXT NOT NULL DEFAULT 'reply.requested' CHECK (
        event IN ('reply.requested', 'reply.followup', 'reply.cancelled')
    ),
    DROP CONSTRAINT agent_webhook_deliveries_connection_id_reply_request_id_key,
    ADD CONSTRAINT agent_webhook_deliveries_one_per_event
        UNIQUE (connection_id, reply_request_id, event);

-- Webhooks set up before typed wakeups keep receiving the old
-- `inbox.available` body for one release; new ones get typed events.
ALTER TABLE external_agent_connections
    ADD COLUMN wakeup_format TEXT NOT NULL DEFAULT 'typed' CHECK (
        wakeup_format IN ('typed', 'inbox')
    );

UPDATE external_agent_connections
SET wakeup_format = 'inbox'
WHERE webhook_url IS NOT NULL;
