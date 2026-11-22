-- M1.15 framed panels: a connected module's panel can answer with a frame,
-- an address on one of the origins its manifest declares. Each frame is a
-- launch: the viewer's browser opens Workspace's host page for it once,
-- which mints the launch token the app's page receives; the app redeems the
-- token once, server to server, within five minutes; and the frame's `done`
-- is checked by asking the app for the result. Only the token's digest is
-- kept.
CREATE TABLE module_frame_launches (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    -- The viewer the frame was opened for.
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    -- The panel whose answer opened it, and the thing it was drawn beside.
    panel TEXT NOT NULL CHECK (char_length(panel) BETWEEN 1 AND 64),
    thing_kind TEXT CHECK (thing_kind IS NULL OR char_length(thing_kind) <= 128),
    thing_id UUID,
    url TEXT NOT NULL CHECK (char_length(url) BETWEEN 1 AND 2048),
    origin TEXT NOT NULL CHECK (char_length(origin) BETWEEN 1 AND 300),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120 AND title !~ '[[:cntrl:]]'),
    height INTEGER NOT NULL CHECK (height BETWEEN 100 AND 2000),
    token_digest BYTEA UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    opened_at TIMESTAMPTZ,
    redeemed_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX module_frame_launches_account
    ON module_frame_launches (account_id, created_at DESC);

SELECT tenancy_protect('module_frame_launches', 'workspace_id');
