# MCP spike

This development-only server exposes the official Rust MCP SDK's Streamable
HTTP transport at `/api/v1/mcp`. It binds only to loopback and requires
`AGENT_WORKSPACE_MCP_BEARER_TOKEN` with at least 32 characters.

```bash
AGENT_WORKSPACE_MCP_BEARER_TOKEN='replace-with-a-random-development-token' \
  cargo run -p mcp-spike
```

The fake work-item store exists only to verify read, write, optimistic
concurrency, retry, authentication, and Codex compatibility behavior. It is
not the application Work Item model.
