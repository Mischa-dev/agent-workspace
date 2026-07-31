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

`item/tool/requestUserInput` is experimental in the 0.146.0 schema. M0 accepts
it only at the provider boundary so a real blocking question can use the same
durable approval record and UI response path; no experimental shape leaks into
the application domain or browser contract.

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

## M3 supervised-process contract

M3 replaces the walking-skeleton's per-run App Server ownership with one lazy,
bounded supervisor per Execution Profile. The process starts in the configured
neutral data root; each typed Thread and turn adapter operation supplies the
exact managed worktree as `cwd`. Dropping a run lease does not terminate a
healthy process, so sequential runs on the same profile reuse the initialized
App Server. The durable scheduler still permits only one active turn for that
profile.

The registry enforces a typed host profile limit (one by default), one pending
adapter operation per leased profile, a 64-event pre-response buffer by
default, the configured JSONL/stderr/request and shutdown bounds, exponential
restart backoff capped at five seconds, and a 30-second circuit after five
consecutive transport starts fail. A transport failure terminates the entire
process group before a later request may restart it. Coordinated application
shutdown wakes active run drivers and then drains and terminates every retained
profile process. Notifications observed before a correlated response are
drained in arrival order; exceeding the event bound fails and terminates the
process.

Raw method names, response shapes, and provider requests remain in the Codex
adapter. The rest of the application receives typed thread IDs, turn IDs,
normalized activity events, approval/question requests, recovery summaries,
and redacted account status. Authenticated profile status exposes only
`stopped`, `ready`, or `circuit_open`, a bounded failure count, and process
start count; it does not expose PIDs, environment, filesystem paths, stderr,
provider JSON, or credentials.

Production mode never uses the bridge spike's automatic approval helper.
Command, file, and permission approvals plus user-input questions are the only
server requests admitted to the durable blocker flow. Every other provider
request receives a bounded JSON-RPC `method not found` response and cannot be
converted into an approval decision.

Deterministic CI starts a fake JSONL App Server, proves initialize/initialized
ordering, performs two sequential Thread/turn starts through the same process,
confirms the neutral process directory and isolated `CODEX_HOME`, rejects a
second profile at host capacity, and exercises coordinated shutdown. Live Red
Hat process-reuse and restart evidence remains part of the open M3 acceptance
gate.

Both operator-assisted authentication ceremonies completed against the pinned
Red Hat binary:

- Browser callback authentication completed with a disposable profile.
- Device-code authentication completed with the retained M0 execution profile.
- The retained-profile probe reported `authenticated = true`,
  `requiresOpenaiAuth = true`, and `rateLimitsAvailable = true`.

No credential, login URL, device code, callback parameter, or cached
authentication material is retained in the repository.

## Live retained-profile exercise

The live bridge used `initialize`, `initialized`, `account/read`,
`account/rateLimits/read`, `thread/start`, `thread/resume`, `turn/start`, and
`turn/interrupt`. It answered a server request for
`item/commandExecution/requestApproval`; accepting that request allowed the
bounded test turn to modify the known fixture file.

The first exercise created provider Thread
`019fb77a-ff64-73c3-960d-01e7d79e25e6` and completed Turn
`019fb77b-003c-7361-8f10-cbdfdc2fed64`. Its observed streamed shapes were:

| Shape | Count |
| --- | ---: |
| `account/rateLimits/updated` | 3 |
| `item/agentMessage/delta` | 43 |
| `item/commandExecution/requestApproval` | 1 |
| `item/completed` | 5 |
| `item/started` | 5 |
| `mcpServer/startupStatus/updated` | 13 |
| `serverRequest/resolved` | 1 |
| `thread/status/changed` | 4 |
| `thread/tokenUsage/updated` | 3 |
| `turn/completed` | 1 |
| `turn/started` | 1 |

The bridge then resumed the same provider Thread and completed Turn
`019fb77b-decc-70e2-831b-f734f7677dfd`.

The fixed interrupt path waited for `turn/started` before sending
`turn/interrupt`. It interrupted Turn
`019fb77e-a2a7-7d40-af41-1406e487cdab` on the same provider Thread and observed
8 `mcpServer/startupStatus/updated` events, 2 `thread/status/changed` events,
1 `turn/started` event, and 1 `turn/completed` event. A subsequent
`thread/resume` on that Thread completed Turn
`019fb77e-e5db-72a1-ba7f-df41736484cc`, including 21
`item/agentMessage/delta` events and 1 `turn/completed` event.

The redacted machine-readable record is
`tools/codex-bridge-spike/fixtures/live-evidence.json`. It retains only
contract methods, correlation IDs, booleans, outcomes, and bounded event
counts. Reasoning, prompts, command output, Message content, arbitrary
repository content, and absolute operator paths are excluded.
