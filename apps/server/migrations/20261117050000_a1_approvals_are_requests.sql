-- A1.14: approvals are requests. The built-in host raises its approvals
-- and questions as requests, and a sponsored run request's approval is a
-- request addressed to its sponsor, so `agent_approvals` goes. Every row
-- already has its request (A1.7 copied each in with its id and kept it
-- current); this brings the copies to the request flow's own shapes:
--
-- * answers are {optionId} or {text}, not {decision} or {answer};
-- * options are objects, approve and deny marked by style;
-- * an approval card in the inbox is a request card;
-- * a push action token points at the request it was minted for;
-- * agents hear `request.raised` instead of `approval.requested`.
--
-- `requests.legacy_source` goes with the old tables it named, and the
-- `approval_respond` connection scope with the tools it opened: answering
-- a request is a write, and who may answer is the request's audience.

SELECT requests_sync_approval(id) FROM agent_approvals ORDER BY id;

-- Answers in the request flow's shape.
UPDATE requests
   SET answer = CASE
           WHEN answer ? 'decision' THEN jsonb_build_object('optionId', answer->>'decision')
           WHEN answer ? 'answer' THEN jsonb_build_object('text', answer->>'answer')
           ELSE answer END
 WHERE legacy_source = 'agent_approval' AND answer IS NOT NULL;

-- Options as objects; an approval's approve and deny are styled.
UPDATE requests request
   SET options = COALESCE((
           SELECT jsonb_agg(
                      CASE WHEN jsonb_typeof(option) = 'string' THEN
                               jsonb_strip_nulls(jsonb_build_object(
                                   'id', option #>> '{}',
                                   'label', CASE option #>> '{}'
                                                WHEN 'approve' THEN 'Approve'
                                                WHEN 'deny' THEN 'Deny'
                                                ELSE option #>> '{}' END,
                                   'style', CASE WHEN request.kind = 'approval'
                                                      AND option #>> '{}' IN ('approve', 'deny')
                                                 THEN option #>> '{}' END))
                           WHEN request.kind = 'approval' AND option->>'id' IN ('approve', 'deny')
                           THEN option || jsonb_build_object('style', option->>'id')
                           ELSE option END
                      ORDER BY position)
           FROM jsonb_array_elements(request.options) WITH ORDINALITY AS item(option, position)
       ), '[]'::jsonb)
 WHERE legacy_source = 'agent_approval';

UPDATE requests SET legacy_source = NULL WHERE legacy_source IS NOT NULL;
ALTER TABLE requests DROP COLUMN legacy_source;

-- Inbox cards: an approval waiting on someone is a request card.
UPDATE notifications
   SET kind = 'request_raised', subject_kind = 'request', subject_id = approval_id
 WHERE kind = 'approval_requested' AND approval_id IS NOT NULL;
ALTER TABLE notifications DROP COLUMN approval_id;

-- A push's Approve, Deny or Answer acts on the request it showed.
ALTER TABLE notification_action_tokens RENAME COLUMN approval_id TO request_id;
ALTER TABLE notification_action_tokens
    DROP CONSTRAINT IF EXISTS notification_action_tokens_approval_id_fkey;
ALTER TABLE notification_action_tokens
    ADD CONSTRAINT notification_action_tokens_request_id_fkey
        FOREIGN KEY (request_id) REFERENCES requests(id) ON DELETE CASCADE;

-- Agents' events about approvals name the request.
UPDATE external_agent_events SET resource_kind = 'request' WHERE resource_kind = 'agent_approval';
CREATE OR REPLACE FUNCTION external_event_visible_to(connection_uuid UUID, resource_type TEXT, resource_uuid UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE resource_type
        WHEN 'message' THEN EXISTS (SELECT 1 FROM messages m WHERE m.id = resource_uuid AND conversation_visible_to(m.conversation_id, c.actor_id))
        WHEN 'reply_request' THEN EXISTS (SELECT 1 FROM reply_requests r WHERE r.id = resource_uuid AND r.agent_actor_id = c.actor_id AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'conversation' THEN conversation_visible_to(resource_uuid, c.actor_id)
        WHEN 'request' THEN request_visible_to(resource_uuid, c.actor_id)
        ELSE true
    END FROM external_agent_connections c WHERE c.id = connection_uuid;
$$;

-- Whether `viewer` may answer an open request, as `requests::may_answer`
-- decides it, for lists that mark what waits on someone (a sidebar's
-- "needs you"). Answering itself still goes through `may_answer`.
CREATE FUNCTION request_answerable_by(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM requests request
        JOIN project_memberships membership
          ON membership.project_id = request.space_id
         AND membership.actor_id = viewer AND membership.enabled
        LEFT JOIN workspace_roles custom ON custom.id = membership.role_id
        WHERE request.id = target
          AND request.status = 'open'
          AND request.raised_by_account_id <> viewer
          AND request_visible_to(request.id, viewer)
          AND (viewer = ANY(request.addressed_account_ids)
               OR (cardinality(request.addressed_role_keys) > 0
                   AND (membership.role = ANY(request.addressed_role_keys)
                        OR custom.key = ANY(request.addressed_role_keys))
                   AND role_allows(membership.role, membership.role_id, request.answer_permission))
               OR (cardinality(request.addressed_account_ids) = 0
                   AND cardinality(request.addressed_role_keys) = 0
                   AND role_allows(membership.role, membership.role_id, request.answer_permission)))
    )
$$;

-- The approval_respond scope is retired: a connection and its credential
-- keep the same scopes, so both change together.
UPDATE external_agent_connections
   SET scopes = array_remove(scopes, 'approval_respond')
 WHERE 'approval_respond' = ANY(scopes);
UPDATE mcp_client_credentials
   SET scopes = array_remove(scopes, 'approval_respond')
 WHERE 'approval_respond' = ANY(scopes);
ALTER TABLE external_agent_connections
    DROP CONSTRAINT external_agent_connections_scopes_check,
    ADD CONSTRAINT external_agent_connections_scopes_check CHECK (
        cardinality(scopes) BETWEEN 1 AND 4
        AND scopes <@ ARRAY['read', 'write', 'events', 'worker_request']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    );
ALTER TABLE mcp_client_credentials
    DROP CONSTRAINT mcp_client_credentials_scopes_check,
    ADD CONSTRAINT mcp_client_credentials_scopes_check CHECK (
        cardinality(scopes) BETWEEN 1 AND 4
        AND scopes <@ ARRAY['read', 'write', 'events', 'worker_request']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    );

DROP FUNCTION requests_sync_approval(UUID);
DROP TABLE agent_approvals;
DROP FUNCTION IF EXISTS maintain_agent_approval_decision_version();
