-- H1: any registered harness adapter can be stored. The database only checks
-- the shape of an adapter kind, host and version; the server validates them
-- against the adapters it has registered (provider/registry.rs), so adding a
-- harness needs no migration. Version pins become ranges in code.

ALTER TABLE provider_connections
    DROP CONSTRAINT IF EXISTS provider_connections_kind_check,
    DROP CONSTRAINT IF EXISTS provider_connections_host_check,
    DROP CONSTRAINT IF EXISTS provider_connections_binary_version_check,
    ADD CONSTRAINT provider_connections_kind_shape_check CHECK (
        kind ~ '^[a-z][a-z0-9_]{0,63}$'
    ),
    ADD CONSTRAINT provider_connections_host_shape_check CHECK (
        host ~ '^[a-z][a-z0-9_]{0,63}$'
    ),
    ADD CONSTRAINT provider_connections_binary_version_shape_check CHECK (
        binary_version ~ '^[0-9]+(\.[0-9]+){0,3}([-+][0-9A-Za-z.-]{1,64})?$'
    );

-- A connection keeps its project, kind and host for life; its binary version
-- may move within the range its adapter accepts.
CREATE OR REPLACE FUNCTION enforce_provider_connection_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.kind IS DISTINCT FROM OLD.kind
       OR NEW.host IS DISTINCT FROM OLD.host THEN
        RAISE EXCEPTION 'Provider Connection identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

ALTER TABLE execution_profiles
    RENAME COLUMN codex_home_path TO home_path;
ALTER TABLE execution_profiles
    RENAME CONSTRAINT execution_profiles_codex_home_path_check
    TO execution_profiles_home_path_check;
ALTER TABLE execution_profiles
    RENAME CONSTRAINT execution_profiles_codex_home_path_unique
    TO execution_profiles_home_path_unique;

CREATE OR REPLACE FUNCTION enforce_execution_profile_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.provider_connection_id IS DISTINCT FROM OLD.provider_connection_id
       OR NEW.owner_actor_id IS DISTINCT FROM OLD.owner_actor_id
       OR NEW.home_path IS DISTINCT FROM OLD.home_path THEN
        RAISE EXCEPTION 'Execution Profile identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
