-- K3.17: every search source reads an index. Threads matched their name and
-- outcome without one; this is the expression the threads source matches.
CREATE INDEX IF NOT EXISTS conversations_thread_search
    ON conversations
    USING GIN (to_tsvector('english', name || ' ' || COALESCE(thread_outcome, '')))
    WHERE kind = 'thread';
