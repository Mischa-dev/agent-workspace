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

Schema capture succeeded on macOS ARM64 with `codex-cli 0.146.0`.

- Complete bundle checksum:
  `1a9a00c1ee35d44c8e04e92b394263544f90acaa7afe3c7023b08d9f0eb0d161`
- V2 bundle checksum:
  `ba073313ab0b84fa37af22ec4a4611de96c679df945c2c09caf5417088ebef3b`
- The Rust bridge completed `initialize` and sent `initialized` over JSONL
  stdio against the pinned native macOS ARM64 binary.
- An unauthenticated disposable profile returned `account/read` successfully
  and rejected `account/rateLimits/read` with JSON-RPC error `-32600`,
  `codex account authentication required to read rate limits`.
- The Red Hat ARM64 host reports `codex-cli 0.146.0` from
  `/opt/agent-workspace/bin/codex`; the pre-existing global Codex remains
  unchanged.
- Lines larger than 1 MiB, malformed JSON, stderr beyond 256 KiB, and requests
  beyond 120 seconds are bounded or rejected. Shutdown targets the spawned
  process group and escalates to a child kill after five seconds.
- The environment is cleared before launch. Only `CODEX_HOME`, `PWD`, and an
  explicit `PATH`, `LANG`, and `LC_ALL` allowlist are passed.

The browser callback, device-code login, authenticated rate-limit read, live
turn/approval/interruption/resume fixture, and restart behavior remain explicit
operator-assisted M0-005 acceptance steps. No credential, login URL or code,
reasoning, command output, Message content, arbitrary repository content, or
absolute operator path is retained in committed fixtures.
