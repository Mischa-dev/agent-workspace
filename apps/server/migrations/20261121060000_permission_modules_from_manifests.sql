-- M1.3o: the module manifests say which module owns each permission; the
-- catalogue's module column follows them. Workspace settings, members and
-- roles are the workspace module's.
UPDATE permission_catalogue
SET module = 'workspace'
WHERE module IN ('settings', 'members');
