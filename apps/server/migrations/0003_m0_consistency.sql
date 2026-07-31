-- Tighten the project-key invariant to the documented 2-10 character form.
ALTER TABLE projects
    DROP CONSTRAINT projects_project_key_check;

ALTER TABLE projects
    ADD CONSTRAINT projects_project_key_check
    CHECK (project_key ~ '^[A-Z][A-Z0-9]{1,9}$');
