-- M9 sponsored execution keeps external agent runtimes outside the Worker
-- boundary. A human-owned profile is selected only after explicit approval.

ALTER TABLE agent_runs
    ADD COLUMN sponsoring_actor_id UUID,
    ADD COLUMN external_execution_request_id UUID,
    ADD CONSTRAINT agent_runs_sponsor_project_fk
        FOREIGN KEY (project_id, sponsoring_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT agent_runs_sponsor_not_requester_check CHECK (
        sponsoring_actor_id IS NULL OR sponsoring_actor_id <> requesting_actor_id
    );

CREATE TABLE external_execution_requests (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    external_agent_connection_id UUID NOT NULL,
    requesting_actor_id UUID NOT NULL,
    sponsoring_actor_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    repository_id UUID NOT NULL,
    prompt TEXT NOT NULL CHECK (
        char_length(prompt) BETWEEN 1 AND 20000
        AND prompt !~ '[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]'
    ),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'approved', 'denied', 'expired', 'canceled', 'failed', 'launched')
    ),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    agent_run_id UUID UNIQUE REFERENCES agent_runs(id),
    failure_category TEXT CHECK (
        failure_category IS NULL OR (
            char_length(failure_category) BETWEEN 1 AND 64
            AND failure_category ~ '^[a-z][a-z0-9_]*$'
        )
    ),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '24 hours',
    resolved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, external_agent_connection_id)
        REFERENCES external_agent_connections(project_id, id),
    FOREIGN KEY (project_id, requesting_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, sponsoring_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (project_id, conversation_id)
        REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, repository_id)
        REFERENCES repositories(project_id, id),
    CHECK (requesting_actor_id <> sponsoring_actor_id),
    CHECK (expires_at > created_at),
    CHECK (
        (status = 'pending' AND resolved_at IS NULL AND agent_run_id IS NULL)
        OR (status IN ('denied', 'expired', 'canceled', 'failed') AND resolved_at IS NOT NULL)
        OR (status IN ('approved', 'launched') AND resolved_at IS NOT NULL AND agent_run_id IS NOT NULL)
    )
);

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_external_execution_request_fk
        FOREIGN KEY (project_id, external_execution_request_id)
        REFERENCES external_execution_requests(project_id, id),
    ADD CONSTRAINT agent_runs_external_request_shape_check CHECK (
        (external_execution_request_id IS NULL AND sponsoring_actor_id IS NULL)
        OR (external_execution_request_id IS NOT NULL AND sponsoring_actor_id IS NOT NULL)
    );

CREATE UNIQUE INDEX agent_runs_one_per_external_execution_request
    ON agent_runs (external_execution_request_id)
    WHERE external_execution_request_id IS NOT NULL;

ALTER TABLE agent_approvals
    ALTER COLUMN agent_run_id DROP NOT NULL,
    ADD COLUMN external_execution_request_id UUID
        REFERENCES external_execution_requests(id),
    ADD COLUMN eligible_actor_id UUID REFERENCES actors(id),
    ADD CONSTRAINT agent_approvals_exactly_one_parent_check CHECK (
        (agent_run_id IS NOT NULL)::integer
        + (external_execution_request_id IS NOT NULL)::integer = 1
    );

CREATE UNIQUE INDEX agent_approvals_one_external_execution_request
    ON agent_approvals (external_execution_request_id)
    WHERE external_execution_request_id IS NOT NULL;

CREATE INDEX external_execution_requests_pending_sponsor
    ON external_execution_requests (sponsoring_actor_id, created_at, id)
    WHERE status = 'pending';

CREATE INDEX external_execution_requests_connection_created
    ON external_execution_requests (external_agent_connection_id, created_at DESC, id DESC);

