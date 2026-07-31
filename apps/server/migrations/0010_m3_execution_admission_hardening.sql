-- M3 execution admission and host-path hardening.
--
-- Migration 0009 is already applied to the retained development database and
-- is immutable. This additive migration closes the admission and concurrency
-- gaps found during its cross-review without rewriting adopted provider or Git
-- state.

LOCK TABLE execution_profiles,
           execution_workers,
           repositories,
           agent_runs
IN SHARE ROW EXCLUSIVE MODE;

-- Runtime roots are host-global security boundaries. Refuse an ambiguous
-- upgrade with an actionable category instead of choosing one owner for a
-- shared path.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM execution_profiles
         GROUP BY codex_home_path
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'Execution Profiles share a CODEX_HOME path'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM repositories
         GROUP BY mirror_path
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'Repositories share a mirror path'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM agent_runs run
          JOIN execution_workers worker
            ON worker.project_id = run.project_id
           AND worker.id = run.execution_worker_id
         WHERE worker.execution_profile_id IS DISTINCT FROM run.execution_profile_id
    ) THEN
        RAISE EXCEPTION 'an Agent Run uses a Profile not assigned to its Worker'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM repositories repository
          JOIN agent_runs run ON run.repository_id = repository.id
         WHERE repository.archived_at IS NOT NULL
           AND run.state IN (
               'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
           )
    ) THEN
        RAISE EXCEPTION 'an archived Repository has a non-terminal Agent Run'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

ALTER TABLE execution_profiles
    ADD CONSTRAINT execution_profiles_codex_home_path_unique
        UNIQUE (codex_home_path);

ALTER TABLE repositories
    ADD CONSTRAINT repositories_mirror_path_unique
        UNIQUE (mirror_path);

-- The composite key makes a Worker's selected Profile part of its durable
-- identity as consumed by Agent Runs. It prevents same-Project Profile
-- substitution even when every individual UUID foreign key is valid.
ALTER TABLE execution_workers
    ADD CONSTRAINT execution_workers_project_id_id_profile_unique
        UNIQUE (project_id, id, execution_profile_id);

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_worker_profile_fk
        FOREIGN KEY (project_id, execution_worker_id, execution_profile_id)
        REFERENCES execution_workers(project_id, id, execution_profile_id);

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
    worker_execution_profile_id UUID;
    profile_owner_actor_id UUID;
    profile_owner_available BOOLEAN;
    repository_available BOOLEAN;
    requires_admission BOOLEAN;
BEGIN
    IF TG_OP = 'INSERT' THEN
        requires_admission := NEW.state IN (
            'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
        );
    ELSE
        requires_admission := NEW.state IS DISTINCT FROM OLD.state
            AND NEW.state IN ('starting', 'working');
    END IF;

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
       OR conversation_repository_id IS DISTINCT FROM NEW.repository_id THEN
        RAISE EXCEPTION 'Agent Run must match a repository-bound Thread'
            USING ERRCODE = '23514';
    END IF;

    IF requires_admission
       AND (
           conversation_archived_at IS NOT NULL
           OR project_archived_at IS NOT NULL
       ) THEN
        RAISE EXCEPTION 'Agent Run requires an active Project and Thread'
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
           AND profile.account_state = 'authenticated',
           worker.execution_profile_id,
           profile.owner_actor_id,
           owner_actor.status = 'active' AND owner_membership.enabled
      INTO worker_available,
           worker_execution_profile_id,
           profile_owner_actor_id,
           profile_owner_available
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
      JOIN actors owner_actor ON owner_actor.id = profile.owner_actor_id
      JOIN project_memberships owner_membership
        ON owner_membership.project_id = profile.project_id
       AND owner_membership.actor_id = profile.owner_actor_id
     WHERE worker.id = NEW.execution_worker_id
       AND worker.project_id = NEW.project_id
     FOR SHARE OF
         worker, worker_membership, worker_actor, connection, profile,
         owner_actor, owner_membership;

    IF worker_execution_profile_id IS DISTINCT FROM NEW.execution_profile_id THEN
        RAISE EXCEPTION 'Agent Run Profile is not assigned to its Worker'
            USING ERRCODE = '23514';
    END IF;

    SELECT repository.enabled
           AND repository.trusted
           AND repository.archived_at IS NULL
           AND repository.preparation_state = 'ready'
      INTO repository_available
      FROM repositories repository
     WHERE repository.id = NEW.repository_id
       AND repository.project_id = NEW.project_id
     FOR SHARE;

    IF requires_admission THEN
        IF requester_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run requester is unavailable'
                USING ERRCODE = '23514';
        END IF;
        IF profile_owner_actor_id IS DISTINCT FROM NEW.requesting_actor_id THEN
            RAISE EXCEPTION 'personal Execution Profile must be owned by the requester'
                USING ERRCODE = '23514';
        END IF;
        IF profile_owner_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Execution Profile owner is unavailable'
                USING ERRCODE = '23514';
        END IF;
        IF worker_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run Worker or Profile is unavailable'
                USING ERRCODE = '23514';
        END IF;
        IF repository_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run Repository is not prepared and available'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER agent_runs_a_validate_scope_and_admission ON agent_runs;

CREATE TRIGGER agent_runs_a_validate_scope_and_admission
BEFORE INSERT OR UPDATE OF
    project_id, conversation_id, execution_worker_id, requesting_actor_id,
    execution_profile_id, repository_id, state
ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION validate_agent_run_scope_and_admission();

-- Archive identity is audit evidence. Once set it cannot be reassigned,
-- retimestamped, or cleared by restoring the same row.
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
       AND OLD.archived_at IS NOT NULL
       AND (
           NEW.archived_at IS DISTINCT FROM OLD.archived_at
           OR NEW.archived_by_actor_id IS DISTINCT FROM OLD.archived_by_actor_id
       ) THEN
        RAISE EXCEPTION 'Repository archive attribution is immutable'
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

    RETURN NEW;
END;
$$;

-- The immediate archive check gives ordinary commands a fast failure. These
-- deferred constraint triggers close the reciprocal-check race at commit: a
-- Repository and its Agent Runs cannot commit in an archived/non-terminal
-- combination regardless of which transaction acquired its row lock first.
CREATE OR REPLACE FUNCTION enforce_repository_run_consistency_at_commit()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    target_repository_id UUID;
BEGIN
    IF TG_TABLE_NAME = 'repositories' THEN
        target_repository_id := NEW.id;
    ELSE
        target_repository_id := NEW.repository_id;
    END IF;

    IF EXISTS (
        SELECT 1
          FROM repositories repository
          JOIN agent_runs run ON run.repository_id = repository.id
         WHERE repository.id = target_repository_id
           AND repository.archived_at IS NOT NULL
           AND run.state IN (
               'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
           )
    ) THEN
        RAISE EXCEPTION 'archived Repository cannot have a non-terminal Agent Run'
            USING ERRCODE = '23514';
    END IF;

    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER repositories_run_consistency_at_commit
AFTER INSERT OR UPDATE
ON repositories
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_run_consistency_at_commit();

CREATE CONSTRAINT TRIGGER agent_runs_repository_consistency_at_commit
AFTER INSERT OR UPDATE
ON agent_runs
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION enforce_repository_run_consistency_at_commit();
