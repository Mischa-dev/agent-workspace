-- T1.4a (owner step 3b of moving tasks into collection records,
-- docs/plans/master/plans/T1-fields-views-tasks.md section 4): one read
-- shape for a task. `task_rows` is a task's core joined to its record, under
-- the column names readers used on work_items: the title, field values and
-- archive state come from the record, which owns them (T1.3), and the rest
-- from the core. Readers move to it file by file
-- (crate::task_rows_guard lists the ones that haven't), and then the copies
-- on work_items go (T1.4d).
--
-- The columns are named one by one, so the view doesn't hold on to the
-- cached columns once they're dropped. `record_version` is the record's own
-- version; `version` stays the task's, which expectedVersion checks.

CREATE VIEW task_rows AS
SELECT core.id, core.project_id, core.number, record.title, core.description,
       core.type_id, core.status_id, core.priority, core.assignee_actor_id,
       core.created_by_actor_id, record.archived_at, record.archived_by_actor_id,
       core.board_rank, core.version, core.created_at, core.updated_at,
       core.next_timeline_sequence, core.parent_work_item_id, core.blocked_reason,
       core.blocked_at, core.blocked_by_actor_id, core.search_vector,
       core.milestone_id, core.start_date, core.due_date, core.estimate,
       record.field_values, core.collection_id,
       record.collection_id AS record_collection_id,
       record.version AS record_version,
       record.updated_at AS record_updated_at,
       record.search_vector AS record_search_vector
FROM work_items core
JOIN collection_records record
  ON record.id = core.id AND record.project_id = core.project_id;
