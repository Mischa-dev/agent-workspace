# K2. The operation layer

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for K2, 2026-11-14. Read-only review of `/home/user/agent-workspace` at fe95a74.
Nothing was built or run. Line numbers are from that tree. "Inferred" marks anything not read
directly in code.

## 1. Goal

Every thing the app can do is written once, as an **operation**: a name, an input type, the
permission it needs, whether it can arrive as a proposal, and one function that does the work
in one transaction. The web app's route and its typed client, the agent tool, the public API
endpoint, the automation action and the audit entry are all produced from that one
definition, so a person and an agent asking for the same thing run the same code and get the
same answer, with the same errors, the same retry safety and the same audit row.

What a user or agent notices: agents' task comments mention people like everyone else's;
retrying any write never doubles it; every tool answers in one envelope; tool names follow one
pattern an agent can guess (`task_update`, `page_update`, `sheet_update`); the tool list,
the public API and the web client can never disagree about what exists; and a new module
gets its web route, agent tool, API endpoint and automation action without anyone writing
them.

## 2. Where the code is today

### 2.1 The two edges, and what sits between them

**Web edge.** About 70 router files each build their own `Router` and are merged in
`lib.rs:153-214`. There are 383 `.route(` calls; the mutating ones are inventoried as 303
method-and-path pairs in `policy_inventory.rs:75` (232 `policy_route!`, 60 `boundary_route!`
plus the MCP protocol entry), with about 177 `get(` handlers beside them (counted by a scan of
route blocks; approximate). A handler takes `AuthenticatedActor` (`auth/mod.rs:49`), checks
CSRF and write access with `require_authenticated_write` (227 call sites), reads an
`Idempotency-Key` header itself where it cares (56 call sites of
`IdempotencyKey::from_headers`), runs its own authorization helper, calls a domain function,
writes its own audit row and maps errors to `AppError`'s JSON `{error:{code,message,details}}`
(`error.rs:235`).

**MCP edge.** `mcp.rs` is 6,700 lines; 189 tools are methods of one `#[tool_router] impl
McpServer` (`mcp.rs:216-3676`), each about 18 lines: description, annotations,
`principal_or!`, and a call into `mcp/<area>.rs` (60 files, about 49,600 lines). Before the
tool runs, the axum middleware `authenticate_mcp` (`mcp.rs:4327`) resolves the bearer, rewrites
keys such as `AWD-12` into ids (`keys_to_ids`, `mcp.rs:5386`) and picks the project the call
acts in **by argument name** (`scoped_project`, `mcp.rs:5906`, tables `PRIMARY_SCOPES` and
`OTHER_SCOPES` at `mcp.rs:5779-5878`). `run_tool` (`mcp.rs:4000`) then refuses tools no role
allows (`mcp/tool_permissions.rs:24`, one hand-kept line per tool) and tools of a module that is
off (`module_off_here`, `mcp.rs:3899`). The tool then checks scope and membership again with its
own helper (`writes::check_access_for` `mcp/writes.rs:2131`, `memory::project_access`,
`can_write_in`, `whiteboards::access`, and others).

**Public API and automations ride on the MCP edge.** `POST /api/public/v1/{tool}`
(`public_api.rs:46-48`, `:72-111`) and three internal callers use `mcp::call_tool_direct`
(`mcp.rs:4250-4325`), which builds fake HTTP request parts and an in-memory rmcp peer on a
`tokio::io::duplex` pipe to run one tool:

| Caller | Where | What it calls |
|---|---|---|
| Public API | `public_api.rs:94` | any tool by name |
| Automations | `automations.rs:2410` (`call_tool`, used at `:2592` `task_update` and `:2619-2623` `message_send`) | `task_update`, `message_send` |
| Bug report links | `bug_reports.rs:968` | `task_create` |
| Collection forms | `bug_reports.rs:1108` | `record_write` |

The OpenAPI document is generated from the tool schemas (`public_api.rs:50`, `hoist` at
`:186`), which is the right idea and is kept.

**Actor and context types.** Nine structs describe "who is doing this", one per area:
`AuthenticatedActor` (`auth/mod.rs:49`), `McpPrincipal` (`mcp.rs:138`), `CommandActor` with
`AuditAttribution::{Browser, Mcp, Job}` (`work_item_commands.rs:39-66`),
`MessageCommandActor` (`conversations.rs:1628`), `memory::Writer` (`memory.rs:119`),
`work_item_fields::Writer` (`work_item_fields.rs:123`), `whiteboards::Caller`
(`whiteboards.rs:42`), `events::Caller` (`events.rs:274`), `launch::LaunchCaller`
(`launch.rs:541`), plus automations' `Acting` (`automations.rs:1752`). They carry the same
four facts (account, kind, via-app, edge) in different shapes.

**Schemas.** `JsonSchema` is derived in 76 files through rmcp's re-export
(`use rmcp::schemars`, e.g. `modules/http.rs:8`), schemars 1.2.2 (`Cargo.lock:2675`). There is
no direct `schemars` dependency in `apps/server/Cargo.toml`.

### 2.2 Four operations traced end to end

**Message send (the good pattern).**
- Web: `POST /api/v1/channels/{id}/messages`, `/threads/{id}/messages`,
  `/conversations/{id}/messages` (`conversations.rs:65-70`) → `send_message`
  (`conversations.rs:1594`) / `send_direct_message` (`:1560`): CSRF check, idempotency key from
  the header, builds `MessageCommandActor` with `via_grant_id: None`.
- MCP: `message_send` router method (`mcp.rs:825`) → `talk::message_send`
  (`mcp/talk.rs:1538`): `can_write`, parses ids, builds content with `message_content`
  (mentions resolved), its own module-off check (`:1580-1592`) → `talk::send`
  (`mcp/talk.rs:1780`): `clientRequestId` is optional and, if absent, a random key is made
  (`:1789`), so an agent that omits it is not retry-safe; maps `AppError` to `ToolResult`
  (`:1824-1835`).
- Both meet in `conversations::append_message_command_with` (`conversations.rs:1695`), which
  owns the transaction, the idempotency reserve (scope `conversation.<id>.message_send`,
  `:1747`), visibility, mentions, outbox and reply rules.
- Written twice: argument parsing and error mapping only.

**Task update (shared command, doubled edge).**
- Web: `PATCH .../work-items/{id}` → `work_items::update_work_item` (`work_items.rs:1071`):
  its own "nothing to change" rule (`:1081-1096`), `normalize_title`, idempotency scope
  `work_items.<id>.update` (`:1116`), request hash over its own field list (`:1117-1140`),
  `lock_project_membership` + `authorize_work_item(Action::WorkItemEdit)` (`:1142-1150`),
  label check, custom fields via `custom_fields::web_field_values`, then
  `update_work_item_by_id` with `audit_action: "work_item.updated"` (`:1199-1222`), then its
  own outcome-to-error mapping (`:1224-1262`).
- MCP: `task_update` → `writes::update_work_item_inner` (`mcp/writes.rs:893`): its own
  "nothing to change" rule (`:942-968`), `normalize_text`+`TITLE_MESSAGE` instead of
  `normalize_title` (`:981-990`), keys and names resolved (labels, milestone, status by name),
  idempotency scope `mcp_work_items.<id>.update` (`:1029-1031`),
  `authorize_write` → `check_access_for` (`:2131`, which also refuses any account kind other
  than person or `external_agent`, `:2146-2153`), then `update_work_item_by_id` with
  `mcp_actor` (`:2821`), audit action names such as `work_item.mcp_status_updated`
  (`:849`), and `finish_update_outcome` mapping to `WriteResult`.
- Meet in `work_item_commands::update_locked_work_item` (`work_item_commands.rs:716`).
- Written twice: validation, authorization call, idempotency scope and hash, audit action
  name, result shape. About 150 to 250 lines each side.

**Page write (shared store, drifted edges).**
- Web: `PATCH /api/v1/pages/{id}` → `update_page_http` (`memory.rs:1374`): no idempotency
  key at all, `project_access(Action::PageEdit)`, `memory::update_page` (`memory.rs:652`),
  audit `page.updated`. Live editing goes through the Yjs document routes instead
  (`memory.rs:65-77`).
