-- K5.8: live connections decide what they hear when they subscribe and again
-- when access changes. Every change to who may read what stages one
-- `account.access_changed` signal for the account, whatever wrote it (a
-- route, a tool, an import, workspace closure), so a connection works out
-- its set again before it hears anything written after the change. The
-- payload names no text; the account is the audience.

CREATE FUNCTION stage_access_changed(p_workspace UUID, p_space UUID, p_account UUID)
RETURNS VOID LANGUAGE sql AS $$
    INSERT INTO event_staging (
        id, workspace_id, space_id, type, class, audience, audience_account_id,
        actor_account_id, via_credential_id, requested_by_account_id, run_id,
        request_id, payload
    )
    SELECT gen_random_uuid(), p_workspace, p_space, 'account.access_changed', 'signal',
           'account', p_account,
           NULLIF(current_setting('aw.actor', true), '')::uuid,
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           jsonb_build_object('accountId', p_account)
    WHERE p_workspace IS NOT NULL AND p_account IS NOT NULL
$$;

CREATE FUNCTION access_changed_space_membership() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    row_ project_memberships%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN row_ := OLD; ELSE row_ := NEW; END IF;
    PERFORM stage_access_changed(
        (SELECT organization_id FROM projects WHERE id = row_.project_id),
        CASE WHEN EXISTS (SELECT 1 FROM projects WHERE id = row_.project_id)
             THEN row_.project_id END,
        row_.actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE OF enabled, role, role_id
    ON project_memberships FOR EACH ROW EXECUTE FUNCTION access_changed_space_membership();

CREATE FUNCTION access_changed_workspace_membership() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    row_ organization_memberships%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN row_ := OLD; ELSE row_ := NEW; END IF;
    IF EXISTS (SELECT 1 FROM organizations WHERE id = row_.organization_id) THEN
        PERFORM stage_access_changed(row_.organization_id, NULL, row_.actor_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE OF enabled, role, role_id
    ON organization_memberships FOR EACH ROW EXECUTE FUNCTION access_changed_workspace_membership();

CREATE FUNCTION access_changed_conversation_member() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    row_ conversation_members%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN row_ := OLD; ELSE row_ := NEW; END IF;
    PERFORM stage_access_changed(
        (SELECT organization_id FROM projects WHERE id = row_.project_id),
        CASE WHEN EXISTS (SELECT 1 FROM projects WHERE id = row_.project_id)
             THEN row_.project_id END,
        row_.actor_id);
    RETURN NULL;
END $$;
CREATE TRIGGER access_changed AFTER INSERT OR DELETE
    ON conversation_members FOR EACH ROW EXECUTE FUNCTION access_changed_conversation_member();
