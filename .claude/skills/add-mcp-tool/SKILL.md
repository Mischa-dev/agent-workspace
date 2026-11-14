---
name: add-mcp-tool
description: Use when adding or changing a tool on Agent Workspace's own MCP server (apps/server/src/mcp.rs and apps/server/src/mcp/*), including its arguments, result, permissions and tests.
---

# Add or change an MCP tool

The app's MCP server is how agents and connected apps do everything a person
can. A tool is three pieces: argument and result types, a function that does
the work, and a method in the tool router. The tests check the rest.

Before you start, read "Rules while the foundations are rebuilt" in
`AGENTS.md`. A tool is one edge of an operation (rule 4): the work lives in a
domain function the HTTP handler calls too, a tool that creates something
takes a `clientRequestId` (rule 9), and its permission comes from the policy,
never a role name or the caller's kind of account (rules 1 and 2). The
structure guards in `apps/server/src/structure_guards.rs` fail on a new
offender.

## 1. Types and the function

Put them in the module for the area, in `apps/server/src/mcp/` (tasks in
`tasks.rs`, pages and skills in `memory_tools.rs`, and so on).

- Arguments: `#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]`,
  `#[serde(rename_all = "camelCase", deny_unknown_fields)]`. The `///` doc
  comment on each field becomes its description in the schema, so write it
  for an agent: what to pass, in which form, and the default.
- Accept what people type: task keys (`AWD-12`), project keys (`AWD`), IDs or
  links. `parse_id` and `resolve_task` in `common.rs` / `tasks.rs` do this.
  Keys are only unique within a workspace, so a new key or name resolver
  fetches `LIMIT 2` and returns nothing on two different matches, as
  `project_by_key` does; never `LIMIT 1` or "the first one".
- Results: `#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]`,
  `#[serde(rename_all = "camelCase")]`. An optional field gets
  `skip_serializing_if`, and a list that may be empty gets
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. Claude Code
  rejects a result that doesn't match the declared outputSchema, and the
  integration tests' `tool()` helper checks every call against it.
- Return `ToolResult<T>`: `ok`, `not_found` ("wasn't found, or you can't see
  it", the same for missing and hidden), `invalid(field, message)`, `denied`,
  `conflict`, `read_only()`. Messages say what to do next.
- Wrap the body in `let result = async { ... }.await;` and finish with
  `result.unwrap_or_else(|error| ToolResult::from_error(error, "tool_name"))`.

## 2. Permissions (never skip)

- Check `principal.can_read()` or `principal.can_write()` first.
- The MCP middleware picks the project from `projectId`, or from the first ID
  argument it knows (`PRIMARY_SCOPES` / `OTHER_SCOPES` in `mcp.rs`). A tool
  that takes a new kind of ID needs a line there, next to related ones.
  Then re-check: the resource's `project_id` must equal
  `principal.project_id`.
- Check access every call: `memory::project_access`, `can_write_in`, or the
  area's own helper. Conversations go through `conversation_visible_to`.
- Writes record an audit row (`insert_audit`) and, for tasks, activity.

## 3. The router method

In `mcp.rs`, inside `#[tool_router] impl McpServer`, add the method next to
related tools (never at the end, never reorder):

```rust
#[tool(
    description = "One or two sentences: what it does, when to use it, what to call next.",
    annotations(title = "Short title", read_only_hint = true, open_world_hint = false)
)]
async fn thing_list(
    &self,
    Parameters(arguments): Parameters<area::ThingListArguments>,
    Extension(parts): Extension<http::request::Parts>,
) -> Json<common::ToolResult<area::ThingListValue>> {
    let principal = principal_or!(parts, unauthenticated());
    Json(area::thing_list(&self.state, principal, arguments).await)
}
```

Writes set `read_only_hint = false` plus `destructive_hint` and
`idempotent_hint`. If the tool changes how agents should work, add a clause
to `SERVER_INSTRUCTIONS`. Name its group in `public_api::tag_for` if its noun
is new; `/mcp-tools.md` and the OpenAPI document pick it up from there.

## 4. Tests

- Every read-only tool needs a two-workspace isolation case:
  `MCP_CASE_TOOLS` and `mcp_cases` in
  `apps/server/tests/integration/workspace_isolation.rs`. The guard test
  `every_get_route_and_read_tool_has_an_isolation_case` fails until it's
  there, and needs no database.
- Every tool that writes needs one too, in
  `apps/server/tests/integration/write_isolation.rs`: `MCP_WRITE_TOOLS` and
  `mcp_cases`, with A's own arguments (which must succeed) and B's IDs
  (which must be refused). Its guard,
  `every_write_route_and_write_tool_has_an_isolation_case`, also needs no
  database. The same goes for a new POST, PUT, PATCH or DELETE route
  (`WRITE_ROUTES` and `http_cases`); the file's top comment says how.
- Agent parity (AWD-91): every web write is listed in
  `apps/server/src/agent_parity.rs` with the tool that does the same thing.
  A new route fails that guard until it's there; a tool that closes a `Gap`
  moves its entry to `Tool`. If the feature is something an agent would use
  while running a project, add its step to
  `tests/integration/awd91_agent_parity.rs`.
- Write an integration test where a bug would be silent: someone outside the
  project reading or writing, a claim or version conflict, a private
  conversation. See `w22_agent_kit.rs` for a compact one. Build it on
  `tests/integration/support/` (the app, test databases, a person, an agent
  and a connected app as `Caller`s, `WorldBuilder`).
- Don't count tools anywhere; other branches add them too.

## 5. Check

`cargo check -p agent-workspace-server`, then
`cargo test -p agent-workspace-server --test integration -- <your test> workspace_isolation write_isolation`
(needs `AGENT_WORKSPACE_DATABASE_URL`; in a cloud session run
`tools/cloud/setup.sh` if it's missing). Before pushing, run the full checks
(see the `steward` skill).
