-- K1.9: the catalogue's shape (K1 §3.3). Every key is a level on a kind of
-- thing or a named capability; holding a level holds every lower one on the
-- same kind. The new keys of §3.3 arrive with defaults that change nothing
-- for people, and every custom role (and every stored limit on apps acting
-- for someone) that holds a new key's parent gets the new key, so a role
-- saved before this migration means the same after it.
--
-- * permission_catalogue: `thing_kind` and `level` filled for the levels,
--   and one row per new key. Kept equal to `access::catalogue::PERMISSIONS`
--   by `roles_and_access::catalogue_matches_sql_and_role_allows_holds_to_the_role`.
-- * Backfill: idempotent (`NOT (key = ANY(...))`), so an interrupted run
--   resumes by running again. It also closes stored sets under level
--   implication (a role holding `tasks.edit` holds `tasks.view`), which is
--   what the server reads since K1.9, so SQL's `role_allows` and Rust agree.

UPDATE permission_catalogue AS catalogue
SET thing_kind = levels.thing_kind, level = levels.level
FROM (VALUES
    ('projects.view', 'space', 'see'),
    ('projects.manage', 'space', 'manage'),
    ('tasks.view', 'task', 'see'),
    ('tasks.edit', 'task', 'edit'),
    ('channels.post', 'conversation', 'comment'),
    ('pages.edit', 'page', 'edit'),
    ('whiteboards.edit', 'whiteboard', 'edit'),
    ('collections.edit', 'record', 'edit'),
    ('collections.build', 'collection', 'manage'),
    ('sheets.edit', 'sheet', 'edit'),
    ('code.view', 'code', 'see'),
    ('code.manage', 'code', 'manage'),
    ('agents.manage', 'agent', 'manage')
) AS levels (key, thing_kind, level)
WHERE catalogue.key = levels.key
  AND (catalogue.thing_kind IS DISTINCT FROM levels.thing_kind
       OR catalogue.level IS DISTINCT FROM levels.level);

