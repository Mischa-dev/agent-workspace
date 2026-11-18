-- H1.63 (strict tenancy): audit rows that belong to no workspace.
--
-- A sign-in, a sign-out, a password change or the first account's
-- bootstrap is recorded with no workspace: it happens before the request
-- has one, and an account is server-wide. Under strict row rules
-- `tenancy_allows(NULL)` is false outside a server-wide scope, so these
-- rows could neither be written nor shown on the account's own security
-- page. Two more permissive policies sit beside `tenancy`:
--
-- - any scope may write a row of no workspace (the server decides what it
--   says; clients never write audit rows);
-- - such a row is read only by the account it is about (the account in
--   the scope's `app.tenancy` value) or a server-wide scope (`tenancy`).
CREATE POLICY tenancy_server_rows_write ON audit_events FOR INSERT
    WITH CHECK (organization_id IS NULL);
CREATE POLICY tenancy_server_rows_read ON audit_events FOR SELECT
    USING (
        organization_id IS NULL
        AND actor_id::text = NULLIF(split_part(split_part(
            coalesce(current_setting('app.tenancy', true), ''), '|', 1), ':', 2), '')
    );
