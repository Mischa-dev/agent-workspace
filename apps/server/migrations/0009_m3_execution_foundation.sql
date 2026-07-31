-- M3 Execution Worker, profile, repository, and Agent Run foundation.
--
-- This migration adopts the accepted M0 execution rows without claiming that
-- a database migration has re-verified provider authentication or on-disk Git
-- state. Project scope is derived from canonical parents for compatibility
-- with the M0 insert path, then protected with composite foreign keys.

LOCK TABLE provider_connections,
           execution_profiles,
           execution_workers,
           repositories,
           conversations,
           project_memberships,
           repository_workspaces,
           provider_sessions,
           agent_runs
IN SHARE ROW EXCLUSIVE MODE;

-- Refuse ambiguous legacy ownership rather than silently moving execution
-- records between Projects.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM execution_profiles profile
          JOIN provider_connections connection
            ON connection.id = profile.provider_connection_id
          LEFT JOIN project_memberships owner_membership
            ON owner_membership.project_id = profile.project_id
           AND owner_membership.actor_id = profile.owner_actor_id
         WHERE connection.project_id IS DISTINCT FROM profile.project_id
            OR owner_membership.actor_id IS NULL
    ) THEN
        RAISE EXCEPTION 'existing Execution Profile ownership is cross-Project or unattributed'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM execution_workers worker
          JOIN provider_connections connection
            ON connection.id = worker.provider_connection_id
          JOIN execution_profiles profile
            ON profile.id = worker.execution_profile_id
          JOIN actors actor
            ON actor.id = worker.actor_id
          LEFT JOIN project_memberships membership
            ON membership.project_id = worker.project_id
           AND membership.actor_id = worker.actor_id
         WHERE connection.project_id IS DISTINCT FROM worker.project_id
            OR profile.project_id IS DISTINCT FROM worker.project_id
            OR profile.provider_connection_id IS DISTINCT FROM worker.provider_connection_id
            OR membership.actor_id IS NULL
            OR membership.role IS DISTINCT FROM 'worker'
            OR actor.kind IS DISTINCT FROM 'execution_worker'
            OR (
                worker.enabled
                AND (
                    actor.status IS DISTINCT FROM 'active'
                    OR membership.enabled IS DISTINCT FROM true
                    OR connection.enabled IS DISTINCT FROM true
                    OR profile.status IS DISTINCT FROM 'available'
                )
            )
    ) THEN
        RAISE EXCEPTION 'existing Execution Worker ownership is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM repository_workspaces workspace
          JOIN repositories repository ON repository.id = workspace.repository_id
          JOIN conversations conversation ON conversation.id = workspace.conversation_id
         WHERE repository.project_id IS DISTINCT FROM conversation.project_id
            OR conversation.kind IS DISTINCT FROM 'thread'
            OR conversation.repository_id IS DISTINCT FROM workspace.repository_id
    ) THEN
        RAISE EXCEPTION 'existing Repository Workspace ownership is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM provider_sessions session
          JOIN conversations conversation ON conversation.id = session.conversation_id
          JOIN execution_profiles profile ON profile.id = session.execution_profile_id
         WHERE conversation.project_id IS DISTINCT FROM profile.project_id
            OR conversation.kind IS DISTINCT FROM 'thread'
            OR conversation.repository_id IS NULL
    ) THEN
        RAISE EXCEPTION 'existing Provider Session ownership is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM agent_runs run
          JOIN conversations conversation ON conversation.id = run.conversation_id
          JOIN execution_workers worker ON worker.id = run.execution_worker_id
          JOIN execution_profiles profile ON profile.id = run.execution_profile_id
          JOIN repositories repository ON repository.id = run.repository_id
          LEFT JOIN project_memberships requester
            ON requester.project_id = conversation.project_id
           AND requester.actor_id = run.requesting_actor_id
          LEFT JOIN repository_workspaces workspace
            ON workspace.id = run.repository_workspace_id
          LEFT JOIN provider_sessions session
            ON session.id = run.provider_session_id
         WHERE conversation.kind IS DISTINCT FROM 'thread'
            OR conversation.repository_id IS DISTINCT FROM run.repository_id
            OR worker.project_id IS DISTINCT FROM conversation.project_id
            OR profile.project_id IS DISTINCT FROM conversation.project_id
            OR repository.project_id IS DISTINCT FROM conversation.project_id
            OR requester.actor_id IS NULL
            OR (
                run.repository_workspace_id IS NOT NULL
                AND (
                    workspace.id IS NULL
                    OR workspace.repository_id IS DISTINCT FROM run.repository_id
                    OR workspace.conversation_id IS DISTINCT FROM run.conversation_id
                )
            )
            OR (
                run.provider_session_id IS NOT NULL
                AND (
                    session.id IS NULL
                    OR session.conversation_id IS DISTINCT FROM run.conversation_id
                    OR session.execution_profile_id IS DISTINCT FROM run.execution_profile_id
                )
            )
    ) THEN
        RAISE EXCEPTION 'existing Agent Run ownership is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM repositories
         GROUP BY project_id, lower(owner), lower(name)
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'existing Repository identities collide case-insensitively'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM agent_runs
         WHERE state IN ('starting', 'working', 'blocked', 'recovering')
         GROUP BY execution_profile_id
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'an Execution Profile already owns multiple active provider turns'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

