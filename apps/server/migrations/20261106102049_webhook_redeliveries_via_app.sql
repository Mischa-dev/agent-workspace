-- AWD-74: a delivery a person's app or script asked to send again (MCP
-- webhook_write "redeliver", or the public API) keeps to that app's agent
-- limit as it is when the delivery is sent, even when the webhook itself
-- was made in the browser. Deliveries an event queued keep the webhook's
-- own attribution (webhook_subscriptions.made_through_app).

ALTER TABLE webhook_deliveries
    ADD COLUMN made_through_app boolean NOT NULL DEFAULT false;
