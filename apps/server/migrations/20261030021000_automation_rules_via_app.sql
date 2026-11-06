-- AWD-74: a rule made by an app or script acting for a person (a connected
-- app or a personal API token) acts within that app's agent limit when it
-- fires later, as the tool call that made it did. via_oauth_grant_id names
-- connected apps only; personal API tokens have no grant, so this records
-- that the rule was made through MCP or the public API at all.

ALTER TABLE automation_rules
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;

UPDATE automation_rules rule
SET made_through_app = true
WHERE rule.via_oauth_grant_id IS NOT NULL
   OR EXISTS (
       SELECT 1 FROM audit_events event
       WHERE event.target_id = rule.id
         AND event.action IN ('automation.created', 'automation.updated')
         AND event.channel = 'mcp'
         AND event.actor_id = rule.owner_actor_id
   );
