-- T1.28: a form is its own definition. Every form that lived on a bug report
-- link (`bug_report_links.form`) is copied here once, under the link's own
-- id, and the link points at it; a public form keeps its link (its token,
-- limits and state), an internal one has none and is filled in by members
-- in the app. The definition is crate::collection_forms::FormConfig:
-- {"intro", "fields": [{"field", "required", "showIf"}], "sections",
-- "sets", "thanks", "dailyLimit"}.

CREATE TABLE forms (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'),
    audience TEXT NOT NULL CHECK (audience IN ('public', 'internal')),
    definition JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(definition) = 'object' AND octet_length(definition::text) <= 32768
    ),
    created_by_actor_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    version BIGINT NOT NULL DEFAULT 1,
    archived_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, collection_id) REFERENCES collections (project_id, id)
);

CREATE INDEX forms_by_workspace ON forms (workspace_id);
CREATE INDEX forms_by_collection ON forms (space_id, collection_id, created_at);
SELECT tenancy_protect('forms', 'workspace_id');

ALTER TABLE bug_report_links ADD COLUMN form_id UUID UNIQUE REFERENCES forms(id);

-- The copy: idempotent, each link's form under the link's id.
INSERT INTO forms (id, workspace_id, space_id, collection_id, name, audience, definition,
                   created_by_actor_id, created_at, updated_at)
SELECT link.id, link.organization_id, link.project_id, link.collection_id, link.name, 'public',
       link.form, link.owner_actor_id, link.created_at, link.updated_at
FROM bug_report_links link
WHERE link.form IS NOT NULL
ON CONFLICT (id) DO NOTHING;

UPDATE bug_report_links SET form_id = id WHERE form IS NOT NULL AND form_id IS NULL;

ALTER TABLE bug_report_links
    DROP CONSTRAINT bug_report_links_form_has_collection,
    DROP COLUMN form,
    ADD CONSTRAINT bug_report_links_form_has_collection
        CHECK ((collection_id IS NULL) = (form_id IS NULL));
