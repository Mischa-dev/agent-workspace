-- H1.6 (build 3R): the workspace column on pages, sheets, whiteboards' parts, collections and record history, expand step (H1 3.3).
-- Each table gets `workspace_id` (nullable for now, foreign key NOT VALID),
-- the fill trigger that sets it on insert from the row's project or
-- parent, and a `tenancy.backfill` job for the rows there are now. The
-- switch migration after this one finishes the copy inline and makes the
-- column required. Parents come before their children.

SELECT tenancy_expand('project_pages');
SELECT tenancy_expand('project_page_comments');
SELECT tenancy_expand('project_page_documents');
SELECT tenancy_expand('project_page_revisions', 'project_pages', 'page_id', 'workspace_id');
SELECT tenancy_expand('project_page_suggestions');
SELECT tenancy_expand('project_instructions');
SELECT tenancy_expand('sheets');
SELECT tenancy_expand('sheet_tabs');
SELECT tenancy_expand('sheet_cells');
SELECT tenancy_expand('sheet_cleared_cells', 'sheet_tabs', 'tab_id', 'workspace_id');
SELECT tenancy_expand('sheet_revisions');
SELECT tenancy_expand('whiteboard_elements', 'whiteboards', 'whiteboard_id', 'organization_id');
SELECT tenancy_expand('whiteboard_files', 'whiteboards', 'whiteboard_id', 'organization_id');
SELECT tenancy_expand('whiteboard_links', 'whiteboards', 'whiteboard_id', 'organization_id');
SELECT tenancy_expand('whiteboard_revisions', 'whiteboards', 'whiteboard_id', 'organization_id');
SELECT tenancy_expand('collections');
SELECT tenancy_expand('collection_views');
SELECT tenancy_expand('collection_records');
SELECT tenancy_expand('collection_record_history');
