-- C1.27: sharing a page with an account, changing the share's level and
-- taking it back change what that account may read, so each stages the same
-- `account.access_changed` signal a membership change gives (K5.8): the
-- account's live connections check the documents they watch again, and a
-- page no longer shared stops sending its changes at once. A link has no
-- account and no live connection of its own, so it stages nothing.

CREATE FUNCTION access_changed_thing_grant() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    row_ thing_grants%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN row_ := OLD; ELSE row_ := NEW; END IF;
    IF row_.grantee_account_id IS NOT NULL
       AND EXISTS (SELECT 1 FROM organizations WHERE id = row_.workspace_id) THEN
        PERFORM stage_access_changed(
            row_.workspace_id,
            CASE WHEN EXISTS (SELECT 1 FROM projects WHERE id = row_.space_id)
                 THEN row_.space_id END,
            row_.grantee_account_id);
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE OF revoked_at, level, expires_at
    ON thing_grants FOR EACH ROW EXECUTE FUNCTION access_changed_thing_grant();
