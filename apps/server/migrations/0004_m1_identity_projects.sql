-- M1 identity, authentication-session, and Project lifecycle schema.
--
-- This migration upgrades the accepted M0 data in place. It deliberately does
-- not create a User, password credential, or browser session for an existing
-- human Actor because none of those authentication facts can be inferred.

ALTER TABLE organizations
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);

ALTER TABLE projects
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD COLUMN archived_by_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT projects_archive_shape_check CHECK (
        archived_at IS NOT NULL OR archived_by_actor_id IS NULL
    );

ALTER TABLE project_memberships
    DROP CONSTRAINT project_memberships_role_check;

ALTER TABLE project_memberships
    ADD CONSTRAINT project_memberships_role_check CHECK (
        role IN ('owner', 'admin', 'member', 'viewer', 'worker')
    );

CREATE TABLE users (
    id UUID PRIMARY KEY,
    actor_id UUID NOT NULL UNIQUE REFERENCES actors(id),
    login_name TEXT NOT NULL CHECK (
        char_length(login_name) BETWEEN 3 AND 64
        AND login_name = lower(login_name)
        AND login_name ~ '^[a-z0-9][a-z0-9._-]{1,62}[a-z0-9]$'
    ),
    display_name TEXT NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    authentication_epoch BIGINT NOT NULL DEFAULT 1 CHECK (authentication_epoch > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_login_name_unique
    ON users (lower(login_name));

CREATE TABLE user_password_credentials (
    user_id UUID PRIMARY KEY REFERENCES users(id),
    password_phc TEXT NOT NULL CHECK (
        char_length(password_phc) <= 1024
        AND password_phc ~ '^\$argon2id\$v=[0-9]+\$[^$]{1,200}\$[^$]{8,}\$[^$]{16,}$'
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE organization_memberships (
    organization_id UUID NOT NULL REFERENCES organizations(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    enabled BOOLEAN NOT NULL DEFAULT true,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, actor_id)
);

CREATE INDEX organization_memberships_actor_active
    ON organization_memberships (actor_id, organization_id)
    WHERE enabled;

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    csrf_token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(csrf_token_hash) = 32),
    authentication_epoch BIGINT NOT NULL CHECK (authentication_epoch > 0),
    rotated_from_session_id UUID UNIQUE REFERENCES sessions(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    idle_expires_at TIMESTAMPTZ NOT NULL,
    absolute_expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (token_hash <> csrf_token_hash),
    CHECK (last_seen_at >= created_at),
    CHECK (idle_expires_at > created_at),
    CHECK (absolute_expires_at > created_at),
    CHECK (idle_expires_at <= absolute_expires_at),
    CHECK (last_seen_at <= absolute_expires_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE INDEX sessions_user_validity
    ON sessions (user_id, idle_expires_at, absolute_expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE project_counters (
    project_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    next_work_item_number BIGINT NOT NULL DEFAULT 1 CHECK (next_work_item_number > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_instructions (
    project_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    content TEXT NOT NULL DEFAULT '' CHECK (char_length(content) <= 100000),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, updated_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

ALTER TABLE conversations
    ADD CONSTRAINT conversations_project_id_id_unique UNIQUE (project_id, id);

CREATE TABLE project_navigation_states (
    project_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    location_kind TEXT NOT NULL DEFAULT 'project_overview' CHECK (
        location_kind IN ('project_overview', 'thread', 'settings')
    ),
    conversation_id UUID,
    context_drawer_open BOOLEAN NOT NULL DEFAULT false,
    filters JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(filters) = 'object'
    ),
    pinned_at TIMESTAMPTZ,
    last_opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, actor_id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    CHECK (
        (location_kind = 'thread' AND conversation_id IS NOT NULL)
        OR
        (location_kind IN ('project_overview', 'settings') AND conversation_id IS NULL)
    )
);

CREATE INDEX project_navigation_states_pinned
    ON project_navigation_states (actor_id, pinned_at DESC)
    WHERE pinned_at IS NOT NULL;

CREATE INDEX project_navigation_states_recent
    ON project_navigation_states (actor_id, last_opened_at DESC);

CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES actors(id),
    project_id UUID REFERENCES projects(id),
    session_id UUID REFERENCES sessions(id),
    authentication_class TEXT NOT NULL CHECK (
        char_length(authentication_class) BETWEEN 1 AND 64
        AND authentication_class ~ '^[a-z][a-z0-9_]*$'
    ),
    action TEXT NOT NULL CHECK (
        char_length(action) BETWEEN 1 AND 120
        AND action ~ '^[a-z][a-z0-9_.]*$'
    ),
    target_kind TEXT NOT NULL CHECK (
        char_length(target_kind) BETWEEN 1 AND 64
        AND target_kind ~ '^[a-z][a-z0-9_]*$'
    ),
    target_id UUID,
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) <= 128),
    correlation_id TEXT CHECK (
        correlation_id IS NULL OR char_length(correlation_id) <= 128
    ),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(metadata) = 'object'
    ),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_project_occurred
    ON audit_events (project_id, occurred_at DESC)
    WHERE project_id IS NOT NULL;

CREATE INDEX audit_events_actor_occurred
    ON audit_events (actor_id, occurred_at DESC)
    WHERE actor_id IS NOT NULL;

CREATE TABLE idempotency_keys (
    id UUID PRIMARY KEY,
    actor_id UUID NOT NULL REFERENCES actors(id),
    scope TEXT NOT NULL CHECK (
        char_length(scope) BETWEEN 1 AND 120
        AND scope ~ '^[a-z][a-z0-9_.]*$'
    ),
    key TEXT NOT NULL CHECK (char_length(key) BETWEEN 1 AND 200),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_status INTEGER CHECK (response_status BETWEEN 100 AND 599),
    response_body JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    UNIQUE (actor_id, scope, key),
    CHECK (expires_at > created_at),
    CHECK (
        (response_status IS NULL AND response_body IS NULL)
        OR
        (response_status IS NOT NULL AND response_body IS NOT NULL)
    )
);

CREATE INDEX idempotency_keys_expiry
    ON idempotency_keys (expires_at);

-- M0 Project owners are the only Organization ownership evidence available
-- during upgrade. Preserve that provable relationship without creating Users.
INSERT INTO organization_memberships (
    organization_id,
    actor_id,
    role,
    enabled
)
SELECT p.organization_id,
       pm.actor_id,
       'owner',
       bool_or(pm.enabled)
  FROM projects p
  JOIN project_memberships pm
    ON pm.project_id = p.id
   AND pm.role = 'owner'
  JOIN actors a
    ON a.id = pm.actor_id
   AND a.kind = 'human'
 GROUP BY p.organization_id, pm.actor_id
ON CONFLICT (organization_id, actor_id) DO NOTHING;

INSERT INTO project_counters (project_id)
SELECT id
  FROM projects
ON CONFLICT (project_id) DO NOTHING;

INSERT INTO project_instructions (project_id)
SELECT id
  FROM projects
ON CONFLICT (project_id) DO NOTHING;

-- An archived M0 Project may be attributed only when it has exactly one
-- enabled human owner. Ambiguous historical archives intentionally remain
-- unattributed instead of inventing an Actor decision.
WITH provable_archive_actors AS (
    SELECT pm.project_id, (array_agg(pm.actor_id))[1] AS actor_id
      FROM project_memberships pm
      JOIN actors a
        ON a.id = pm.actor_id
       AND a.kind = 'human'
     WHERE pm.role = 'owner'
       AND pm.enabled
     GROUP BY pm.project_id
    HAVING count(*) = 1
)
UPDATE projects p
   SET archived_by_actor_id = proven.actor_id
  FROM provable_archive_actors proven
 WHERE p.id = proven.project_id
   AND p.archived_at IS NOT NULL
   AND p.archived_by_actor_id IS NULL;

ALTER TABLE projects
    ADD CONSTRAINT projects_archive_actor_membership_fk
    FOREIGN KEY (id, archived_by_actor_id)
    REFERENCES project_memberships(project_id, actor_id);

-- M0 did not enforce Actor-kind/Project-role compatibility. Refuse an
-- ambiguous legacy state rather than silently rewriting historical authority.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM project_memberships pm
          JOIN actors a ON a.id = pm.actor_id
         WHERE (a.kind = 'execution_worker') <> (pm.role = 'worker')
    ) THEN
        RAISE EXCEPTION
            'existing Project membership role is incompatible with its Actor kind'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

CREATE OR REPLACE FUNCTION validate_user_actor_kind()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    actor_kind TEXT;
BEGIN
    SELECT kind
      INTO actor_kind
      FROM actors
     WHERE id = NEW.actor_id
       FOR UPDATE;

    IF actor_kind IS NOT NULL AND actor_kind <> 'human' THEN
        RAISE EXCEPTION 'a User must reference a human Actor'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER users_validate_human_actor
BEFORE INSERT OR UPDATE OF actor_id
ON users
FOR EACH ROW
EXECUTE FUNCTION validate_user_actor_kind();

CREATE OR REPLACE FUNCTION validate_project_membership_actor_role()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    actor_kind TEXT;
BEGIN
    SELECT kind
      INTO actor_kind
      FROM actors
     WHERE id = NEW.actor_id
       FOR UPDATE;

    IF actor_kind IS NOT NULL
       AND ((actor_kind = 'execution_worker') <> (NEW.role = 'worker')) THEN
        RAISE EXCEPTION 'Execution Worker Actors require the Worker Project role and vice versa'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER project_memberships_validate_actor_role
BEFORE INSERT OR UPDATE OF actor_id, role
ON project_memberships
FOR EACH ROW
EXECUTE FUNCTION validate_project_membership_actor_role();

CREATE OR REPLACE FUNCTION validate_actor_kind_change()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.kind IS NOT DISTINCT FROM OLD.kind THEN
        RETURN NEW;
    END IF;

    IF NEW.kind <> 'human'
       AND EXISTS (SELECT 1 FROM users WHERE actor_id = NEW.id) THEN
        RAISE EXCEPTION 'an Actor referenced by a User must remain human'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM project_memberships pm
         WHERE pm.actor_id = NEW.id
           AND ((NEW.kind = 'execution_worker') <> (pm.role = 'worker'))
    ) THEN
        RAISE EXCEPTION 'Actor kind is incompatible with an existing Project role'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER actors_validate_kind_change
BEFORE UPDATE OF kind
ON actors
FOR EACH ROW
EXECUTE FUNCTION validate_actor_kind_change();

CREATE OR REPLACE FUNCTION validate_project_navigation_location()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_kind TEXT;
BEGIN
    IF NEW.location_kind <> 'thread' THEN
        RETURN NEW;
    END IF;

    SELECT kind
      INTO conversation_kind
      FROM conversations
     WHERE project_id = NEW.project_id
       AND id = NEW.conversation_id;

    IF conversation_kind IS NOT NULL AND conversation_kind <> 'thread' THEN
        RAISE EXCEPTION 'a Thread navigation location must reference a Thread'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER project_navigation_states_validate_location
BEFORE INSERT OR UPDATE OF project_id, location_kind, conversation_id
ON project_navigation_states
FOR EACH ROW
EXECUTE FUNCTION validate_project_navigation_location();

CREATE OR REPLACE FUNCTION enforce_project_archive_attribution()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.archived_at IS NULL AND NEW.archived_by_actor_id IS NOT NULL THEN
        RAISE EXCEPTION 'an active Project cannot have archive attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'INSERT'
       AND NEW.archived_at IS NOT NULL
       AND NEW.archived_by_actor_id IS NULL THEN
        RAISE EXCEPTION 'a newly archived Project requires Actor attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NULL
       AND NEW.archived_at IS NOT NULL
       AND NEW.archived_by_actor_id IS NULL THEN
        RAISE EXCEPTION 'archiving a Project requires Actor attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_by_actor_id IS NOT NULL
       AND NEW.archived_at IS NOT NULL
       AND NEW.archived_by_actor_id IS NULL THEN
        RAISE EXCEPTION 'archive attribution cannot be removed'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER projects_enforce_archive_attribution
BEFORE INSERT OR UPDATE
ON projects
FOR EACH ROW
EXECUTE FUNCTION enforce_project_archive_attribution();

CREATE OR REPLACE FUNCTION enforce_project_counter_monotonicity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.next_work_item_number < OLD.next_work_item_number THEN
        RAISE EXCEPTION 'a Project counter cannot move backwards'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'DELETE'
       AND OLD.next_work_item_number > 1 THEN
        RAISE EXCEPTION 'an allocated Project counter cannot be deleted'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER project_counters_enforce_monotonicity
BEFORE UPDATE OR DELETE
ON project_counters
FOR EACH ROW
EXECUTE FUNCTION enforce_project_counter_monotonicity();

CREATE OR REPLACE FUNCTION enforce_allocated_project_key_immutability()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_key IS DISTINCT FROM OLD.project_key
       AND EXISTS (
           SELECT 1
             FROM project_counters pc
            WHERE pc.project_id = OLD.id
              AND pc.next_work_item_number > 1
       ) THEN
        RAISE EXCEPTION 'a Project key is immutable after its first counter allocation'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER projects_enforce_allocated_key_immutability
BEFORE UPDATE OF project_key
ON projects
FOR EACH ROW
EXECUTE FUNCTION enforce_allocated_project_key_immutability();
