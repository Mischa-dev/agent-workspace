-- H1.63 (strict tenancy): rows that belong to no workspace in tables that
-- otherwise do. Under strict row rules `tenancy_allows(NULL)` is false
-- outside a server-wide scope, so these rows could be neither written nor
-- read from a request. More permissive policies sit beside `tenancy`.
--
-- Audit rows: a sign-in, a sign-out, a password change or the first
-- account's bootstrap is recorded with no workspace (it happens before the
-- request has one, and an account is server-wide).
-- - any scope may write one (the server decides what it says; clients
--   never write audit rows);
-- - it is read only by the account it is about (the account in the
--   scope's `app.tenancy` value) or a server-wide scope (`tenancy`).
CREATE POLICY tenancy_server_rows_write ON audit_events FOR INSERT
    WITH CHECK (organization_id IS NULL);
CREATE POLICY tenancy_server_rows_read ON audit_events FOR SELECT
    USING (
        organization_id IS NULL
        AND actor_id::text = NULLIF(split_part(split_part(
            coalesce(current_setting('app.tenancy', true), ''), '|', 1), ':', 2), '')
    );

-- The server's own jobs: a request may queue work for no workspace (a
-- scheduler wake, a sync) in the same transaction as its change, and sees
-- the job it queued (`RETURNING id`). A job holds ids and kinds only (rule
-- 10); claiming, running and pruning stay server-wide scopes.
CREATE POLICY tenancy_server_rows_write ON background_jobs FOR INSERT
    WITH CHECK (organization_id IS NULL);
CREATE POLICY tenancy_server_rows_read ON background_jobs FOR SELECT
    USING (organization_id IS NULL);
-- A request may also call off a server job it no longer wants (keeping a
-- workspace cancels its deletion): only a queued one, and only to
-- `canceled`.
CREATE POLICY tenancy_server_rows_cancel ON background_jobs FOR UPDATE
    USING (organization_id IS NULL AND state = 'queued')
    WITH CHECK (organization_id IS NULL AND state = 'canceled');
