-- K2.2 (build 1O): the operation layer records the edge a write came
-- through truthfully. Calls through the public API and automation rules
-- used to be recorded as MCP calls; they get their own channels.
ALTER TABLE audit_events DROP CONSTRAINT IF EXISTS audit_events_channel_check;
ALTER TABLE audit_events ADD CONSTRAINT audit_events_channel_check CHECK (
    channel IS NULL OR channel IN ('browser', 'mcp', 'api', 'automation', 'host_cli', 'system')
);
