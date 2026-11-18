-- C1.38 (lane D2, documents): pages inside pages. A page's parent is another
-- page of the same space; the things registry carries it as parent_id (no
-- link table). Children follow their parent's space through the key when a
-- page moves (ON UPDATE CASCADE), and `position` orders siblings.
ALTER TABLE project_pages
    ADD COLUMN parent_page_id UUID,
    ADD COLUMN position BIGINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT project_pages_parent_not_self CHECK (parent_page_id <> id),
    ADD CONSTRAINT project_pages_parent_fk
        FOREIGN KEY (project_id, parent_page_id) REFERENCES project_pages(project_id, id)
        ON UPDATE CASCADE;

CREATE INDEX project_pages_children ON project_pages (parent_page_id, position)
    WHERE parent_page_id IS NOT NULL;

CREATE OR REPLACE VIEW thing_source_project_pages AS
SELECT page.id, 'page'::text AS kind, project.organization_id AS workspace_id,
       page.project_id AS space_id, page.parent_page_id AS parent_id, NULL::uuid AS access_parent_id,
       left(page.title, 300) AS title, NULL::text AS status,
       page.created_by_actor_id AS owner_account_id, page.created_by_actor_id AS created_by_account_id,
       page.updated_by_actor_id AS updated_by_account_id, page.version,
       page.created_at, page.updated_at, page.archived_at, NULL::timestamptz AS deleted_at
FROM project_pages page
JOIN projects project ON project.id = page.project_id;

DROP TRIGGER things_sync ON project_pages;
CREATE TRIGGER things_sync AFTER INSERT OR DELETE OR UPDATE OF
    project_id, parent_page_id, title, updated_by_actor_id, version, updated_at, archived_at
    ON project_pages FOR EACH ROW EXECUTE FUNCTION things_sync('thing_source_project_pages', '{page}');