INSERT INTO permission_catalogue (key, module, thing_kind, level, scope, required, default_for)
VALUES
    ('spaces.browse', 'projects', NULL, NULL, 'workspace', false, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('tasks.comment', 'tasks', 'task', 'comment', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('tasks.suggest', 'tasks', 'task', 'suggest', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('tasks.edit_own', 'tasks', 'task', 'edit_own', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('views.share', 'tasks', NULL, NULL, 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('dashboards.view', 'tasks', 'dashboard', 'see', 'space', false, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('dashboards.edit', 'tasks', 'dashboard', 'edit', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('conversations.see', 'channels', 'conversation', 'see', 'space', true, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('conversations.members', 'channels', NULL, NULL, 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('direct.start', 'messages', NULL, NULL, 'workspace', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('replies.steer', 'channels', NULL, NULL, 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('pages.see', 'pages', 'page', 'see', 'space', true, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('pages.comment', 'pages', 'page', 'comment', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('pages.suggest', 'pages', 'page', 'suggest', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('pages.edit_own', 'pages', 'page', 'edit_own', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('whiteboards.comment', 'whiteboards', 'whiteboard', 'comment', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('whiteboards.suggest', 'whiteboards', 'whiteboard', 'suggest', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('whiteboards.edit_own', 'whiteboards', 'whiteboard', 'edit_own', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('collections.comment', 'collections', 'record', 'comment', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('collections.suggest', 'collections', 'record', 'suggest', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('data.connect', 'collections', NULL, NULL, 'space', false, ARRAY['owner', 'admin']::text[]),
    ('data.share', 'collections', NULL, NULL, 'space', false, ARRAY['owner', 'admin']::text[]),
    ('sheets.comment', 'sheets', 'sheet', 'comment', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('sheets.suggest', 'sheets', 'sheet', 'suggest', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('sheets.edit_own', 'sheets', 'sheet', 'edit_own', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('runs.environments', 'runs', NULL, NULL, 'space', false, ARRAY['owner', 'admin']::text[]),
    ('requests.answer_sensitive', 'runs', NULL, NULL, 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('agents.create', 'agents', NULL, NULL, 'workspace', false, ARRAY['owner', 'admin']::text[]),
    ('webhooks.manage', 'settings', NULL, NULL, 'both', false, ARRAY['owner', 'admin']::text[]),
    ('automations.manage', 'settings', NULL, NULL, 'space', false, ARRAY['owner', 'admin']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, thing_kind = EXCLUDED.thing_kind, level = EXCLUDED.level,
    scope = EXCLUDED.scope, required = EXCLUDED.required, default_for = EXCLUDED.default_for;

-- Which key a stored set gains when it holds which: a new key from its
-- parent (§3.3, "Back-compatibility of new keys"), and a lower level from a
-- higher one on the same kind. NULL parent: every stored set gains it (the
-- required keys, and `spaces.browse`, whose parent `projects.view` every
-- role holds).
CREATE TEMPORARY TABLE catalogue_backfill (key TEXT NOT NULL, parent TEXT) ON COMMIT DROP;
INSERT INTO catalogue_backfill (key, parent) VALUES
    ('spaces.browse', NULL),
    ('conversations.see', NULL),
    ('pages.see', NULL),
    ('tasks.view', 'tasks.edit'),
    ('code.view', 'code.manage'),
    ('tasks.comment', 'tasks.edit'),
    ('tasks.suggest', 'tasks.edit'),
    ('tasks.edit_own', 'tasks.edit'),
    ('views.share', 'tasks.edit'),
    ('dashboards.view', 'tasks.view'),
    ('dashboards.edit', 'tasks.edit'),
    ('conversations.members', 'channels.post'),
    ('direct.start', 'channels.post'),
    ('replies.steer', 'channels.post'),
    ('pages.comment', 'pages.edit'),
    ('pages.suggest', 'pages.edit'),
    ('pages.edit_own', 'pages.edit'),
    ('whiteboards.comment', 'whiteboards.edit'),
    ('whiteboards.suggest', 'whiteboards.edit'),
    ('whiteboards.edit_own', 'whiteboards.edit'),
    ('collections.comment', 'collections.edit'),
    ('collections.suggest', 'collections.edit'),
    ('data.connect', 'collections.build'),
    ('data.share', 'collections.build'),
    ('sheets.comment', 'sheets.edit'),
    ('sheets.suggest', 'sheets.edit'),
    ('sheets.edit_own', 'sheets.edit'),
    ('runs.environments', 'settings.manage'),
    ('requests.answer_sensitive', 'runs.approve'),
    ('agents.create', 'agents.manage'),
    ('webhooks.manage', 'settings.manage'),
    ('automations.manage', 'settings.manage');

-- `dashboards.view` comes from `tasks.view`, which `tasks.edit` brings, so
-- it is added after the first pass gave `tasks.view`: run the pass twice.
DO $$
BEGIN
    FOR pass IN 1..2 LOOP
        UPDATE workspace_roles AS role
        SET permissions = role.permissions || ARRAY(
                SELECT backfill.key FROM catalogue_backfill backfill
                WHERE (backfill.parent IS NULL OR backfill.parent = ANY(role.permissions))
                  AND NOT (backfill.key = ANY(role.permissions))
                ORDER BY backfill.key)
        WHERE NOT role.built_in
          AND EXISTS (
              SELECT 1 FROM catalogue_backfill backfill
              WHERE (backfill.parent IS NULL OR backfill.parent = ANY(role.permissions))
                AND NOT (backfill.key = ANY(role.permissions)));

        UPDATE workspace_roles AS role
        SET agent_limit = role.agent_limit || ARRAY(
                SELECT backfill.key FROM catalogue_backfill backfill
                WHERE (backfill.parent IS NULL OR backfill.parent = ANY(role.agent_limit))
                  AND NOT (backfill.key = ANY(role.agent_limit))
                ORDER BY backfill.key)
        WHERE role.agent_limit IS NOT NULL
          AND EXISTS (
              SELECT 1 FROM catalogue_backfill backfill
              WHERE (backfill.parent IS NULL OR backfill.parent = ANY(role.agent_limit))
                AND NOT (backfill.key = ANY(role.agent_limit)));

        UPDATE organization_memberships AS membership
        SET agent_limit = membership.agent_limit || ARRAY(
                SELECT backfill.key FROM catalogue_backfill backfill
                WHERE (backfill.parent IS NULL OR backfill.parent = ANY(membership.agent_limit))
                  AND NOT (backfill.key = ANY(membership.agent_limit))
                ORDER BY backfill.key)
        WHERE membership.agent_limit IS NOT NULL
          AND EXISTS (
              SELECT 1 FROM catalogue_backfill backfill
              WHERE (backfill.parent IS NULL OR backfill.parent = ANY(membership.agent_limit))
                AND NOT (backfill.key = ANY(membership.agent_limit)));
    END LOOP;
END
$$;
