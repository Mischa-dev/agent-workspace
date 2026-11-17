-- Observe mode notes a row the scope would refuse once per transaction, not
-- once per row. A query that read many rows outside its scope (sign-in
-- lookups before the scope is known, a list across workspaces) wrote one
-- log line for each of them: tens of thousands a test run, and as many on
-- a busy server. The first row is enough to find the statement and its call
-- site, which is what a strict-mode rehearsal needs (the log line carries
-- the statement).
--
-- PARALLEL RESTRICTED: it sets a transaction-local note, which a parallel
-- worker can't.
CREATE OR REPLACE FUNCTION tenancy_refused(workspace UUID) RETURNS BOOLEAN
LANGUAGE plpgsql STABLE PARALLEL RESTRICTED AS $$
BEGIN
    IF coalesce(current_setting('app.tenancy_strict', true), '') = 'on' THEN
        RETURN false;
    END IF;
    IF coalesce(current_setting('app.tenancy_noted', true), '') = '' THEN
        PERFORM pg_catalog.set_config('app.tenancy_noted', 'on', true);
        RAISE LOG 'tenancy: a row of workspace % outside a % scope (noted once per transaction)',
            workspace, split_part(coalesce(current_setting('app.tenancy', true), ''), ':', 1);
    END IF;
    RETURN true;
END;
$$;
