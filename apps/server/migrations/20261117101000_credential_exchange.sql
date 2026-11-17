-- K1.23: exchange, rotation, revocation and reach.
--
-- A secret now says how far it reaches: one space (`space`, what every
-- older person's credential was) or every workspace its account is in
-- (`account`, ruling R19). New secrets are `aw_sk_…`; older `aw_mcp_` and
-- `aw_ext_` values keep working. Operations make, rotate and revoke them,
-- and keep the request id that made one, so a retry never mints a second.
--
-- Rotating keeps the old value working for a grace period: the old row
-- stays active, names its replacement, and expires at the end of the grace.
--
-- A secret (or a federated assertion) exchanges at the token endpoint for a
-- short-lived access token; the token names the credential it came from,
-- and stops at once when that credential is revoked or lapses.
ALTER TABLE account_credentials
    DROP CONSTRAINT mcp_client_credentials_token_prefix_check,
    ADD CONSTRAINT account_credentials_token_prefix_check CHECK (
        char_length(token_prefix) BETWEEN 8 AND 24
        AND token_prefix ~ '^aw_(mcp|ext|sk)_[A-Za-z0-9_-]+$'
    ),
    ADD COLUMN reach TEXT NOT NULL DEFAULT 'space' CHECK (reach IN ('space', 'account')),
    ADD COLUMN created_by_actor_id UUID REFERENCES actors(id),
    ADD COLUMN request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) BETWEEN 1 AND 200);

CREATE UNIQUE INDEX account_credentials_request
    ON account_credentials (created_by_actor_id, request_id)
    WHERE request_id IS NOT NULL;

ALTER TABLE access_tokens
    ADD COLUMN credential_id UUID REFERENCES account_credentials(id),
    ADD COLUMN scopes TEXT[] NOT NULL DEFAULT ARRAY['read', 'write']::TEXT[] CHECK (
        cardinality(scopes) BETWEEN 1 AND 5
        AND scopes <@ ARRAY['read', 'write', 'events', 'approval_respond', 'worker_request']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    );

CREATE INDEX access_tokens_credential ON access_tokens (credential_id)
    WHERE credential_id IS NOT NULL AND revoked_at IS NULL;

-- A secret that belongs to an agent's connection holds what the connection
-- holds, in the connection's workspace (K1.6: connections belong to the
-- workspace, not the space a secret is made in). Nothing here depends on
-- the account's kind: a secret without a connection reads and writes only.
CREATE OR REPLACE FUNCTION validate_mcp_credential_actor_class()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    connection_workspace_id UUID;
    connection_actor_id UUID;
    connection_status TEXT;
    connection_scopes TEXT[];
BEGIN
    IF NEW.external_agent_connection_id IS NULL THEN
        IF NOT (NEW.scopes <@ ARRAY['read', 'write']::TEXT[]) THEN
            RAISE EXCEPTION 'a secret without a connection only reads and writes'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    SELECT workspace_id, actor_id, status, scopes
      INTO connection_workspace_id, connection_actor_id, connection_status, connection_scopes
      FROM external_agent_connections
     WHERE id = NEW.external_agent_connection_id
       FOR SHARE;
    IF connection_workspace_id IS NOT DISTINCT FROM
           (SELECT organization_id FROM projects WHERE id = NEW.project_id)
       AND connection_actor_id IS NOT DISTINCT FROM NEW.actor_id
       AND connection_status = 'active'
       AND connection_scopes IS NOT DISTINCT FROM NEW.scopes THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'a secret and its connection do not match'
        USING ERRCODE = '23514';
END;
$$;
