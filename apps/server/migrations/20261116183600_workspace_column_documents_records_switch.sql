-- H1.6 (build 3R): the workspace column on pages, sheets, whiteboards' parts, collections and record history, switch step (H1 3.3).
-- Finishes the backfill inline (whether or not the job ran), makes
-- `workspace_id` NOT NULL through a validated CHECK, validates its foreign
-- key and indexes it where no index leads with it. Parents first.

SELECT tenancy_switch('project_pages');
SELECT tenancy_switch('project_page_comments');
SELECT tenancy_switch('project_page_documents');
SELECT tenancy_switch('project_page_revisions');
SELECT tenancy_switch('project_page_suggestions');
SELECT tenancy_switch('project_instructions');
SELECT tenancy_switch('sheets');
SELECT tenancy_switch('sheet_tabs');
SELECT tenancy_switch('sheet_cells');
SELECT tenancy_switch('sheet_cleared_cells');
SELECT tenancy_switch('sheet_revisions');
SELECT tenancy_switch('whiteboard_elements');
SELECT tenancy_switch('whiteboard_files');
SELECT tenancy_switch('whiteboard_links');
SELECT tenancy_switch('whiteboard_revisions');
SELECT tenancy_switch('collections');
SELECT tenancy_switch('collection_views');
SELECT tenancy_switch('collection_records');
SELECT tenancy_switch('collection_record_history');
