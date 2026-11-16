-- H1.4 (build 3R): the workspace column on conversations, messages and what hangs off them, notifications, outbox events, reply requests, questions, decisions and references, switch step (H1 3.3).
-- Finishes the backfill inline (whether or not the job ran), makes
-- `workspace_id` NOT NULL through a validated CHECK, validates its foreign
-- key and indexes it where no index leads with it. Parents first.

SELECT tenancy_switch('conversations');
SELECT tenancy_switch('conversation_entries');
SELECT tenancy_switch('conversation_members');
SELECT tenancy_switch('conversation_notification_levels');
SELECT tenancy_switch('conversation_pins');
SELECT tenancy_switch('conversation_read_states');
SELECT tenancy_switch('messages');
SELECT tenancy_switch('message_link_previews');
SELECT tenancy_switch('message_mentions');
SELECT tenancy_switch('message_reactions');
SELECT tenancy_switch('message_revisions');
SELECT tenancy_switch('message_work_item_references');
SELECT tenancy_switch('notifications');
SELECT tenancy_switch('notification_occurrences');
SELECT tenancy_switch('notification_project_mutes');
SELECT tenancy_switch('outbox_events');
SELECT tenancy_switch('person_questions');
SELECT tenancy_switch('project_decisions');
SELECT tenancy_switch('push_deliveries');
SELECT tenancy_switch('reply_requests');
SELECT tenancy_switch('thing_references');
