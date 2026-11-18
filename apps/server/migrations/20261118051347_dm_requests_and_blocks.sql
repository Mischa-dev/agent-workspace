-- C2.39: who may message whom. Every kind of account has the same rules,
-- and they are the account's own, server-wide (R19, R20).
--
-- dm_policy: 'anyone' (anyone you share a workspace with, the default) or
-- 'ask' (ask me first: someone's first message is a request you accept or
-- decline, and nothing more reaches you until you accept).
ALTER TABLE account_profiles
    ADD COLUMN dm_policy TEXT NOT NULL DEFAULT 'anyone'
        CHECK (dm_policy IN ('anyone', 'ask'));

-- A block hides your DM with them, refuses new messages both ways and keeps
-- their mentions out of your inbox. The blocked account isn't told.
CREATE TABLE account_blocks (
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    blocked_account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, blocked_account_id),
    CHECK (account_id <> blocked_account_id)
);
CREATE INDEX account_blocks_blocked ON account_blocks (blocked_account_id, account_id);

-- One request a DM, made by its first message to someone who asks first.
-- A DM the recipient has already written in is never a request.
CREATE TABLE dm_requests (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    requester_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    recipient_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'accepted', 'declined')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    answered_at TIMESTAMPTZ,
    CHECK (requester_id <> recipient_id),
    CHECK ((state = 'pending') = (answered_at IS NULL))
);
CREATE INDEX dm_requests_recipient ON dm_requests (recipient_id, state);

-- Whether a DM is out of `account`'s list: they blocked the other person,
-- or declined the other person's request. Readable still, by link.
CREATE FUNCTION direct_hidden_for(conversation UUID, account UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversation_members other
        JOIN account_blocks block
          ON block.account_id = account AND block.blocked_account_id = other.actor_id
        WHERE other.conversation_id = conversation AND other.actor_id <> account
          AND EXISTS (SELECT 1 FROM conversations dm WHERE dm.id = conversation AND dm.kind = 'dm')
    ) OR EXISTS (
        SELECT 1 FROM dm_requests request
        WHERE request.conversation_id = conversation
          AND request.recipient_id = account
          AND request.state = 'declined'
    )
$$;
