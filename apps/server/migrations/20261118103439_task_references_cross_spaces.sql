-- A message's task reference names the task, wherever it lives: a task's
-- discussion moves to another space with the task (C1.4) while the tasks
-- its comments name stay where they are, so the reference keeps its
-- message's space and points at the task by id alone.
ALTER TABLE message_work_item_references
    DROP CONSTRAINT message_work_item_references_project_id_work_item_id_fkey,
    ADD CONSTRAINT message_work_item_references_work_item_fk
        FOREIGN KEY (work_item_id) REFERENCES work_items(id);
