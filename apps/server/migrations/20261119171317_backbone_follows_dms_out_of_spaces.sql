-- K5.8 and K5.20 after K4.15: a DM or group lives in no space.
--
-- 1. Its members' rows name no project, so the access signal takes the
--    workspace from the conversation itself (the direct scope, or the
--    workspace a kept one stays in): its participants' live connections
--    still work out their set again.
-- 2. A file sent there straight to the store is in no space either.

CREATE OR REPLACE FUNCTION access_changed_conversation_member() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    row_ conversation_members%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN row_ := OLD; ELSE row_ := NEW; END IF;
    PERFORM stage_access_changed(
        COALESCE((SELECT organization_id FROM projects WHERE id = row_.project_id),
                 (SELECT workspace_id FROM conversations WHERE id = row_.conversation_id)),
        CASE WHEN EXISTS (SELECT 1 FROM projects WHERE id = row_.project_id)
             THEN row_.project_id END,
        row_.actor_id);
    RETURN NULL;
END $$;

ALTER TABLE file_uploads ALTER COLUMN space_id DROP NOT NULL;
