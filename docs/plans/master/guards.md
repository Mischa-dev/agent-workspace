# Guard registry

One id per guard, one file, one allowance format (a per-file count that never
fails when it drops, so branches merge in either order). From the P0 plan,
section 6.3; a slice that adds a guard adds its row here in the same pull
request, and a slice that builds one marks it built.

Several plans proposed the same guard under different names. Each guard has one id, one
file and one allowance format (3.8: a per-file count that never fails when it drops, so
branches merge in either order). P0 builds the first version and owns the rule text; the
workstream in the "Tightens" column owns making it stricter later (new patterns, a reason and
a removing slice per allowance entry, folding a ratchet to zero). The other plans' names in
the last column are not separate guards: those slices point here and extend this one.

| Guard id | Rule | Built by | Tightens | Was also proposed as |
|---|---|---|---|---|
| `raw_role_checks`, `custom_role_allows`, `workspace_role_enum` | 1 | P0.26 | K1.1 (adds `access_bypasses`: `agent_limit_hides(`, `agent_limit_projects_allowing(`, `Subject::new(`, `policy::authorize(`; reason and removing slice per entry) | K1 guard 1 (`access/guard.rs`, `access/allowlist.rs`) |
| `account_kind_checks` | 2 | P0.26 | K1.1, then each area slice of K1 | K1 guard 1 kinds; A1 G5; A2 "no rule by account kind"; C1 guard 4; K2 guard 5. A new folder (`runs/`, `delivery/`, `discussion/`, `ops`) has no allowance entry, so it is held at zero automatically |
| `web_role_name_gates`, `web_account_kind_gates` | 1, 2 | P0.27 (the second added in review: `kind === 'external_agent'`, `kind === 'human'` in a condition) | K1.26, W1.37 | K1's `apps/web/src/access.guard.test.ts` |
| `agent_runs_sql`, `active_run_states_literal` | 3 | P0.26 | A1.2 (adds the new run tables and `LEGACY_RUN_SQL` reasons) | A1 G1, G4 |
| `requests_sql`, `hosts_sql` | 3 | A1.7, A1.11 | A1 | A1 G2, G3 |
| `conversation_inserts`, `message_inserts`, `task_core_updates`, `direct_tool_calls` | 4 | P0.26 | C1.2 (threads only in `conversations/threads.rs`), K2.6 (`call_tool_direct`) | C1 guards 1, 2; K2 guard 3 |
| `task_view_code` | 4 | T1.1 | T1.39 | T1 guard 1 |
| `process_state`, `background_spawns` | 5 | P0.26 | K5.1 (adds `broadcast::channel`, `tokio::fs`, reason and bound per entry) | K5 guard 2 (`process_local.rs`) |
| `session_advisory_locks`, `pooler_safe_sql` | 14 | K5.2 (phase 1, ruling R24) | K5 | K5 guard 3; R24 rule 3 |
| `nothing_per_workspace` (10,000 idle workspaces) | 14 | H1 (its R24 slice) | H1 | R24 rule 6 |
| `link_tables`, `comment_tables` | 6 | P0.26 | K3.21, C1.3 | K3 guard 4; C1 guard 2 |
| `conversation_listings` | 7 | P0.0 | K4.3 (adds the direct scope: no membership row may reference it), then K1.20 | C1 guard 3; C2 "listing lint"; K4 G2 |
| `vendor_names_core`, `web_vendor_names`, `personal_hosts`, tool-description test | 8 | P0.26, P0.27, P0.32 (P0.32 rewrites the vendor names; K2.1 is soft on it) | K2.1 (adds kind-rule wording: "people only", "agents can't") | A1 G6; A2 "no vendor names"; K2.1 `descriptions_name_no_vendor_or_kind_rule` |
| create guard (`clientRequestId` on creates) | 9 | P0.16 | K2.6 (conventions over the registry) | K2 guard 4 |
| in-workspace privacy net | 9 | P0.22 (+ P0.38's account cases) | each slice adds its cases | C2 "private-conversation net" |
| `notification_detail_text`, `event_payload_text` | 10 | P0.36, K3.21 | K3, A2 | K3 guard 3; A2 "thin payloads" |
| `tenant_tables`, `new_table_names` | 11 | H1.3, K4.3 | H1, K4 | H1 6.2; K4 G4 |
| `one_space_creator`, `space_kind_literals` | 11 | K4.3 | K4.17 | K4 G1 |
| `module_ids_by_hand` | 12 | M1.1 | M1.2 onward | M1 guard 1 |
| `hand_built_links` | 13 | K4.3 (server), W1.14c (web) | K4.18, W1.14c | K4 G5; W1 guard 3 |
| `outbound_clients` | 15 | H1.2 | H1 | H1.2's builder guard |
| `history_never_cut` | 16 | H1 (phase A) | H1 | H1 6.2 |
| `handwritten_api`, `whole_project_refetch` | 17 | W1.3, W1.4 | W1 | W1 guards 1, 2 |

The old `conversation_visible_to_raw` guard is withdrawn. In its place is
`conversation_readable_by_is_one_call` (P0.28, ruling R32), which keeps the wrapper to one
call. The ids `conversation_listings` (P0.0) and the P0.26 ids in this table are stable: K4.3's
G2 and K1.1's ratchet build on them by these names, so no slice renames them. Two plans can't create a guard with an id already in this table; a slice that adds a
new guard adds its row to `docs/plans/master/guards.md` in the same pull request.

Other guard rails P0 adds beyond the rules: no flaky passes (P0.3), web changes run the Rust
tests that read them (P0.3), connected-client contracts (P0.17), trigger texts (P0.29), every
tool refusal matches its schema and every outcome has an HTTP status (P0.9), id-routed families
are gated or exempt with a reason (P0.7).

## Built so far

| Guard id | Where | Slice |
|---|---|---|
| `conversation_listings` | `apps/server/src/structure_guards.rs` | P0.0 |
| id-routed families gated or exempt | `apps/server/src/modules/gate.rs` tests | P0.7 |
| refusals fit each tool's schema; every outcome has a status | `apps/server/src/mcp.rs`, `public_api.rs` tests | P0.9 |
| create guard (`clientRequestId` on creates) | `mcp.rs` test `creates_take_a_client_request_id` | P0.16 |
| connected-client contracts | `apps/server/tests/integration/compat_clients.rs` | P0.17 |
| in-workspace privacy net (`KNOWN_GAPS` only shrinks) | `apps/server/tests/integration/privacy_inside_workspace.rs` | P0.22 |
| read-only tools change nothing (`READ_TOOLS_THAT_WRITE` only shrinks) | `workspace_isolation.rs` `check_mcp` | P0.23 |
| `raw_role_checks`, `custom_role_allows`, `workspace_role_enum`, `account_kind_checks`, `agent_runs_sql`, `active_run_states_literal`, `conversation_inserts`, `message_inserts`, `task_core_updates`, `direct_tool_calls`, `process_state`, `background_spawns`, `vendor_names_core`, `link_tables`, `comment_tables` | `apps/server/src/structure_guards.rs` | P0.26 |
| `web_role_name_gates`, `web_account_kind_gates`, `web_vendor_names` | `apps/web/src/structure.test.ts` | P0.27 |
| `conversation_readable_by_is_one_call` | `apps/server/src/structure_guards.rs` | P0.28 |
| trigger refusal texts | `apps/server/src/domain.rs` test `admission_refusal_texts_match_the_trigger` | P0.29 |
| master plan consistency (`plan.py`) | `tools/ci/checks.sh` step "master plan" | P0.30 |
| front-door links resolve | `tools/ci/test_docs_links.py` | P0.31 |
| tool titles, descriptions and schemas name no vendor | `mcp.rs` test `tool_descriptions_name_no_vendor` | P0.32 |
| `personal_hosts` (no tailnet address or home path outside docs/history) | `tools/ci/test_personal_hosts.py` | P0.33 |
| runner SDK tests run in CI | `tools/ci/checks.sh` steps "runner SDK tests" | P0.34 |
| `notification_detail_text` | `apps/server/src/structure_guards.rs` | P0.36 |
| account routes walked or excused (`ACCOUNT_ROUTES_EXCUSED` only shrinks) | `privacy_inside_workspace.rs` `every_account_route_is_walked_or_excused` | P0.38 |
