-- K1.30: who runs the server is a role, not "whoever has the bootstrap
-- audit row". `operator` sees Settings › Server; bootstrap-admin grants it,
-- and the operator logins in the server's configuration hold it too.
CREATE TABLE server_roles (
    account_id UUID NOT NULL REFERENCES actors(id),
    role TEXT NOT NULL CHECK (role IN ('operator')),
    granted_by_account_id UUID REFERENCES actors(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, role)
);

INSERT INTO server_roles (account_id, role)
SELECT DISTINCT actor_id, 'operator'
FROM audit_events
WHERE action = 'identity.bootstrap_admin' AND outcome = 'succeeded' AND actor_id IS NOT NULL
ON CONFLICT DO NOTHING;
