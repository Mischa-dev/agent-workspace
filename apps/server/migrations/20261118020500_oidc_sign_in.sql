-- H1.18: plain OpenID Connect sign-in. Both tables are about accounts,
-- which are server-wide, so neither belongs to a workspace.

-- A sign-in on its way through the issuer: its state, bound to the browser
-- that started it, taken once by the callback. The PKCE verifier and the
-- nonce are derived from the state, so neither is stored.
CREATE TABLE oidc_login_attempts (
    state_hash BYTEA PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    browser_hash BYTEA NOT NULL CHECK (octet_length(browser_hash) = 32),
    -- Linking the identity to the account signed in with this session.
    link_session_id UUID REFERENCES sessions(id) ON DELETE CASCADE,
    -- A path on this site to go back to after signing in.
    return_to TEXT CHECK (return_to IS NULL OR (char_length(return_to) <= 2048 AND return_to LIKE '/%')),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX oidc_login_attempts_expiry ON oidc_login_attempts (expires_at);

-- An issuer's identity, linked to one account here.
CREATE TABLE oidc_identities (
    issuer TEXT NOT NULL CHECK (char_length(issuer) BETWEEN 1 AND 500),
    subject TEXT NOT NULL CHECK (char_length(subject) BETWEEN 1 AND 255),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The verified email it signed in with first, for the account page.
    email TEXT CHECK (email IS NULL OR char_length(email) <= 320),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer, subject)
);
CREATE INDEX oidc_identities_user ON oidc_identities (user_id);

-- A session can say it began with single sign-on.
ALTER TABLE sessions
    DROP CONSTRAINT sessions_sign_in_method_check,
    ADD CONSTRAINT sessions_sign_in_method_check CHECK (
        sign_in_method IS NULL OR sign_in_method IN ('password', 'workos', 'invitation', 'oidc')
    );
