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

Provider item notifications are reduced to an adapter-owned kind and bounded
stable identity. The identity uses the provider item or turn identifier when
present and a SHA-256 digest of the bounded JSONL notification otherwise. Only
that normalized key reaches durable event persistence; raw payloads and method
names do not reach Conversation or browser types. Approval and question request
IDs are similarly bounded or digested before uniqueness checks.

Question requests retain at most 20 bounded question identifiers and 40 safe
choice labels. The first bounded question prompt becomes the public summary;
arbitrary provider metadata is discarded. A typed browser answer is mapped to
the pinned App Server `answers` object inside the adapter. Approval decisions
remain the distinct `accept`/`decline` shape. Durable response claiming occurs
before transport so a crash or ambiguous write cannot send a human decision
twice; the conservative failure path preserves the worktree and audit row.

Deterministic CI starts a fake JSONL App Server, proves initialize/initialized
ordering, performs two sequential Thread/turn starts through the same process,
confirms the neutral process directory and isolated `CODEX_HOME`, rejects a
second profile at host capacity, and exercises coordinated shutdown. The live
Red Hat process-reuse and restart evidence is recorded in the M3 application
exercise below.

## M3 account adapter contract

The application account boundary uses only the pinned `account/read`,
`account/rateLimits/read`, `account/login/start`, `account/login/cancel`, and
`account/logout` methods. Browser and device-code results are converted to
bounded typed ceremonies. Login identifiers stay inside the adapter; browser
responses receive only an `http` or `https` URL, device responses receive the
verification URL and bounded user code, and no provider response body or
profile file is exposed on an error.

Each ceremony is bound in bounded server memory to one Execution Profile,
Actor, authenticated session, login kind, and idempotency key for ten minutes.
Only one ceremony per active host-profile slot is retained. Replaying the same
completed start returns the same ceremony, while another session, Actor, key,
kind, or profile fails closed. Status polling through `account/read` removes an
expired ceremony and clears a completed ceremony once authentication is
observed. The App Server owns and validates its browser callback state; Agent
Workspace does not implement a second callback endpoint or accept callback
parameters.

API-key login is an explicit settings action. The value exists only in the
bounded request body and the child JSONL request, is zeroized by the server
handler after the awaited adapter call, and is never stored in PostgreSQL,
ceremony state, browser cache data, logs, fixtures, or response DTOs. Logout
first changes durable admission state, stops an active profile lease and its
run safely, invokes the provider logout on a fresh isolated process, then
terminates that process. Cancellation and logout are safe to repeat.

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

## M3 live application exercise

The exact accepted M3 release reused one supervised App Server for a real
browser-driven Thread. A completed turn emitted command lifecycle, approval,
file-change, agent-message, and completion activity. Two command approvals were
resolved through the durable application blocker flow before the known fixture
file changed. The application persisted one consolidated Worker Message rather
than token-delta rows.

A second turn was active when systemd stopped the application process group.
On startup, the supervisor resumed the same provider Thread and read the exact
persisted provider Turn. The provider reported that the turn had been
interrupted, so the application conservatively made the Agent Run terminal
`interrupted`, idled the Provider Session, retained the exact branch/base and
worktree, and exposed a safe retry instead of inventing continuation.

The provider process environment contained only `CODEX_HOME`, `PWD`, `PATH`,
`LANG`, and `LC_ALL`; the npm wrapper added only package-location metadata.
Provider-started MCP children received no database, GitHub, browser-session,
webhook, or application-secret environment keys. The inspected service logs,
durable acceptance rows, and rendered browser output contained no credential
patterns, runtime paths, or disallowed control characters.

One pinned behavior remains explicit: asking the hosted model to call
`request_user_input` in the current execution mode produced a normal agent
message stating that the tool was unavailable, not an App Server
`item/tool/requestUserInput` request. Command approval requests are live and
fully supported. The adapter continues to accept and test the pinned
user-input request shape, but M3 acceptance does not claim that every hosted
model invocation can elect to emit that experimental request.
