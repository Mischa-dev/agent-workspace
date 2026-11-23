-- Deploy fix, found rehearsing the upgrade on a copy of the live data:
-- 20261117050000 retires the `approval_respond` scope by taking it off each
-- external connection and then off each MCP credential. The credential
-- trigger (`validate_mcp_credential_actor_class`) only lets a credential's
-- scopes change while its connection is still active and holds the same
-- scopes, so a credential left behind on a removed connection stopped that
-- migration. Such a credential can no longer be used, since its connection
-- is gone. Take the scope off those first, with the trigger set aside for
-- this one statement, so the next migration finds nothing it can't update.
-- Where 20261117050000 already ran, no credential holds the scope and this
-- changes nothing.

ALTER TABLE mcp_client_credentials
    DISABLE TRIGGER mcp_client_credentials_validate_actor_class;

UPDATE mcp_client_credentials credential
   SET scopes = array_remove(credential.scopes, 'approval_respond')
 WHERE 'approval_respond' = ANY(credential.scopes)
   AND NOT EXISTS (
           SELECT 1
             FROM external_agent_connections connection
            WHERE connection.id = credential.external_agent_connection_id
              AND connection.status = 'active'
              AND connection.project_id IS NOT DISTINCT FROM credential.project_id
              AND connection.actor_id = credential.actor_id);

ALTER TABLE mcp_client_credentials
    ENABLE TRIGGER mcp_client_credentials_validate_actor_class;
