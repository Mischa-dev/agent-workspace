-- A2.6 (Lane E): a person's outgoing webhook is a connection.
--
-- The X4 webhook system (`webhook_subscriptions`, `webhook_deliveries`, its
-- own fan-out and sender) goes: each subscription becomes a connection of its
-- owner's account with a webhook, pushed by the one delivery path
-- (`delivery::push`) with ids only and Standard Webhooks signatures (the
-- `X-Workspace-*` alias headers stay). Its event types become one
-- subscription's kinds, its project that subscription's space, and it hears
-- everything its owner can see there, their own changes included. A paused
-- or turned-off webhook is a connection whose pushes are stopped. The old
-- delivery log is not carried over.
--
-- * agent_connections.made_through_app / via_grant_id: a connection a
--   person's app made carries the app's delegation limit when it is pushed
--   to, and stops pushing once the app is disconnected.
-- * agent_connections.last_test_*: what the webhook answered to the last
--   test ping (`connection_test`, sent on the job queue).

ALTER TABLE agent_connections
    ADD COLUMN made_through_app BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN via_grant_id UUID REFERENCES mcp_oauth_grants(id) ON DELETE SET NULL,
    ADD COLUMN last_test_at TIMESTAMPTZ,
    ADD COLUMN last_test_status INTEGER,
    ADD COLUMN last_test_error TEXT CHECK (last_test_error IS NULL OR char_length(last_test_error) <= 200);

INSERT INTO agent_connections (
    id, home_workspace_id, account_id, reaches_all, is_default, name, webhook_url,
    webhook_format, push_cursor, read_cursor, state, disabled_reason,
    consecutive_failures, last_delivered_at, created_by_account_id, version,
    created_at, updated_at, made_through_app, via_grant_id
)
SELECT subscription.id, subscription.organization_id, subscription.owner_actor_id, false, false,
       subscription.name, subscription.url, 'standard',
       counter.next_position - 1, counter.next_position - 1,
       CASE WHEN subscription.state = 'active' THEN 'active' ELSE 'push_disabled' END,
       CASE
           WHEN subscription.state = 'paused' THEN 'Paused. Save the webhook to turn it back on.'
           WHEN subscription.state = 'disabled' THEN left(COALESCE(subscription.disabled_reason, 'Turned off.'), 300)
       END,
       subscription.consecutive_failures, subscription.last_delivery_at,
       subscription.owner_actor_id, subscription.version,
       subscription.created_at, subscription.updated_at,
       subscription.made_through_app, subscription.via_oauth_grant_id
FROM webhook_subscriptions subscription, event_position_counter counter
WHERE subscription.url ~ '^https?://';

INSERT INTO connection_signing_secrets (id, connection_id, secret_id, state)
SELECT gen_random_uuid(), subscription.id, subscription.secret_id, 'active'
FROM webhook_subscriptions subscription
JOIN agent_connections connection ON connection.id = subscription.id
WHERE subscription.secret_id IS NOT NULL;

INSERT INTO event_subscriptions (
    id, connection_id, kinds, workspace_ids, space_ids, includes_dms, echoes,
    min_reason, set_by_account_id, created_at, updated_at
)
SELECT gen_random_uuid(), subscription.id, subscription.event_types,
       ARRAY[subscription.organization_id],
       CASE WHEN subscription.project_id IS NULL THEN NULL ELSE ARRAY[subscription.project_id] END,
       true, true, 'visible', subscription.owner_actor_id,
       subscription.created_at, subscription.updated_at
FROM webhook_subscriptions subscription
JOIN agent_connections connection ON connection.id = subscription.id;

DROP TABLE webhook_deliveries;
DROP TABLE webhook_subscriptions;

-- Sheets and records are things a webhook hears about too (`sheet.updated`,
-- `record.*`): checked like a page or a task, as the reader sees them now.
CREATE OR REPLACE FUNCTION delivery_thing_visible(thing UUID, recipient UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN EXISTS (SELECT 1 FROM conversations c WHERE c.id = thing)
            THEN conversation_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM requests r WHERE r.id = thing)
            THEN request_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM runs r WHERE r.id = thing)
            THEN run_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM whiteboards w WHERE w.id = thing)
            THEN whiteboard_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM work_items w WHERE w.id = thing)
            THEN subject_visible_to('task', thing, recipient)
        WHEN EXISTS (SELECT 1 FROM project_pages p WHERE p.id = thing)
            THEN subject_visible_to('page', thing, recipient)
        WHEN EXISTS (SELECT 1 FROM sheets s WHERE s.id = thing)
            THEN subject_visible_to('sheet', thing, recipient)
        WHEN EXISTS (SELECT 1 FROM collection_records r WHERE r.id = thing)
            THEN subject_visible_to('record', thing, recipient)
        ELSE true
    END
$$;
