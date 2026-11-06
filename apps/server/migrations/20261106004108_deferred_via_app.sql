-- AWD-74: work a person's app or script set up to happen later (a
-- connected app or a personal API token) is done within that app's agent
-- limit as it is when the work runs, as the tool call that set it up was.
-- via_oauth_grant_id names connected apps only; personal API tokens have
-- no grant, so these record that it was set up through MCP or the public
-- API at all.
--
-- * scheduled_messages.made_through_app: sent within the app's limit.
-- * webhook_subscriptions.made_through_app: delivers only events the app's
--   limit lets it see.

ALTER TABLE scheduled_messages
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;

UPDATE scheduled_messages
SET made_through_app = true
WHERE via_oauth_grant_id IS NOT NULL;

ALTER TABLE webhook_subscriptions
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;

UPDATE webhook_subscriptions subscription
SET made_through_app = true
WHERE subscription.via_oauth_grant_id IS NOT NULL
   OR EXISTS (
       SELECT 1 FROM audit_events event
       WHERE event.target_id = subscription.id
         AND event.action IN ('webhook.created', 'webhook.updated')
         AND event.channel = 'mcp'
         AND event.actor_id = subscription.owner_actor_id
   );
