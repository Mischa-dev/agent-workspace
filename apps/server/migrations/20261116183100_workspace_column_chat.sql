-- H1.4 (build 3R): the workspace column on conversations, messages and what hangs off them, notifications, outbox events, reply requests, questions, decisions and references, expand step (H1 3.3).
-- Each table gets `workspace_id` (nullable for now, foreign key NOT VALID),
-- the fill trigger that sets it on insert from the row's project or
-- parent, and a `tenancy.backfill` job for the rows there are now. The
-- switch migration after this one finishes the copy inline and makes the
-- column required. Parents come before their children.

SELECT tenancy_expand('conversations');
SELECT tenancy_expand('conversation_entries', 'conversations', 'conversation_id', 'workspace_id');
SELECT tenancy_expand('conversation_members');
SELECT tenancy_expand('conversation_notification_levels');
SELECT tenancy_expand('conversation_pins', 'conversations', 'conversation_id', 'workspace_id');
SELECT tenancy_expand('conversation_read_states');
SELECT tenancy_expand('messages');
SELECT tenancy_expand('message_link_previews');
SELECT tenancy_expand('message_mentions');
SELECT tenancy_expand('message_reactions');
SELECT tenancy_expand('message_revisions');
SELECT tenancy_expand('message_work_item_references');
SELECT tenancy_expand('notifications');
SELECT tenancy_expand('notification_occurrences', 'notifications', 'notification_id', 'workspace_id');
SELECT tenancy_expand('notification_project_mutes');
SELECT tenancy_expand('outbox_events');
SELECT tenancy_expand('person_questions');
SELECT tenancy_expand('project_decisions');
SELECT tenancy_expand('push_deliveries', 'push_subscriptions', 'subscription_id', 'organization_id');
SELECT tenancy_expand('reply_requests');
SELECT tenancy_expand('thing_references');
