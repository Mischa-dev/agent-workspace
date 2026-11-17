-- C2.28: a message being written follows its writer across devices. One
-- draft per account and conversation, saved a moment after typing stops.
-- When two devices save, the newer one wins and the other is kept as a
-- recoverable copy for a day. Only the writer ever reads a draft, and only
-- while they can still read the conversation.

CREATE TABLE message_drafts (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    content JSONB NOT NULL CHECK (jsonb_typeof(content) = 'object'
                                  AND octet_length(content::text) <= 65536),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    previous_content JSONB CHECK (previous_content IS NULL
        OR (jsonb_typeof(previous_content) = 'object'
            AND octet_length(previous_content::text) <= 65536)),
    previous_updated_at TIMESTAMPTZ,
    PRIMARY KEY (account_id, conversation_id)
);

-- "Drafts" lists one account's drafts in one workspace, newest first.
CREATE INDEX message_drafts_by_account
    ON message_drafts (account_id, workspace_id, updated_at DESC);
