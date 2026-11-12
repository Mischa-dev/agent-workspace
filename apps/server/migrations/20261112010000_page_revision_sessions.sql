-- AWD-53 (live pages, criterion 10): a page's history says which agent
-- session made each change, so a person can follow a version back to the
-- run or runner session (and its thread) that wrote it. Set when the writer
-- is an agent with an active session in the page's project; people keep
-- none.
ALTER TABLE project_page_revisions
    ADD COLUMN agent_session_id UUID REFERENCES agent_sessions(id);
