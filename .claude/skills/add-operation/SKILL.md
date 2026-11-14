---
name: add-operation
description: Use when adding or changing anything the app can do (a write, or a read agents and the web share) in Agent Workspace's server; an operation in src/<module>/ops.rs gives the web route, the MCP tool, the v2 API, the automation action and the audit row at once.
---

# Add an operation

Everything the app can do is an operation (K2): one struct in
`apps/server/src/<module>/ops.rs` implementing `ops::Operation`. The web app
calls it through `POST /api/v2/w/{workspace}/ops/{name}` (`callOp` in the web
app), agents as the MCP tool of the same name, scripts through the same v2
route with a bearer, and automation rules as a `run_operation` action. The
dispatcher (`ops/dispatch.rs`) checks the caller, the module, the target and
the request id, runs it in one transaction and writes the audit row. Never
write a hand route or a hand tool for something new.

`memory/ops.rs` (`decision_record`, `decision_list`) is the worked example:
copy it.

## 1. The input

```rust
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingCreateInput {
    /// The space. Doc comments become the schema's descriptions: write them
    /// for an agent.
    pub space_id: IdRef,
    pub title: String,
}

impl Targeted for ThingCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}
```

- References take what people paste: `ops::refs::IdRef` (an id or a link),
  `MessageRef`. Never a bare `String` id.
- The target is what the call acts on: `Target::thing("task", id)` for an
  existing thing, `Target::Space` for something new in a space,
  `Target::Workspace(None)` when only the workspace matters, `Target::Account`
  for the caller's own things. Dispatch finds where it lives; a thing the
  caller can't see is `not_found`. A new thing kind needs a line in
  `ops/target.rs` `KINDS` (its SQL must honour `conversation_visible_to`
  where conversations are involved).
- An old argument name stays accepted with `#[serde(alias = "oldName")]`.
- Don't add `workspace`, `clientRequestId` or `preview` fields: the edges
  read those themselves.

## 2. The operation

```rust
pub struct ThingCreate;

impl Operation for ThingCreate {
    const NAME: &'static str = "thing_create";       // <thing>_<verb>
    const KIND: OpKind = OpKind::Create;            // describe, query, create, change, suggest
    const MODULE: &'static str = "things";          // a manifest id
    const PERMISSION: Option<Perm> = Some(Perm::ThingsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher; // None for reads
    const TITLE: &'static str = "Create a thing";
    const DESCRIPTION: &'static str = "What it does and what to call next. \
        No vendor, model or account-kind words (K2.1's ratchet).";
    type Input = ThingCreateInput;
    type Output = ThingView;                        // Serialize + JsonSchema

    async fn run(call: &mut OpCall<'_>, input: ThingCreateInput) -> Result<ThingView, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let id = things::create(tx, ...).await?;     // AppError converts with `?`
        call.audit_target("thing", id, serde_json::json!({}));
        things::get_in(&mut **tx, id).await?.ok_or(OpError::Unavailable)
    }
}
```

- Writes run in `call.tx()`; the dispatcher commits after the audit row.
  Read back what you return through the same transaction.
- No permission checks inside `run`: `PERMISSION` and the target are checked
  before it runs. No account-kind checks either: a person and an agent with
  the same role get the same answer.
- Errors: `OpError::invalid(field, message)`, `OpError::not_found(what)`, or
  `?` on the domain's `AppError`. Messages say what to do next.
- `Uuid` fields in outputs need `#[schemars(with = "String")]`.

## 3. Register it

In the module's `src/modules/core/<id>/mod.rs`, `register`:

```rust
module.operations(crate::things::ops::register_ops);
```

with `pub fn register_ops(registry: &mut ops::Registry) { registry.add::<ThingCreate>(); }`
in the module's `ops.rs`.

## 4. Regenerate and test

- `OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current`
  writes the web types (`apps/web/src/api/generated/`), the catalogue and
  `docs/api/v2.openapi.json`. Commit them.
- The web calls it with `callOp('thing_create', input, { requestId })`.
- An integration test where a bug would be silent: retries answered once,
  someone outside the space gets `not_found`, a private conversation stays
  private. Add the operation to `MCP_WRITE_TOOLS` and a case in
  `tests/integration/write_isolation.rs` (reads: `workspace_isolation.rs`).
- If it replaces a hand-written tool, delete the tool from the `mcp.rs`
  router, its `tool_permissions.rs` line and its function; an operation and a
  tool may not share a name (`an_operation_never_shadows_a_hand_written_tool`).
