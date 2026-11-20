-- A2.16: a conversation's memory page is seen only by readers of that
-- conversation, wherever a page is checked: its attachments, proposals and
-- change sets, discussion and every other reader of `subject_visible_to`,
-- not only the page routes (`page_visible_to`). The page branch carries the
-- rule itself; the other branches are as 20261115060000_discussion.sql
-- made them.
CREATE OR REPLACE FUNCTION subject_visible_to(target_kind TEXT, target_id UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE target_kind
        WHEN 'whiteboard' THEN whiteboard_visible_to(target_id, viewer)
        WHEN 'task' THEN EXISTS (
            SELECT 1 FROM work_items item
            WHERE item.id = target_id
              AND subject_space_member(viewer, item.project_id)
              AND subject_sees_tasks(viewer, item.project_id))
        WHEN 'page' THEN EXISTS (
            SELECT 1 FROM project_pages page
            WHERE page.id = target_id
              AND subject_space_member(viewer, page.project_id)
              AND (page.memory_for_kind IS DISTINCT FROM 'conversation'
                   OR COALESCE(conversation_visible_to(page.memory_for_id, viewer), false)))
        WHEN 'sheet' THEN EXISTS (
            SELECT 1 FROM sheets sheet
            WHERE sheet.id = target_id
              AND subject_space_member(viewer, sheet.project_id))
        WHEN 'record' THEN EXISTS (
            SELECT 1 FROM collection_records record
            JOIN collections collection ON collection.id = record.collection_id
            WHERE record.id = target_id
              AND subject_space_member(viewer, record.project_id)
              AND (NOT collection.acts_like_task
                   OR subject_sees_tasks(viewer, record.project_id)))
        ELSE false
    END
$$;