ALTER TABLE provider_connections
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD COLUMN binary_version TEXT NOT NULL DEFAULT '0.146.0',
    ADD CONSTRAINT provider_connections_binary_version_check CHECK (
        binary_version = '0.146.0'
    ),
    ADD CONSTRAINT provider_connections_capabilities_shape_check CHECK (
        jsonb_typeof(capabilities) = 'object'
        AND octet_length(capabilities::text) <= 8192
    );

-- The M0 fixture used short capability names. All existing connections are
-- the pinned local Codex connection, so adopt the documented M3 vocabulary.
UPDATE provider_connections
   SET capabilities = jsonb_build_object(
           'supports_streaming', true,
           'supports_commands', true,
           'supports_file_changes', true,
           'supports_approvals', true,
           'supports_questions', true,
           'supports_resume', true,
           'supports_interrupt', true,
           'supports_pull_request_creation', false,
           'supports_remote_execution', false
       ),
       updated_at = now();

ALTER TABLE execution_profiles
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD COLUMN account_state TEXT NOT NULL DEFAULT 'unknown' CHECK (
        account_state IN (
            'unknown', 'authenticated', 'authentication_required', 'logged_out'
        )
    ),
    ADD COLUMN max_active_turns SMALLINT NOT NULL DEFAULT 1 CHECK (
        max_active_turns = 1
    ),
    ADD COLUMN last_health_at TIMESTAMPTZ,
    ADD COLUMN last_failure_category TEXT,
    ADD CONSTRAINT execution_profiles_name_length_check CHECK (
        char_length(name) BETWEEN 1 AND 120
    ),
    ADD CONSTRAINT execution_profiles_codex_home_path_check CHECK (
        char_length(codex_home_path) BETWEEN 1 AND 4096
        AND left(codex_home_path, 1) = '/'
    ),
    ADD CONSTRAINT execution_profiles_failure_category_check CHECK (
        last_failure_category IS NULL
        OR (
            char_length(last_failure_category) BETWEEN 1 AND 64
            AND last_failure_category ~ '^[a-z][a-z0-9_]{0,63}$'
        )
    );

ALTER TABLE execution_workers
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);

ALTER TABLE repositories
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD COLUMN preparation_state TEXT NOT NULL DEFAULT 'pending' CHECK (
        preparation_state IN ('pending', 'ready', 'failed')
    ),
    ADD COLUMN prepared_at TIMESTAMPTZ,
    ADD COLUMN preparation_failure_category TEXT,
    ADD COLUMN archived_at TIMESTAMPTZ,
    ADD COLUMN archived_by_actor_id UUID,
    ADD CONSTRAINT repositories_owner_length_check CHECK (
        char_length(owner) BETWEEN 1 AND 100
    ),
    ADD CONSTRAINT repositories_name_length_check CHECK (
        char_length(name) BETWEEN 1 AND 100
    ),
    ADD CONSTRAINT repositories_clone_url_length_check CHECK (
        char_length(clone_url) BETWEEN 1 AND 2048
    ),
    ADD CONSTRAINT repositories_default_branch_length_check CHECK (
        char_length(default_branch) BETWEEN 1 AND 255
    ),
    ADD CONSTRAINT repositories_mirror_path_check CHECK (
        char_length(mirror_path) BETWEEN 1 AND 4096
        AND left(mirror_path, 1) = '/'
    ),
    ADD CONSTRAINT repositories_failure_category_check CHECK (
        preparation_failure_category IS NULL
        OR (
            char_length(preparation_failure_category) BETWEEN 1 AND 64
            AND preparation_failure_category ~ '^[a-z][a-z0-9_]{0,63}$'
        )
    ),
    ADD CONSTRAINT repositories_preparation_shape_check CHECK (
        (
            preparation_state = 'pending'
            AND prepared_at IS NULL
            AND preparation_failure_category IS NULL
        )
        OR
        (
            preparation_state = 'ready'
            AND prepared_at IS NOT NULL
            AND preparation_failure_category IS NULL
        )
        OR
        (
            preparation_state = 'failed'
            AND prepared_at IS NULL
            AND preparation_failure_category IS NOT NULL
        )
    ),
    ADD CONSTRAINT repositories_archive_shape_check CHECK (
        (archived_at IS NULL AND archived_by_actor_id IS NULL)
        OR
        (archived_at IS NOT NULL AND archived_by_actor_id IS NOT NULL AND NOT enabled)
    );

