-- Reliability and security (2026-10-28): whoever set the server up from its
-- terminal (`bootstrap-admin`) is one of its operators, the people who see
-- Settings › Workspace › Server. Every session load asks, so find that one
-- audit row without reading through a person's whole audit history.
CREATE INDEX audit_events_bootstrap_admins
    ON audit_events (actor_id)
    WHERE action = 'identity.bootstrap_admin' AND outcome = 'succeeded';
