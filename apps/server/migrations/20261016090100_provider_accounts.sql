-- S6: provider accounts belong to people, not projects. One account holds a
-- sign-in (its private home directory, and later its API key in encrypted
-- secrets) and is shared by every project its owner chooses to run it in.
-- An execution profile is now that choice: "this project uses this account
-- of mine". Login state lives on the account and is mirrored onto its
-- profiles, so admission checks keep reading the profile row.

CREATE TABLE provider_accounts (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    owner_actor_id UUID NOT NULL REFERENCES actors(id),
    adapter_kind TEXT NOT NULL CHECK (adapter_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120 AND name !~ '[[:cntrl:]]'
    ),
    home_path TEXT NOT NULL UNIQUE CHECK (
        char_length(home_path) BETWEEN 1 AND 4096 AND left(home_path, 1) = '/'
    ),
    status TEXT NOT NULL DEFAULT 'authentication_required' CHECK (
        status IN ('available', 'unavailable', 'authentication_required', 'disabled')
    ),
    account_state TEXT NOT NULL DEFAULT 'unknown' CHECK (
        account_state IN (
            'unknown', 'authenticated', 'authentication_required', 'logged_out'
        )
    ),
    last_health_at TIMESTAMPTZ,
    last_failure_category TEXT CHECK (
        last_failure_category IS NULL
        OR last_failure_category ~ '^[a-z][a-z0-9_]{0,63}$'
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, owner_actor_id),
    UNIQUE (organization_id, id)
);

CREATE INDEX provider_accounts_owner
    ON provider_accounts (organization_id, owner_actor_id, created_at, id);

ALTER TABLE execution_profiles
    ADD COLUMN provider_account_id UUID,
    ADD COLUMN preferred BOOLEAN NOT NULL DEFAULT false;

-- Every existing profile already has its own private home, so each becomes
-- its owner's account; nothing is merged and no sign-in is lost.
INSERT INTO provider_accounts (
    id, organization_id, owner_actor_id, adapter_kind, name, home_path,
    status, account_state, last_health_at, last_failure_category,
    created_at, updated_at
)
SELECT gen_random_uuid(), project.organization_id, profile.owner_actor_id,
       connection.kind, profile.name, profile.home_path,
       CASE WHEN profile.status = 'disabled' THEN 'authentication_required'
            ELSE profile.status END,
       profile.account_state, profile.last_health_at,
       profile.last_failure_category, profile.created_at, profile.updated_at
  FROM execution_profiles profile
  JOIN projects project ON project.id = profile.project_id
  JOIN provider_connections connection ON connection.id = profile.provider_connection_id;

UPDATE execution_profiles profile
   SET provider_account_id = account.id
  FROM provider_accounts account
 WHERE account.home_path = profile.home_path;

-- The oldest profile per person and project is the one their runs used.
UPDATE execution_profiles profile
   SET preferred = true
 WHERE profile.id = (
     SELECT candidate.id FROM execution_profiles candidate
      WHERE candidate.project_id = profile.project_id
        AND candidate.owner_actor_id = profile.owner_actor_id
      ORDER BY candidate.created_at, candidate.id
      LIMIT 1
 );

ALTER TABLE execution_profiles
    ALTER COLUMN provider_account_id SET NOT NULL,
    ADD CONSTRAINT execution_profiles_account_owner_fk
        FOREIGN KEY (provider_account_id, owner_actor_id)
        REFERENCES provider_accounts(id, owner_actor_id),
    ADD CONSTRAINT execution_profiles_project_account_unique
        UNIQUE (project_id, provider_account_id),
    DROP CONSTRAINT execution_profiles_home_path_unique,
    DROP CONSTRAINT execution_profiles_home_path_check,
    DROP COLUMN home_path;

CREATE UNIQUE INDEX execution_profiles_one_preferred_per_owner
    ON execution_profiles (project_id, owner_actor_id)
    WHERE preferred;

