-- Deploy fix, found rehearsing the upgrade on a copy of the live data:
-- the default task workflow (types, statuses and the moves between them)
-- was provisioned for every project, the workspace's hidden Messages
-- project included. 20261117173117 moves whatever else such a project
-- holds to the workspace's own space, one table at a time, so it moved the
-- transitions before the statuses they name and stopped on their key; had
-- it got through, the space would have had a second default workflow. A
-- hidden project's workflow is scaffolding like its counters and
-- instructions: where nothing uses it (no task, template or pull request
-- result), it goes. Where the hidden projects are already gone this changes
-- nothing.

CREATE TEMP TABLE unused_direct_workflows ON COMMIT DROP AS
SELECT direct.id AS project_id
FROM projects direct
WHERE direct.kind = 'direct'
  AND NOT EXISTS (SELECT 1 FROM work_items item WHERE item.project_id = direct.id)
  AND NOT EXISTS (SELECT 1 FROM task_templates template WHERE template.project_id = direct.id)
  AND NOT EXISTS (
          SELECT 1 FROM github_pull_request_automation_results result
          WHERE result.project_id = direct.id);

DELETE FROM work_item_status_transitions
WHERE project_id IN (SELECT project_id FROM unused_direct_workflows);
DELETE FROM work_item_statuses
WHERE project_id IN (SELECT project_id FROM unused_direct_workflows);
DELETE FROM work_item_types
WHERE project_id IN (SELECT project_id FROM unused_direct_workflows);
