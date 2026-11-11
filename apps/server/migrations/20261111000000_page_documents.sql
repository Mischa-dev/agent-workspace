-- AWD-53 slice 3: pages become live documents that people and agents edit
-- at the same time.
--
-- project_page_documents holds a page's collaborative document: one encoded
-- yrs (Yjs) update carrying the whole state, block ids included. Once a page
-- has a row here, the document is the page's source of truth, and
-- project_pages.body is the Markdown derived from it, rewritten in the same
-- transaction as every change so search, page_get and exports keep working.
-- Pages convert when first opened (by the editor or by an agent's block
-- tools); a page that's never opened keeps only its Markdown.
--
-- generation names the document's lineage. Edits merge into it; if the
-- stored document ever has to be rebuilt instead (say, it can no longer be
-- read), it gets a new generation, and open editors holding the old one
-- start over rather than merging stale state back in.
--
-- The page row is the lock: every write takes project_pages FOR UPDATE
-- first, so the document and the derived body change together.
CREATE TABLE project_page_documents (
    page_id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    generation UUID NOT NULL,
    state BYTEA NOT NULL CHECK (octet_length(state) BETWEEN 1 AND 5242880),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, page_id) REFERENCES project_pages(project_id, id)
);

-- page.updated: a page's content or details changed. Ids and the page's
-- version only; open editors fetch what changed over HTTP.
ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification', 'whiteboard', 'page'
        )
    );
