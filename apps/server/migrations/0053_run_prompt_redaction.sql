-- Run prompts are copies of their opening message, including sponsored runs
-- and retries. Keep provenance so removing the message can redact every copy.
ALTER TABLE agent_runs ADD COLUMN prompt_message_id UUID,
    ADD CONSTRAINT agent_runs_prompt_message_fk
        FOREIGN KEY (project_id, prompt_message_id) REFERENCES messages(project_id, id);
CREATE INDEX agent_runs_prompt_message ON agent_runs(prompt_message_id)
    WHERE prompt_message_id IS NOT NULL;

-- Older starters wrote the message and run in one transaction, with the same
-- timestamp, conversation and author. Only use an unambiguous match; the text
-- may already have been edited or removed. Retries inherit their root's link.
WITH candidates AS (
    SELECT run.id, min(message.id::text)::uuid AS message_id
    FROM agent_runs run JOIN messages message
      ON message.conversation_id = run.conversation_id
     AND message.author_actor_id = run.requesting_actor_id
     AND message.created_at = run.created_at
    WHERE run.predecessor_run_id IS NULL
    GROUP BY run.id HAVING count(*) = 1
)
UPDATE agent_runs run SET prompt_message_id = candidates.message_id
FROM candidates WHERE run.id = candidates.id;

WITH RECURSIVE provenance AS (
    SELECT id, prompt_message_id FROM agent_runs WHERE prompt_message_id IS NOT NULL
    UNION
    SELECT child.id, parent.prompt_message_id
    FROM agent_runs child JOIN provenance parent ON child.predecessor_run_id = parent.id
)
UPDATE agent_runs run SET prompt_message_id = provenance.prompt_message_id
FROM provenance WHERE run.id = provenance.id AND run.prompt_message_id IS NULL;

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
       OR NEW.prompt_message_id IS DISTINCT FROM OLD.prompt_message_id
       OR (NEW.prompt IS DISTINCT FROM OLD.prompt AND NOT (
           NEW.prompt = '[Message removed]'
           AND EXISTS (SELECT 1 FROM messages message
                       WHERE message.id = OLD.prompt_message_id
                         AND message.removed_at IS NOT NULL
                         AND current_setting('aw.message_command', true) = message.id::text)
       ))
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
           OR (NEW.prompt IS DISTINCT FROM OLD.prompt AND NOT (
               NEW.prompt = '[Message removed]'
               AND EXISTS (SELECT 1 FROM agent_runs run
                           JOIN messages message ON message.id = run.prompt_message_id
                           WHERE run.id = OLD.agent_run_id
                             AND message.removed_at IS NOT NULL
                             AND current_setting('aw.message_command', true) = message.id::text)
           ))
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

-- No general prompt editing: only a message removal may redact these copies.
-- Queued work must not execute a placeholder, and retries are refused by the
-- shared run command once the source message is removed.
CREATE FUNCTION redact_run_prompt_copies(target_message UUID)
RETURNS SETOF UUID LANGUAGE plpgsql AS $$
DECLARE
    target_conversation UUID;
BEGIN
    IF current_setting('aw.message_command', true) IS DISTINCT FROM target_message::text THEN
        RAISE EXCEPTION 'Only message removal can redact a run prompt' USING ERRCODE = '23514';
    END IF;
    SELECT conversation_id INTO STRICT target_conversation
    FROM messages WHERE id = target_message AND removed_at IS NOT NULL;

    UPDATE agent_runs
    SET prompt = '[Message removed]',
        state = CASE WHEN state IN ('created', 'queued') THEN 'canceled' ELSE state END,
        updated_at = now()
    WHERE prompt_message_id = target_message;

    UPDATE external_execution_requests request
    SET prompt = '[Message removed]', version = version + 1, updated_at = now()
    FROM agent_runs run
    WHERE request.agent_run_id = run.id AND run.prompt_message_id = target_message
      AND request.prompt <> '[Message removed]';

    -- These hashes also came from the prompt, even though their responses
    -- contain only IDs. A retry of the removed prompt must not execute again.
    UPDATE idempotency_keys replay
    SET request_hash = sha256('removed run prompt'::bytea)
    WHERE (replay.scope = 'thread.' || replace(target_conversation::text, '-', '') || '.agent_run_start'
           AND replay.response_body->>'runId' IN (
               SELECT id::text FROM agent_runs WHERE prompt_message_id = target_message))
       OR (replay.scope = 'external_execution_thread.' || replace(target_conversation::text, '-', '') || '.request'
           AND replay.response_body->>'requestId' IN (
               SELECT external_execution_request_id::text FROM agent_runs
               WHERE prompt_message_id = target_message));

    RETURN QUERY SELECT id FROM agent_runs WHERE prompt_message_id = target_message;
END;
$$;

-- Cover messages removed before this migration as well.
DO $$
DECLARE
    message_id UUID;
BEGIN
    FOR message_id IN
        SELECT DISTINCT message.id FROM messages message
        JOIN agent_runs run ON run.prompt_message_id = message.id
        WHERE message.removed_at IS NOT NULL
    LOOP
        PERFORM set_config('aw.message_command', message_id::text, true);
        PERFORM redact_run_prompt_copies(message_id);
    END LOOP;
    PERFORM set_config('aw.message_command', '', true);
END;
$$;
