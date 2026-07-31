# Codex App Server compatibility

## Pinned contract

- Codex CLI: `0.146.0`
- Transport: JSONL over stdio
- Schema generation:
  `codex app-server generate-json-schema --out schemas/codex-app-server/0.146.0`
- Experimental schema: not enabled
- Client identity: `agent_workspace`

The generated schema bundle and `SHA256SUMS` are committed under
`schemas/codex-app-server/0.146.0`.

## Stable methods in M0

- `initialize`, followed by the `initialized` notification
- `account/read`
- `account/login/start`
- `account/login/cancel`
- `account/logout`
- `account/rateLimits/read`
- `thread/start`
- `thread/resume`
- `thread/read`
- `turn/start`
- `turn/interrupt`

M0 handles `item/started`, `item/completed`, agent-message deltas,
command-output deltas, `turn/completed`, `error`, approval requests,
user-input requests, and `serverRequest/resolved`.

## Verification record

Schema capture succeeded on macOS ARM64. Authentication, headless Red Hat
behavior, live turn fixtures, approval behavior, restart behavior, and known
failure modes are completed by M0-005 and recorded here without credentials or
raw reasoning.
