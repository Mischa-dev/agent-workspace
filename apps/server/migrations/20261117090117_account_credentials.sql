-- K1.22: one credentials table. An account's credentials are one table,
-- `account_credentials`, with a kind; the long-lived secrets people's
-- older project tokens and agents' connections hold today are its first
-- kind, `secret`, kept with their prefix and digest so every token issued
-- before keeps working. Later kinds (setup links, app delegations, trusted
-- outside issuers) join it as they move over.
ALTER TABLE mcp_client_credentials RENAME TO account_credentials;

ALTER TABLE account_credentials
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'secret'
        CHECK (kind IN ('secret'));

ALTER INDEX mcp_client_credentials_actor_project RENAME TO account_credentials_account_space;
ALTER INDEX mcp_client_credentials_active_expiry RENAME TO account_credentials_active_expiry;
ALTER INDEX mcp_client_credentials_external_connection RENAME TO account_credentials_connection;

-- Credentials belong to the account, not a workspace (ruling R19): the table
-- is global, and `workspace_id` narrows a credential to one workspace.
DROP POLICY IF EXISTS tenancy ON account_credentials;
ALTER TABLE account_credentials NO FORCE ROW LEVEL SECURITY;
ALTER TABLE account_credentials DISABLE ROW LEVEL SECURITY;
