-- Let an OAuth client (such as a cloud agent product) sign in as one of the
-- owner's agents instead of as the owner.
--
-- An agent grant is backed by an ordinary agent credential row. Its secret is
-- never issued; the row exists so every existing agent check (credential
-- active, connection active, scopes match) applies unchanged. Disabling,
-- rotating, or removing the connection revokes the backing credential and so
-- cuts off the grant too.

ALTER TABLE mcp_oauth_authorization_codes
    ADD COLUMN external_agent_connection_id UUID
        REFERENCES external_agent_connections(id);

ALTER TABLE mcp_oauth_grants
    ADD COLUMN external_agent_connection_id UUID
        REFERENCES external_agent_connections(id),
    ADD COLUMN backing_credential_id UUID
        REFERENCES mcp_client_credentials(id),
    ADD CONSTRAINT mcp_oauth_grants_agent_backing CHECK (
        (external_agent_connection_id IS NULL) = (backing_credential_id IS NULL)
    );
