-- A signed-out session's live tickets stay refused on every instance: each
-- revoke (sign-out, a password change, an administrator's, a rotated
-- cookie) stages one `session.revoked` signal into the event log. Live
-- instances keep it in their window of recent frames, so a connection that
-- opens later with a ticket cut before the revoke is refused wherever it
-- lands; an instance whose window starts after the ticket sends it back to
-- `api`, which checks the session. The payload names the session only.
--
-- The log is per workspace, and the account is the audience wherever the
-- signal sits, so it goes in one workspace the account belongs to that the
-- revoking transaction may write in (row rules show only those). An account
-- in no workspace has nothing a live connection could hear.

CREATE FUNCTION stage_session_revoked() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO event_staging (
        id, workspace_id, space_id, type, class, audience, audience_account_id,
        actor_account_id, via_credential_id, requested_by_account_id, run_id,
        request_id, payload
    )
    SELECT gen_random_uuid(), home.organization_id, NULL, 'session.revoked', 'signal',
           'account', account.actor_id,
           NULLIF(current_setting('aw.actor', true), '')::uuid,
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           jsonb_build_object('sessionId', NEW.id)
    FROM users account
    CROSS JOIN LATERAL (
        SELECT membership.organization_id
        FROM organization_memberships membership
        WHERE membership.actor_id = account.actor_id AND membership.enabled
        ORDER BY membership.organization_id
        LIMIT 1
    ) home
    WHERE account.id = NEW.user_id;
    RETURN NULL;
END $$;

CREATE TRIGGER session_revoked AFTER UPDATE OF revoked_at ON sessions
    FOR EACH ROW WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL)
    EXECUTE FUNCTION stage_session_revoked();
