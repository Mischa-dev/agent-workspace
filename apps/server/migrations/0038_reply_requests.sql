-- Agents answering on their own.
--
-- A reply request asks one agent to answer one message. A human DM to an
-- agent asks for a required reply. A group message asks every agent member for
-- an optional one, which the agent may pass. Agent messages in a group ask the
-- other agents only while the conversation's turn allowance lasts; each human
-- message resets it.
--
-- Agents claim a request before answering. The claim is a lease: a lapsed lease
-- becomes "expired" and waits for a person to retry it instead of being handed
-- out again silently.

ALTER TABLE conversations
    ADD COLUMN agent_turns_remaining INTEGER NOT NULL DEFAULT 6
        CHECK (agent_turns_remaining >= 0);

CREATE TABLE reply_requests (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    message_id UUID NOT NULL,
    agent_actor_id UUID NOT NULL,
    required BOOLEAN NOT NULL,
    mentioned BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN (
            'pending', 'claimed', 'replied', 'passed', 'released',
            'expired', 'superseded'
        )
    ),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    lease_expires_at TIMESTAMPTZ,
    reply_message_id UUID,
    reason TEXT CHECK (reason IS NULL OR char_length(reason) <= 2000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (message_id, agent_actor_id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (conversation_id, message_id)
        REFERENCES messages(conversation_id, id),
    FOREIGN KEY (project_id, agent_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (conversation_id, reply_message_id)
        REFERENCES messages(conversation_id, id),
    CHECK ((status = 'claimed') = (lease_expires_at IS NOT NULL)),
    CHECK ((status = 'replied') = (reply_message_id IS NOT NULL)),
    CHECK (required = false OR status <> 'passed')
);

CREATE INDEX reply_requests_open_by_agent
    ON reply_requests (agent_actor_id, created_at)
    WHERE status IN ('pending', 'claimed');

CREATE INDEX reply_requests_by_conversation
    ON reply_requests (conversation_id, created_at);

CREATE INDEX reply_requests_claimed_leases
    ON reply_requests (lease_expires_at)
    WHERE status = 'claimed';

-- Optional push wakeups. The database stays the source of truth: a webhook
-- only tells the agent to check its inbox.
ALTER TABLE external_agent_connections
    ADD COLUMN webhook_url TEXT CHECK (
        webhook_url IS NULL
        OR (char_length(webhook_url) <= 500 AND webhook_url ~ '^https?://')
    ),
    ADD COLUMN webhook_secret TEXT,
    ADD CONSTRAINT external_agent_connections_webhook_pair
        CHECK ((webhook_url IS NULL) = (webhook_secret IS NULL));

CREATE TABLE agent_webhook_deliveries (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES external_agent_connections(id),
    reply_request_id UUID NOT NULL REFERENCES reply_requests(id),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connection_id, reply_request_id)
);

CREATE INDEX agent_webhook_deliveries_due
    ON agent_webhook_deliveries (next_attempt_at)
    WHERE delivered_at IS NULL;

ALTER TABLE external_agent_connections
    ADD COLUMN last_wakeup_at TIMESTAMPTZ,
    ADD COLUMN last_wakeup_error TEXT CHECK (
        last_wakeup_error IS NULL OR char_length(last_wakeup_error) <= 200
    );
