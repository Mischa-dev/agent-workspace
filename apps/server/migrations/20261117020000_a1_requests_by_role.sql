-- A1.8: requests are raised, answered and withdrawn by role, through the
-- request operations (requests::ops). A question someone was asked is a
-- request now: person_questions goes, and its inbox cards point at the
-- request with the same id (A1.7 copied every question in with its id).

-- Who sees a request: whoever raised it; in a space, its members who can
-- see the conversation and run it belongs to; a workspace-level request
-- (no space), the workspace members it is addressed to by account or role.
CREATE FUNCTION request_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM requests request
        WHERE request.id = target
          AND (
              request.raised_by_account_id = viewer
              OR (request.space_id IS NOT NULL
                  AND EXISTS (
                      SELECT 1 FROM project_memberships membership
                      WHERE membership.project_id = request.space_id
                        AND membership.actor_id = viewer AND membership.enabled)
                  AND (request.conversation_id IS NULL
                       OR conversation_visible_to(request.conversation_id, viewer))
                  AND (request.run_id IS NULL OR run_visible_to(request.run_id, viewer)))
              OR (request.space_id IS NULL
                  AND EXISTS (
                      SELECT 1 FROM organization_memberships membership
                      WHERE membership.organization_id = request.workspace_id
                        AND membership.actor_id = viewer AND membership.enabled
                        AND (viewer = ANY(request.addressed_account_ids)
                             OR membership.role = ANY(request.addressed_role_keys))))
          )
    )
$$;

-- An event about a request goes to whoever may see the request.
CREATE OR REPLACE FUNCTION external_event_visible_to(connection_uuid UUID, resource_type TEXT, resource_uuid UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT CASE resource_type
        WHEN 'message' THEN EXISTS (SELECT 1 FROM messages m WHERE m.id = resource_uuid AND conversation_visible_to(m.conversation_id, c.actor_id))
        WHEN 'reply_request' THEN EXISTS (SELECT 1 FROM reply_requests r WHERE r.id = resource_uuid AND r.agent_actor_id = c.actor_id AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'agent_approval' THEN EXISTS (SELECT 1 FROM agent_approvals a JOIN agent_runs r ON r.id = a.agent_run_id WHERE a.id = resource_uuid AND conversation_visible_to(r.conversation_id, c.actor_id))
        WHEN 'conversation' THEN conversation_visible_to(resource_uuid, c.actor_id)
        WHEN 'request' THEN request_visible_to(resource_uuid, c.actor_id)
        ELSE true
    END FROM external_agent_connections c WHERE c.id = connection_uuid;
$$;

-- Questions people were asked are requests already (same ids).
UPDATE external_agent_events SET resource_kind = 'request' WHERE resource_kind = 'person_question';
UPDATE notifications
   SET kind = 'request_raised', subject_kind = 'request'
 WHERE subject_kind = 'question';
UPDATE requests SET legacy_source = NULL WHERE legacy_source = 'person_question';
DROP FUNCTION requests_sync_question(UUID);
DROP TABLE person_questions;
