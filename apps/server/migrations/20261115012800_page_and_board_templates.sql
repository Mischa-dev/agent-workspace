-- C1.24: templates for pages and boards. A page or board marked as a
-- template shows in the "New … from template" gallery of everyone who can see
-- it; copying one fills {{date}}, {{author}} and {{project}}. Built-in
-- templates ship with the server and have no rows. Existing pages and boards
-- stay ordinary ones.

ALTER TABLE project_pages
    ADD COLUMN is_template BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX project_pages_templates
    ON project_pages (project_id, lower(title))
    WHERE is_template AND archived_at IS NULL;

ALTER TABLE whiteboards
    ADD COLUMN is_template BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX whiteboards_templates
    ON whiteboards (organization_id, lower(title))
    WHERE is_template AND deleted_at IS NULL;
