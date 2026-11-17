-- K5.7: an MCP session a client ended with DELETE stays ended on every
-- server, not only the one that saw the DELETE. Only a digest of the
-- session id is kept; retention clears rows after 30 days.
CREATE TABLE mcp_ended_sessions (
    session_digest BYTEA PRIMARY KEY CHECK (octet_length(session_digest) = 32),
    ended_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX mcp_ended_sessions_ended_at ON mcp_ended_sessions (ended_at);
