-- K1.5 with A1.3: a host's account (an app) belongs to whoever owns the host
-- (`hosts.owner_account_id` or `hosts.owner_workspace_id`), so every agent
-- and app keeps exactly one owner now that hosts make accounts with no
-- membership.
--
-- Backfill touches only accounts still unset, so a stopped run resumes by
-- running again.
--
-- Triggers added (ruling R16): `hosts_fill_account_owner` on `hosts`
-- writes `actors.owner_*` of the host's account when unset. Removed in
-- K1.6, when `accounts::create_agent` names the owner and the host paths
-- call it.

UPDATE actors account
SET owner_account_id = host.owner_account_id,
    owner_workspace_id = CASE WHEN host.owner_account_id IS NULL THEN host.owner_workspace_id END
FROM hosts host
WHERE host.account_id = account.id
  AND account.owner_account_id IS NULL AND account.owner_workspace_id IS NULL;

CREATE FUNCTION hosts_fill_account_owner()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE actors
    SET owner_account_id = NEW.owner_account_id,
        owner_workspace_id = CASE WHEN NEW.owner_account_id IS NULL THEN NEW.owner_workspace_id END
    WHERE id = NEW.account_id
      AND owner_account_id IS NULL AND owner_workspace_id IS NULL
      AND account_kind IN ('agent', 'app');
    RETURN NULL;
END;
$$;

CREATE TRIGGER hosts_fill_account_owner
AFTER INSERT ON hosts
FOR EACH ROW EXECUTE FUNCTION hosts_fill_account_owner();
