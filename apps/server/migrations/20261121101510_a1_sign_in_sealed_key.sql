-- A1.17 step 4: the built-in host carries out sign-ins. An API key to sign
-- in with is sealed beside its sign-in (the envelope `secrets` uses, bound
-- to the sign-in's id and account) until the host takes it, once, when it
-- fetches the command that names the sign-in. It is never kept in plain
-- text, and never in a command.
ALTER TABLE provider_login_ceremonies
    ADD COLUMN api_key_ciphertext BYTEA,
    ADD COLUMN api_key_nonce BYTEA,
    ADD COLUMN api_key_wrapped_key BYTEA,
    ADD COLUMN api_key_wrap_nonce BYTEA,
    ADD COLUMN api_key_master_key_id TEXT,
    ADD CONSTRAINT provider_login_ceremonies_api_key_whole CHECK (
        (api_key_ciphertext IS NULL) = (api_key_nonce IS NULL)
        AND (api_key_ciphertext IS NULL) = (api_key_wrapped_key IS NULL)
        AND (api_key_ciphertext IS NULL) = (api_key_master_key_id IS NULL)
    );