CREATE OR REPLACE FUNCTION validate_external_execution_request()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    request_scope_valid BOOLEAN;
    linked_run_valid BOOLEAN;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT connection.status = 'active'
               AND requester.kind = 'external_agent'
               AND requester.status = 'active'
               AND requester_membership.enabled
               AND requester_organization_membership.enabled
               AND sponsor.kind = 'human'
               AND sponsor.status = 'active'
               AND sponsor_user.status = 'active'
               AND sponsor_membership.enabled
               AND sponsor_membership.role IN ('owner', 'admin', 'member')
               AND sponsor_organization_membership.enabled
               AND project.archived_at IS NULL
               AND conversation.kind = 'thread'
               AND conversation.archived_at IS NULL
               AND parent.archived_at IS NULL
               AND repository.enabled
               AND repository.trusted
               AND repository.archived_at IS NULL
               AND repository.preparation_state = 'ready'
          INTO request_scope_valid
          FROM external_agent_connections connection
          JOIN actors requester ON requester.id = NEW.requesting_actor_id
          JOIN project_memberships requester_membership
            ON requester_membership.project_id = NEW.project_id
           AND requester_membership.actor_id = requester.id
          JOIN projects project ON project.id = NEW.project_id
          JOIN organization_memberships requester_organization_membership
            ON requester_organization_membership.organization_id = project.organization_id
           AND requester_organization_membership.actor_id = requester.id
          JOIN actors sponsor ON sponsor.id = NEW.sponsoring_actor_id
          JOIN users sponsor_user ON sponsor_user.actor_id = sponsor.id
          JOIN project_memberships sponsor_membership
            ON sponsor_membership.project_id = NEW.project_id
           AND sponsor_membership.actor_id = sponsor.id
          JOIN organization_memberships sponsor_organization_membership
            ON sponsor_organization_membership.organization_id = project.organization_id
           AND sponsor_organization_membership.actor_id = sponsor.id
          JOIN conversations conversation ON conversation.id = NEW.conversation_id
           AND conversation.project_id = NEW.project_id
          JOIN conversations parent ON parent.id = conversation.parent_channel_id
          JOIN repositories repository ON repository.id = NEW.repository_id
           AND repository.project_id = NEW.project_id
         WHERE connection.id = NEW.external_agent_connection_id
           AND connection.project_id = NEW.project_id
           AND connection.actor_id = NEW.requesting_actor_id
         FOR SHARE OF connection, requester, requester_membership, project,
                      requester_organization_membership, sponsor, sponsor_user,
                      sponsor_membership, sponsor_organization_membership,
                      conversation, parent, repository;
        IF request_scope_valid IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'External execution request scope is unavailable'
                USING ERRCODE = '23514';
        END IF;
    ELSE
        IF NEW.project_id IS DISTINCT FROM OLD.project_id
           OR NEW.external_agent_connection_id IS DISTINCT FROM OLD.external_agent_connection_id
           OR NEW.requesting_actor_id IS DISTINCT FROM OLD.requesting_actor_id
           OR NEW.sponsoring_actor_id IS DISTINCT FROM OLD.sponsoring_actor_id
           OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
           OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
           OR NEW.prompt IS DISTINCT FROM OLD.prompt
           OR NEW.created_at IS DISTINCT FROM OLD.created_at
           OR NEW.expires_at IS DISTINCT FROM OLD.expires_at THEN
            RAISE EXCEPTION 'External execution request identity is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF OLD.status <> 'pending'
           AND (NEW.status IS DISTINCT FROM OLD.status
                OR NEW.agent_run_id IS DISTINCT FROM OLD.agent_run_id) THEN
            RAISE EXCEPTION 'Resolved external execution request is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.version IS DISTINCT FROM OLD.version + 1 THEN
            RAISE EXCEPTION 'External execution request version must advance once'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    IF NEW.status IN ('approved', 'launched') THEN
        SELECT run.external_execution_request_id = NEW.id
               AND run.project_id = NEW.project_id
               AND run.requesting_actor_id = NEW.requesting_actor_id
               AND run.sponsoring_actor_id = NEW.sponsoring_actor_id
               AND run.conversation_id = NEW.conversation_id
               AND run.repository_id = NEW.repository_id
          INTO linked_run_valid
          FROM agent_runs run
         WHERE run.id = NEW.agent_run_id
         FOR SHARE;
        IF linked_run_valid IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'External execution request Agent Run is inconsistent'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER external_execution_requests_validate
BEFORE INSERT OR UPDATE
ON external_execution_requests
FOR EACH ROW
EXECUTE FUNCTION validate_external_execution_request();

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
       AND profile.provider_connection_id = connection.id
      JOIN actors owner_actor ON owner_actor.id = profile.owner_actor_id
      JOIN project_memberships owner_membership
        ON owner_membership.project_id = profile.project_id
       AND owner_membership.actor_id = profile.owner_actor_id
     WHERE worker.id = NEW.execution_worker_id
       AND worker.project_id = NEW.project_id
     FOR SHARE OF worker, worker_membership, worker_actor, connection, profile,
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

CREATE OR REPLACE FUNCTION enforce_agent_run_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.execution_worker_id IS DISTINCT FROM OLD.execution_worker_id
       OR NEW.requesting_actor_id IS DISTINCT FROM OLD.requesting_actor_id
       OR NEW.sponsoring_actor_id IS DISTINCT FROM OLD.sponsoring_actor_id
       OR NEW.external_execution_request_id IS DISTINCT FROM OLD.external_execution_request_id
       OR NEW.execution_profile_id IS DISTINCT FROM OLD.execution_profile_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.prompt IS DISTINCT FROM OLD.prompt
       OR NEW.authorization_snapshot IS DISTINCT FROM OLD.authorization_snapshot
       OR NEW.predecessor_run_id IS DISTINCT FROM OLD.predecessor_run_id THEN
        RAISE EXCEPTION 'Agent Run creation identity is immutable' USING ERRCODE = '23514';
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

