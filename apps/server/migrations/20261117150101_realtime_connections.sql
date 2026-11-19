-- K5.10: who is connected, on any instance, for presence. A row lives as
-- long as one live connection: written when it opens, its `seen_at`
-- renewed every 15 seconds, deleted when it closes. A row whose instance
-- stopped without closing it stops counting once `seen_at` is a minute old
-- and is swept later. No content; accounts are server-wide (R19).
CREATE TABLE realtime_connections (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    instance_id UUID NOT NULL,
    opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX realtime_connections_account ON realtime_connections (account_id, seen_at);
CREATE INDEX realtime_connections_seen ON realtime_connections (seen_at);
