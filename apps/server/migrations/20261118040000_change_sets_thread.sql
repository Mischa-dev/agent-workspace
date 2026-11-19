-- C1.22 (lane D2, documents): ask in a page comment. A change an agent (or
-- anyone) made because a comment thread asked names that thread, so the
-- thread can show "Edited the page · Undo" next to the reply. Proposals
-- already had `thread_id`; it now holds the thread that asked, too.

ALTER TABLE document_change_sets
    ADD COLUMN thread_id UUID REFERENCES conversations(id);
CREATE INDEX document_change_sets_thread
    ON document_change_sets (thread_id, created_at) WHERE thread_id IS NOT NULL;
