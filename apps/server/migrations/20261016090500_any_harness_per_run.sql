-- H1: a project's @agent worker can run on any harness its people have
-- connected. The harness comes from the execution profile (and through it
-- the provider account); the profile's own provider connection only has to
-- belong to the same project and be enabled, instead of being the worker's.
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
    requester_kind TEXT;
    requester_user_available BOOLEAN;
    requester_organization_available BOOLEAN;
    sponsor_available BOOLEAN;
    worker_available BOOLEAN;
    profile_owner_actor_id UUID;
    profile_owner_available BOOLEAN;
    expected_profile_owner_actor_id UUID;
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
        RAISE EXCEPTION 'Agent Run Thread does not exist' USING ERRCODE = '23503';
    END IF;
    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Agent Run Project is inconsistent' USING ERRCODE = '23514';
    END IF;
    IF conversation_kind IS DISTINCT FROM 'thread'
       OR conversation_repository_id IS DISTINCT FROM NEW.repository_id THEN
        RAISE EXCEPTION 'Agent Run must match a repository-bound Thread' USING ERRCODE = '23514';
    END IF;
    IF requires_admission AND (
        conversation_archived_at IS NOT NULL OR project_archived_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'Agent Run requires an active Project and Thread' USING ERRCODE = '23514';
    END IF;

    SELECT membership.enabled AND actor.status = 'active', actor.kind
      INTO requester_available, requester_kind
      FROM project_memberships membership
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE membership.project_id = NEW.project_id
       AND membership.actor_id = NEW.requesting_actor_id
     FOR SHARE OF membership, actor;

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
        AND (requester_organization_available IS DISTINCT FROM false)
        AND (
            requester_kind = 'external_agent'
            OR requester_user_available IS DISTINCT FROM false
        );

    IF NEW.sponsoring_actor_id IS NULL THEN
        sponsor_available := requester_kind = 'human';
        expected_profile_owner_actor_id := NEW.requesting_actor_id;
    ELSE
        SELECT sponsor.kind = 'human'
               AND sponsor.status = 'active'
               AND membership.enabled
               AND membership.role IN ('owner', 'admin', 'member')
               AND sponsor_user.status = 'active'
               AND organization_membership.enabled
          INTO sponsor_available
          FROM projects sponsor_project
          JOIN project_memberships membership
            ON membership.project_id = sponsor_project.id
           AND membership.actor_id = NEW.sponsoring_actor_id
          JOIN actors sponsor ON sponsor.id = membership.actor_id
          JOIN users sponsor_user ON sponsor_user.actor_id = sponsor.id
          JOIN organization_memberships organization_membership
            ON organization_membership.organization_id = sponsor_project.organization_id
           AND organization_membership.actor_id = sponsor.id
         WHERE sponsor_project.id = NEW.project_id
         FOR SHARE OF sponsor_project, membership, sponsor, sponsor_user,
                      organization_membership;
        sponsor_available := sponsor_available
            AND requester_kind = 'external_agent'
            AND NEW.external_execution_request_id IS NOT NULL;
        expected_profile_owner_actor_id := NEW.sponsoring_actor_id;
    END IF;

    SELECT worker.enabled
           AND worker_membership.enabled
           AND worker_membership.role = 'worker'
           AND worker_actor.kind = 'execution_worker'
           AND worker_actor.status = 'active'
           AND connection.enabled
           AND profile_connection.enabled
           AND profile.status = 'available'
           AND profile.account_state = 'authenticated',
           profile.owner_actor_id,
           owner_actor.status = 'active' AND owner_membership.enabled
      INTO worker_available, profile_owner_actor_id, profile_owner_available
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
      JOIN provider_connections profile_connection
        ON profile_connection.id = profile.provider_connection_id
       AND profile_connection.project_id = profile.project_id
      JOIN actors owner_actor ON owner_actor.id = profile.owner_actor_id
      JOIN project_memberships owner_membership
        ON owner_membership.project_id = profile.project_id
       AND owner_membership.actor_id = profile.owner_actor_id
     WHERE worker.id = NEW.execution_worker_id
       AND worker.project_id = NEW.project_id
     FOR SHARE OF worker, worker_membership, worker_actor, connection, profile,
                  profile_connection,
                  owner_actor, owner_membership;

    SELECT repository.enabled AND repository.trusted
           AND repository.archived_at IS NULL
           AND repository.preparation_state = 'ready'
      INTO repository_available
      FROM repositories repository
     WHERE repository.id = NEW.repository_id
       AND repository.project_id = NEW.project_id
     FOR SHARE;

    IF requires_admission THEN
        IF requester_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run requester is unavailable' USING ERRCODE = '23514';
        END IF;
        IF sponsor_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run sponsor is unavailable' USING ERRCODE = '23514';
        END IF;
        IF profile_owner_actor_id IS DISTINCT FROM expected_profile_owner_actor_id THEN
            RAISE EXCEPTION 'personal Execution Profile must be owned by the requester or sponsor'
                USING ERRCODE = '23514';
        END IF;
        IF profile_owner_available IS DISTINCT FROM true OR worker_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run Worker or Profile is unavailable' USING ERRCODE = '23514';
        END IF;
        IF repository_available IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'Agent Run Repository is not prepared and available' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
