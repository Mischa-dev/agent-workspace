-- H1.24: "Delete the workspace" (`workspace.delete`) in the SQL permission
-- catalogue, beside `policy::PERMISSIONS`. Owners hold it by default.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('workspace.delete', 'settings', 'workspace', false, ARRAY['owner']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;
