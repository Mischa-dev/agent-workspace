-- Deploy fix, found rehearsing the upgrade on a copy of the live data:
-- agents connected before connections had their own owner kept the secret
-- that signs their webhook pushes in the workspace's hidden Messages
-- project. 20261117173117 deletes the secrets of those projects as
-- scaffolding, so a connection still signing with one stopped it (and
-- deleting them would have cut off that agent's wakeups). Give each such
-- secret the shape new ones have (`delivery::connections::new_signing_secret`):
-- owned by the connection's account in its home workspace, with no space.
-- Where the hidden projects are already gone this changes nothing.

UPDATE secrets secret
   SET project_id = NULL,
       owner_actor_id = connection.account_id,
       updated_at = now(),
       version = secret.version + 1
  FROM connection_signing_secrets signing
  JOIN agent_connections connection ON connection.id = signing.connection_id
  JOIN projects direct ON direct.kind = 'direct'
 WHERE signing.secret_id = secret.id
   AND secret.project_id = direct.id
   AND secret.organization_id = connection.home_workspace_id;