CREATE OR REPLACE FUNCTION enforce_execution_profile_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.provider_connection_id IS DISTINCT FROM OLD.provider_connection_id
       OR NEW.owner_actor_id IS DISTINCT FROM OLD.owner_actor_id
       OR NEW.provider_account_id IS DISTINCT FROM OLD.provider_account_id THEN
        RAISE EXCEPTION 'Execution Profile identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

-- A profile may only use an account of the same workspace and harness, and
-- starts with the account's login state.
CREATE OR REPLACE FUNCTION validate_execution_profile_account()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    account_organization_id UUID;
    account_kind TEXT;
    account_status TEXT;
    account_login_state TEXT;
    project_organization_id UUID;
    connection_kind TEXT;
BEGIN
    SELECT account.organization_id, account.adapter_kind, account.status,
           account.account_state
      INTO account_organization_id, account_kind, account_status,
           account_login_state
      FROM provider_accounts account
     WHERE account.id = NEW.provider_account_id
     FOR SHARE;
    SELECT project.organization_id INTO project_organization_id
      FROM projects project WHERE project.id = NEW.project_id;
    SELECT connection.kind INTO connection_kind
      FROM provider_connections connection
     WHERE connection.id = NEW.provider_connection_id;
    IF account_organization_id IS DISTINCT FROM project_organization_id THEN
        RAISE EXCEPTION 'Execution Profile account belongs to another workspace'
            USING ERRCODE = '23514';
    END IF;
    IF account_kind IS DISTINCT FROM connection_kind THEN
        RAISE EXCEPTION 'Execution Profile account uses another harness'
            USING ERRCODE = '23514';
    END IF;
    NEW.account_state := account_login_state;
    IF NEW.status IS DISTINCT FROM 'disabled' THEN
        NEW.status := account_status;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER execution_profiles_validate_account
BEFORE INSERT
ON execution_profiles
FOR EACH ROW
EXECUTE FUNCTION validate_execution_profile_account();

-- Copies what changed on the account to its profiles; a health check that
-- only touches last_health_at leaves the profiles' login state alone.
CREATE OR REPLACE FUNCTION mirror_provider_account_state()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.account_state IS NOT DISTINCT FROM OLD.account_state
       AND NEW.status IS NOT DISTINCT FROM OLD.status
       AND NEW.last_failure_category IS NOT DISTINCT FROM OLD.last_failure_category THEN
        RETURN NEW;
    END IF;
    UPDATE execution_profiles profile
       SET account_state = CASE
               WHEN NEW.account_state IS DISTINCT FROM OLD.account_state
               THEN NEW.account_state ELSE profile.account_state END,
           status = CASE
               WHEN profile.status = 'disabled' THEN profile.status
               WHEN NEW.status IS DISTINCT FROM OLD.status THEN NEW.status
               ELSE profile.status END,
           last_health_at = NEW.last_health_at,
           last_failure_category = NEW.last_failure_category,
           version = profile.version + 1,
           updated_at = now()
     WHERE profile.provider_account_id = NEW.id;
    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_accounts_mirror_state
AFTER UPDATE OF status, account_state, last_health_at, last_failure_category
ON provider_accounts
FOR EACH ROW
EXECUTE FUNCTION mirror_provider_account_state();

CREATE OR REPLACE FUNCTION enforce_provider_account_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.organization_id IS DISTINCT FROM OLD.organization_id
       OR NEW.owner_actor_id IS DISTINCT FROM OLD.owner_actor_id
       OR NEW.adapter_kind IS DISTINCT FROM OLD.adapter_kind
       OR NEW.home_path IS DISTINCT FROM OLD.home_path THEN
        RAISE EXCEPTION 'Provider Account identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_accounts_enforce_identity
BEFORE UPDATE
ON provider_accounts
FOR EACH ROW
EXECUTE FUNCTION enforce_provider_account_identity();
