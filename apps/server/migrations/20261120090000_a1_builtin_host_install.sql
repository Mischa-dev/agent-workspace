-- A1.17: the built-in host as its own program. It enrols once for the
-- whole install (it serves every workspace's built-in `hosts` row, ruling
-- R24), with a one-time code the server writes to a file the host reads on
-- its first start. The code and the token it is exchanged for are kept only
-- as SHA-256 digests.
CREATE TABLE builtin_host_installs (
    id UUID PRIMARY KEY,
    code_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    code_expires_at TIMESTAMPTZ NOT NULL,
    token_prefix TEXT UNIQUE CHECK (token_prefix IS NULL OR char_length(token_prefix) = 12),
    token_digest BYTEA CHECK (token_digest IS NULL OR octet_length(token_digest) = 32),
    enrolled_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((enrolled_at IS NULL) = (token_digest IS NULL)),
    CHECK ((token_prefix IS NULL) = (token_digest IS NULL))
);
