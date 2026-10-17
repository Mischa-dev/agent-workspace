-- Acting from a push notification (W18): Approve, Deny and Reply buttons on
-- the phone's notification, without opening the app.
--
-- The service worker has no session cookie or CSRF token it can rely on, so
-- each push carries one random token for the notification it shows. Only the
-- token's sha256 is stored. A token lets its holder do only the actions
-- listed here, only as the notification's recipient, and only until it
-- expires; the command it runs rechecks the recipient's current access, as
-- the app does (principle 9). After one action is used, the token answers
-- only a retry of that same action, so Approve can't be followed by Deny.

CREATE TABLE notification_action_tokens (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    actions TEXT[] NOT NULL CHECK (
        cardinality(actions) BETWEEN 1 AND 3
        AND actions <@ ARRAY['approve', 'deny', 'reply']::TEXT[]
    ),
    expires_at TIMESTAMPTZ NOT NULL,
    used_action TEXT CHECK (used_action IN ('approve', 'deny', 'reply')),
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((used_action IS NULL) = (used_at IS NULL))
);

CREATE INDEX notification_action_tokens_expiry
    ON notification_action_tokens (expires_at);
