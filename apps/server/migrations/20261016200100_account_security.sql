-- S7 and S9: account security, personal API tokens, appearance.

-- Appearance follows the person across devices. The browser keeps a cached
-- copy for painting before the app loads.
ALTER TABLE users
    ADD COLUMN appearance JSONB CHECK (
        appearance IS NULL
        OR (jsonb_typeof(appearance) = 'object' AND octet_length(appearance::text) <= 8192)
    ),
    ADD COLUMN appearance_version BIGINT NOT NULL DEFAULT 0 CHECK (appearance_version >= 0);

-- What a person sees in their list of signed-in devices.
ALTER TABLE sessions
    ADD COLUMN user_agent TEXT CHECK (user_agent IS NULL OR char_length(user_agent) <= 300),
    ADD COLUMN sign_in_method TEXT CHECK (
        sign_in_method IS NULL OR sign_in_method IN ('password', 'workos', 'invitation')
    );

-- TOTP two-factor for password sign-in. The shared secret is sealed like
-- every other secret (a per-row data key under the server's master key);
-- the server needs it to check codes, so it can't be hashed.
CREATE TABLE user_totp_factors (
    user_id UUID PRIMARY KEY REFERENCES users(id),
    ciphertext BYTEA NOT NULL CHECK (octet_length(ciphertext) BETWEEN 17 AND 1024),
    nonce BYTEA NOT NULL CHECK (octet_length(nonce) = 12),
    wrapped_key BYTEA NOT NULL CHECK (octet_length(wrapped_key) BETWEEN 32 AND 512),
    wrap_nonce BYTEA CHECK (wrap_nonce IS NULL OR octet_length(wrap_nonce) = 12),
    master_key_id TEXT NOT NULL CHECK (
        char_length(master_key_id) BETWEEN 1 AND 64
        AND master_key_id ~ '^[A-Za-z0-9_.-]+$'
    ),
    -- Set once the person typed a code from their app.
    confirmed_at TIMESTAMPTZ,
    -- The last 30-second step a code was accepted for; a code works once.
    last_used_step BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX user_totp_factors_master_key ON user_totp_factors (master_key_id);

CREATE TABLE user_recovery_codes (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    code_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(code_digest) = 32),
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX user_recovery_codes_user ON user_recovery_codes (user_id) WHERE used_at IS NULL;

-- The step between a correct password and a correct code.
CREATE TABLE login_challenges (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    authentication_epoch BIGINT NOT NULL,
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts BETWEEN 0 AND 10),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);

CREATE INDEX login_challenges_expiry ON login_challenges (expires_at);
