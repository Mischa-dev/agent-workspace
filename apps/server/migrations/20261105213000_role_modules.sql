-- AWD-75 (modules) on AWD-74 (roles): a role can turn modules off for the
-- people and agents who hold it, on top of the workspace's own switches
-- (`modules.<id>` settings). It only narrows: a module the workspace has off
-- stays off whatever a role says.
--
-- * workspace_roles.modules_off: module ids (such as 'runs') off for holders.
--   The server checks they're modules that can be switched off; here, a
--   bounded list of lower-case ids. The owner role never narrows: owners
--   always have every module the workspace has on.

ALTER TABLE workspace_roles
    ADD COLUMN modules_off TEXT[] NOT NULL DEFAULT '{}' CHECK (
        cardinality(modules_off) <= 32
        AND array_position(modules_off, NULL) IS NULL
        AND array_to_string(modules_off, ',') ~ '^([a-z_]{1,40}(,[a-z_]{1,40})*)?$'
    ),
    ADD CONSTRAINT workspace_roles_owner_keeps_modules CHECK (
        key <> 'owner' OR cardinality(modules_off) = 0
    );
