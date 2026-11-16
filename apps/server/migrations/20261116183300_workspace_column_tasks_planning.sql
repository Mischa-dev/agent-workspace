-- H1.5 (build 3R): the workspace column on tasks and their satellite tables, labels, milestones, counters, recurring tasks, templates, imports, views and fields, expand step (H1 3.3).
-- Each table gets `workspace_id` (nullable for now, foreign key NOT VALID),
-- the fill trigger that sets it on insert from the row's project or
-- parent, and a `tenancy.backfill` job for the rows there are now. The
-- switch migration after this one finishes the copy inline and makes the
-- column required. Parents come before their children.

SELECT tenancy_expand('work_items');
SELECT tenancy_expand('field_definitions');
SELECT tenancy_expand('project_counters');
SELECT tenancy_expand('project_labels');
SELECT tenancy_expand('project_milestones');
SELECT tenancy_expand('project_navigation_states');
SELECT tenancy_expand('project_task_settings');
SELECT tenancy_expand('recurring_tasks');
SELECT tenancy_expand('recurring_task_occurrences');
SELECT tenancy_expand('saved_views');
SELECT tenancy_expand('saved_view_pins');
SELECT tenancy_expand('task_imports');
SELECT tenancy_expand('task_import_links');
SELECT tenancy_expand('task_templates');
SELECT tenancy_expand('work_item_acceptance_criteria');
SELECT tenancy_expand('work_item_activity');
SELECT tenancy_expand('work_item_claims');
SELECT tenancy_expand('work_item_comments');
SELECT tenancy_expand('work_item_comment_mentions');
SELECT tenancy_expand('work_item_comment_references');
SELECT tenancy_expand('work_item_comment_revisions');
SELECT tenancy_expand('work_item_conversation_links');
SELECT tenancy_expand('work_item_dependencies');
SELECT tenancy_expand('work_item_handoffs');
SELECT tenancy_expand('work_item_labels');
SELECT tenancy_expand('work_item_links');
SELECT tenancy_expand('work_item_relations');
SELECT tenancy_expand('work_item_repository_links');
SELECT tenancy_expand('work_item_status_transitions');
SELECT tenancy_expand('work_item_statuses');
SELECT tenancy_expand('work_item_timeline_entries');
SELECT tenancy_expand('work_item_types');
SELECT tenancy_expand('work_item_watchers');
SELECT tenancy_expand('work_item_worker_invocations');
