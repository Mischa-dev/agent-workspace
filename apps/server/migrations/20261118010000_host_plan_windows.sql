-- A1.24: plan windows over the host protocol.
--
-- A host names the provider accounts it runs on in `describe`
-- (`accounts: [{ref, name, plan?}]`) and reports their plan windows on
-- `POST /api/v1/host/windows`. Each described account is a provider
-- account bound to the host by its reference; it has no home on this
-- server (only the built-in host's own accounts do), so the server never
-- reads it itself. Its windows go into the same usage tables the built-in
-- host's reads fill, so the capacity alerts, the inbox and push notices and
-- the suggested batch work the same whichever host reports.

ALTER TABLE provider_accounts
    ADD COLUMN host_id UUID REFERENCES hosts(id),
    ADD COLUMN host_account_ref TEXT CHECK (
        host_account_ref IS NULL
        OR (char_length(host_account_ref) BETWEEN 1 AND 200 AND host_account_ref !~ '[[:cntrl:]]')
    ),
    ALTER COLUMN home_path DROP NOT NULL,
    -- An account is either the built-in host's own (a home here) or one a
    -- host reports (its reference there).
    ADD CONSTRAINT provider_accounts_home_or_host CHECK (
        (home_path IS NULL) = (host_account_ref IS NOT NULL)
        AND (host_account_ref IS NULL OR host_id IS NOT NULL)
    );

CREATE UNIQUE INDEX provider_accounts_host_ref
    ON provider_accounts (host_id, host_account_ref)
    WHERE host_account_ref IS NOT NULL;
