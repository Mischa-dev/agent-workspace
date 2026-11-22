-- C1.37 (lane D2, documents): a page can move to another space. Its live
-- document is keyed by (project_id, page_id), so the key follows the page's
-- row when it moves (ids never change, so only a move ever cascades).
ALTER TABLE project_page_documents
    DROP CONSTRAINT project_page_documents_project_id_page_id_fkey,
    ADD CONSTRAINT project_page_documents_project_id_page_id_fkey
        FOREIGN KEY (project_id, page_id) REFERENCES project_pages(project_id, id)
        ON UPDATE CASCADE;
