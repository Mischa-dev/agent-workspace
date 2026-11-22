-- A2.28: every webhook push is a Standard Webhooks push. The older reply
-- wakeup bodies (`typed_v1`, `inbox_v1`) and the `x-workspace-*` headers are
-- gone, so a connection no longer has a format. Receivers set up for the
-- older bodies verify the Standard Webhooks headers instead (the runner SDKs'
-- `WorkspaceChannel` already does).
ALTER TABLE agent_connections DROP COLUMN webhook_format;
