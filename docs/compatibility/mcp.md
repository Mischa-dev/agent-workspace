# MCP compatibility

## Pinned development contract

- SDK: official Rust SDK `rmcp` `3.0.1`
- Transport: Streamable HTTP at `/api/v1/mcp`
- Listener: application loopback listener reached through SSH forwarding
- Authentication: high-entropy bearer token from
  `AGENT_WORKSPACE_MCP_BEARER_TOKEN`
- Request body ceiling: 1 MiB
- Session mode: legacy sessions enabled so Codex 0.146.0 can negotiate its
  supported protocol; JSON responses are preferred for request-response tools

The token is hashed before a constant-time comparison. It is never stored in
the fake work-item records or emitted by the server. Missing and incorrect
credentials return HTTP 401.

## Tools and annotations

- `get_fake_work_item(id)` is read-only, non-destructive, idempotent, and
  closed-world.
- `update_fake_work_item(id, expected_version, status)` is a write,
  non-destructive, idempotent, and closed-world operation.

The update response returns the current record and the fields that differ on an
optimistic concurrency conflict. Repeating a successful update with the same
expected version and status returns the current record with
`idempotentRetry: true` and does not increment the version again.

The in-memory store and its `demo-1` seed are explicitly development-only. M0
does not define the version-one Work Item schema or create an Agent Workspace
Thread for MCP calls.

## Verification record

Automated tests cover token strength, missing-token rejection, authenticated
initialization, read, update, conflict details, and idempotent retry.

The live M0-006 sequence completed on July 31, 2026:

- exact `codex-cli 0.146.0` ran from an additive project-local installation
  with a separate `CODEX_HOME`
- Codex reached the Red Hat loopback listener through SSH forwarding
- the initialization request selected MCP protocol `2025-06-18`; `rmcp`
  recognized that version and echoed it as the negotiated protocol
- Codex called `get_fake_work_item`, then
  `update_fake_work_item(expected_version=1, status=in_progress)`, then read
  the record again
- the initial record was version 1 in `ready`; the final record was version 2
  in `in_progress`
- the operation created no Agent Workspace Conversation or Thread

The bearer token remained in an owner-only file and child process environment.
It did not appear in terminal output, Codex output, or the redacted fixture at
`tools/mcp-spike/fixtures/live-evidence.json`.
