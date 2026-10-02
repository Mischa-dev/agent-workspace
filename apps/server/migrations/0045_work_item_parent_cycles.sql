-- A task can't end up under itself or under one of its own subtasks
-- (docs/plans/mcp-tools.md, Phase 2).
--
-- task_update checks this first and explains the problem with the path it
-- found; this trigger guarantees it for every writer. Task writes in a
-- project take turns on the project row (work_item_commands::lock_membership),
-- so the tree can't change between that check and this one.
--
-- An INSERT can't make a cycle, because a new task has no subtasks yet.
-- A parent in another project is already refused by
-- work_items_parent_project_fk (project_id, parent_work_item_id).

CREATE FUNCTION prevent_work_item_parent_cycle()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    found_cycle BOOLEAN;
BEGIN
    -- Walk up from the new parent. The depth bound only matters if a cycle
    -- were already stored; trees are at most 20 levels deep.
    WITH RECURSIVE ancestors(id, depth) AS (
        SELECT parent_work_item_id, 1
          FROM work_items
         WHERE project_id = NEW.project_id AND id = NEW.parent_work_item_id
        UNION ALL
        SELECT item.parent_work_item_id, ancestors.depth + 1
          FROM work_items item
          JOIN ancestors ON item.id = ancestors.id
         WHERE item.project_id = NEW.project_id AND ancestors.depth < 1000
    )
    SELECT NEW.parent_work_item_id = NEW.id
           OR EXISTS (SELECT 1 FROM ancestors WHERE id = NEW.id)
      INTO found_cycle;
    IF found_cycle THEN
        RAISE EXCEPTION 'A Work Item cannot be nested under itself or its own subtask'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER work_items_prevent_parent_cycle
BEFORE UPDATE OF parent_work_item_id ON work_items
FOR EACH ROW
WHEN (NEW.parent_work_item_id IS NOT NULL
      AND NEW.parent_work_item_id IS DISTINCT FROM OLD.parent_work_item_id)
EXECUTE FUNCTION prevent_work_item_parent_cycle();