- MCP: `page_write` → `memory_tools::page_write` (`mcp/memory_tools.rs:476`): create-or-update
  in one tool, no `clientRequestId` (a retried create makes two pages), `can_write_in` then
  `memory::project_access`, **an agent's new text for a page someone else made is turned into a
  suggestion** (`:528-541`, using `page_docs/suggestions.rs:42-48`, `must_suggest` = "is an
  agent and not the creator"), audit `page.mcp_written`.
- Meet in `memory::create_page` / `memory::update_page`.
- Written twice: authorization, audit name, idempotency (missing on both), and a permission
  rule (suggest-instead-of-edit) that exists only on one edge and is decided by account kind.

**Sheet cell edit (shared engine, doubled transaction wrapper).**
- Web: `POST /api/v1/projects/{project_id}/sheets/{sheet_id}/changes` → `change_sheet`
  (`sheets/http.rs:765`): idempotency scope `sheets.<sheet>.changes`, `sheets::apply`, audit
  `sheet.changed` and outbox **only when the revision moved** (`:833-850`).
- MCP: `sheet_write` (`mcp/sheet_tools.rs:968`): also creates sheets and changes tabs; scope
  `mcp_sheets.<project>.sheet_write` (`:1049`), its own module-off check (`:1040`),
  `sheets::apply` (`:1180`), outbox when touched, audit **always** (`:1221-1233`).
- Meet in `sheets::apply` (`sheets.rs:863`).
- Written twice: transaction wrapper, idempotency, audit rule (already different).

Another, smaller example shows the naming drift: `memory::DecisionInput` takes
`sourceMessageId` (`memory.rs:965-974`); the MCP `decision_record` takes `fromMessageId`
(`mcp/memory_tools.rs:1565-1582`). Neither edge is retry-safe (`memory.rs:1482-1514`,
`mcp/memory_tools.rs:1670`).

### 2.3 Counts

| Measure | Count | Source |
|---|---|---|
| MCP tools | 189 (+5 prompts) | `mcp.rs` `#[tool(` ×189 |
| Tool kinds in manifests | change 82, query 57, create 28, describe 20, suggest 2 | `modules/*.json` `tools` maps |
| Write tools | about 113 | E report, F5 |
| Write tools without `clientRequestId` | 83 of 113 | E report, F5 (parsed structs) |
| Web mutating routes | 303 | `policy_inventory.rs` |
| … mapped to a tool | 204 routes naming 100 distinct tools | `agent_parity.rs` `Tool(` |
| … "person's own act" | 73 | `agent_parity.rs` `Person(` |
| … plumbing | 26 | `agent_parity.rs` `Plumbing(` |
| Web GET handlers | about 177 | route scan |
| Web `requestJson` calls | 443 (non-test) | `apps/web/src` |
| Result envelopes on MCP | 6 | E report F4 |

So about 100 write operations exist on both edges (two edge implementations each), about 13
write tools exist only on MCP (claims and handoffs `mcp/tasks.rs:3274,3407`, reply leases
`mcp/replies.rs:461,727,836`, `page_edit`, `task_reorder`), and most reads exist on both
edges with separate SQL (task lists twice: `mcp/tasks.rs:600-1420` and `work_items/list.rs`).
Deduplicated, the registry will hold roughly 230 to 260 operations (estimate: 189 tools, plus
the person-only writes that become ordinary operations with a permission, minus tools that
merge).

### 2.4 Hard-coded account-kind differences found on this path

Listed for K1, which owns their replacement; the operation layer must not carry them over.

| Where | Rule | Replaced by |
|---|---|---|
| `page_docs/suggestions.rs:42-48`, used at `mcp/memory_tools.rs:528-541` | An agent's edit to a page it didn't create becomes a suggestion | K1's suggest-without-edit role setting on pages; K2's `PROPOSABLE` |
| `mcp/writes.rs:2146-2153` | Task writes refuse accounts other than person or `external_agent` | K1's access function |
| `agent_parity.rs` `Person("adding an agent makes its credential")` ×9, `Person("credentials are made and revoked by people")` ×4, `Person("ownership passes between people")`, `Person("secrets never pass through an agent")` ×9, `Person("registers a program the host runs")` ×3 | Whole operations exist only on the web edge because "only a person" may do them | Ordinary operations with permissions K1 names (`agents.manage`, `credentials.manage`, `secrets.manage`, `hosts.manage`), default-off in starting roles |
| Descriptions: `settings_set` "agents only read settings" (`mcp.rs:292`), `agent-interface.md:221` "people only", "never agents" | Copy states a kind rule | Generated permission text (§3.9) |

### 2.5 What to keep

- `append_message_command_with`, `work_item_commands`, `memory::record_decision`,
  `collections::records::write`, `sheets::apply`, `whiteboards::edit::apply`: the domain
  functions operations wrap.
- `idempotency::{reserve, complete}` (`idempotency.rs:131,210`) with its 24-hour keys
  (`:191`) and the advisory lock: the operation layer uses it as is.
- `ToolResult<T>` (`mcp/common.rs:46`): the canonical envelope grows from it.
- The per-request credential recheck (`mcp.rs:4755`, `resolve_bearer_principal`) and rate
  limiting at the edges.
- `ResumeAnySession` (`mcp.rs:4087-4178`): old-revision clients without sticky sessions.
- Role- and module-filtered tool lists with private cache scope (`mcp.rs:3772-3809`).
- The `JobRegistry` pattern (`jobs.rs:362`, each module calls `register_jobs`): operations
  register the same way.
- The route-inventory scan (`policy_inventory.rs:1820-1883`), isolation guards
  (`tests/integration/workspace_isolation.rs`, `write_isolation.rs`) and the agent-facing
  wording of descriptions and messages.

### 2.6 What is in the way

- `#[tool_router]` generates tool methods from per-method attributes, so tool metadata
  (permission, module, kind, target) lives in four side tables keyed by name strings:
  `tool_permissions.rs`, `modules/*.json`, `public_api::tag_for` (`public_api.rs:131`),
  `SPANNING_LISTS` (`mcp.rs:3883`), `ALSO_SERVED_BY`, `keys_to_ids`' per-tool lists,
  `identifier_project`.
- The project a call acts in is chosen by argument name before the tool runs, so every tool
  rechecks by convention (E F6).
- Six envelopes; `public_api::status_for` maps unknown outcomes to 200 (`public_api.rs:118-127`).
- Idempotency scopes differ per edge (`work_items.*` vs `mcp_work_items.*`), so a web and an
  MCP retry of the same request can't replay each other, and switching an edge changes the
  scope.
- Web API types are hand-written (I report F4), so a renamed server field fails silently.

## 3. The target design

### 3.1 Shape in one picture

```
 edges (generated)                     ops kernel (apps/server/src/ops/)
 ------------------                    ---------------------------------------------
 web  POST /api/v2/w/{ws}/ops/{name}  \   (session + CSRF)
 api  POST /api/v2/w/{ws}/ops/{name}  \   (bearer)  Caller --> dispatch(name, Caller, Value)
      POST /api/v2/direct/ops/{name}  /   (DMs and groups, ruling R20)
 mcp  tools/call {name}                 ---->  1 find entry (or alias)
 automation action RunOperation         /      2 decode input (serde, schema)
 job / proposal acceptance             /       3 resolve refs (keys, links -> ids)
                                               4 locate target -> workspace, space
                                               5 module gate (M1), access (K1)
                                               6 limits: H1 metering::admit, once per write (K5 store)
                                               7 tx: db::scoped_tx (app.tenancy), idempotency reserve
                                               8 propose (C1) or run
                                               9 audit + events (K3), complete, commit
                                              10 Envelope -> edge renders it
 module code: src/<module>/ops.rs  -- impl Operation for TaskUpdate { run(...) }
              calls the existing domain function (work_item_commands, ...)
```

### 3.2 Choice: trait, macro or derive

| Option | For | Against |
|---|---|---|
| Proc-macro derive (`#[derive(Operation)]` or `#[operation(...)]`) | Least boilerplate per op | Needs a second crate (`agent-workspace-macros`), against "one server crate until boundaries are proven"; errors are hard to read; rmcp's macros already show the cost (side tables) |
| `macro_rules!` declaring an op | No new crate | Every op becomes macro syntax; rustfmt and rust-analyzer help less; metadata still stringly |
| **Plain trait with associated consts and types, erased by one generic function** | Ordinary Rust; consts are checked by the compiler; schemas from `JsonSchema`; registry is a `Vec` built like `JobRegistry` | About 25 lines per op instead of 10 |

**Pick: the plain trait.** The per-op cost is paid once and is readable; the generic
`entry::<O>()` does the erasure; a 10-line `macro_rules! register!` may shorten the
registration list but holds no logic.

### 3.3 The Rust definition

New module `apps/server/src/ops/`:

```
ops/mod.rs          Operation trait, OpKind, OpMeta, re-exports
ops/context.rs      Caller, Edge, OpContext, Authorized<T>
ops/target.rs       Target, Targeted, Resolve (keys and links to ids)
ops/error.rs        OpError, ConflictReason, DenyReason, RetryCategory
ops/envelope.rs     Envelope<T>, Page<T>, Outcome
ops/registry.rs     OpEntry, Registry, register_all(), aliases
ops/dispatch.rs     the pipeline in 3.1
ops/web.rs          axum routes for the web and the public API
ops/mcp.rs          rmcp list_tools/call_tool bridge, resources, tasks
ops/automation.rs   RunOperation action, action catalogue
ops/codegen/        ts.rs, openapi.rs, catalog.rs (tests write and check files)
ops/conventions.rs  #[cfg(test)] convention checks over the registry
```

**The trait.**

```rust
// ops/mod.rs
pub enum OpKind { Describe, Query, Create, Change, Suggest }

pub enum Idempotency {
    /// The dispatcher reserves `request_id` in its transaction before `run`.
    Dispatcher,
    /// The wrapped command reserves it itself (message send); the dispatcher
    /// passes `ctx.request_id` through and never reserves twice.
    Command,
    /// Reads.
    None,
}

pub trait Operation: Send + Sync + 'static {
    const NAME: &'static str;            // "task_update"
    const KIND: OpKind;
    const MODULE: &'static str;          // manifest id: "tasks"
    const PERMISSION: Option<Perm>;      // K1's catalogue key; None = anyone who can see the target
    const PROPOSABLE: bool = false;      // may arrive as a proposal (C1) when access says "suggest"
    const IDEMPOTENCY: Idempotency;
    const LONG: bool = false;            // may run as a job (MCP Tasks extension)
    const PREVIEW: bool = false;         // accepts `preview: true` and rolls back
    const TITLE: &'static str;           // "Update task"
    const DESCRIPTION: &'static str;     // for agents and docs; vendor-free (3.9)
    const EMITS: &'static [&'static str] = &[]; // K3 event kinds, for triggers and docs

    type Input: DeserializeOwned + Serialize + JsonSchema + Targeted + Resolve + Send;
    type Output: Serialize + JsonSchema + Send;

    fn run(call: &mut OpCall<'_>, input: Self::Input)
        -> impl Future<Output = Result<Self::Output, OpError>> + Send;

    /// For PROPOSABLE ops: what the proposal shows (a diff, a preview). The
    /// default stores the input as is.
    fn propose(call: &mut OpCall<'_>, input: &Self::Input)
        -> impl Future<Output = Result<ProposalDraft, OpError>> + Send { default_draft::<Self>(call, input) }
}
```

`Perm` is a type alias of K1's `access::Permission`. Until K1.2 lands it is today's
`policy::Permission` (`policy.rs:191`), so `PERMISSION: Option<Perm>` compiles from day one,
and K1.2 changes the alias. `ops::Target` (below) maps one to one onto `access::Target`
(`Account` has no access target: it is the caller's own).

**Context.**

```rust
// ops/context.rs
// The caller is K1's `access::Caller` (account, kind for mechanics only, `via`, `requested_by`,
// `run_id`, `reach`), built by the edge: `pub use crate::access::Caller`. Until K1.2 merges,
// this file holds a stopgap struct with the same fields and names (`via` carrying today's
// credential scopes and `AgentLimits`), replaced by the re-export in K1.2. The edge and the
// trace id are the operation's, not the caller's, so they live in `OpContext`.
pub use crate::access::Caller;

pub enum Edge {
    Web { session_id: Uuid },
    Mcp { credential_id: Option<Uuid>, oauth_grant_id: Option<Uuid>,
          connection_id: Option<Uuid>, client_name: String, protocol: McpRevision },
    PublicApi { credential_id: Uuid, version: ApiVersion },
    Automation { rule_id: Uuid, run_id: Uuid },
    Job { job_id: Uuid, source: &'static str },
    Proposal { proposal_id: Uuid, accepted_by: Uuid },
}

pub struct OpContext {                    // what `run` sees; built by dispatch
    pub caller: Caller,
    pub edge: Edge,
    pub trace_id: Option<String>,         // x-request-id (lib.rs:129)
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,           // today's project id
    pub target: Authorized<Target>,       // cannot be built outside ops::dispatch
    pub request_id: Option<RequestId>,    // Some for every write
    pub op: &'static str,
}

pub struct OpCall<'a> {
    pub state: &'a AppState,
    pub ctx: &'a OpContext,
    tx: TxSlot<'a>,                       // begun by dispatch for Dispatcher-idempotent writes
    effects: Effects,                     // events and audit facts collected for step 9
}
impl OpCall<'_> {
    pub fn tx(&mut self) -> &mut Transaction<'static, Postgres>;
    pub fn emit(&mut self, event: EventDraft);       // K3's events::emit; outbox until then
    // EventDraft is an owned `events::NewEvent` (K3 §3.3.2: type, thing, version, changes,
    // audience, payload); dispatch fills workspace and space and calls `events::emit` at step 9,
    // after `events::set_context` at the start of the transaction.
    pub fn audit_target(&mut self, kind: &'static str, id: Uuid, metadata: serde_json::Value);
}

// Adapters so wrapped functions need no change in the first step:
impl From<&OpContext> for CommandActor { .. }       // AuditAttribution from Edge
impl From<&OpContext> for memory::Writer { .. }
impl From<&OpContext> for MessageCommandActor { .. }
impl From<&OpContext> for whiteboards::Caller { .. }
```

`AuditAttribution` gains `Automation`, `PublicApi` and `Proposal` variants so the audit row
records the edge truthfully (today automations pose as MCP, `automations.rs:2383-2399`).

**Targets and references.** Each input names what it acts on; this replaces the
argument-name tables (E F6).

```rust
// ops/target.rs
pub enum Target {
    Account,                              // the caller's own things (notifications, Later)
    Workspace(Uuid),
    Space(Uuid),
    Thing { kind: ThingKind, id: Uuid, expected_version: Option<i64> },
    Things(Vec<(ThingKind, Uuid)>),       // bulk; all must share one workspace
}
pub trait Targeted { fn target(&self) -> Target; }

/// Typed references accept what people type, and resolve before access.
pub struct TaskRef(RefInput);   // "AWD-12", a uuid, or a link
pub struct ConversationRef(RefInput);
pub struct MessageRef(RefInput);
pub trait Resolve {
    fn resolve(&mut self, r: &mut Resolver<'_>) -> impl Future<Output = Result<(), OpError>> + Send;
}
```

Dispatch locates the target's workspace and space through K3's `things::locate(conn, id)`
(which returns kind, workspace, space, access thing, version and tombstone)
(until K3 lands, a per-kind lookup table in `ops/target.rs`, one SQL per kind; the
conversation lookup goes through `conversation_visible_to`). A thing the caller can't see is
`not_found`, the same as a missing one.

**Errors (one list).**

```rust
// ops/error.rs
pub enum OpError {
    Unauthenticated,
    NotFound { what: &'static str },
    Invalid { field: Cow<'static, str>, message: Cow<'static, str> },
    Denied { reason: DenyReason, message: Cow<'static, str> },
    Conflict { reason: ConflictReason, message: Cow<'static, str>, current: Option<serde_json::Value> },
    RateLimited { retry_after_seconds: u64 },
    LimitReached { limit: Cow<'static, str>, resets_at: Option<DateTime<Utc>> }, // H1's plan or spend limit
    WorkspaceSuspended,                   // H1: a suspended workspace refuses writes
    TooLarge { limit_bytes: usize },
    Unavailable,                          // logged with the op name; never leaks detail
}
pub enum DenyReason { Role { permission: Perm }, ReadOnlyCredential, ModuleOff { module: String },
                      Delegation, AccountPaused, HostRevoked }   // HostRevoked: A1
pub enum ConflictReason { Version { current_version: i64 }, State { code: &'static str },
                          Transition, RequestIdReused, Archived }
pub enum RetryCategory { Retry, Reread, FixInput, Reauthenticate, AskForAccess, Module, Wait, DoNotRetry }
impl From<AppError> for OpError { .. }    // the mapping in mcp/common.rs:118-147, made the only one
```

**Envelope (one shape, every edge).**

```json
{ "outcome": "ok", "value": { ... }, "requestId": "c-41", "replayed": false }
{ "outcome": "proposed", "value": { "proposalId": "…", "target": {"kind":"page","id":"…"} } }
{ "outcome": "started", "value": { "jobId": "…", "pollWith": "job_get" } }
{ "outcome": "conflict", "code": "version", "message": "Someone changed this first (now version 8). Read it again, then retry.",
  "retryCategory": "reread", "details": { "currentVersion": 8 } }
{ "outcome": "invalid_request", "field": "title", "message": "A title is one line of 1 to 240 characters.",
  "retryCategory": "fix_input" }
```

Error outcomes: `unauthenticated`, `not_found`, `invalid_request`, `permission_denied`,
`conflict`, `rate_limited`, `limit_reached`, `workspace_suspended`, `too_large`,
`temporarily_unavailable` (`limit_reached` and `workspace_suspended` were asked for by H1 and
A1). `outcome` stays the
discriminator (as in `ToolResult`, `mcp/common.rs:40-45`), field names are camelCase, `message`
is always present on errors and says what to do next. The envelope's JSON Schema is one
`oneOf` generated per op from `Envelope<O::Output>`, so a refusal from the dispatcher always
matches the declared output schema (fixing the workaround in `refusal`, `mcp.rs:3971`).

HTTP status from outcome, for both the web and public routes: ok 200, proposed 202, started
202, unauthenticated 401, permission_denied 403, not_found 404, conflict 409, too_large 413,
invalid_request 422, rate_limited 429 with `Retry-After`, limit_reached 402,
workspace_suspended 423, temporarily_unavailable 503.

**Lists (one paging rule).**

```rust
pub struct PageArgs { pub cursor: Option<String>, pub limit: Option<u32> }  // flattened into inputs
pub struct Page<T> { pub items: Vec<T>, pub next_cursor: Option<String>, pub total: Option<u64> }
```

Cursors are opaque, signed with the server's key and bound to the op name and the filter
hash (the pattern `external_event_poll` already uses for signed cursors, inferred from
`external_events.rs`). `limit` defaults to 50, max 200 unless an op says otherwise.

**Request ids and versions.**

- Every op with kind create, change or suggest has `IDEMPOTENCY != None`. The request id comes
  from the edge: the web's `Idempotency-Key` header (already sent by `api/client.ts:65`), the
  public API's `Idempotency-Key` header, MCP's `clientRequestId` argument (kept as the name
  agents know; stripped from the input before decode). If an MCP or API caller omits it, the
  dispatcher derives one from `(account, op, canonical input hash, 10-minute bucket)`, so a
  blind retry within ten minutes replays instead of doubling. The web always sends one.
- Scope: `op.<name>.<target id>` (fits `MAX_IDEMPOTENCY_SCOPE_BYTES` 120, `idempotency.rs:12`),
  one scope for every edge, so a retry through a different edge replays too. During the wrap
  phase an op may declare `const LEGACY_SCOPES: &[(EdgeKind, &str)]` that `reserve` also
  checks, for 48 hours after the switch (keys live 24 hours, `idempotency.rs:191`).
- The replay stores the canonical envelope. Aliases adapt it on the way out (3.6).
- Change ops on a versioned thing carry `expectedVersion` in the input and return it through
  `Target::Thing { expected_version }`; a mismatch is `Conflict { reason: Version }` with the
  current projection in `details.current` where the op provides one. Ops on things with other
  counters (`expectedRevision` for sheets, `expectedSceneSeq` for whiteboards) keep their
  counter but name it `expectedVersion` in canonical inputs, with the old name as a serde alias.

**Proposals (ties to K1 and C1, rulings R2 and R14).** For a `PROPOSABLE` op dispatch calls
K1's `access::write_mode_locked`, whose three answers are `Grant { mode: Direct }` (allow),
`Grant { mode: Propose }` (suggest only) and `Denied` (deny); for any other op it calls
`access::check_locked`. Suggest only on a `PROPOSABLE` op makes dispatch call `O::propose`,
whose `ProposalDraft` is the input to C1's `Proposable::prepare`, and then C1's
`proposals::create`, answering `proposed`. A non-proposable op never asks the three-way question, so it is allowed
or `Denied`. Any caller may also pass
`asProposal: true` to a proposable op to suggest even when allowed to edit. Deciding a
proposal is C1's op `proposal_decide` (ruling R26): it applies the prepared change through
C1's `Proposable::apply` inside a K2 dispatch with `Edge::Proposal { proposal_id, accepted_by }`,
`actor` = the proposer (so the edit carries their name, as the end state's test item 4 wants),
and access checked as: proposer may suggest, acceptor may edit; part of a proposal can be
accepted and a stale part refused. K2 has no `proposal_accept` and never re-runs a stored
input. A1's `request_answer` on a `proposal` request calls `proposal_decide` and nothing else.
`record_update_many` (K2.11) and `task_update` (K2.13) are `PROPOSABLE` (T1.41), besides
`page_update`. Today's page rule (`page_docs/suggestions.rs:42`) moves behind
K1's role setting; until then the access shim (`ops/access_shim.rs`, deleted by K2.17) returns
suggest only exactly where `must_suggest` would, so behaviour does not change in the wrap phase.

**Registry.**

```rust
// ops/registry.rs
pub struct OpMeta { name, kind, module, permission, proposable, idempotency, long, preview,
                    title, description, emits, input_schema: fn() -> Schema, output_schema: fn() -> Schema }
pub struct OpEntry {
    pub meta: OpMeta,
    pub call: for<'a> fn(&'a AppState, OpContext, serde_json::Value)
        -> BoxFuture<'a, Result<serde_json::Value, OpError>>,
    pub target_of: fn(&serde_json::Value) -> Result<Target, OpError>,
}
pub fn entry<O: Operation>() -> OpEntry { .. }       // the one generic eraser

pub struct Registry { by_name: BTreeMap<&'static str, OpEntry>, aliases: BTreeMap<&'static str, Alias> }
impl Registry {
    pub fn add<O: Operation>(&mut self) -> &mut Self;   // panics on a duplicate name (test-caught)
}
pub fn registry() -> &'static Registry {              // OnceLock, built once
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| { let mut r = Registry::default();
        crate::memory::register_ops(&mut r);           // one line per module, like jobs.rs:362
        crate::conversations::register_ops(&mut r);
        /* ... */ r })
}
```

Each module owns `src/<module>/ops.rs` with its op structs and a `pub fn register_ops(r: &mut
Registry)`. This is the hook M1's self-registering modules call.

### 3.4 Two real operations in the new form

**`decision_record`: an op that runs in the dispatcher's transaction.**

```rust
// apps/server/src/memory/ops.rs
pub struct DecisionRecord;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionRecordInput {
    /// The space. Not needed when you pass sourceMessageId or conversationId.
    #[serde(alias = "projectId")]
    pub space_id: Option<SpaceRef>,
    /// What was decided, in one line.
    pub title: String,
    /// Why, the options considered, and anything a newcomer should know.
    pub detail: Option<String>,
    /// The message where it was decided: its id, or a link to it.
    #[serde(alias = "fromMessageId")]
    pub source_message_id: Option<MessageRef>,
    /// The conversation or thread where it was decided.
    #[serde(alias = "conversationId")]
    pub source_conversation_id: Option<ConversationRef>,
    /// An earlier decision this one replaces.
    pub supersedes: Option<Uuid>,
    /// Also pin the decision to its conversation.
    #[serde(default)]
    pub pin: bool,
}
impl Targeted for DecisionRecordInput {
    fn target(&self) -> Target {
        // the space named, else the space of the message or conversation (resolved first)
        target::space_of_any(&self.space_id, &self.source_message_id, &self.source_conversation_id)
    }
}

impl Operation for DecisionRecord {
    const NAME: &'static str = "decision_record";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;   // new: retry-safe on every edge
    const TITLE: &'static str = "Record a decision";
    const DESCRIPTION: &'static str = "Record what was decided, why, and where, so nobody has to \
        reread the discussion. Pass the message or conversation it came from; supersedes replaces \
        an earlier decision.";
    const EMITS: &'static [&'static str] = &["decision.recorded"];
    type Input = DecisionRecordInput;
    type Output = DecisionView;                                  // memory.rs, now JsonSchema

    async fn run(call: &mut OpCall<'_>, input: Self::Input) -> Result<DecisionView, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::invalid("spaceId", "Pick a space."))?;
        let writer = memory::Writer::from(call.ctx);
        let domain = memory::DecisionInput {
            title: input.title, detail: input.detail,
            source_message_id: input.source_message_id.map(|r| r.id()),
            source_conversation_id: input.source_conversation_id.map(|r| r.id()),
            supersedes: input.supersedes,
        };
        let id = memory::record_decision(call.tx(), writer, space_id, &domain).await?;
        if input.pin { memory::pin_decision_to_source(call.tx(), writer, &domain, id).await?; }
        call.audit_target("decision", id, json!({}));
        call.emit(EventDraft::thing("decision.recorded", ThingKind::Decision, id));
        memory::get_decision_in(call.tx(), id, call.ctx.caller.account_id).await?
            .ok_or(OpError::Unavailable)
    }
}
```

What disappears: `record_decision_http` (`memory.rs:1482-1514`) becomes a 10-line wrapper,
then is deleted; `memory_tools::decision_record` (`mcp/memory_tools.rs:1670-1760`) and its
argument and value structs are deleted; the pin lookup moves into `memory`.

**`message_send`: an op whose command owns its transaction and idempotency.**

```rust
// apps/server/src/conversations/ops.rs
pub struct MessageSend;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSendInput {
    /// A channel, thread, DM or group: its id or a link.
    pub conversation_id: ConversationRef,
    /// The message as plain text. @handles of people and agents here become mentions.
    pub text: Option<String>,
    /// Structured content instead of text, for exact mentions and task links.
    pub content: Option<MessageContent>,
    /// Files uploaded first with file_upload to this conversation (up to 10).
    #[serde(default)]
    pub attachment_ids: Vec<FileRef>,
    /// Reply to this message in the same conversation.
    pub reply_to_message_id: Option<MessageRef>,
}
impl Targeted for MessageSendInput {
    fn target(&self) -> Target { Target::thing(ThingKind::Conversation, self.conversation_id.id()) }
}

impl Operation for MessageSend {
    const NAME: &'static str = "message_send";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";               // "channels" decided by target kind (M1)
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;  // append_message_command_with reserves
    const TITLE: &'static str = "Send a message";
    const DESCRIPTION: &'static str = "...";               // today's text, mcp.rs:816
    const EMITS: &'static [&'static str] = &["message.created"];
    type Input = MessageSendInput;
    type Output = SentMessage;                              // today's mcp/talk.rs value, moved

    async fn run(call: &mut OpCall<'_>, input: Self::Input) -> Result<SentMessage, OpError> {
        let content = conversations::content_for_send(call.state, call.ctx, input.text, input.content).await?;
        let key = call.ctx.request_id.clone().expect("writes have a request id").into();
        let outcome = conversations::append_message_command_with(
            call.state, MessageCommandActor::from(call.ctx), input.conversation_id.id(), None,
            key, content, &input.attachment_ids.ids(), None,
            AppendExtras { reply_to_message_id: input.reply_to_message_id.map(|m| m.id()), ..Default::default() },
        ).await?;
        call.mark_replayed(outcome.idempotent_replay);
        Ok(SentMessage::from(outcome))
    }
}
```

The web's three send routes (`conversations.rs:65-70`) become `message_send` with the
conversation in the body; `expected_kind` (`conversations.rs:1604`) is dropped because the
kind check was only a guard against the wrong URL. The web gains `via_grant_id` handling for
free (always `None` from a session). The MCP module-off check (`mcp/talk.rs:1580-1592`)
disappears into dispatch step 5.

### 3.5 Generation

One registry, five products. Each is generated by code in `ops/codegen/`, and every generated
file has a test that regenerates it in memory and fails on any difference. Running the test
with `OPS_BLESS=1` writes the files.

| Product | How | Where | Drift check |
|---|---|---|---|
| Web and API routes (ruling R4: one generated route for both) | `ops::web::router()` mounts `POST /api/v2/w/{workspace}/ops/{name}` for workspace ops, `POST /api/v2/direct/ops/{name}` for direct-scope ops (DMs and groups, ruling R20) and `POST /api/v2/ops/{name}` for account ops (`Target::Account`). `{workspace}` is an id or a slug (slugs through K4.5's `workspace_slugs`; ids only before it). A session cookie plus CSRF makes `Edge::Web`; a bearer makes `Edge::PublicApi`; a request carrying both is refused. Body limit per op (default 1 MiB, `file_upload` 6 MiB) | runtime, merged in `lib.rs` next to `api::router()` | `policy_inventory` scan sees one generic route; inventory entries for ops come from the registry |
| MCP tools | `ops::mcp` turns each `OpEntry` into an `rmcp::model::Tool` (name, title, description, input schema, output schema, annotations from kind: `read_only_hint` for describe/query, `idempotent_hint` for writes with request ids, `destructive_hint` for ops flagged destructive) and serves `call_tool` through `dispatch` | runtime | names unique across registry, aliases and legacy router (test) |
| OpenAPI | OpenAPI 3.1 document for the route above at `/api/v2/openapi.json`; `/api/public/v1/{tool}` stays frozen (3.8) | runtime + `docs/api/v2.openapi.json` | regen test |
| Web types and client | TypeScript from the input and output JSON Schemas | `apps/web/src/api/generated/operations.ts` (types only), `apps/web/src/api/generated/client.ts` (one generic call), `apps/web/src/api/generated/catalog.json` (name, kind, module, permission, proposable, emits) | regen test; web `tsc -b` catches consumers |
| Automation actions | every create/change op with `automation: true` (default for writes whose target is a space or thing) is offered as `RunOperation { operation, input }` | `ops::automation`, read by `automations.rs` | catalogue regen test |
| Audit entries | dispatch writes one row per applied write: `action = op name`, `target_kind`, `target_id`, `outcome`, `request_id`, edge columns from `Caller` | `audit_events` today; K3 derives the trail from events later | n/a |

**TypeScript generator.** Written in Rust (`ops/codegen/ts.rs`, about 400 lines) over the
JSON Schema subset schemars emits here: objects, `required`, `$ref`/`$defs`, string enums,
`oneOf` with a `const` tag (serde internally tagged enums), arrays, nullable unions,
`additionalProperties` maps and primitives. Anything outside the subset fails the test with the
op's name, so a new shape is noticed, not mistranslated. Alternatives considered: `ts-rs` or
`specta` need a second derive on every type (the schemas already exist); generating from
OpenAPI with an npm tool adds a dependency and a second build step on the web side, and the
check would then run only in web CI, while drift starts on the server.

Generated client, kept tiny for the initial-load budget (types are erased; the runtime is one
function and a name set):

```ts
// apps/web/src/api/generated/client.ts  (generated, do not edit)
import { requestJson } from '../client'
import type { Operations, OpName, Envelope } from './operations'

export async function callOp<N extends OpName>(
  name: N,
  input: Operations[N]['input'],
  options: { workspaceId?: string; requestId?: string; signal?: AbortSignal } = {},
): Promise<Operations[N]['output']> {
  const path = options.workspaceId
    ? `/api/v2/w/${encodeURIComponent(options.workspaceId)}/ops/${name}`
    : `/api/v2/ops/${name}`   // direct-scope ops use /api/v2/direct/ops/ (the catalogue says which)
  const envelope = await requestJson<Envelope<Operations[N]['output']>>(path, {
    method: 'POST', json: input, idempotencyKey: options.requestId, signal: options.signal,
  })
  return unwrap(name, envelope)   // ok -> value; proposed/started -> typed marker; error -> ApiError
}
```

`requestJson` learns the envelope: a non-ok `outcome` becomes `ApiError` with `code` =
outcome, `details` = the envelope's details (`api/errors.ts:7-33` is unchanged in shape).

**CI.** `tools/ci/changed-areas.sh:20-35` classifies `apps/web/*` as web-only. Add
`apps/web/src/api/generated/*` to the rust group too, so a hand edit to a generated file runs
the Rust drift test. The other direction (a server change that alters a schema) already runs
Rust, where the drift test fails, and the regenerated TS then runs web typecheck because the
PR touches `apps/web`.

### 3.6 Conventions

**Names.** `<thing>_<verb>`, lower snake case, singular thing. `thing` is a K3 thing kind or
a module's own noun (`task`, `page`, `sheet`, `record`, `collection`, `whiteboard`,
`conversation`, `message`, `thread`, `channel`, `decision`, `file`, `label`, `field`, `view`,
`milestone`, `template`, `run`, `approval`, `request`, `space`, `project`, `workspace`, `role`,
`member`, `invitation`, `setting`, `module`, `notification`, `later`, `search`, `briefing`,
`job`, `proposal`, …). Verbs by kind:

| Kind | Meaning | Verbs allowed |
|---|---|---|
| describe | one thing, with what it links to | `get` (alias `open` kept where it exists) |
| query | many things, paged | `list`, `search`, `history` |
| create | a new thing | `create`, `create_many`, `send`, `upload`, `import`, `record`, `start`, `schedule`, `duplicate`, `forward` |
| change | an existing thing changes | `update`, `update_many`, `archive`, `restore`, `delete`, `move`, `reorder`, `claim`, `release`, `link`, `unlink`, `react`, `pin`, `mark_read`, `decide`, `respond`, `assign`, `set`, `cancel`, `retry` |
| suggest | produce something for a person to accept; never changes the thing itself | `suggest`, `draft`, `summarize`, `analyze` |

The closed verb list is data in `ops/conventions.rs`; adding a verb is a reviewed one-line
change. "suggest" as a kind (a draft or analysis) is separate from `PROPOSABLE` (a change that
may be held as a proposal).

**Writes.** Every create, change and suggest op: request id (3.3), the envelope, an audit row,
at least one declared event. Every change op on a versioned thing: `expectedVersion` required
(updates) or optional (claims, reactions, marks, where "last write wins" is the right rule and
the op says so in its description).

**Bulk.** `<thing>_create_many` and `<thing>_update_many`: `items` (create) or `ids` plus one
change (update), all or nothing in one transaction, errors carry `details.index`, cap 100 by
default. A generic `Many<O>` wrapper derives a bulk op from a single op that implements
`Batchable` (runs each item's `run` in the same transaction); ops with cross-item references
(`task_create_many`'s subtasks) keep their own implementation.

**Previews.** Ops with `PREVIEW = true` take `preview: true`, run normally, return the
envelope with `value` and `"preview": true`, and roll back. This is today's task batch
behaviour (`work_item_commands.rs:711-713`), made general. Where an MCP client supports
multi round-trip `input_required` (2026-07-28), a destructive op asks for confirmation that
way; preview stays for older clients and for the web.

**Rename plan for today's tools.** Canonical names are introduced with the module's slice.
The old name becomes an **alias**: callable on MCP and public API v1, **not listed** in
`tools/list` for clients on 2026-07-28 (so the list shrinks instead of doubling), and listed
alongside for clients on 2025-06-18 and 2025-11-25 only if the client's previous list had it
(simplest: list aliases for old revisions until the window closes). An alias keeps its old
argument names and its old result shape byte for byte, through an adapter function per alias
(`ops/registry.rs::Alias { canonical, adapt_in, adapt_out }`).

| Today | Canonical | Note |
|---|---|---|
| `projects_list` | `project_list` | plural fixed |
| `project_open`, `task_open`, `conversation_open`, `thing_open` | `project_get`, `task_get`, `conversation_get`, `thing_get` | `open` stays an alias |
| `page_write` | `page_create`, `page_update` | the alias dispatches on `pageId` present |
| `page_edit` | `page_update_blocks` | block edits are a change of the page |
| `sheet_write` | `sheet_create`, `sheet_update` | cells and tabs in `sheet_update` |
| `record_write` | `record_create_many`, `record_update_many` | collection forms move to `record_create_many` |
| `collection_write`, `view_write`, `label_write`, `field_write`, `milestone_write`, `template_write`, `recurring_write`, `automation_write`, `webhook_write`, `environment_write`, `agent_profile_write`, `invitation_write`, `layout_preset_write`, `project_template_write`, `bug_report_link_write`, `repository_write`, `role_write` | `<thing>_create` + `<thing>_update` (+ `_archive`/`_delete` where the `_write` did that) | one alias each |
| `whiteboard_edit` / `whiteboard_update` | `whiteboard_update_elements` / `whiteboard_update` | |
| `message_edit`, `task_comment_edit`, `page_comment_update` | `message_update`, `task_comment_update`, `page_comment_update` | comment ops become C1's message ops later |
| `task_mark_blocked`, `task_mark_ready_for_review` | `task_update` with `blocked` / `status` | aliases keep today's arguments |
| `task_criteria`, `task_depend`, `task_relate`, `task_watch` | `task_criteria_update`, `task_link` with a link kind (K3), `task_watch_set` | |
| `task_link_pull_request` | `task_link` | already an alias in practice |
| `mark_read` | `conversation_mark_read` | |
| `my_work` | `task_list` with `mine: true` | |
| `briefing` | `briefing_get` | |
| `settings_set`, `pin_set`, `project_member_set`, `project_layout_set` | `setting_update`, `pin_set` (kept), `member_update`, `layout_update` | |
| `ai_summarize`, `ai_analyze` | `conversation_summarize`, `thing_analyze` | kind suggest |
| `run_start`, `run_request`, `task_launch`, `launch_*` | A1 decides the run ops; K2 supplies aliases | |
| `external_event_poll`, `inbox_list` | A2's delivery ops; aliases kept | |

**Alias window.** Aliases are served for at least **180 days** after their canonical op ships
and are removed only when, additionally, no call has used the alias for 30 days (counted in
`operation_alias_usage`, 3.8). Removal is one slice per batch, with the owner's go-ahead
(open question 1). Calls through an alias add `"_meta": {"deprecated": "use task_update"}`
on MCP and `Deprecation`/`Sunset` headers on the public API.

**Operations other plans add** (the content review's request, accepted). Each is defined by its
owner's slice in its owner's module (`src/<module>/ops.rs`) and gets its agent tool from the
registry like any other; K2 only checks names and conventions:

| Owner | Operations |
|---|---|
| C1 | `page_move`, `file_source_browse`, `file_attach`, `file_keep_copy`, `space_upload_target_set`, `conversation_tab_set`, `proposal_decide` |
| C2 | `dm_open`, `group_open`, `account_block`, `account_unblock`, `dm_request_answer`, `account_setting_set` |
| K4 | `invite_link_create`, `invite_link_revoke`, `invite_link_list` (ruling R30: invitation links are K4's) |
| M1 | `workspace_shape_set` |
| K5 | `file_upload_start`, `file_upload_finish`, `workspace_store_set` |

`dm_open` and `group_open` are direct-scope operations (`/api/v2/direct/ops/...`). The verbs
`browse`, `attach`, `keep_copy`, `open`, `block`, `unblock` and `answer` join the closed verb
list in `ops/conventions.rs` in the slice that first needs each.

### 3.7 The MCP surface

**What rmcp 3.9 gives and what must be written here.** Read in this tree: the server already
answers 2026-07-28 clients next to older ones (the comment at `mcp.rs:3734-3737` notes those
clients need list cache hints, which are set with `with_ttl_ms` and `CacheScope`),
`StreamableHttpServerConfig::with_legacy_session_mode(true)` (`mcp.rs:4211`) keeps sessions for
older revisions, and `CallToolResponse` already has a non-`Complete` variant
(`mcp.rs:4297-4302`, "tool asked for input", i.e. multi round-trip). Public notes say the Rust
SDK implements 2026-07-28 and the Tasks extension (SEP-2663) server runtime; I could not read
the 3.9 source here, so slice K2.1 confirms each with a test before anything depends on it.

| Need | rmcp provides (to confirm in K2.1) | Written here |
|---|---|---|
| 2026-07-28 stateless + `server/discover` | yes, with legacy session mode for older revisions | `ResumeAnySession` stays for older revisions; set `MCP_PROTOCOL_VERSION` (`mcp.rs:125`) from rmcp's latest and stop resuming sessions as 2025-06-18 only (`mcp.rs:4157`): resume with the revision the client sends in `mcp-protocol-version` |
| Older revisions 2025-06-18 and 2025-11-25 | yes | matrix test |
| Dynamic tool list | `ServerHandler::list_tools`/`call_tool` are already overridden (`mcp.rs:3766-3809`) | merge registry tools with the legacy `#[tool_router]` during migration; filtered by K1 reach and M1 modules from `OpMeta`, not from `tool_permissions.rs` |
| Tasks extension | server runtime with an in-memory store (inferred) | a `TaskStore` over Postgres (`operation_tasks` table + `jobs`), so any process answers `tasks/get`, `tasks/update`, `tasks/cancel`, with the caller's access checked on each |
| Resources | trait methods | `list_resources`, `read_resource`, templates from describe ops flagged `resource: "pages/{id}"` |
| Skills over MCP | not inferred | resources under the extension's skill URI scheme for each space's skills (`skill_list`/`skill_get` data, `memory.rs` skill pages) |
| Server card | no | `GET /.well-known/mcp/server-card.json` generated from the registry and `SERVER_INSTRUCTIONS` |
| Client ID Metadata Documents, client credentials | partly (`mcp/oauth.rs` is ours) | K1 owns credentials; K2 wires the MCP auth metadata |

**Long operations.** Ops with `LONG = true` (`project_export`, `task_import`,
`usage_batch_launch`, `whiteboard_render` for big boards, and any `*_many` above a threshold)
enqueue a job (`jobs::enqueue`, `jobs.rs:138`) and return `started`. A 2026-07-28 client that
negotiated Tasks receives an MCP task handle whose id is the job id; others get
`{outcome:"started", value:{jobId}}` and poll the `job_get` op. `operation_tasks` keeps
`(job_id, workspace_id, account_id, op, created_at, result envelope)` so the answer survives a
restart and is readable only by its caller.

**Descriptions.** Vendor-free and kind-free: no provider, model, harness or code host names
(the `Codex` wording at `mcp.rs:3113-3116`, `:3152`, `:3260`, `:3577`, `:2555`, `:3608`,
`:3629`, `:687`, `:1331`, `:3696` and `mcp/runs.rs:23,257-288` is rewritten in K2.1), and no
"people only" or "agents can't": permission text is generated from `PERMISSION` ("Needs
pages.edit in the space") and appended by the bridge.

**Which workspace a call acts in** (rulings R4, R19). MCP keeps one endpoint, `/mcp`, plus the
alias `/w/<slug>/mcp`. Every tool takes an optional `workspace` argument (id or slug). A
credential that reaches one workspace needs none. A credential that reaches more than one
(any account kind: a person's app, an agent in several workspaces, a host) must name it, by
the argument or the alias path; a call that names none is `invalid_request` with
`field: "workspace"` listing the workspaces it reaches; the alias path and the argument
disagreeing is refused. A target that lies in another workspace than the one named is
`not_found`. Direct-scope tools (DMs, groups) need no workspace; their targets are in the
direct scope and the caller's participation decides (K1 3.10). Tool lists are the union over
the reached workspaces, filtered by module and permission per workspace.

**Server instructions.** `SERVER_INSTRUCTIONS` (`mcp.rs:135`) stays hand-written, with a test
that every tool name it mentions exists in the registry or the alias table, and its
idempotency sentence becomes true (every write).

### 3.8 The public API

- **Versioning.** v1 (`/api/public/v1/{tool}`, `public_api.rs:40-48`) keeps its paths and is
  frozen: today's tool names and result shapes, served through the alias adapters, so scripts
  keep working. v2 is the one generated route of 3.5 (`/api/v2/w/{workspace}/ops/{name}`,
  `/api/v2/direct/ops/{name}`, `/api/v2/ops/{name}`), shared with the web (ruling R4):
  canonical names, the envelope, status from outcome. v1 gets `Deprecation` when v2 ships and is retired no sooner than 12 months
  later (the same window MCP uses for its revisions). Within v2, additive changes only;
  removing or renaming an op or a field needs v3.
- **Workspace in the path.** The path's workspace (id or slug) must equal the target's
  workspace or the call is `not_found`; a bearer whose credential does not reach that
  workspace gets `not_found` too (ruling R19). Dispatch runs every operation inside H1's
  `db::scoped_tx` (H1 3.4, ruling R42), whose first statement is
  `set_config('app.tenancy', 'w:<account>|<workspace id>', true)` (transaction-local), the one
  key H1's row rules read; there is no `app.workspace_id`. Direct-scope operations scope to
  the direct scope's id the same way; account operations use H1's member scope. The web routes use the same rule (K4's "workspace in every
  URL", server side).
- **Auth.** Bearer only, through K1's credential resolver (today `resolve_bearer_principal`,
  `mcp.rs:4755`): personal API tokens, OAuth tokens of apps acting as a person, agent
  tokens, run-scoped tokens. No cookies, no CSRF.
- **Rate limits.** Step 6 calls H1's `metering::admit` once per write (ruling R13), which uses
  K5's shared limiter (`Limits::check_cost`), keyed by credential, account and workspace, with
  a cost per op from its kind (read 1, write 2, bulk 1 per item, long 10). Reads call
  `check_cost` directly. Before H1.25 lands, dispatch calls K5.6's `check_cost` itself. 429
  with `Retry-After`.
- **Bodies.** Per-op limit from `OpMeta`, so `file_upload` accepts the 4 MiB file it accepts
  on MCP (fixing the 1 MiB cap at `public_api.rs:43`).

### 3.9 Tables

```sql
-- apps/server/migrations/<ts>_operation_alias_usage.sql
CREATE TABLE operation_alias_usage (
    workspace_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    alias        text NOT NULL CHECK (char_length(alias) BETWEEN 1 AND 80),
    day          date NOT NULL,
    calls        bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, alias, day)
);
-- written with INSERT ... ON CONFLICT DO UPDATE SET calls = calls + 1; pruned after 400 days by retention.

-- apps/server/migrations/<ts>_operation_tasks.sql
CREATE TABLE operation_tasks (
    job_id       uuid PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id   uuid NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    op           text NOT NULL,
    result       jsonb,                       -- the final envelope, at most 1 MiB
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK (result IS NULL OR pg_column_size(result) <= 1048576)
);
CREATE INDEX operation_tasks_account ON operation_tasks (account_id, created_at DESC);
```

`workspace_id` references today's `organizations` table until K4's rename; the column name is
already the target one. No backfill. The jobs table name is assumed (`jobs.rs`).

### 3.10 Automations

`automations::Action` (`automations.rs:332-369`) gains:

```rust
/// Run any operation the rule's maker could run, as the maker, with {{variables}} filled in.
RunOperation { operation: String, input: serde_json::Value },
```

Dispatched with `Edge::Automation { rule_id, run_id }`, `requested_by` = the trigger's
account where there is one, request id `automation-<run>-<index>` (the pattern at
`automations.rs:2456`). `SetField` and `PostMessage` become thin translations onto
`task_update` and `message_send` through `dispatch`, not `call_tool_direct`. The builder
lists actions from `catalog.json` (writes with `automation: true` in modules that are on),
with an input form drawn from the op's schema (W1's form piece). Triggers stay K3 events; each
op's `EMITS` tells the builder which triggers it can cause.

## 4. Migration path

Rules: nothing is renamed or removed in the same slice that introduces its replacement; old
and new share one code path from the first slice; the live database needs only the two small
additive tables above.

1. **Kernel beside the old code (K2.2, K2.3).** `ops/` exists, with the dispatcher calling an
   access shim built on today's `policy::authorize` and `actor_can_access_project`, today's
   module state and today's rate limits (the shim is `ops/access_shim.rs`, deleted by K2.17
   once K1.2, K1.10 and K1.12 have merged). The MCP handler serves registry ops and the legacy
   router side by side (registry wins on a name clash, which a test forbids anyway).
   `call_tool_direct` first looks in the registry and only then builds its fake request.
2. **Wrap, per module (K2.5 onward).** For each operation: define the op around the existing
   domain function; point the old MCP tool name at it (as the canonical name when the name
   already fits, as an alias otherwise); point the old web route's handler at
   `ops::dispatch` (the handler shrinks to "decode path params into the op input, call,
   render the old response"); switch the module's web `api.ts` functions to `callOp` keeping
   their exported names, so components don't change. Old idempotency scopes stay as
   `LEGACY_SCOPES`. Behaviour must not change except where two edges differed; each such
   difference is listed in the slice and decided there (usually: take the stricter rule and
   the richer side effects).
3. **Contract, per batch (K2.22).** At least one release after a module's wrap slice: delete
   the old web routes (the web no longer calls them; a stale tab reloads on the 404 through
   the existing auth-expired path, inferred), delete the module's `mcp/<area>.rs` argument
   and result structs and tool methods, its lines in `tool_permissions.rs`, `PRIMARY_SCOPES`,
   `keys_to_ids` and `public_api::tag_for`; drop `LEGACY_SCOPES` (48 hours have passed).
4. **Finish (K2.22).** When the legacy router is empty: delete `#[tool_router]`,
   `call_tool_direct`, `keys_to_ids`, `scoped_project` and the scope tables, `Scoped<T>`
   (`mcp.rs:39-47`), `WriteResult`, `ReadResult`, `BatchResult`; the manifests' `tools` maps
   are generated from the registry (or read from it, if M1 has made manifests code).
5. **Aliases sunset (K2.23)** after the window in 3.6.

**Which module first.** Decisions (`decision_record`, `decision_list`) as the template: one
shared domain function already used by both edges (`memory.rs:978`), small, untouched by other
workstreams, and it shows the retry-safety gain. Then messages (the cleanest command,
`append_message_command_with`), then the generation-3 modules (sheets, collections,
whiteboards), then pages (proposals arrive here with K1 and C1), then the rest. Tasks wait
for T1's tasks-as-records slices to settle (PR #251 and its follow-ups), then go in three
slices. Runs, approvals and launch are defined in the registry directly by A1's new run
service rather than wrapped; K2 adds only their aliases.

**How `agent_parity.rs` evolves.**
- K2.3: a new variant `Op(&'static str)` means "this route is a thin wrapper over the
  registry op of that name"; the test checks the op exists. Generated routes don't appear in
  the inventory scan at all (one generic `/ops/{name}` route), so parity for them is true by
  construction.
- Per module slice: entries move from `Tool(...)` to `Op(...)`; `Person(...)` entries become
  ops with K1 permissions (with K1's slices), so `Person` disappears.
- K2.22: `agent_parity.rs` is replaced by `ops/conventions.rs::no_hand_written_edges`: every
  mutating route found by the scan is either the generated op route or listed in
  `HAND_WRITTEN_EDGES` with a `Boundary` reason (`SessionAuthentication`, `OAuthProtocol`,
  `McpProtocol`, `GithubWebhook`, `ClientTelemetry`, `NotificationActionToken`,
  `BugReportLinkToken`, `AgentCredentialExchange`, `DocumentSync` for Yjs streams and
  `FileTransfer` for uploads and downloads that stream bytes). A `Legacy(op)` reason is
  allowed only while the named op exists, and only until its contract slice.
- `policy_inventory`'s hand-kept `Action` labels for op routes are replaced by the registry's
  `PERMISSION`, which is the value dispatch actually checks (fixing E F7).

## 5. Slices

Sizes: S about a day, M a few days, L about a week. "Beside" = can run in parallel.

### K2.1 Protocol matrix and vendor-free descriptions (S)
Write the test that pins what the MCP server does today for each revision, and fix the words
every agent reads first.
- Changes: `tests/integration/mcp_revisions.rs` (new): for 2025-06-18, 2025-11-25 and
  2026-07-28, run initialize or `server/discover`, `tools/list` (cache hints, filtered), one
  read `tools/call`, one write `tools/call`, a resumed session id, and record whether rmcp
  3.9 offers the Tasks capability. `mcp.rs` descriptions and `mcp/runs.rs` messages: replace
  "Codex" with "run"/"the agent" (§3.7 list), standup prompt (`mcp.rs:3696`). New unit test
  `descriptions_name_no_vendor_or_kind_rule` over `tool_catalog()`: a deny list (model and
  harness vendor names; "people only"; "agents can't"; "never agents") with an allowlist for
  the `github_*`/`pull_request_*` tools until A1's code-host seam.
- The vendor-name rewrite is P0.32's; here only what goes beyond it (the revision matrix and
  the description test's kind-rule words).
- Depends on: P0.32 (soft: if P0.32 has not merged, the rewrite is done here and P0.32 is done).
- Acceptance: the matrix test passes against the current server; the description test fails
  on reintroducing "Codex" in a description.
- Beside: everything.

### K2.2 The ops kernel (M)
- Changes: `apps/server/src/ops/{mod,context,target,error,envelope,registry,dispatch}.rs`;
  `schemars = "1.2"` as a direct dependency (same version rmcp uses, `Cargo.lock:2676`);
  `From<&OpContext>` adapters for `CommandActor`, `memory::Writer`, `MessageCommandActor`,
  `whiteboards::Caller`; `AuditAttribution::{Automation, PublicApi, Proposal}`
  (`work_item_commands.rs:47`) and their `record_audit_for` arms; the access shim
  (`ops/access_shim.rs`) over `policy::authorize` with the page `must_suggest` mapped to
  suggest only (`Grant { mode: Propose }`); `From<AppError> for OpError`.
- Depends on: K2.1 not required.
- Acceptance (unit tests in `ops/dispatch.rs`, a test-only `EchoCreate` op over a scratch
  table in `tests/integration/ops_kernel.rs` with real PostgreSQL): a write with a request id
  replays the same envelope on retry and `replayed: true`; the same id with a different input
  is `conflict`/`request_id_reused`; a caller outside the workspace gets `not_found`; a
  read-only credential gets `permission_denied`/`read_only_credential`; an `AppError` maps as
  the table in 3.3 says; `Envelope` schema for every op is a `oneOf` that includes every error
  outcome.
- Beside: K2.1.

### K2.3 Generated edges: web, MCP, the v2 route, automations (M)
- Changes: `ops/web.rs` (routes in 3.5, merged in `lib.rs:156` next to `api::router()`,
  next to related lines); `ops/mcp.rs` (list and call merge registry ops with
  `McpServer::tools()`, in `mcp.rs:3766-3816`; alias lookup; `refusal` uses the envelope);
  `public_api.rs`: the v2 OpenAPI document for the one generated route (ruling R4), v1 frozen
  on its old paths and `Deprecation` header off until the first alias exists; the MCP
  `workspace` argument on every tool and the `/w/<slug>/mcp` alias, refused when they
  disagree (rulings R4, R19); `mcp::call_tool_direct` looks in the registry first (`mcp.rs:4250`);
  `automations.rs`: `Action::RunOperation` and dispatch-based `SetField`/`PostMessage`;
  `policy_inventory.rs`: the generic op route as one `boundary_route!` with a new
  `MutationAction::Operation`; `agent_parity.rs`: `Op(..)` variant.
  `migrations/<ts>_operation_alias_usage.sql`.
- Depends on: K2.2; H1.20 (soft: `db::scoped_tx`; until it lands dispatch begins a plain transaction and runs the same `set_config` first).
- Acceptance: `tests/integration/ops_edges.rs`: the test op called through
  `/api/v2/w/{workspace}/ops/{name}` with a session (+ CSRF) and with a bearer, MCP and an
  automation rule returns the same envelope and writes one audit
  row each with the right edge columns; a workspace mismatch between path and target is 404;
  the MCP tool list for a viewer omits the write op; a credential reaching two workspaces is
  refused a call that names neither (`invalid_request`, field `workspace`) and a call whose
  alias path and argument disagree; a request with both a cookie and a bearer is refused;
  `workspace_isolation` and
  `write_isolation` gain cases for the test op through each edge.
- Beside: K2.4.

### K2.4 Code generation and drift checks (M)
- Changes: `ops/codegen/{ts,openapi,catalog}.rs`; generated
  `apps/web/src/api/generated/{operations.ts,client.ts,catalog.json}`,
  `docs/api/v2.openapi.json`; `apps/web/src/api/client.ts`: envelope-aware error;
  `tools/ci/changed-areas.sh`: generated paths count as rust too.
- Depends on: K2.2 (K2.3 for the route shapes in `client.ts`).
- Acceptance: `ops::codegen::tests::generated_files_are_current` fails after changing a field
  in the test op until `OPS_BLESS=1` is run; a schema outside the subset fails with the op's
  name; `apps/web/src/api/generated/client.test.ts` (stubbed `fetch`) checks path, method,
  idempotency header and that a `conflict` envelope throws `ApiError` with `code: 'conflict'`
  and `details.currentVersion`; initial-load budget unchanged (types are `import type`).
- Beside: K2.3.

### K2.5 Template slice: decisions (S)
The worked example every later slice copies.
- Changes: `memory/ops.rs` with `DecisionRecord` (3.4) and `DecisionList` (query, `Page`);
  `memory.rs:1482` `record_decision_http` and the decisions list handler become wrappers over
  dispatch; `mcp/memory_tools.rs:1565-1760` removed from the router in favour of registry ops
  of the same names (old `fromMessageId`/`conversationId` accepted as serde aliases, so no
  alias entry is needed); web `memory/api.ts` decision calls use `callOp`;
  `.claude/skills/add-operation/SKILL.md` (new, the steps of this slice), with
  `add-mcp-tool/SKILL.md` pointing to it for anything new; an `AGENTS.md` line for P0 to merge
  ("a new write is an operation in `src/<module>/ops.rs`; never a hand-written route or tool").
- Depends on: K2.3, K2.4.
- Acceptance: `tests/integration/ops_decisions.rs`: recording twice with one request id makes
  one decision (web header and MCP `clientRequestId`); a decision from a private conversation
  the caller can't see is `not_found`; the web response body for `POST
  /api/v1/projects/{id}/decisions` is unchanged (snapshot of fields); the isolation cases for
  `decision_record` still pass.
- Beside: K2.6.

### K2.6 Conventions guard and aliases (S)
- Changes: `ops/conventions.rs` tests (§6); `ops/registry.rs` `Alias` with `adapt_in`,
  `adapt_out`, usage counting into `operation_alias_usage`, the `_meta.deprecated` hint and v1
  headers; the rename table of 3.6 as data (`ops/aliases.rs`), each row inert until its op
  exists.
- Depends on: K2.3.
- Acceptance: unit tests: an op named `task_frobnicate` fails the verb rule; a create op with
  `IDEMPOTENCY = None` fails; an alias call returns the old shape byte for byte (golden JSON
  for `page_write` create, using K2.10's op once it lands; a test alias until then) and counts
  one call.
- Beside: K2.5.

### K2.7 Messages (M)
- Changes: `conversations/ops.rs`: `message_send` (3.4), `message_update` (alias
  `message_edit`), `message_delete`, `message_react`, `message_forward`,
  `conversation_mark_read` (alias `mark_read`) built on one extracted
  `conversations::mark_seen(tx, actor, conversation, sequence)` that also calls
  `notifications::read_for_conversation` (fixing B F5, `mcp/talk.rs:1395-1437` vs
  `conversations.rs:2756-2795`), `message_history`, `message_schedule`,
  `scheduled_message_update`, `message_preview_hide`. Web routes in `conversations.rs`,
  `message_edits.rs`, `reactions.rs`, `forwarding.rs`, `scheduled_messages.rs` become
  wrappers; web `conversations/api.ts`, `threads/*` calls use `callOp`.
- The mark-seen fix (B F5) is done by P0.5; here only what goes beyond it: the op over P0.5's
  shared function.
- Depends on: K2.5, K2.6, P0.5.
- Acceptance: `tests/integration/ops_messages.rs`: reading through an app still marks the
  conversation's mention notifications read (P0.5's case, kept); an agent's `message_send` without
  `clientRequestId` retried within ten minutes sends once; existing `message_edits.rs`,
  `message_reactions.rs`, `m2_conversations_http.rs` pass unchanged.
- Beside: K2.9, K2.11, K2.12, K2.14.

### K2.8 Threads and conversations (M)
- Changes: `thread_start`, `thread_update`, `conversation_start`, `conversation_update`,
  `conversation_continue`, `channel_create`, `pin_set`, `conversation_members_update`, all over
  C1.2's `conversations::create_thread(tx, actor, parent, origin, name)` (B F4 / E F3; ruling
  R9: C1.2 owns it). DMs and groups (`conversation_start` with participants) are direct-scope
  ops on `/api/v2/direct/ops/` (ruling R20; K4.24 owns the rule: one DM per pair). `work_items.rs:1556`, `launch.rs:1809,2965` and
  `mcp/replies.rs:1028` call it too.
- Depends on: K2.7, C1.2; K4.24 (soft: for the direct-scope routing of DMs).
- Acceptance: `tests/integration/ops_threads.rs`: a thread started by an agent and one started
  by a person from the same message behave alike (auto-named, renamed by first message,
  audited); thread creation from a message in a private channel the caller can't see is 404.
- Beside: K2.10, K2.11.

### K2.9 Reply requests (M)
- Changes: move claim, progress, pass, release and stop transitions from `mcp/replies.rs:461,
  727,836` into typed functions in `replies.rs` (E F8); ops `reply_claim`, `reply_send`,
  `reply_progress`, `reply_pass`, `reply_release`, `reply_stop`, `reply_retry`, and web
  routes for claim and release so people can see and use them (E's owner question 1;
  defaults to yes).
- Depends on: K2.7. Coordinate with A2 (delivery may rename these; aliases cover it).
- Acceptance: existing `message_replies.rs` passes; a new test claims through the web route
  and releases through MCP with one generation fence.
- Beside: K2.8, K2.10.

### K2.10 Pages, skills and page discussion (M)
- Changes: `memory/ops.rs`: `page_create`, `page_update`, `page_update_blocks` (alias
  `page_edit`), `page_get`, `page_list`, `skill_get`, `skill_list`; `page_docs/ops.rs`:
  comment and suggestion ops; `page_write` alias dispatching on `pageId`. `page_update` is
  `PROPOSABLE`; the agent-kind rule moves to the access shim (unchanged behaviour) until K1's
  role setting replaces it. Page create and update gain idempotency.
- Depends on: K2.6; C1.7 (the `proposals` table, for `outcome: proposed`; until then the shim
  calls today's `suggest_text`, `mcp/memory_tools.rs:621`, and answers `proposed` with the
  suggestion id); K1.12 (soft: `write_mode`; the shim until then).
- Acceptance: `awd53_*` tests pass; a person whose role is set to suggest-only on pages (once
  K1 lands; a shim flag before) gets `proposed` from the web route too; a retried
  `page_create` makes one page.
- Beside: K2.8, K2.9, K2.11.

### K2.11 Sheets and collections (M)
- Changes: `sheets/ops.rs`: `sheet_create`, `sheet_update` (cells, tabs, rows), `sheet_get`,
  `sheet_list`; `collections/ops.rs`: `collection_*`, `record_create_many`,
  `record_update_many` (`PROPOSABLE`, T1.41), `record_list` (alias `record_query`); collection forms and bug
  report links call `dispatch` (`bug_reports.rs:968,1108`). One audit rule for sheets:
  audit when the revision moved (web's rule; MCP audited always, `mcp/sheet_tools.rs:1221`).
  Remove the tools' own module checks (`mcp/sheet_tools.rs:64`, `mcp/collection_tools.rs`).
- Depends on: K2.6.
- Acceptance: `awd54_sheets.rs`, `awd90_collections.rs`, `awd110_*` pass; a cell edit through
  the web and through MCP each write one audit row and one event; `bug_report_links.rs`
  passes with no `call_tool_direct` left in `bug_reports.rs`.
- Beside: K2.7 to K2.10, K2.12.

### K2.12 Whiteboards (S)
- Changes: `whiteboards/ops.rs` over `whiteboards::access` and `whiteboards::edit::apply`;
  the web operations route (`whiteboards/http.rs:404`) gains the audit row the MCP side writes
  (`mcp/whiteboard_tools.rs:754`).
- Depends on: K2.6.
- Acceptance: `whiteboard_update_elements` through web and MCP both audit; existing whiteboard
  tests pass.
- Beside: K2.11.

### K2.13 Tasks core writes (M)
- Changes: `work_items/ops.rs`: `task_create`, `task_update` (`PROPOSABLE`, T1.41; absorbing `task_mark_blocked`,
  `task_mark_ready_for_review` as aliases and routing them through `update_locked_work_item`,
  C report finding 1), `task_archive`, `task_restore`, `task_comment_create` (alias
  `task_comment`) over P0.4's `work_item_commands::append_comment` that resolves mentions and
  references and writes both projections (E F2 / C finding 6: done by P0.4; here only the op
  over it), `task_comment_update`,
  `task_comment_delete`, `task_criteria_update`. The web board drag (`work_items.rs:2158`)
  calls `task_update` with a placement (C finding 1). One audit action per op
  (`task_update`), replacing `work_item.updated`/`work_item.mcp_status_updated`; history
  readers keep accepting the old names.
- Depends on: K2.6, P0.4; T1's tasks-as-records slices in flight (rebase on them; the ops wrap
  whatever `work_item_commands` is then).
- Acceptance: `ops_tasks.rs`: an agent's task comment "@alice …" still notifies Alice (P0.4's
  case, through the op); a
  board drag emits `changedFields: ["status","position"]` like MCP does; web and MCP task
  updates share one idempotency scope (a retry through the other edge replays); `m4_*`,
  `mcp_tasks.rs`, `mcp_task_board.rs`, `mcp_task_criteria.rs` pass.
- Beside: K2.14, K2.15.

### K2.14 Tasks: claims, links and planning (M)
- Changes: claims and handoffs move from `mcp/tasks.rs:3274,3407` into `work_item_commands`
  (E F8); ops `task_claim`, `task_release`, `task_move`, `task_reorder`, `task_link`
  (absorbing `task_depend`, `task_relate`, `task_link_pull_request` as aliases, link kinds
  from K3), `task_watch_set`, `label_*`, `field_*`, `view_*`, `milestone_*`, `template_*`,
  `recurring_*`, `task_import`, `plan_import`, `task_create_many`, `task_update_many`,
  `bug_report_link_*`; web routes in `work_items/*.rs` become wrappers.
- Depends on: K2.13; K3.15 (soft: `link_create`; until then the op writes today's link
  tables).
- Acceptance: `mcp_task_batches.rs`, `mcp_planning.rs`, `mcp_labels.rs`, `awd89_*`,
  `awd102_task_move.rs` pass; a person can claim and release from the web.
- Beside: K2.15, K2.16.

### K2.15 Task and space reads (M)
- Changes: `task_get`, `task_list` (one op over the web list engine `work_items/list.rs`
  with a typed filter; the MCP engine `mcp/tasks.rs:596-1420` becomes a translation of
  arguments into it, C finding 2), `my_work` alias, `dependency_graph`, `task_ready_list`,
  `briefing_get`, `project_get`, `project_list`, `thing_get`, `search`.
- Depends on: K2.13; T1 if it has already merged the record query engine (then wrap that).
- Acceptance: cursors issued before the switch keep working for one release (`mcp/tasks.rs:1112`
  promise); `task_list` through web and MCP returns the same ids for the same filter (new test).
- Beside: K2.14.

### K2.16 Inbox, notifications, Later, questions, files (M)
- Changes: ops for `notification_*`, `later_*`, `question_*`/`ask_person`, `file_upload`
  (6 MiB body), `file_list`, `file_get`, `file_delete`, `people_search`.
- Depends on: K2.6.
- Acceptance: `file_upload` through `/api/v2/w/{workspace}/ops/file_upload` accepts a 4 MiB
  file; existing tests pass.
- Beside: K2.13 to K2.15.

### K2.17 Spaces, workspace, members, roles, settings (M)
- Changes: ops for project and workspace settings, members, invitations, roles, layouts,
  templates, modules; the `Person(...)` routes become ops with K1's permissions.
- Space operations (`space_*`) and space membership operations are K4.7's and K4.8's; this
  slice covers the rest (workspace settings, workspace members, invitations, roles, layouts,
  templates, modules) and deletes `ops/access_shim.rs`.
- Depends on: K1.2, K1.10, K1.12 (this slice is the one that switches the shim to K1); K4.7,
  K4.8 (soft: the `space_*` names, aliases `project_*`).
- Acceptance: an agent given the admin role can do what a person admin can on these routes
  (new test); `m7_*`, `layouts.rs`, `modules*.rs` pass.
- Beside: K2.18.

### K2.18 Automations, webhooks, code and environments (M)
- Changes: ops for `automation_*`, `webhook_*`, `repository_*`, `pull_request_*`,
  `environment_*`, `github_integration_*`; automation builder reads `catalog.json`.
- Depends on: K2.3; A1's code-host seam for the code ops' wording.
- Acceptance: an automation with `RunOperation { operation: "page_update" }` changes a page as
  its maker and the audit row says `edge: automation`; `automations.rs` has no
  `call_tool_direct`.
- Beside: K2.17.

### K2.19 Runs, approvals, launch, usage: aliases only (S)
- Changes: once A1's run ops exist in the registry, alias table rows for `run_*`,
  `task_launch`, `launch_*`, `approval_*`, `usage_*`, `model_catalog_write`.
- Depends on: A1.3, A1.8 (the run and request ops in the registry).
- Acceptance: old tool names keep their shapes (golden JSON).
- Beside: anything.

### K2.20 MCP resources, skills and server card (M)
- Changes: `ops/mcp.rs` resources from describe ops flagged with a resource template;
  Skills-over-MCP resources for each space's skills; `GET
  /.well-known/mcp/server-card.json`; resume old sessions with the client's revision.
- Depends on: K2.10 (page and skill ops), K2.1's matrix.
- Acceptance: matrix test extended: `resources/list` and `resources/read` of a guide page on
  2026-07-28, with a page in a private space refused; Hermes-style clients with resources off
  still work (no change to tools).
- Beside: K2.21.

### K2.21 MCP Tasks extension for long operations (M)
- Changes: `migrations/<ts>_operation_tasks.sql`; a Postgres `TaskStore`; `job_get` op;
  `LONG` ops return `started`; `project_export`, `task_import` and large `*_many` move to it.
- Depends on: K2.20 not required; K5's leases if job claiming changes (assumed unchanged).
  Job claiming stays a `SKIP LOCKED` row claim in a short transaction (pooler-safe, ruling R24
  rule 3); the MCP task's result is read through `api` from `operation_tasks`, so a `work`
  instance driven by K5.18's tick can finish it.
- Acceptance: on 2026-07-28 a client gets a task handle and `tasks/get` from a second server
  process (two `AppState`s on one database in the test) returns the result; on 2025-06-18 the
  same call returns `started` and `job_get` finishes it; another account's `tasks/get` is
  not found.
- Beside: K2.20.

### K2.22 Contract: delete hand-written edges (M, repeat per batch)
- Changes: per batch of modules whose wrap slice shipped a release ago: delete old web
  routes, `mcp/<area>.rs` structs and methods, their lines in `tool_permissions.rs`,
  `PRIMARY_SCOPES`/`OTHER_SCOPES`, `keys_to_ids`, `tag_for`, manifest `tools` maps (now
  generated), `LEGACY_SCOPES`. Final batch: `#[tool_router]`, `call_tool_direct`, `Scoped<T>`,
  `WriteResult`, `ReadResult`, `BatchResult`; `agent_parity.rs` replaced by
  `no_hand_written_edges`.
- Depends on: the module's wrap slice plus one release; K2.6.
- Acceptance: the conventions tests in §6 all pass with the legacy allowlists empty; the
  isolation tests pass; `mcp.rs` is under 1,500 lines (transport, auth, bridge).
- Beside: other batches.

### K2.23 Alias sunset (S)
- Changes: remove aliases whose window passed and whose 30-day usage is zero; v1 retirement
  when its 12 months pass.
- Depends on: owner decision (open question 1).
- Acceptance: removed aliases answer "unknown tool, use X" for one more release.

## 6. Guard rails

All in `apps/server/src/ops/conventions.rs` (unit tests, no database) unless noted.

1. **No hand-written edges.** The route scan of `policy_inventory.rs:1820` is extended: every
   mutating route is the generated op route or in `HAND_WRITTEN_EDGES` with a boundary reason
   from a closed enum; a `Legacy(op)` reason must name an existing op. The legacy MCP router's
   tool names must all be in `LEGACY_TOOLS`; a new tool not in the registry fails.
2. **Frozen lists only shrink.** `tools/ci/frozen-lists.sh` (run by CI next to
   `changed-areas.sh`): a PR that adds a line to `HAND_WRITTEN_EDGES`, `LEGACY_TOOLS` or the
   `#[tool_router]` block fails unless the line carries `// allowed: <reason>` reviewed in the
   PR. This avoids pinning counts (AGENTS.md) while stopping growth.
3. **No internal calls through a transport.** A test fails if `call_tool_direct` or
   `McpServer::tools().call` appears outside `public_api.rs` v1 (until K2.22 deletes it); code
   calls `ops::dispatch` or the domain function.
4. **Conventions over the registry:** name matches `^[a-z]+(_[a-z]+)*$`, the verb is in the
   kind's list; writes have a request id; change ops on versioned kinds have `expectedVersion`
   in the input schema; query ops take `cursor`/`limit` and return `Page<T>`; every op has a
   title and a description of 40 to 1,200 characters with no vendor names and no kind rules;
   every write declares `EMITS`; `PROPOSABLE` only on change ops; input schemas use camelCase
   and `deny_unknown_fields` (checked as `additionalProperties: false`).
5. **No account-kind checks in operations.** A source scan fails on `ActorKind::` or
   `actor_kind ==` inside any `src/**/ops.rs` (K1 owns the wider rule).
6. **Isolation for every op.** `workspace_isolation.rs` and `write_isolation.rs` take their
   tool lists from the registry: every read op needs an `mcp_cases` entry, every write op a
   write case. The guard tests stay database-free as today.
7. **Generated files current** (K2.4) and **outputs match schemas**: the integration test
   helper that checks results against `outputSchema` checks the web route's body against the
   same schema.
8. **Web side** (W1 owns, K2 asks): an ESLint rule bans `requestJson(` with a literal path
   in folders whose module has moved to `callOp`, listed in one array that only grows.
9. **Skill and rule.** `add-operation` skill; `add-mcp-tool` says "for anything new, use
   add-operation"; P0 adds the AGENTS.md rule.

## 7. Risks and open questions

| Risk | How we'd notice | Fallback |
|---|---|---|
| rmcp 3.9 lacks part of 2026-07-28 or Tasks, or behaves differently per revision | K2.1 matrix test | Keep legacy session mode for all; hand-write `server/discover` and the Tasks methods as JSON-RPC handlers in front of rmcp (the `authenticate_mcp` layer already parses bodies, `mcp.rs:4327`); upgrade rmcp when it catches up |
| Live agents break on renamed tools or changed envelopes | alias usage table; agent errors in logs | Aliases keep old shapes byte for byte; nothing is removed without zero usage for 30 days |
| A wrapped edge changes behaviour silently (scope, audit name, validation message) | existing integration tests per area; golden JSON for aliases | Each wrap slice lists the differences it settles; history readers accept old audit names |
| Idempotency replay across the switch decodes an old stored body | replay decode error in logs | `LEGACY_SCOPES` with the old decoder for 48 hours |
| The generic dispatcher adds a database round trip per call (target lookup) | latency metric per op | Lookup reuses the transaction; describe/query ops may skip the separate lookup when the op's own query enforces access (flag, reviewed) |
| Module slices collide with T1 (tasks), C1 (comments, proposals), A1 (runs), K4 (space names) | merge conflicts | Wrap whatever their current function is; canonical names for those areas come from their plans, K2 supplies aliases |
| The TS generator meets a schema shape it can't express | regen test fails with the op name | Add the shape to the subset, or annotate the type with `#[schemars(with = ...)]` |
| 250 ops in one `tools/list` is a lot of context for agents | agent feedback | Lists are already filtered by role and module; M1 can add per-connection tool sets; Skills point agents at the few they need |

Questions only the owner can answer:

1. **Alias window.** Recommendation: 180 days and 30 days of no use, removal on your go-ahead
   per batch. Alternative: keep aliases forever (cost: one adapter each).
2. **One envelope, including for old tool names?** Recommendation: no; old names keep their
   old shapes until they go, new names use the envelope (this answers E's second question
   without breaking connected agents).
3. **Should people get claim, release and reply-lease controls in the web app?**
   Recommendation: yes (K2.9, K2.14 add the web side), since parity runs both ways.
4. **Public API v1 retirement.** Recommendation: 12 months after v2, matching MCP's window.

## 8. Touch points

| Workstream | K2 needs | K2 provides |
|---|---|---|
| K1 | `access::check_locked` (allow or deny) and `access::write_mode_locked` (`Grant{Direct}` allow, `Grant{Propose}` suggest only, `Denied` deny; ruling R2) and `sql_filter` for query ops; `access::Caller` with `reach` (ruling R19); the permission catalogue type `Perm`; credential resolution for bearer tokens (successor of `resolve_bearer_principal`); run-scoped tokens that carry `requested_by`; delegation narrowing (today `AgentLimits`) | `PERMISSION` on every op (the list of what is checked where), the access shim to replace, and the list of kind-based rules in §2.4 |
| K3 | `things::locate(conn, id) -> Located`; `events::set_context` and `events::emit(tx, NewEvent)`; link kinds for `task_link`; the audit trail read from events | Every op declares `EMITS`; dispatch emits in the same transaction; audit rows name the op |
| K4 | Space ids equal project ids (assumed); `space_*` names (K4.7, K4.8 own the space ops); workspace slugs (K4.5); the direct scope (K4.23) | `/api/v2/w/{workspace}/ops/...` and `/api/v2/direct/ops/...` (ruling R4, R20), `spaceId` with `projectId` alias in canonical inputs |
| K5 | Shared rate limiter with a cost argument (`Limits::check_cost`, used by H1's `admit`); leases unchanged for jobs; pooler-safe rules (`SET LOCAL` only) | Cost class per op; stateless MCP across processes; `operation_tasks` readable from any process |
| T1 | Its current task and record command functions as each slice lands | Task ops wrap them; T1's new view and record writes are ops from day one |
| A1 | Run, request and host operations defined as ops | Registry, envelope, aliases for `run_*`, `launch_*`, `approval_*`; `Edge::Job` and `requested_by` |
| A2 | Delivery and subscription ops | Ops for replies (K2.9); `EMITS` metadata for subscriptions |
| M1 | Module registration calling `register_ops`; module-on checks by `(workspace, space, module, account)` | Each op's `MODULE`; the generated manifest `tools` map until manifests become code |
| C1 | `proposals::create`, `Proposable::prepare`/`apply` and `proposal_decide` applying inside a dispatch with `Edge::Proposal` (ruling R26); C1.2's `create_thread` (R9) | `PROPOSABLE`, `propose()`, `asProposal`, `outcome: proposed` |
| C2 | nothing specific | inbox, notification and search ops |
| W1 | Consumption of `callOp` and the generated types; the ESLint rule in §6.8; query keys per op | `apps/web/src/api/generated/*`, `catalog.json` for palettes and automation forms |
| H1 | `db::scoped_tx` and the `app.tenancy` key (H1 3.4); `metering::admit` | Every operation runs in `scoped_tx`, scoped by `set_config('app.tenancy', 'w:<account>|<id>', true)`; `admit` called once per write; export can enumerate ops per module |
| P0 | Merge the `AGENTS.md` rule; take or leave E F9 wording (K2.1 does it if P0 doesn't); `create_thread` if P0 does B F4 | |

## 9. Changes to the end state

1. **"Every write takes a request id … and the version it expects."** Creates have no version
   to expect, and some changes are deliberately last-write-wins (reactions, read marks,
   claims with their own fencing). Proposed wording: "every write takes a request id so a
   retry is safe; every change to a versioned thing takes the version it expects, unless the
   operation is defined as last-write-wins and says so."
2. **"suggest" is used for two things.** The fifth kind (an operation that produces a draft
   or analysis for a person) and suggest-instead-of-edit (a change held as a proposal) are
   different. Proposed addition to §5: "An operation of any change kind may also be
   *proposable*: when the caller's role grants suggest but not edit, the change is held as a
   proposal. The *suggest* kind is for operations whose only output is something to accept."
3. **Workspace in API addresses.** §5 "The web app" puts the workspace in browser URLs. Add:
   "API and public API addresses carry the workspace too, and a call whose target is in
   another workspace is not found." It gives the database's row rules their key per request.
4. **Public API versions.** Add to §5: "The public API is versioned in its path; the tool
   names agents knew before the operation layer keep working for a stated window."

## Triggers (ruling R16)

K2 adds no trigger. Dispatch sets `app.tenancy` (through `db::scoped_tx`) and K3's acting context with `SET LOCAL`
in each transaction; nothing is copied between tables.

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| K2.1 | Protocol matrix and vendor-free descriptions (rewrite done by P0.32) | S | P0.32 (soft) | all |
| K2.2 | The ops kernel | M | none | K2.1 |
| K2.3 | Generated edges: web, MCP, the v2 route, automations | M | K2.2, H1.20 (soft: `db::scoped_tx`; a plain transaction with the same `set_config` until it lands) | K2.4 |
| K2.4 | Code generation and drift checks | M | K2.2 (K2.3 for routes) | K2.3 |
| K2.5 | Template slice: decisions, add-operation skill | S | K2.3, K2.4 | K2.6 |
| K2.6 | Conventions guard and aliases | S | K2.3 | K2.5 |
| K2.7 | Messages (mark-seen fix done by P0.5) | M | K2.5, K2.6, P0.5 | K2.9, K2.10, K2.11, K2.12, K2.16 |
| K2.8 | Threads and conversations (C1.2's create_thread) | M | K2.7, C1.2, K4.24 (soft) | K2.10, K2.11 |
| K2.9 | Reply requests as typed functions and ops | M | K2.7; coordinate A2 | K2.8, K2.10 |
| K2.10 | Pages, skills, page discussion (proposable) | M | K2.6, C1.7, K1.12 (soft) | K2.8, K2.9, K2.11 |
| K2.11 | Sheets and collections | M | K2.6 | K2.7 to K2.10, K2.12 |
| K2.12 | Whiteboards | S | K2.6 | K2.11 |
| K2.13 | Tasks core writes (mentions fix done by P0.4) | M | K2.6, P0.4; T1 slices in flight | K2.16, K2.17 |
| K2.14 | Tasks: claims, links, planning | M | K2.13, K3.15 (soft) | K2.15, K2.16 |
| K2.15 | Task and space reads (one list engine) | M | K2.13; T1 record query if merged | K2.14 |
| K2.16 | Inbox, notifications, Later, questions, files | M | K2.6 | K2.13 to K2.15 |
| K2.17 | Workspace, members, roles, settings (space ops are K4.7, K4.8) | M | K1.2, K1.10, K1.12, K4.7 (soft), K4.8 (soft) | K2.18 |
| K2.18 | Automations, webhooks, code, environments | M | K2.3; A1 code-host seam (wording) | K2.17 |
| K2.19 | Runs, approvals, launch, usage: aliases | S | A1.3, A1.8 | any |
| K2.20 | MCP resources, skills, server card | M | K2.10, K2.1 | K2.21 |
| K2.21 | MCP Tasks extension for long operations | M | K2.3 | K2.20 |
| K2.22 | Contract: delete hand-written edges (per batch) | M | each module's wrap slice + one release | other batches |
| K2.23 | Alias sunset and public v1 retirement | S | owner decision; window passed | any |

Module slices K2.7 to K2.18 follow the K2.5 template and the `add-operation` skill, so any
workstream's agent can pick one up; each is one pull request that wraps, switches the web
callers and keeps the old routes as wrappers until K2.22.

## Review changes

- §3.1 and §3.5: one generated route for web and bearer, `POST /api/v2/w/{workspace}/ops/{name}` (id or slug), account ops at `/api/v2/ops/{name}`, direct-scope ops at `/api/v2/direct/ops/{name}`, OpenAPI at `/api/v2/openapi.json` (rulings R4, R20).
- §3.1 step 6 and §3.8: limits are H1's `metering::admit` over K5's `Limits::check_cost`, once per write (R13).
- §3.3: `Caller` is K1's `access::Caller` (stopgap struct until K1.2); `Edge` and the trace id moved into `OpContext`; `Perm` is an alias of `access::Permission`; `ops::Target` maps onto `access::Target` (R2).
- §3.3: `EventDraft` is an owned `events::NewEvent`; `things::locate(conn, id)` (K3's interface).
- §3.3 proposals: dispatch calls `write_mode_locked` for proposable ops and `check_locked` otherwise, with K1's three answers (R2, R14); the shim names its removing slice.
- §3.7: the `workspace` argument and `/w/<slug>/mcp` alias; any credential reaching more than one workspace names it, agents included; direct-scope tools need none (R4, R19, R20).
- §3.8: v1 keeps its paths; v2 is the shared route; a bearer that doesn't reach the path's workspace gets `not_found` (R4, R19).
- Duplicates (R10): K2.1's rewrite is P0.32's, K2.7's fix P0.5's, K2.13's fix P0.4's; each slice keeps only what goes beyond.
- K2.8 uses C1.2's `create_thread` (R9); DMs go to the direct scope.
- K2.17 leaves space ops to K4.7 and K4.8 and deletes the shim.
- K2.21: job claiming and results are pooler-safe and tick-driven (R24 rule 3, 4).
- Depends named with slice ids (K2.1, K2.7, K2.8, K2.10, K2.13, K2.14, K2.17, K2.19); touch points for K1, K3, K4, K5 updated.
- Added the trigger table (R16: none).
- Content review requests and ruling R26 (accepted): `proposal_accept` replaced by C1's `proposal_decide` applying through `Proposable::apply` in a dispatch; `ProposalDraft` is the input to `Proposable::prepare`; `record_update_many` and `task_update` are `PROPOSABLE`; K2.8 depends on C1.2 with no fallback (R9); a table of operations other plans add (C1, C2, K4, M1, K5) with the verbs that join the list.
- §3.3 errors: added `limit_reached`, `workspace_suspended` and `DenyReason::HostRevoked` (assumed by H1 and A1 in HANDBACKS, unmet before).

### Second pass (agents and hosting review, R32 to R42)

- §3.1 step 7, §3.8, §8 and the trigger note: every operation runs in H1's `db::scoped_tx`, scoped by `set_config('app.tenancy', 'w:<account>|<id>', true)`; `app.workspace_id` removed (R42, H1 3.4).
- §3.1: `admit` once per write, stated in the pipeline (R13).
- K2.3 renamed and its acceptance in K2.16 use the R4 path; no `/api/public/v2/` remains (R33).
- R37: `paused` and `pending_review` stay values inside A2's `ok` results; nothing added to the envelope.
