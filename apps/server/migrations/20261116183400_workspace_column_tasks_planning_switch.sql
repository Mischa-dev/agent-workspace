-- H1.5 (build 3R): the workspace column on tasks and their satellite tables, labels, milestones, counters, recurring tasks, templates, imports, views and fields, switch step (H1 3.3).
-- Finishes the backfill inline (whether or not the job ran), makes
-- `workspace_id` NOT NULL through a validated CHECK, validates its foreign
-- key and indexes it where no index leads with it. Parents first.

SELECT tenancy_switch('work_items');
SELECT tenancy_switch('field_definitions');
SELECT tenancy_switch('project_counters');
SELECT tenancy_switch('project_labels');
SELECT tenancy_switch('project_milestones');
SELECT tenancy_switch('project_navigation_states');
SELECT tenancy_switch('project_task_settings');
SELECT tenancy_switch('recurring_tasks');
SELECT tenancy_switch('recurring_task_occurrences');
SELECT tenancy_switch('saved_views');
SELECT tenancy_switch('saved_view_pins');
SELECT tenancy_switch('task_imports');
SELECT tenancy_switch('task_import_links');
SELECT tenancy_switch('task_templates');
SELECT tenancy_switch('work_item_acceptance_criteria');
SELECT tenancy_switch('work_item_activity');
SELECT tenancy_switch('work_item_claims');
SELECT tenancy_switch('work_item_comments');
SELECT tenancy_switch('work_item_comment_mentions');
SELECT tenancy_switch('work_item_comment_references');
SELECT tenancy_switch('work_item_comment_revisions');
SELECT tenancy_switch('work_item_conversation_links');
SELECT tenancy_switch('work_item_dependencies');
SELECT tenancy_switch('work_item_handoffs');
SELECT tenancy_switch('work_item_labels');
SELECT tenancy_switch('work_item_links');
SELECT tenancy_switch('work_item_relations');
SELECT tenancy_switch('work_item_repository_links');
SELECT tenancy_switch('work_item_status_transitions');
SELECT tenancy_switch('work_item_statuses');
SELECT tenancy_switch('work_item_timeline_entries');
SELECT tenancy_switch('work_item_types');
SELECT tenancy_switch('work_item_watchers');
SELECT tenancy_switch('work_item_worker_invocations');
