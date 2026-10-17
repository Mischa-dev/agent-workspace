-- W19: email. A workspace's way of sending mail (SMTP first), each
-- person's email address once they confirm it, emailed password resets,
-- and when each person's email digest last went out.

-- How a workspace sends mail. `provider_config` is the provider's own
-- settings (for SMTP: host, port, security, username), read only by that
-- provider's module; its password is an encrypted workspace secret
-- (`mail.smtp_password`), never a column here.
CREATE TABLE workspace_mail_settings (
    organization_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (provider IN ('smtp')),
    provider_config JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(provider_config) = 'object' AND octet_length(provider_config::text) <= 4000),
    from_address TEXT NOT NULL CHECK (char_length(from_address) BETWEEN 3 AND 254 AND from_address !~ '[[:cntrl:][:space:]<>]'),
    from_name TEXT CHECK (from_name IS NULL OR (char_length(from_name) BETWEEN 1 AND 120 AND from_name !~ '[[:cntrl:]]')),
    enabled BOOLEAN NOT NULL DEFAULT true,
    last_test_at TIMESTAMPTZ,
    last_test_ok BOOLEAN,
    last_test_error TEXT CHECK (last_test_error IS NULL OR char_length(last_test_error) <= 400),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A person's email address. Mail goes only to a confirmed one; a change
-- starts unconfirmed again. The confirmation link's token is kept only as
-- a sha256 digest.
CREATE TABLE user_email_addresses (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    address TEXT NOT NULL CHECK (char_length(address) BETWEEN 3 AND 254 AND address !~ '[[:cntrl:][:space:]<>]'),
    verified_at TIMESTAMPTZ,
    verification_digest BYTEA CHECK (verification_digest IS NULL OR octet_length(verification_digest) = 32),
    verification_expires_at TIMESTAMPTZ,
    verification_sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One confirmed owner per address, so a reset by address finds one person.
CREATE UNIQUE INDEX user_email_addresses_verified
    ON user_email_addresses (lower(address)) WHERE verified_at IS NOT NULL;
CREATE UNIQUE INDEX user_email_addresses_verification
    ON user_email_addresses (verification_digest) WHERE verification_digest IS NOT NULL;

-- Emailed password reset links: single use, short lived, and tied to the
-- account's authentication epoch so a password change or sign-out
-- everywhere also kills the link. Only the token's digest is stored.
CREATE TABLE password_reset_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    authentication_epoch BIGINT NOT NULL,
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);

CREATE INDEX password_reset_tokens_user ON password_reset_tokens (user_id, created_at);

-- When a person's email digest last went out in a workspace.
CREATE TABLE notification_email_digests (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    last_sent_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (organization_id, actor_id)
);
