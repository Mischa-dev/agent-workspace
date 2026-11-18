-- C2.20: a system entry says what happened in a conversation ("Lena made
-- this into a page"). It carries ids and kinds only (rule 10); readers
-- resolve names and titles live, with their own access.
ALTER TABLE conversation_entries
    ADD COLUMN system_event JSONB
        CHECK (system_event IS NULL OR (kind = 'system' AND jsonb_typeof(system_event) = 'object'));
