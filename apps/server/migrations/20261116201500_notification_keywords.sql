-- C2.13: keyword alerts. A person lists words; a message with one of them,
-- in a conversation they can read and haven't muted, tells them. Each
-- account keeps its own list per workspace, at most 50 words.

CREATE TABLE notification_keywords (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    keyword TEXT NOT NULL CHECK (
        char_length(keyword) BETWEEN 2 AND 60
        AND keyword = btrim(keyword)
        AND keyword !~ '[[:cntrl:]]'
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX notification_keywords_one_per_word
    ON notification_keywords (account_id, workspace_id, lower(keyword));

-- Matching a message looks up the keywords of a workspace's accounts.
CREATE INDEX notification_keywords_by_workspace
    ON notification_keywords (workspace_id, account_id);
