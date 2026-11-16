-- M1.2: module state in a table (docs/plans/master/plans/M1-modules.md,
-- section 3.3). Until now a workspace switched a module with a
-- `modules.<id>` row in `settings`; from here `module_installs` holds it.
--
-- * module_installs: one row per module switched in a workspace (space_id
--   NULL) or narrowed in one of its spaces (space_id set). A core module
--   with no row is on, as before, so new workspaces and new modules need no
--   rows. A space row can only turn a module off there; the server refuses
--   turning one on in a space while the workspace has it off.
-- * Backfill: every workspace switch in `settings`, with its version.
-- * Trigger `settings_module_switch_mirror`: a write of a `modules.<id>`
--   setting at workspace scope through the old path (an older server, a
--   restore script) is mirrored into the table, so nothing that still
--   writes the setting falls out of step. The server itself writes only the
--   table now; the settings rows stay as they were and are no longer read.
-- * workspace_roles.modules_off: the check widens from 32 ids of `[a-z_]` to
--   200 module keys, so a role can name a built or connected module
--   (`local.playtest`) once those exist (M1.6).

CREATE TABLE module_installs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- NULL: the workspace's own switch. Set: a narrowing in that space.
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL CHECK (
        module_key ~ '^[a-z][a-z0-9_]{0,39}(\.[a-z][a-z0-9_]{0,39})?$'
    ),
    state TEXT NOT NULL CHECK (state IN ('on', 'off')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    changed_by_actor_id UUID REFERENCES actors(id),
    -- The connected app a person switched it through, when they did.
    via_app_actor_id UUID REFERENCES actors(id),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX module_installs_workspace
    ON module_installs (workspace_id, module_key) WHERE space_id IS NULL;
CREATE UNIQUE INDEX module_installs_space
    ON module_installs (workspace_id, space_id, module_key) WHERE space_id IS NOT NULL;
CREATE INDEX module_installs_by_space
    ON module_installs (space_id) WHERE space_id IS NOT NULL;

INSERT INTO module_installs (
    id, workspace_id, module_key, state, version, changed_by_actor_id, changed_at
)
SELECT gen_random_uuid(),
       setting.organization_id,
       substring(setting.key FROM 9),
       CASE WHEN setting.value = 'false'::jsonb THEN 'off' ELSE 'on' END,
       setting.version,
       setting.updated_by_actor_id,
       setting.updated_at
FROM settings setting
WHERE setting.scope = 'workspace'
  AND setting.key LIKE 'modules.%'
  AND jsonb_typeof(setting.value) = 'boolean'
  AND substring(setting.key FROM 9) ~ '^[a-z][a-z0-9_]{0,39}$'
ON CONFLICT DO NOTHING;

CREATE FUNCTION module_installs_mirror_setting() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        -- An unset switch is on.
        IF OLD.scope = 'workspace' AND OLD.key LIKE 'modules.%' THEN
            UPDATE module_installs
            SET state = 'on', version = version + 1, changed_at = now()
            WHERE workspace_id = OLD.organization_id
              AND space_id IS NULL
              AND module_key = substring(OLD.key FROM 9)
              AND state <> 'on';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.scope = 'workspace'
       AND NEW.key LIKE 'modules.%'
       AND jsonb_typeof(NEW.value) = 'boolean'
       AND substring(NEW.key FROM 9) ~ '^[a-z][a-z0-9_]{0,39}$' THEN
        INSERT INTO module_installs (
            id, workspace_id, module_key, state, changed_by_actor_id
        ) VALUES (
            gen_random_uuid(),
            NEW.organization_id,
            substring(NEW.key FROM 9),
            CASE WHEN NEW.value = 'false'::jsonb THEN 'off' ELSE 'on' END,
            NEW.updated_by_actor_id
        )
        ON CONFLICT (workspace_id, module_key) WHERE space_id IS NULL DO UPDATE
        SET state = EXCLUDED.state,
            version = module_installs.version + 1,
            changed_by_actor_id = EXCLUDED.changed_by_actor_id,
            changed_at = now()
        WHERE module_installs.state <> EXCLUDED.state;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER settings_module_switch_mirror
    AFTER INSERT OR UPDATE OR DELETE ON settings
    FOR EACH ROW EXECUTE FUNCTION module_installs_mirror_setting();

-- A role names module keys, built and connected ones included.
DO $$
DECLARE
    found TEXT;
BEGIN
    FOR found IN
        SELECT constraint_row.conname
        FROM pg_constraint constraint_row
        WHERE constraint_row.conrelid = 'workspace_roles'::regclass
          AND constraint_row.contype = 'c'
          AND pg_get_constraintdef(constraint_row.oid) LIKE '%array_to_string(modules_off%'
    LOOP
        EXECUTE format('ALTER TABLE workspace_roles DROP CONSTRAINT %I', found);
    END LOOP;
END;
$$;

ALTER TABLE workspace_roles
    ADD CONSTRAINT workspace_roles_modules_off_keys CHECK (
        cardinality(modules_off) <= 200
        AND array_position(modules_off, NULL) IS NULL
        AND array_to_string(modules_off, ',')
            ~ '^([a-z_][a-z0-9_]{0,39}(\.[a-z][a-z0-9_]{0,39})?(,[a-z_][a-z0-9_]{0,39}(\.[a-z][a-z0-9_]{0,39})?)*)?$'
    );
