-- T1.22 Delegate: who a task is handed to for the work (often an agent),
-- beside who owns it (the assignee). Either can be a person or an agent, in
-- any combination. Like the assignee, the delegate is an active member of
-- the task's project (an account that can be given work, as the assignee's
-- trigger in 0015 says).

ALTER TABLE work_items ADD COLUMN delegate_actor_id UUID;

ALTER TABLE work_items
    ADD CONSTRAINT work_items_delegate_member
    FOREIGN KEY (project_id, delegate_actor_id)
    REFERENCES project_memberships(project_id, actor_id);

-- "Delegated to me" lists.
CREATE INDEX work_items_project_delegate
    ON work_items (project_id, delegate_actor_id)
    WHERE delegate_actor_id IS NOT NULL;
CREATE INDEX work_items_delegate
    ON work_items (delegate_actor_id)
    WHERE delegate_actor_id IS NOT NULL;

CREATE OR REPLACE FUNCTION validate_work_item_delegate()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    delegate_kind TEXT;
    delegate_active BOOLEAN;
BEGIN
    IF NEW.delegate_actor_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT a.kind, a.status = 'active' AND pm.enabled
      INTO delegate_kind, delegate_active
      FROM project_memberships pm
      JOIN actors a ON a.id = pm.actor_id
     WHERE pm.project_id = NEW.project_id
       AND pm.actor_id = NEW.delegate_actor_id
     FOR KEY SHARE OF pm, a;

    IF delegate_kind NOT IN ('human', 'external_agent')
       OR delegate_active IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'A task''s delegate must be an active member of its project'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_items_validate_delegate
BEFORE INSERT OR UPDATE OF project_id, delegate_actor_id
ON work_items
FOR EACH ROW
EXECUTE FUNCTION validate_work_item_delegate();

-- Tasks read through task_rows show their delegate too (a column added at
-- the end, so the view keeps its others).
CREATE OR REPLACE VIEW task_rows AS
SELECT core.id, core.project_id, core.number, record.title, core.description,
       core.type_id, core.status_id, core.priority, core.assignee_actor_id,
       core.created_by_actor_id, record.archived_at, record.archived_by_actor_id,
       core.board_rank, core.version, core.created_at, core.updated_at,
       core.next_timeline_sequence, core.parent_work_item_id, core.blocked_reason,
       core.blocked_at, core.blocked_by_actor_id,
       record.search_vector || core.description_vector AS search_vector,
       core.milestone_id, core.start_date, core.due_date, core.estimate,
       record.field_values, core.collection_id,
       record.collection_id AS record_collection_id,
       record.updated_at AS record_updated_at,
       record.search_vector AS record_search_vector,
       core.description_vector,
       core.delegate_actor_id
FROM work_items core
JOIN collection_records record
  ON record.id = core.id AND record.project_id = core.project_id;