ALTER TABLE repository_workspaces
    ADD COLUMN project_id UUID;

UPDATE repository_workspaces workspace
   SET project_id = conversation.project_id
  FROM conversations conversation
 WHERE conversation.id = workspace.conversation_id;

ALTER TABLE repository_workspaces
    ALTER COLUMN project_id SET NOT NULL;

ALTER TABLE provider_sessions
    ADD COLUMN project_id UUID,
    ADD COLUMN repository_id UUID,
    ADD CONSTRAINT provider_sessions_provider_thread_id_length_check CHECK (
        char_length(provider_thread_id) BETWEEN 1 AND 512
    );

UPDATE provider_sessions session
   SET project_id = conversation.project_id,
       repository_id = conversation.repository_id
  FROM conversations conversation
 WHERE conversation.id = session.conversation_id;

ALTER TABLE provider_sessions
    ALTER COLUMN project_id SET NOT NULL,
    ALTER COLUMN repository_id SET NOT NULL;

ALTER TABLE agent_runs
    ADD COLUMN project_id UUID,
    ADD COLUMN provider_turn_id TEXT,
    ADD CONSTRAINT agent_runs_authorization_snapshot_shape_check CHECK (
        jsonb_typeof(authorization_snapshot) = 'object'
        AND octet_length(authorization_snapshot::text) <= 65536
    ),
    ADD CONSTRAINT agent_runs_failure_reason_length_check CHECK (
        failure_reason IS NULL OR char_length(failure_reason) <= 2000
    ),
    ADD CONSTRAINT agent_runs_provider_turn_id_length_check CHECK (
        provider_turn_id IS NULL
        OR char_length(provider_turn_id) BETWEEN 1 AND 512
    );

UPDATE agent_runs run
   SET project_id = conversation.project_id
  FROM conversations conversation
 WHERE conversation.id = run.conversation_id;

ALTER TABLE agent_runs
    ALTER COLUMN project_id SET NOT NULL;

-- Composite candidate keys make Project ownership enforceable without
-- replacing the UUID primary keys used by existing code.
ALTER TABLE conversations
    ADD CONSTRAINT conversations_project_id_id_repository_id_unique
        UNIQUE (project_id, id, repository_id);

ALTER TABLE provider_connections
    ADD CONSTRAINT provider_connections_project_id_id_unique
        UNIQUE (project_id, id);

ALTER TABLE execution_profiles
    ADD CONSTRAINT execution_profiles_project_id_id_unique
        UNIQUE (project_id, id),
    ADD CONSTRAINT execution_profiles_project_id_id_connection_unique
        UNIQUE (project_id, id, provider_connection_id);

ALTER TABLE execution_workers
    ADD CONSTRAINT execution_workers_project_id_id_unique
        UNIQUE (project_id, id);

ALTER TABLE repository_workspaces
    ADD CONSTRAINT repository_workspaces_project_id_id_unique
        UNIQUE (project_id, id),
    ADD CONSTRAINT repository_workspaces_scope_unique
        UNIQUE (project_id, id, repository_id, conversation_id);

ALTER TABLE provider_sessions
    ADD CONSTRAINT provider_sessions_project_id_id_unique
        UNIQUE (project_id, id),
    ADD CONSTRAINT provider_sessions_scope_unique
        UNIQUE (
            project_id, id, conversation_id, execution_profile_id, repository_id
        );

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_project_id_id_unique UNIQUE (project_id, id);

