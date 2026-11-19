-- K1.25: acting context on every action. Every write knows who acted,
-- through which credential, for whom and in which run; the audit trail
-- keeps all of it, and a role can be held to what its holder's requester
-- could have done.
ALTER TABLE workspace_roles
    -- When a holder acts on someone's request (a run's requester), what it
    -- may do is also limited to what that account may do.
    ADD COLUMN act_within_requester BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE audit_events
    -- The credential the call came through (a secret, a token or an app's
    -- grant), with no foreign key: tokens lapse and are cleared.
    ADD COLUMN via_credential_id UUID,
    ADD COLUMN requested_by_account_id UUID REFERENCES actors(id),
    ADD COLUMN run_id UUID;

-- What the edge set for this transaction (transaction-local settings, so
-- pooled connections never carry them over).
CREATE FUNCTION current_via_credential() RETURNS UUID
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(
        NULLIF(current_setting('aw.via_credential', true), ''),
        NULLIF(current_setting('aw.via_grant', true), '')
    )::uuid
$$;

CREATE FUNCTION current_requested_by() RETURNS UUID
LANGUAGE sql STABLE AS $$
    SELECT NULLIF(current_setting('aw.requested_by', true), '')::uuid
$$;
