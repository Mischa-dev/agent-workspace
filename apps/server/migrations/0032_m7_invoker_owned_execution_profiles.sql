-- M7 makes one Project Worker usable with each human's own compatible
-- Execution Profile. The profile stored on execution_workers is retained only
-- as the bootstrap/default profile for upgrades and no longer authorizes a run.

ALTER TABLE agent_runs
    DROP CONSTRAINT agent_runs_worker_profile_fk;

COMMENT ON COLUMN execution_workers.execution_profile_id IS
    'Bootstrap/default profile only; Agent Run admission selects an active profile owned by the requester.';

CREATE INDEX execution_profiles_project_owner_connection
    ON execution_profiles (project_id, owner_actor_id, provider_connection_id, created_at, id);

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
    requester_user_available BOOLEAN;
    requester_organization_available BOOLEAN;
    worker_available BOOLEAN;
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

    -- Historical development fixtures may predate browser identity bootstrap.
    -- When the v1 User and Organization membership rows exist, lock and
    -- revalidate them so disable cannot race admission.
    SELECT status = 'active'
      INTO requester_user_available
      FROM users
     WHERE actor_id = NEW.requesting_actor_id
     FOR SHARE;

    SELECT organization_membership.enabled
      INTO requester_organization_available
      FROM projects requester_project
      JOIN organization_memberships organization_membership
        ON organization_membership.organization_id = requester_project.organization_id
       AND organization_membership.actor_id = NEW.requesting_actor_id
     WHERE requester_project.id = NEW.project_id
     FOR SHARE OF requester_project, organization_membership;

    requester_available := requester_available
        AND (requester_user_available IS DISTINCT FROM false)
        AND (requester_organization_available IS DISTINCT FROM false);

    SELECT worker.enabled
           AND worker_membership.enabled
           AND worker_membership.role = 'worker'
           AND worker_actor.kind = 'execution_worker'
           AND worker_actor.status = 'active'
           AND connection.enabled
           AND profile.status = 'available'
           AND profile.account_state = 'authenticated',
           profile.owner_actor_id,
           owner_actor.status = 'active' AND owner_membership.enabled
      INTO worker_available,
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
