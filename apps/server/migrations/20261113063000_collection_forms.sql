-- AWD-110: forms that add a record to a collection, grown from bug report
-- links. A link with a collection is a form: anyone with it fills in the
-- fields `form` asks for at /forms/{token}, and each answer becomes a record
-- there, made as the link's maker. `form` holds {"intro", "fields":
-- [{"field", "required"}], "thanks"}; see crate::collection_forms.
ALTER TABLE bug_report_links
    ADD COLUMN collection_id UUID,
    ADD COLUMN form JSONB CHECK (
        form IS NULL OR (jsonb_typeof(form) = 'object' AND octet_length(form::text) <= 16384)
    ),
    ADD CONSTRAINT bug_report_links_form_has_collection
        CHECK ((collection_id IS NULL) = (form IS NULL)),
    ADD CONSTRAINT bug_report_links_collection
        FOREIGN KEY (project_id, collection_id) REFERENCES collections (project_id, id);

CREATE INDEX bug_report_links_collection
    ON bug_report_links (collection_id, created_at DESC)
    WHERE collection_id IS NOT NULL;