ALTER TABLE execution_profiles
    ADD CONSTRAINT execution_profiles_provider_project_fk
        FOREIGN KEY (project_id, provider_connection_id)
        REFERENCES provider_connections(project_id, id),
    ADD CONSTRAINT execution_profiles_owner_membership_fk
        FOREIGN KEY (project_id, owner_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

ALTER TABLE execution_workers
    ADD CONSTRAINT execution_workers_actor_membership_fk
        FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT execution_workers_provider_project_fk
        FOREIGN KEY (project_id, provider_connection_id)
        REFERENCES provider_connections(project_id, id),
    ADD CONSTRAINT execution_workers_profile_connection_fk
        FOREIGN KEY (project_id, execution_profile_id, provider_connection_id)
        REFERENCES execution_profiles(project_id, id, provider_connection_id);

ALTER TABLE repositories
    ADD CONSTRAINT repositories_archive_actor_membership_fk
        FOREIGN KEY (project_id, archived_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id);

ALTER TABLE repository_workspaces
    ADD CONSTRAINT repository_workspaces_repository_project_fk
        FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    ADD CONSTRAINT repository_workspaces_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id)
        REFERENCES conversations(project_id, id, repository_id);

ALTER TABLE provider_sessions
    ADD CONSTRAINT provider_sessions_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id)
        REFERENCES conversations(project_id, id, repository_id),
    ADD CONSTRAINT provider_sessions_profile_project_fk
        FOREIGN KEY (project_id, execution_profile_id)
        REFERENCES execution_profiles(project_id, id),
    ADD CONSTRAINT provider_sessions_repository_project_fk
        FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id);

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_conversation_repository_fk
        FOREIGN KEY (project_id, conversation_id, repository_id)
        REFERENCES conversations(project_id, id, repository_id),
    ADD CONSTRAINT agent_runs_requester_membership_fk
        FOREIGN KEY (project_id, requesting_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT agent_runs_worker_project_fk
        FOREIGN KEY (project_id, execution_worker_id)
        REFERENCES execution_workers(project_id, id),
    ADD CONSTRAINT agent_runs_profile_project_fk
        FOREIGN KEY (project_id, execution_profile_id)
        REFERENCES execution_profiles(project_id, id),
    ADD CONSTRAINT agent_runs_repository_project_fk
        FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    ADD CONSTRAINT agent_runs_workspace_scope_fk
        FOREIGN KEY (
            project_id, repository_workspace_id, repository_id, conversation_id
        )
        REFERENCES repository_workspaces(
            project_id, id, repository_id, conversation_id
        ),
    ADD CONSTRAINT agent_runs_provider_session_scope_fk
        FOREIGN KEY (
            project_id, provider_session_id, conversation_id,
            execution_profile_id, repository_id
        )
        REFERENCES provider_sessions(
            project_id, id, conversation_id, execution_profile_id, repository_id
        );

CREATE UNIQUE INDEX repositories_project_identity_ci_unique
    ON repositories (project_id, lower(owner), lower(name));

CREATE UNIQUE INDEX repositories_project_github_id_unique
    ON repositories (project_id, github_repository_id)
    WHERE github_repository_id IS NOT NULL;

CREATE INDEX repositories_ready_selection
    ON repositories (project_id, created_at, id)
    WHERE enabled
      AND trusted
      AND archived_at IS NULL
      AND preparation_state = 'ready';

CREATE UNIQUE INDEX agent_runs_one_active_turn_per_profile
    ON agent_runs (execution_profile_id)
    WHERE state IN ('starting', 'working', 'blocked', 'recovering');

CREATE INDEX agent_runs_queue_order
    ON agent_runs (created_at, id)
    WHERE state IN ('created', 'queued');

CREATE OR REPLACE FUNCTION derive_repository_workspace_scope()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_project_id UUID;
    conversation_repository_id UUID;
    conversation_kind TEXT;
    repository_project_id UUID;
BEGIN
    SELECT conversation.project_id, conversation.repository_id, conversation.kind
      INTO conversation_project_id, conversation_repository_id, conversation_kind
      FROM conversations conversation
     WHERE conversation.id = NEW.conversation_id
     FOR SHARE;

    SELECT repository.project_id
      INTO repository_project_id
      FROM repositories repository
     WHERE repository.id = NEW.repository_id
     FOR SHARE;

    IF conversation_project_id IS NULL OR repository_project_id IS NULL THEN
        RAISE EXCEPTION 'Repository Workspace parent does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF conversation_kind IS DISTINCT FROM 'thread'
       OR conversation_repository_id IS DISTINCT FROM NEW.repository_id
       OR repository_project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Repository Workspace must match a repository-bound Thread'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Repository Workspace Project is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER repository_workspaces_derive_scope
BEFORE INSERT OR UPDATE OF project_id, repository_id, conversation_id
ON repository_workspaces
FOR EACH ROW
EXECUTE FUNCTION derive_repository_workspace_scope();

CREATE OR REPLACE FUNCTION derive_provider_session_scope()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_project_id UUID;
    conversation_repository_id UUID;
    conversation_kind TEXT;
    profile_project_id UUID;
BEGIN
    SELECT conversation.project_id, conversation.repository_id, conversation.kind
      INTO conversation_project_id, conversation_repository_id, conversation_kind
      FROM conversations conversation
     WHERE conversation.id = NEW.conversation_id
     FOR SHARE;

    SELECT profile.project_id
      INTO profile_project_id
      FROM execution_profiles profile
     WHERE profile.id = NEW.execution_profile_id
     FOR SHARE;

    IF conversation_project_id IS NULL OR profile_project_id IS NULL THEN
        RAISE EXCEPTION 'Provider Session parent does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF conversation_kind IS DISTINCT FROM 'thread'
       OR conversation_repository_id IS NULL
       OR profile_project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Provider Session must match a repository-bound Thread and Profile'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Provider Session Project is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.repository_id IS NULL THEN
        NEW.repository_id := conversation_repository_id;
    ELSIF NEW.repository_id IS DISTINCT FROM conversation_repository_id THEN
        RAISE EXCEPTION 'Provider Session Repository is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_sessions_derive_scope
BEFORE INSERT OR UPDATE OF project_id, conversation_id, execution_profile_id, repository_id
ON provider_sessions
FOR EACH ROW
EXECUTE FUNCTION derive_provider_session_scope();

CREATE OR REPLACE FUNCTION validate_execution_worker_configuration()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    actor_kind TEXT;
    actor_status TEXT;
    membership_role TEXT;
    membership_enabled BOOLEAN;
    connection_enabled BOOLEAN;
    profile_status TEXT;
    profile_connection_id UUID;
BEGIN
    SELECT actor.kind, actor.status, membership.role, membership.enabled
      INTO actor_kind, actor_status, membership_role, membership_enabled
      FROM actors actor
      JOIN project_memberships membership
        ON membership.actor_id = actor.id
       AND membership.project_id = NEW.project_id
     WHERE actor.id = NEW.actor_id
     FOR SHARE OF actor, membership;

    SELECT connection.enabled
      INTO connection_enabled
      FROM provider_connections connection
     WHERE connection.id = NEW.provider_connection_id
       AND connection.project_id = NEW.project_id
     FOR SHARE;

    SELECT profile.status, profile.provider_connection_id
      INTO profile_status, profile_connection_id
      FROM execution_profiles profile
     WHERE profile.id = NEW.execution_profile_id
       AND profile.project_id = NEW.project_id
     FOR SHARE;

    IF actor_kind IS DISTINCT FROM 'execution_worker'
       OR membership_role IS DISTINCT FROM 'worker' THEN
        RAISE EXCEPTION 'Execution Worker requires an Execution Worker Actor and Worker membership'
            USING ERRCODE = '23514';
    END IF;

    IF profile_connection_id IS DISTINCT FROM NEW.provider_connection_id THEN
        RAISE EXCEPTION 'Execution Worker Profile must use its Provider Connection'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.enabled
       AND (
           actor_status IS DISTINCT FROM 'active'
           OR membership_enabled IS DISTINCT FROM true
           OR connection_enabled IS DISTINCT FROM true
           OR profile_status IS DISTINCT FROM 'available'
       ) THEN
        RAISE EXCEPTION 'Enabled Execution Worker dependencies must be available'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER execution_workers_validate_configuration
BEFORE INSERT OR UPDATE OF
    project_id, actor_id, provider_connection_id, execution_profile_id, enabled
ON execution_workers
FOR EACH ROW
EXECUTE FUNCTION validate_execution_worker_configuration();

CREATE OR REPLACE FUNCTION validate_agent_run_scope_and_admission()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_project_id UUID;
    conversation_repository_id UUID;
    conversation_kind TEXT;
    conversation_archived_at TIMESTAMPTZ;
    project_archived_at TIMESTAMPTZ;
    requester_available BOOLEAN;
    worker_available BOOLEAN;
    repository_available BOOLEAN;
BEGIN
    SELECT conversation.project_id,
           conversation.repository_id,
           conversation.kind,
           conversation.archived_at,
           project.archived_at
      INTO conversation_project_id,
           conversation_repository_id,
           conversation_kind,
           conversation_archived_at,
           project_archived_at
      FROM conversations conversation
      JOIN projects project ON project.id = conversation.project_id
     WHERE conversation.id = NEW.conversation_id
     FOR SHARE OF conversation, project;

    IF conversation_project_id IS NULL THEN
        RAISE EXCEPTION 'Agent Run Thread does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Agent Run Project is inconsistent'
            USING ERRCODE = '23514';
    END IF;

    IF conversation_kind IS DISTINCT FROM 'thread'
       OR conversation_repository_id IS DISTINCT FROM NEW.repository_id
       OR conversation_archived_at IS NOT NULL
       OR project_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Agent Run requires an active repository-bound Thread'
            USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active'
      INTO requester_available
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.requesting_actor_id
     FOR SHARE OF membership, actor;

    SELECT worker.enabled
           AND worker_membership.enabled
           AND worker_membership.role = 'worker'
           AND worker_actor.kind = 'execution_worker'
           AND worker_actor.status = 'active'
           AND connection.enabled
           AND profile.status = 'available'
      INTO worker_available
      FROM execution_workers worker
      JOIN project_memberships worker_membership
        ON worker_membership.project_id = worker.project_id
       AND worker_membership.actor_id = worker.actor_id
      JOIN actors worker_actor ON worker_actor.id = worker.actor_id
      JOIN provider_connections connection
        ON connection.id = worker.provider_connection_id
       AND connection.project_id = worker.project_id
      JOIN execution_profiles profile
        ON profile.id = NEW.execution_profile_id
       AND profile.project_id = worker.project_id
       AND profile.provider_connection_id = connection.id
     WHERE worker.id = NEW.execution_worker_id
       AND worker.project_id = NEW.project_id
     FOR SHARE OF worker, worker_membership, worker_actor, connection, profile;

    SELECT repository.enabled
           AND repository.trusted
           AND repository.archived_at IS NULL
      INTO repository_available
      FROM repositories repository
     WHERE repository.id = NEW.repository_id
       AND repository.project_id = NEW.project_id
     FOR SHARE;

    IF requester_available IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Agent Run requester is unavailable'
            USING ERRCODE = '23514';
    END IF;
    IF worker_available IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Agent Run Worker or Profile is unavailable'
            USING ERRCODE = '23514';
    END IF;
    IF repository_available IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Agent Run Repository is unavailable'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_runs_a_validate_scope_and_admission
BEFORE INSERT OR UPDATE OF
    project_id, conversation_id, execution_worker_id, requesting_actor_id,
    execution_profile_id, repository_id
ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION validate_agent_run_scope_and_admission();

CREATE OR REPLACE FUNCTION enforce_provider_connection_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.kind IS DISTINCT FROM OLD.kind
       OR NEW.host IS DISTINCT FROM OLD.host
       OR NEW.binary_version IS DISTINCT FROM OLD.binary_version THEN
        RAISE EXCEPTION 'Provider Connection identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_connections_enforce_identity
BEFORE UPDATE
ON provider_connections
FOR EACH ROW
EXECUTE FUNCTION enforce_provider_connection_identity();

CREATE OR REPLACE FUNCTION enforce_execution_profile_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.provider_connection_id IS DISTINCT FROM OLD.provider_connection_id
       OR NEW.owner_actor_id IS DISTINCT FROM OLD.owner_actor_id
       OR NEW.codex_home_path IS DISTINCT FROM OLD.codex_home_path THEN
        RAISE EXCEPTION 'Execution Profile identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER execution_profiles_enforce_identity
BEFORE UPDATE
ON execution_profiles
FOR EACH ROW
EXECUTE FUNCTION enforce_execution_profile_identity();

CREATE OR REPLACE FUNCTION enforce_execution_worker_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.actor_id IS DISTINCT FROM OLD.actor_id THEN
        RAISE EXCEPTION 'Execution Worker identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER execution_workers_enforce_identity
BEFORE UPDATE
ON execution_workers
FOR EACH ROW
EXECUTE FUNCTION enforce_execution_worker_identity();

CREATE OR REPLACE FUNCTION enforce_repository_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.github_repository_id IS DISTINCT FROM OLD.github_repository_id
       OR NEW.owner IS DISTINCT FROM OLD.owner
       OR NEW.name IS DISTINCT FROM OLD.name
       OR NEW.clone_url IS DISTINCT FROM OLD.clone_url
       OR NEW.mirror_path IS DISTINCT FROM OLD.mirror_path THEN
        RAISE EXCEPTION 'Repository identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER repositories_enforce_identity
BEFORE UPDATE
ON repositories
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_identity();

CREATE OR REPLACE FUNCTION enforce_repository_archive()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.archived_at IS NULL AND NEW.archived_by_actor_id IS NOT NULL THEN
        RAISE EXCEPTION 'Active Repository cannot have archive attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NULL
       AND NEW.archived_at IS NOT NULL THEN
        IF NEW.archived_by_actor_id IS NULL OR NEW.enabled THEN
            RAISE EXCEPTION 'Archiving a Repository requires attribution and disablement'
                USING ERRCODE = '23514';
        END IF;

        IF EXISTS (
            SELECT 1
              FROM agent_runs run
             WHERE run.repository_id = OLD.id
               AND run.state IN (
                   'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
               )
        ) THEN
            RAISE EXCEPTION 'Repository with a non-terminal Agent Run cannot be archived'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NOT NULL
       AND NEW.archived_at IS NULL
       AND NEW.archived_by_actor_id IS NOT NULL THEN
        RAISE EXCEPTION 'Restored Repository must clear archive attribution'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.archived_at IS NOT NULL
       AND NEW.archived_at IS NOT NULL
       AND OLD.archived_by_actor_id IS NOT NULL
       AND NEW.archived_by_actor_id IS NULL THEN
        RAISE EXCEPTION 'Repository archive attribution cannot be removed'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER repositories_enforce_archive
BEFORE INSERT OR UPDATE OF archived_at, archived_by_actor_id, enabled
ON repositories
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_archive();

CREATE OR REPLACE FUNCTION enforce_repository_workspace_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.branch_name IS DISTINCT FROM OLD.branch_name
       OR NEW.base_commit IS DISTINCT FROM OLD.base_commit THEN
        RAISE EXCEPTION 'Repository Workspace identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER repository_workspaces_enforce_identity
BEFORE UPDATE
ON repository_workspaces
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_workspace_identity();

CREATE OR REPLACE FUNCTION enforce_provider_session_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.execution_profile_id IS DISTINCT FROM OLD.execution_profile_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.provider_thread_id IS DISTINCT FROM OLD.provider_thread_id THEN
        RAISE EXCEPTION 'Provider Session identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER provider_sessions_enforce_identity
BEFORE UPDATE
ON provider_sessions
FOR EACH ROW
EXECUTE FUNCTION enforce_provider_session_identity();

CREATE OR REPLACE FUNCTION enforce_agent_run_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.execution_worker_id IS DISTINCT FROM OLD.execution_worker_id
       OR NEW.requesting_actor_id IS DISTINCT FROM OLD.requesting_actor_id
       OR NEW.execution_profile_id IS DISTINCT FROM OLD.execution_profile_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.prompt IS DISTINCT FROM OLD.prompt
       OR NEW.authorization_snapshot IS DISTINCT FROM OLD.authorization_snapshot THEN
        RAISE EXCEPTION 'Agent Run identity is immutable'
            USING ERRCODE = '23514';
    END IF;

    IF OLD.repository_workspace_id IS NOT NULL
       AND NEW.repository_workspace_id IS DISTINCT FROM OLD.repository_workspace_id THEN
        RAISE EXCEPTION 'Agent Run Repository Workspace binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.provider_session_id IS NOT NULL
       AND NEW.provider_session_id IS DISTINCT FROM OLD.provider_session_id THEN
        RAISE EXCEPTION 'Agent Run Provider Session binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.provider_turn_id IS NOT NULL
       AND NEW.provider_turn_id IS DISTINCT FROM OLD.provider_turn_id THEN
        RAISE EXCEPTION 'Agent Run provider turn binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_runs_b_enforce_identity
BEFORE UPDATE
ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION enforce_agent_run_identity();
