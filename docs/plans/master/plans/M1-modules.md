# M1. Modules: the plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for workstream M1, 2026-11-14. Read-only review of `/home/user/agent-workspace` at
fe95a74. Builds on the "Modular, hostable Workspace" plan (`/home/user/health-check/modular-plan.txt`,
its workstreams M1 to M5), `docs/plans/modules.md` (AWD-75), `docs/plans/collections.md`
(AWD-90 and AWD-110) and the health check (`F-content-modules.md` F1, F3, F5, F8; `I-web-app.md`
F3, F6, F11).

## 1. Goal

Today a module is a JSON file that says what a feature owns, plus a switch. When this
workstream is done, a module **builds** its part of the app: the server and the web app are
put together at start-up from what each module registers (its kinds of thing, operations,
events, permissions, settings, search, export, routes, navigation, tabs, panels, embeds,
commands), so adding a core module is one server folder, one web folder and a manifest. A
person or an agent can make a module out of data ("Playtest feedback": collections, views, a
form, rules, a role, pages, a sidebar entry), save any space or module as a package file, and
install a package with a preview of exactly what it adds. An outside app can install as a
connected module with its own account and admin-approved scopes, add operations and events,
and show panels that Workspace draws natively on a laptop or a phone, or its own page in a
sandboxed frame for secrets. The first such app is an agent host's "create an agent" flow.

## 2. Where the code is today

### 2.1 The manifest and what reads it

- **The format.** `apps/server/src/modules/manifest.rs:73-125` (`Manifest`): id, name,
  description, version, `kind` (`Core | Built | Connected`, `:16-25`; the last two used
  nowhere), `switchable`, `needs`, `works_with`, `permissions`, `tools` (name to verb),
  `routes`, `events {emits, listens}`, `notifications`, `search`, `automations {triggers,
  actions}`, `things`, `links`, `embeds`, `slots` (`pages, parts, sidebar, home, settings,
  palette, shortcuts, slash, setup`, `:40-71`), `settings`, `nouns`. Shared contract types are
  in `modules/contract.rs` (`ToolVerb :40`, `LinkKind :60`, `FieldType :125`, `ViewType :150`,
  `PieceType :166`, `Piece :188`, `ThingDecl :238`, `EmbedDecl :261`, `Thing :283`).
- **Fifteen manifests** in `modules/*.json`, plus `modules/presets.json` (four module presets)
  and six workspace templates in `modules/templates/`.
- **Server reading.** Compiled in with `include_str!` in a hand list, `CORE_SOURCES`
  (`manifest.rs:129-169`), parsed once (`core()`, `:176-193`). Lookups by pattern:
  `of_tool :230`, `of_route :237`, `of_part :241`, `of_setting :245`, `of_notification :253`,
  `of_search :257`, `of_trigger :261`, `of_event :269`, `of_thing :273`, `thing_decl :280`.
- **Web reading.** A Vite plugin projects the manifests into `virtual:modules/slots/<kind>`,
  `virtual:modules/switchable` and `virtual:modules/names` (`apps/web/vite.config.ts:47-95`).
  `modules/ownership.ts`, `pageOwners.ts` and `names.ts` read those; `ModuleGate.tsx:29-60`
  shows the turned-off page. `modules/pieces/declared.ts:7-14` globs the manifests' `things`
  and `embeds` for the pieces gallery and page embeds.
- **What a manifest controls.** Only claims and the on/off switch. Guard tests
  (`modules/mod.rs:89-412`; web `modules/slots.test.ts`) fail when a route, tool, part,
  permission, setting, setup step, event, notification kind, search kind, trigger or action
  isn't claimed by exactly one module. They don't make anything work (F1).

### 2.2 The bitmask, and what is actually stored

- `ModuleSet(u32)` (`modules/state.rs:25`), one bit per core module **by position in
  `CORE_SOURCES`**, capped by `MAX_CORE = 32` (`manifest.rs:172`, `.take(MAX_CORE)` at
  `:190`). It is `Copy`, built per request from switches (`from_switches :97`, `closed :109`).
- **No bitmask is stored anywhere.** The stored state is string-keyed already:
  - the workspace switch is a `settings` row, key `modules.<id>`, scope `workspace`, value
    `false` when off (`state.rs:19-21, 225-255`). Absent means on. One writer:
    `modules::http::write_switches` (`modules/http.rs:283-305`), called by the Modules route
    and by workspace templates (`workspace_templates.rs:737`). Each switch is also a
    hand-written settings definition (`settings.rs:487-500`, `module_switch :504`), and the
    generic settings routes refuse the keys (`settings.rs:517-519`);
  - a role's narrowing is `workspace_roles.modules_off TEXT[]`
    (`migrations/20261105213000_role_modules.sql`), constrained to at most 32 entries matching
    `[a-z_]{1,40}`: no digits, no dots, so no room for namespaced module ids.
  So "migrating stored bitmasks" is a non-task. What needs migrating is the settings rows
  (into a module state table), the role column's constraint, and the module lists inside
  templates and presets.
- **Per space.** There is no per-project module state. Modules are workspace-wide, narrowed
  by the holder's project role (or workspace role in the hidden spaces) (`state.rs:329-389`).
  The project's kind special-cases what is on: `in_project` (`state.rs:161-170`: a standard
  project needs Projects; the workspace space has only Pages), and `allows_conversations_in`
  (`:148-154`: DMs follow Messages, channels follow Channels).

### 2.3 The guards that exist

- HTTP gate `modules/gate.rs:242-282` (`refuse_off_modules`, a route layer in
  `lib.rs:217-220`): only for `/api/v1/projects/{id}/…` and `/api/v1/organizations/{id}/…`
  (`scope :77-90`), follows workspace switches only, not role narrowing (`:12-13`), with a
  process-local 10-second cache (`:46-57`). Id-routed content (pages, boards) is not gated
  (F5, confirmed by the lead).
- MCP: `tools/list` filtering (`mcp.rs:3798`, `allows_tool`), and refusal on call
  (`mcp.rs:4043-4075`, `module_off_here :3897-3936`, with hand special cases for Messages and
  Channels `:3922`, `SPANNING_LISTS :3882`, `ALSO_SERVED_BY :3941`).
- Notifications: one generated SQL condition (`state.rs:596-658`), which reads the
  `settings` table directly and special-cases `messages` (`:603`) and `pages` (`:609`).
- Events to webhooks and automations: `events.rs:226-264` (`shut_out`).
- Web: `ModuleGate`, `experience.moduleOn` (the server adds every off module's parts to
  `hidden`, `experience/layout.ts:30-45`), navigation filters.

### 2.4 What is hand-wired: the edit list for a new module today

Taking Sheets (the newest module, AWD-54) as the worked example of "a module with its own kind
of thing", here is every place a new module is edited in by hand. The F report counted 13
groups; counting each file it is about 31.

Server:

1. `modules/<id>.json`, the manifest.
2. `apps/server/src/modules/manifest.rs:129-169`, a `CORE_SOURCES` line.
3. `apps/server/src/settings.rs:487-500`, a `module_switch(...)` definition.
4. `apps/server/src/lib.rs:153-215`, `.merge(sheets::http::router())` (`:200`).
5. `apps/server/src/mcp.rs:109` (`mod sheet_tools`) and the `#[tool]` methods (`:2078-2119`).
6. `apps/server/src/mcp/tool_permissions.rs:132-136`, the tool's permission row.
7. `apps/server/src/policy.rs`: the `Action` (`:75`), its list (`:126`), its permission
   (`:171`), the `Permission` (`:207`), its info row (`:441-445`), the group list
   `PERMISSION_MODULES` (`:589`), and the role defaults (`:1173`, `:1408`).
8. `apps/server/src/policy_inventory.rs:1601-1613`, each route's policy row.
9. `apps/server/src/modules/things.rs`: `KINDS` (`:51`), a resolver SQL (`:125-220`), the
   `resolver()` match (`:221`), a `links_sql` arm (`:379`).
10. A migration widening `thing_references`' CHECKs
    (`migrations/20261113060000_thing_references.sql:18-24`) and its URL regex (`:38-56`).
11. `apps/server/src/search.rs`: `SearchKind` (`:30`), `ALL` (`:42`), the project filter by
    module id (`:152-162`), the SQL.
12. `apps/server/src/events.rs`: `EVENT_TYPES` (`:34`) and `Subject` fields (`:71`).
13. `apps/server/src/webhooks.rs:1483`, the payload mapping.
14. `apps/server/src/outbox.rs:41-73, 311, 372, 547`, a resource kind and payload variant,
    plus a migration redefining `outbox_events_resource_kind_check`
    (`migrations/20261112193000_sheets.sql`).
15. `apps/server/src/realtime.rs:210-230, 278-281`, the visibility lists.
16. The module's own "is it off" helper and its call sites (`mcp/sheet_tools.rs:64-70,
    217, 406, 1040`); the same pattern in about 45 places across 30 files (section 2.5).
17. `apps/server/src/automations.rs`, `TRIGGER_KINDS`, `ACTION_KINDS`.
18. Export, per feature (`work_items/imports.rs:2206` exports tasks only).
19. `modules/presets.json:14` and the five templates that list modules
    (`modules/templates/*.json:13`) and permissions.
20. Integration isolation lists (`tests/integration/workspace_isolation.rs`,
    `write_isolation.rs`).

Web:

21. `apps/web/src/app/App.tsx:56, 361-367`, a lazy import and a `<Route>`.
22. `apps/web/src/shell/ProjectSidebar.tsx:131-135`, a sidebar row.
23. `apps/web/src/shell/CommandPalette.tsx:135`, a palette entry.
24. `apps/web/src/experience/registry.json`, the part, and the part again in each of the 14
    presets that use `only` (`:391, 428, 461, 471, 481, 504, 538, 577, 610, 648, …`).
25. `apps/web/src/experience/parts.ts:17`, a hand copy of the id.
26. `apps/web/src/settings/workspace/roles.ts:24`, the permission key.
27. `apps/web/src/realtime/liveDocuments.ts:52-56, 147-151` and
    `useRealtimeConnection.ts:327`, live updates.
28. `apps/web/src/memory/embeds/PageEmbed.tsx:9, 26-28` and `embedModel.ts:10-14, 34-40,
    59-69`, the page embed.
29. `apps/web/src/memory/live/LivePage.tsx:57, 74, 241, 404, 427-438`, the editor's embed
    picker.
30. `apps/web/src/memory/embeds/moduleEmbed.ts:19` (`OPENS`), for a kind that opens as a thing.
31. `apps/web/src/modules/ModuleGate.tsx:35-39`, for a page routed by the thing's id.

### 2.5 Module ids matched by hand

Outside `modules/`, the server names module ids as string literals in about 45 places. The
main ones: `mcp.rs:3922`; `automations.rs:626, 650-651, 2879`; `events.rs:237, 260`;
`search.rs:155-160`; `work_item_move.rs:569, 1298, 1344, 1891`; `workspace_templates.rs:937`;
`usage/batch.rs:185`; `usage/ready.rs:164`; `mcp/whiteboard_tools.rs:78, 82, 120, 646`;
`mcp/automation_tools.rs:76, 81, 111, 127`; `mcp/later_tools.rs:170, 317`; `mcp/tasks.rs:1002,
3106`; `mcp/task_batch.rs:1296`; `mcp/collection_tools.rs:75, 247, 350, 549, 941, 1238`;
`mcp/sheet_tools.rs:70, 218, 407, 1041`; `mcp/views_data.rs:625`; `mcp/agents.rs:219`;
`mcp/code.rs:240`; `mcp/briefing.rs:141, 393`; `mcp/bug_report_tools.rs:41`; `mcp/admin.rs:754`;
`mcp/task_move.rs:179`; `mcp/talk.rs:828-937`. Inside `modules/`: `state.rs:127, 150-152,
163-165, 516, 548, 603, 609` and `gate.rs:189`. Web: `shell/ProjectRail.tsx:122, 202, 249`,
`shell/HomeSidebar.tsx:13`. Most of these are "is module X on for this caller here", written
again per tool because each tool checks its own module. Once operations name their module
(K2), the dispatcher does it once.

### 2.6 What already has the right shape (keep)

- The manifest as the one shared file both sides read, and the claim guards
  (`modules/mod.rs`, `slots.test.ts`). They stay, rewritten to check the registry.
- `settings/registry.ts:53-95` on the web: one line per page with a lazy import, which nav,
  search and routing all read. The web registration copies this pattern.
- `experience/registry.json` and `shows(part)`: real, cheap, enforced
  (`registry.test.tsx`; I report F11). Extended, not replaced.
- The notification switch SQL generated once from manifests (`state.rs:596`): the pattern for
  every SQL filter a module contributes.
- Workspace templates (`workspace_templates.rs`, AWD-76): a portable definition, checked on
  save and on use, applied in one transaction, idempotent ("using the same template twice
  changes nothing", `:16-19`), with a record of each use (`workspace_template_uses`). This is
  most of a package installer already. `collection_describe`'s portable definition
  (`docs/plans/collections.md`, "Agent tools") is the right shape for a package's collections.
- The outbound address check (`outbound.rs:5, 61`: private addresses refused unless the
  operator allows them): reused for fetching connected-module manifests and calling apps.
- Connected apps (`connected_apps.rs`) and the `integration` actor kind
  (`migrations/0002_walking_skeleton.sql:7`): the seed of an app account (K1 owns it).

### 2.7 What is in the way

- Compile-time hand lists (`CORE_SOURCES`, `KINDS`, `EVENT_TYPES`, `SearchKind`,
  `ResourceKind`, `PERMISSION_MODULES`, settings, `App.tsx`, sidebar, palette).
- `ModuleSet` indexes modules by position and can't hold a module that isn't compiled in.
- Module state lives in `settings`, which is keyed by fixed definitions (`settings.rs:487`).
- Per-tool module checks, so "on" is decided in 45 places.
- Collections and sheets embeds hard-coded (`PageEmbed.tsx:8-40`) while manifests declare
  `"embeds": []` (F3).
- The role column's id pattern and cap.

### 2.8 What this plan keeps and changes from the earlier modular plan

Kept from `modular-plan.txt`: the three sorts of module and one manifest for all; "off means
hidden, never deleted"; modules meet through events and links; no outside code in the server;
data-first building blocks (fields, collections, forms, rules) with collections as the proof;
packages as one file with a preview and admin approval for anything that grants access or sends
data out; templates becoming packages; panels as blocks first, frames second; apps getting an
identity like agents. Its M2 (fields on tasks) and M4 (collections and forms) are done or
belong to T1 now.

Changed: (1) its M1 was "switches as one manifest per module"; that shipped (AWD-75) and this M1
is the next step, **registration**, which the earlier plan didn't design. (2) Modules were
"workspace-wide, narrowed per role"; the end state gives spaces modules, so a space may narrow
and built modules install into a space. (3) Frames were "on a separate domain with a
short-lived token"; this plan serves a script-free host page with a per-response CSP instead,
so self-hosters need no second domain, and the app's page keeps its own origin for secrets. (4)
Packages carry agent **templates**, not profiles that become accounts. (5) The earlier plan left
the agent-host link undesigned; here the host's creation steps are the first connected module,
and A1/A2 own the rest of the link. (6) Presets and templates move from "on" lists to "off"
lists so new modules need no edits there.

## 3. The target design

### 3.1 Words

- **Module id**: a string. Core: `[a-z][a-z0-9_]{0,39}` with no dot (`tasks`). Built and
  connected: `<namespace>.<name>`, each part the same pattern (`local.playtest_feedback`,
  `examplehost.agents`). `local.` is the namespace of modules made in this workspace; a
  package's modules use the package publisher's namespace. Core ids never contain a dot, so
  nothing outside can take one.
- **Description**: what a module adds, as data (the manifest, version 2). The same shape for
  all three sorts.
- **Registration**: the description plus the functions that do the work. Core modules supply
  functions in Rust; built modules get generic functions from the modules they're built from
  (Collections, Automations, Pages); connected modules get functions that call the app.
- **Install**: a module present in a workspace (or a space) with a state, `on` or `off`.

### 3.2 The registration interface (server)

One folder per core module: `apps/server/src/modules/core/<id>/mod.rs`. The folder holds the
registration (and, for new modules, the feature code; existing feature code stays where it is
and is referenced from there). `build.rs` lists the folders and writes
`$OUT_DIR/core_modules.rs`:

```rust
// generated by build.rs from apps/server/src/modules/core/*/mod.rs
#[path = "…/core/sheets/mod.rs"] pub mod sheets;
#[path = "…/core/tasks/mod.rs"]  pub mod tasks;
// …
pub static ALL: &[&'static dyn crate::modules::CoreModule] = &[&sheets::MODULE, &tasks::MODULE, …];
```

`apps/server/src/modules/core.rs` is `include!(concat!(env!("OUT_DIR"), "/core_modules.rs"));`.
`build.rs` adds `cargo:rerun-if-changed=src/modules/core` and `=../../modules`. Order in
`ALL` is the manifest's `order` field, then id, so `needs` resolve topologically without a
hand list (a test refuses a cycle). Choice: a generated list instead of `inventory`/`linkme`
link-time collection, because it needs no new crate, works the same in test binaries, and is
readable.

```rust
// apps/server/src/modules/registry.rs

/// What every core module implements. The only privilege a core module has
/// is that these functions run in this process.
pub trait CoreModule: Send + Sync + 'static {
    /// The manifest, `include_str!("../../../../../../modules/<id>.json")`.
    fn manifest_source(&self) -> &'static str;
    /// Adds everything the module brings. Called once at start-up.
    fn register(&self, module: &mut Registrar<'_>);
}

/// One module's contributions, as `register` fills them in. Every method
/// records the module's id beside what it adds, so nothing is unowned.
pub struct Registrar<'a> { module: ModuleId, into: &'a mut RegistryBuilder }

impl Registrar<'_> {
    pub fn kind(&mut self, kind: KindRegistration);            // K3's kind provider
    pub fn operation(&mut self, op: &'static ops::OpDef);      // K2's operation
    pub fn event(&mut self, event: EventRegistration);         // K3's event type
    pub fn permission(&mut self, p: &'static access::PermissionDef); // K1's catalogue
    pub fn setting(&mut self, s: &'static settings::SettingDef);
    pub fn notification(&mut self, kind: &'static str);
    pub fn search(&mut self, source: &'static dyn search::SearchSource); // K3
    pub fn export(&mut self, exporter: &'static dyn ExportPart);         // H1 reads these
    pub fn trigger(&mut self, t: automations::TriggerDef);
    pub fn action(&mut self, a: automations::ActionDef);
    pub fn setup_step(&mut self, step: &'static str);
    pub fn routes(&mut self, router: fn() -> axum::Router<AppState>); // legacy, until K2 generates
    pub fn realtime(&mut self, event_type: &'static str, audience: events::DefaultAudience); // a default K3 `Audience` per event type; every event carries its own `Audience`, so no `LiveScope`
    pub fn module_filter(&mut self, f: ModuleFilter); // e.g. channels vs messages by space kind
    pub fn maker(&mut self, m: &'static dyn chat_to_work::MessageMaker); // C2's "Make…" (C2 3.7)
    pub fn slash(&mut self, s: SlashEntry);           // C2 defines the entry shape (C2 3.7)
}

pub struct KindRegistration {
    pub kind: &'static str,                       // "sheet"
    pub provider: &'static dyn things::KindProvider, // K3: resolve, space_of, fields, title
    pub href: &'static str,                       // from the manifest's thing decl
    pub id_routes: &'static [&'static str],       // "/api/v1/sheets/{id}*": the gate resolves the space
}

pub struct EventRegistration {
    pub event_type: &'static str,                 // "sheet.updated"
    pub kind: &'static str,                       // the kind it is about
    pub payload: fn() -> schemars::Schema,
}

/// Which module a call needs, beyond the operation's own: "conversations in the
/// direct space need messages, elsewhere channels" stops being an `if` in five files.
pub struct ModuleFilter { pub module: &'static str, pub rule: fn(SpaceKind) -> Option<&'static str> }
```

The **registry** is built once in `AppState::new` and is the only place module ownership is
answered:

```rust
pub struct Registry {
    modules: Vec<RegisteredModule>,                 // core, ordered
    by_id: HashMap<ModuleId, usize>,
    operations: HashMap<&'static str, usize>,       // op name -> module
    kinds: HashMap<&'static str, usize>,
    events: HashMap<&'static str, usize>,
    parts: HashMap<String, usize>,                  // layout parts (web side, from manifests)
    permissions, settings, notifications, search, triggers, actions, routes…
}

impl Registry {
    pub fn build(core: &[&'static dyn CoreModule], extra: &[&'static dyn CoreModule]) -> Result<Self, RegistryError>;
    pub fn global() -> &'static Registry;           // OnceLock, for code without AppState
    pub fn module_of_op(&self, op: &str) -> Option<&RegisteredModule>;
    pub fn module_of_kind(&self, kind: &str) -> Option<&RegisteredModule>;
    pub fn module_of_route(&self, template: &str) -> Option<&RegisteredModule>;
    // … one per contribution kind; today's `manifest::of_*` become thin wrappers.
    pub fn router(&self) -> axum::Router<AppState>; // merges every module's routes
    pub fn describe(&self, id: &str) -> Option<&ModuleDescription>;
}
```

`build` fails (and the server refuses to start; a test fails first) when two modules claim the
same thing, a `needs` names an unknown module or forms a cycle, or a contribution names a
kind no module registers. `extra` exists for tests: the scaffold test (section 6) passes a
dummy module from the test tree.

**The description as data.** `ModuleDescription` is today's `Manifest` grown (all new fields
optional, so today's files parse unchanged):

```rust
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleDescription {
    pub format: Option<u32>,          // 2; absent = 1
    pub id: String, pub name: String, pub description: String,
    pub version: String,              // semver for built and connected; core keeps an integer, read as "N"
    pub kind: ModuleKind,
    pub switchable: bool,
    pub order: Option<u32>,
    pub needs: Vec<String>, pub works_with: Vec<String>,
    pub icon: Option<String>,
    pub permissions: Vec<PermissionDecl>,  // string (core, as today) or {key, label, description, defaults}
    pub tools: BTreeMap<String, ToolVerb>, // kept; becomes "operations" once K2 lands
    pub operations: Vec<OperationDecl>,    // connected: name, verb, input schema, permission, endpoint
    pub routes: Vec<String>,
    pub events: EventDecl, pub notifications: Vec<String>, pub search: Vec<String>,
    pub automations: AutomationDecl,
    pub things: Vec<ThingDecl>, pub links: Vec<LinkDecl>, pub embeds: Vec<EmbedDecl>,
    pub slots: Slots,                       // unchanged
    pub parts: Vec<PartDecl>,               // NEW: layout parts declared by the module (3.4)
    pub panels: Vec<PanelDecl>,             // NEW: connected modules (3.7)
    pub web: Option<WebDecl>,               // NEW: {"folder": "sheets"} for the web registration
    pub settings: Vec<String>, pub nouns: Vec<String>,
}
```

Core modules supply it from the JSON file; built modules generate it from their parts
(`module_describe`, 3.5); connected modules fetch it from the app (3.7). Every reader
(`module_list`, the Modules settings page, the Roles page, agents) reads descriptions, never
the Rust side, so the three sorts look the same.

**How the neighbours hang off it.**

| Neighbour | What it registers through M1 | Who owns the type |
| --- | --- | --- |
| K2 operations | `module.operation(&OP)`; the dispatcher asks `Registry::module_of_op` and checks the module is on where the call acts, once, before access. Web route, MCP tool, API endpoint and automation action are generated from the op, so `lib.rs` merges, `#[tool]` methods, `tool_permissions.rs` and `policy_inventory.rs` rows stop being per-module edits | K2 (`ops::OpDef`) |
| K3 kinds and events | `module.kind(...)`, `module.event(...)`; `things.rs` `KINDS`/resolvers, `EVENT_TYPES`, outbox kinds and realtime visibility are read from the registry | K3 (`things::KindProvider`, `events`) |
| K3 search | `module.search(&SOURCE)`; `SearchKind` becomes the registered list | K3 |
| K1 permissions | `module.permission(&DEF)`; the role editor groups by module from the registry, not `PERMISSION_MODULES` (`policy.rs:589`) or `roles.ts:8` | K1 |
| H1 export | `module.export(&PART)`: each module writes its own part of a workspace archive | H1 reads, M1 provides the slot |
| Settings | `module.setting(&DEF)`; `settings::REGISTRY` is assembled | today's `settings.rs` |
| Web parts | from the manifest (`slots`, `parts`, `panels`) and the web registration (3.4) | M1 |

Until K1, K2 and K3 land, core modules register their legacy pieces through `routes(...)`,
`setting(...)` and the existing static lists; each neighbour's slice swaps the static list for
the registry reading in its own pull request.

### 3.3 Module state: a table instead of bits

```sql
-- apps/server/migrations/2026MMDDhhmmss_module_installs.sql
CREATE TABLE module_installs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- NULL: the workspace's own switch. A space row can only narrow (turn
    -- off) a core module, or install a built or connected module there.
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL CHECK (
        module_key ~ '^[a-z][a-z0-9_]{0,39}(\.[a-z][a-z0-9_]{0,39})?$'),
    -- For built and connected modules: their definition. NULL for core.
    module_id UUID REFERENCES modules(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('on', 'off')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    changed_by_actor_id UUID REFERENCES actors(id),
    via_app_actor_id UUID REFERENCES actors(id),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((module_key LIKE '%.%') = (module_id IS NOT NULL))
);
CREATE UNIQUE INDEX module_installs_workspace
    ON module_installs (workspace_id, module_key) WHERE space_id IS NULL;
CREATE UNIQUE INDEX module_installs_space
    ON module_installs (workspace_id, space_id, module_key) WHERE space_id IS NOT NULL;

-- Backfill, in the same migration: every switched-off module today.
INSERT INTO module_installs (id, workspace_id, module_key, state, version, changed_at)
SELECT gen_random_uuid(), organization_id, substring(key FROM 9), 'off', version, updated_at
FROM settings
WHERE scope = 'workspace' AND key LIKE 'modules.%' AND value = 'false'::jsonb
ON CONFLICT DO NOTHING;
```

(Order of migrations: M1.2 creates `module_installs` without `module_id` and its CHECK; M1.6
adds `module_id` with the CHECK when it creates `modules`, and M1.9 adds
`modules.package_install_id` when it creates `package_installs`. Each is a separate,
timestamp-named file, so none assumes another pending one has run. Column names follow the brief: `workspace_id`, `space_id` referencing today's `projects`
until K4's rename. Two partial indexes rather than `NULLS NOT DISTINCT`, so nothing depends on
the server's PostgreSQL version.) A core module with no row is on, as today. Choice: keep "no
row means on" rather than writing fifteen rows per workspace, because it matches today's
semantics and needs no backfill for new workspaces or new core modules.

Also in that migration: widen `workspace_roles.modules_off` (drop the anonymous CHECK, add a
named one allowing 200 entries matching the `module_key` pattern). K1 later replaces
`modules_off` with a module-use grant in role data; M1 only needs the column to hold any id.

**In memory.** `ModuleSet` stays `Copy` and keeps its API, but becomes a bit set over the
**registry's** core index with no fixed cap: `struct ModuleSet([u64; CORE_WORDS])`, where
`build.rs` emits `CORE_WORDS = ceil(core count / 64)`. That removes `MAX_CORE` and the 32-bit
limit without touching the 45 call sites. Built and connected modules are not in `ModuleSet`;
they are in a per-workspace value the registry-driven checks use:

```rust
pub struct WorkspaceModules {
    pub core: ModuleSet,                        // switches narrowed, as today
    pub installed: Arc<[InstalledModule]>,      // built and connected, with state and space
    pub version: i64,                           // max(module_installs.version) for caching
}
pub async fn workspace_modules(conn, workspace_id) -> Result<WorkspaceModules, AppError>;
pub async fn for_account(conn, workspace_id, space_id: Option<Uuid>, account_id) -> Result<WorkspaceModules, AppError>;
impl WorkspaceModules {
    pub fn is_on(&self, module: &str, space: Option<Uuid>) -> bool; // core or installed
    pub fn object_on(&self, part_kind: &str, object_id: Uuid) -> bool; // built-module parts (3.5)
}
```

Choice: two representations (bits for core, a list for the rest) instead of one string set,
because the hot paths (tool lists, notification reads) only ever ask about core modules and
keep their cost, while built and connected modules are always looked up through the
registry, never by literal.

**Per space.** End state says a space has its own modules. Rule: a space row may turn a core
module **off** in that space (never on if the workspace has it off), and is where a built
module installs when it belongs to one space. `in_project` and `allows_conversations_in`
become data: K4's space kinds carry `modules_allowed` (the workspace space: `pages`;
personal: everything but `projects`), and the Channels/Messages rule is one
`ModuleFilter` registered by the Messages module.

**Caching.** The gate's process-local cache (`gate.rs:53-57`) keys on workspace and keeps a
10-second limit. Every switch write emits a `module.switched` event on K3's log and calls
`realtime::publish_ephemeral(Invalidate)` (K5), which clears caches in every process. Until K5,
the TTL stays.

### 3.4 The web side

**One registration per web module**, `apps/web/src/<folder>/module.ts` (the folder named in the
manifest's `web.folder`; `pages` lives in `memory/`, `tasks` in `work-items/`):

```ts
// apps/web/src/sheets/module.ts. No static imports except types and defineModule.
import { defineModule } from '../modules/define'

export default defineModule({
  id: 'sheets',
  routes: [{ path: '/projects/:projectId/sheets/*', shell: 'project', load: () => import('./SheetsArea') }],
  sidebar: [{ scope: 'project', part: 'area.sheets', icon: 'table', label: 'Sheets', to: ({ base }) => `${base}/sheets`, order: 50 }],
  palette: [{ id: 'p:sheets', scope: 'project', label: 'Sheets', icon: 'table',
              keywords: ['spreadsheet', 'formulas'], to: ({ projectPath }) => projectPath('/sheets') }],
  embeds: [{ match: '/projects/:projectId/sheets/:sheetId', load: () => import('./SheetEmbed'),
             picker: { label: 'Sheet range', icon: 'table', part: 'area.sheets', load: () => import('./SheetEmbedPicker') } }],
  live: [{ kinds: ['sheet.updated'], load: () => import('./live') }],   // W1's handlers by event kind (W1 3.6), not cache keys
  settings: [],   // SettingsPage rows, as settings/registry.ts takes them
  tabs: [],       // { scope: 'project' | 'thing:<kind>', part, label, load }
  composer: [],   // composer extras: { part, label, icon, load }
  things: {},     // W1: { [kind]: { href, detail?, chip? } }
  capture: [],    // W1: quick capture and share-in entries { id, label, form }
  home: [], slash: [], shortcuts: [], setup: [],
})
```

**One registration contract (with W1).** M1 owns `defineModule`, `modules/registry.ts`, the
`module.ts` files and the generated `parts.ts` (M1.4, M1.5); W1 draws the shell from them
(W1.13) and does the embed dispatch (W1.26), and defines no second registration. So
`defineModule` carries the fields W1's shell needs (W1 3.4): routes with a `handle`
(`space: 'home' | 'direct' | 'space' | 'none'`, `layout`, `part`, `title`) and paths relative to
their `scope` (`workspace` or `space`, under `/w/<slug>/...`); navigation entries with `phone?:
'tab' | 'drawer'` and a lazy `badge?`; `things`; `capture`; `live` handlers by event kind. The
`/projects/...` paths in the example are today's and move to relative paths with W1.14.

`apps/web/src/modules/registry.ts` collects them with
`import.meta.glob('../*/module.ts', { eager: true })`, checks each `id` against
`virtual:modules/names`, and exposes `routes()`, `sidebarRows(scope)`, `paletteEntries(scope)`,
`settingsPages()`, `embedFor(href)`, `liveHandlers()`, `tabs(scope)`, `composerExtras()`. The
shell reads these: `App.tsx`'s module routes become a loop (the non-module routes, such as
sign-in, stay written out), `ProjectSidebar.tsx:96-139` maps `sidebarRows('project')`,
`CommandPalette.tsx:133-145` maps `paletteEntries`, `settings/registry.ts` concatenates module
pages, `PageEmbed.tsx` dispatches on `embedFor`, `LivePage.tsx`'s toolbar lists registered
pickers, `liveDocuments.ts` runs registered handlers. The shapes copy `settings/registry.ts`'s
pattern (I report F6).

**Lazy loading.** Each `module.ts` holds only data and `() => import()` thunks; a test reads
every `module.ts` source and fails on a static non-type import other than `defineModule`, so
nothing pulls a module's code into the first load. The registrations themselves are part of
the initial load (about 15 small objects); the budget test counts them. Panels, built-module
screens and the frame host are their own lazy chunks.

**Layout parts.** `registry.json` stays the layout registry and every existing part stays
there. Two extensions:

1. A manifest may declare parts itself (`parts: [{ id, group, label, description, default,
   presets: ["writing", "research"] }]`). The Vite plugin and the server's `layouts.rs`
   (`:125`) merge manifest parts into the registry at build and start-up. `presets` adds the
   part to those presets' `only` lists, so a new part no longer means editing 14 presets.
   `parts.ts` is generated from the merged registry by the Vite plugin
   (`virtual:experience/parts`) instead of copied by hand.
2. Parts of built and connected modules are dynamic: `module.<module_key>` (the module's
   whole navigation) and `panel.<module_key>.<panel>` (one panel). `layouts.rs` accepts ids
   matching those patterns for modules installed in the workspace. An `only` preset shows a
   dynamic part unless a layout hides it: an admin installed it on purpose.

**Built and connected modules on the web** have no `module.ts`. The modules response
(`GET /api/v1/organizations/{id}/modules`, extended) carries their navigation entries and
panels; generic components draw them: `modules/BuiltNav.tsx` (sidebar rows that open a
collection view, a page or a form), `modules/panels/PanelHost.tsx` (blocks, lazy; it fetches,
signs and times out, and renders through W1.25's `PanelBlocks({blocks, onAction})`), and
`modules/panels/FrameHost.tsx` (frames, lazy). They are registered once, by the core
`modules` web registration.

**Adding a core module, before and after.**

| | Before (section 2.4) | After M1 and its neighbours |
| --- | --- | --- |
| Server | about 20 edits in 15 files plus 2 migrations widening checks | `apps/server/src/modules/core/<id>/` (registration and code), new migration files for its own tables |
| Web | about 11 edits in 10 files | `apps/web/src/<folder>/` with `module.ts` |
| Shared | manifest, presets, 5 templates | `modules/<id>.json` |
| Edits elsewhere | about 31 | none. Presets and templates list modules to turn **off**, so a new module is on in every preset unless one says otherwise |

The "after" for operations, kinds, events, permissions and search depends on K2, K3 and K1
landing; M1's slices move each hand list to the registry as soon as its owner's type exists,
and move the rest (routes, settings, notifications, triggers, parts, web) straight away.
Presets and templates: `modules/presets.json` and template `modules` lists change meaning from
"on" lists to "off" lists in format 2 (a format-1 file is still read as an on-list, which
keeps saved templates working).

### 3.5 Built modules

A built module is a named bundle of data that installs into a space or a workspace. It holds
**parts**, each an existing object owned by its own module: collections (with their fields
and views), forms, automations, roles, pages and page templates, navigation entries, layout
choices, skills (pages with a skill description), agent profiles (as templates, 3.6). It
needs the modules its parts come from (a module holding a collection needs `collections`).
Switching it off hides every part, refuses operations on them, and leaves the data; switching
it on brings everything back. Removing it detaches or archives the parts; nothing is deleted.

```sql
CREATE TABLE modules (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- The space it belongs to; NULL for a workspace-wide module.
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL CHECK (module_key ~ '^[a-z][a-z0-9_]{0,39}\.[a-z][a-z0-9_]{0,39}$'),
    kind TEXT NOT NULL CHECK (kind IN ('built', 'connected')),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60 AND name = btrim(name)),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    icon TEXT CHECK (icon IS NULL OR icon ~ '^[a-z0-9-]{1,40}$'),
    -- The description (manifest format 2): generated for built modules,
    -- the fetched manifest for connected ones.
    description_doc JSONB NOT NULL CHECK (jsonb_typeof(description_doc) = 'object'
                                          AND octet_length(description_doc::text) <= 262144),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),   -- expected version for writes
    package_install_id UUID REFERENCES package_installs(id) ON DELETE SET NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX modules_key ON modules (workspace_id, module_key) WHERE archived_at IS NULL;

CREATE TABLE module_parts (
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    part_kind TEXT NOT NULL CHECK (part_kind IN ('collection', 'form', 'automation', 'role',
        'page', 'page_template', 'nav', 'layout', 'skill', 'agent_template', 'channel')),
    object_id UUID NOT NULL,
    -- Stable key inside the package it came from (upgrades match on it).
    package_key TEXT CHECK (package_key IS NULL OR package_key ~ '^[a-z][a-z0-9_]{0,63}$'),
    -- The part's portable form's hash when installed; a different hash now
    -- means "changed here" during an upgrade.
    installed_hash TEXT,
    position INT NOT NULL DEFAULT 0,
    PRIMARY KEY (module_id, part_kind, object_id),
    UNIQUE (part_kind, object_id)          -- one module owns a part
);

CREATE TABLE module_nav (
    id UUID PRIMARY KEY,
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 40),
    icon TEXT,
    -- {"kind":"collection_view","collection":…,"view":…} | {"kind":"page","page":…}
    -- | {"kind":"form","form":…} | {"kind":"panel","panel":"…"}
    target JSONB NOT NULL CHECK (jsonb_typeof(target) = 'object'),
    position INT NOT NULL DEFAULT 0
);
```

A `module_nav` row is itself a `module_parts` row (`part_kind = 'nav'`), so the "one owner"
rule covers it. Choice: one link table for ownership instead of a `module_id` column on every
T1 and C1 table, because it adds nothing to tables other workstreams own, and the filter is
one generated SQL fragment:

```rust
/// True when the object isn't a part of a built module that is off for the reader.
/// Generated once, like `notification_switches_sql`.
pub fn object_on_sql(part_kind: &str, id_column: &str, reader_column: &str) -> String;
```

T1's collection list, record query and search, and the dispatcher's scope check, add this
fragment. A collection that is in no module stays exactly as today.

**Operations** (all K2 operations, each with an agent tool; names follow `<thing>_<verb>`):

| Operation | Input | Permission (K1) | Effect |
| --- | --- | --- | --- |
| `module_list` (exists, extended) | `space?` | read | Every module (core, built, connected) with state, source and parts count |
| `module_describe` | `module` | read | The module's description and its parts in portable form: the shape `package_export` writes and `module_write` takes |
| `module_write` | `module?`, `name`, `description`, `icon`, `space?`, `parts[]` (by id, or new definitions inline as `collection_describe` gives them), `nav[]`, `expectedRevision`, `clientRequestId` | `modules.build` | Create or change a built module. Parts given by definition are created through their own modules' operations (`collection_write`, `automation_write`, `page_write`), as the caller, in one transaction |
| `module_part_add` / `module_part_remove` | `module`, `part {kind, id}` | `modules.build` | Attach an existing object, or detach one (it stays, as a plain object) |
| `module_switch` (today's `PUT …/modules`) | `switches {key: on}`, `space?`, `preset?` | `modules.manage` | Writes `module_installs` rows, audited, emits `module.switched` |
| `module_remove` | `module`, `parts: "archive" | "keep"` | `modules.manage` | Archives the module. `archive` archives its parts (hidden, restorable); `keep` detaches them so they live on as plain collections and pages |
| `module_restore` | `module` | `modules.manage` | Undoes `module_remove` with `archive` |

Two new permissions, `modules.build` (make and change built modules) and `modules.manage`
(switch, remove, install packages and connected modules), held by owners and admins by
default and grantable by role to anyone, agents included (K1's catalogue).

Events: `module.created`, `module.updated`, `module.switched`, `module.removed`, each about the
module as a thing (kind `module`, registered by the core `modules` module).

**In the app.** Settings › Modules lists all three sorts. "New module" opens a builder: name and
icon, then add parts (an existing collection, or a new one from the collection builder), then
navigation entries, then "save as package". An agent does the same with `module_write`
("make me a playtest tracker" in one call, or `package_preview` of a drafted package first).

### 3.6 Packages

A package is a file that bundles a setup. It never holds secrets, credentials, tokens, member
lists or content beyond what the author put in on purpose (page templates and starter pages).

**Format** (`agent-workspace-package/1`): a zip with a `package.json` at the root, or the same
document as one JSON file with every part inline (what agents write and the API takes).

```
package.json                     the manifest (below)
switches.json                    core modules to turn off (format 2: off-lists)
modules/<key>.json               built module definitions: name, icon, parts by key, nav
collections/<key>.json           collection_describe's definition, links as "@<key>"
forms/<key>.json                 form definition, without a token
automations/<key>.json           trigger, conditions, actions; outbound URLs flagged
roles/<key>.json                 name, base role, permissions, modules off, defaults
layouts/<key>.json               preset id, part changes, words
channels.json                    channel names and topics
pages/<key>.md                   Markdown with front matter: title, template: true|false, skill
skills/<name>/SKILL.md (+files)  Agent Skills, installed as skill pages
agents/<key>.json                agent templates: name, purpose, instructions, suggested role, skills
```

```json
{
  "format": "agent-workspace-package/1",
  "id": "studio.game_studio",
  "version": "1.2.0",
  "name": "Game studio",
  "description": "Bugs with build and platform fields, playtest feedback, builds and release notes.",
  "publisher": { "name": "Studio Tools", "url": "https://example.org" },
  "target": "space",
  "requires": { "contract": ">=2", "modules": ["tasks", "collections", "pages", "automations"] },
  "contents": [
    { "kind": "module", "key": "playtest", "path": "modules/playtest.json" },
    { "kind": "collection", "key": "feedback", "path": "collections/feedback.json" },
    { "kind": "collection", "key": "builds", "path": "collections/builds.json" },
    { "kind": "form", "key": "feedback_form", "path": "forms/feedback_form.json" },
    { "kind": "automation", "key": "triage", "path": "automations/triage.json" },
    { "kind": "role", "key": "tester", "path": "roles/tester.json" },
    { "kind": "page", "key": "release_notes_template", "path": "pages/release_notes_template.md" },
    { "kind": "agent", "key": "triage_agent", "path": "agents/triage_agent.json" }
  ],
  "needsAdmin": ["form:feedback_form", "role:tester"],
  "digest": "sha256:…"
}
```

`target` is `space` (installs into one space, creating it if asked) or `workspace` (also sets
switches, roles, the default role and the default layout, as workspace templates do today).
Cross-references use `@<key>`; an installer resolves them to ids. A digest over the canonical
contents lets a directory and an upgrade check what they're installing.

**Agents in packages.** An `agents/<key>.json` is an agent **template**, not an account: name,
avatar, purpose, instructions, the five plain parts from the end state (what it's for, where it
works, what it may do, what it must not do, when it asks a person), a suggested role, and the
skills it should hold. Installing shows it in "Add an agent › create one" (A2) as a starting
point and installs its skills as skill pages. It never creates an account, a connection or a
credential, and never names a host, backend or model (principle 6).

**Tables.**

```sql
CREATE TABLE packages (            -- packages this workspace has seen (installed or uploaded)
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    package_key TEXT NOT NULL,      -- "studio.game_studio"
    version TEXT NOT NULL,          -- semver
    name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
    document JSONB NOT NULL CHECK (octet_length(document::text) <= 5242880),  -- inline form
    digest TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('builtin', 'directory', 'upload', 'export', 'legacy_template')),
    source_ref TEXT,                -- directory entry or URL
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, package_key, version)
);

CREATE TABLE package_installs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    package_id UUID NOT NULL REFERENCES packages(id),
    state TEXT NOT NULL CHECK (state IN ('installing', 'installed', 'failed', 'superseded', 'removed')),
    plan JSONB NOT NULL,            -- the preview the person approved
    plan_hash TEXT NOT NULL,
    summary TEXT[] NOT NULL DEFAULT '{}',
    installed_by_actor_id UUID NOT NULL REFERENCES actors(id),
    via_app_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE TABLE package_objects (      -- what each install made, for upgrades and removal
    install_id UUID NOT NULL REFERENCES package_installs(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL,
    package_key TEXT NOT NULL,      -- the content's key, "feedback"
    object_kind TEXT NOT NULL,
    object_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('created', 'reused', 'updated')),
    installed_hash TEXT NOT NULL,
    PRIMARY KEY (install_id, package_key, object_kind)
);
```

**Operations.**

| Operation | Input | Permission | Effect |
| --- | --- | --- | --- |
| `package_export` | `from: {space} | {module} | {workspace}`, `include: {pages, skills, agents, starterContent}` | `modules.build` | Writes a package from a live setup through each part's portable form, runs the no-secrets check, returns it (and stores it with `source = 'export'`) |
| `package_preview` | `package` (inline document, uploaded file id, or directory entry), `target {space?, newSpace?}` | read | The plan: `[{action: create|reuse|update|conflict|skip, kind, key, name, needsAdmin, reason}]`, modules it switches, permissions roles would get, data that leaves the workspace (outbound automations), and `planHash` |
| `package_install` | `package`, `target`, `planHash`, `choices {key: "keep_mine"|"take_theirs"}`, `clientRequestId` | `modules.manage`; `needsAdmin` items need `roles.manage` or `settings.manage` as today | Applies the plan. Refused if the plan changed since the preview (`planHash`) |
| `package_upgrade_preview` / `package_upgrade` | `install`, new `package` | as above | Three-way plan and apply (below) |
| `package_list` | `space?` | read | Installed packages with versions and available upgrades |
| `package_remove` | `install`, `parts: "archive" | "keep"` | `modules.manage` | As `module_remove`, for everything the install created |

Install runs in one transaction for anything up to the sizes templates allow today (10
collections, 20 roles, 20 channels, `workspace_templates.rs:55-58`; packages raise these to 50
collections and 200 parts in total). Each part is idempotent by `(install_id, package_key)`,
so a retried install with the same `clientRequestId` changes nothing. Install never deletes:
existing objects with the same name are reused (as templates do today, `:16-19`) and listed as
`reuse` in the preview.

**Upgrade rules.** For each content key: base = `installed_hash`, local = the object's portable
form now, incoming = the new version.

- unchanged locally: apply the new version;
- changed locally, unchanged upstream: keep;
- changed both: conflict, default "keep mine", the person can take theirs per part;
- added upstream: create; added fields and views are always added;
- removed upstream: detach (the object stays, marked "no longer in the package"); never delete;
- a field whose type changes upstream: a new field beside the old one (types never change,
  `fields.rs`), values not moved.

A new major version is shown as such but follows the same rules: nothing is deleted.

**Where templates fit.** Workspace templates become `target: "workspace"` packages and
project templates (`work_items/project_templates.rs`) become `target: "space"` packages:

- the six built-in templates move to `modules/packages/<id>/package.json` (same contents, in
  package form) and show in the directory as "built in";
- `TemplateDefinition` (`workspace_templates.rs:69`) is read as a package by a converter
  (`packages::from_template`), so saved `workspace_templates` rows and `project_templates`
  rows keep working without a data migration; they list as packages with
  `source = 'legacy_template'`;
- `workspace_template_list` and the apply route stay as names for `package_list` and
  `package_install` for a release, then the old routes and tools go (slice M1.12 names when);
- `workspace_template_uses` is read beside `package_installs` by the setup path until the
  contract step copies it in.

### 3.7 Connected modules

An outside app becomes a module by publishing a manifest at its own address. Nothing it
supplies runs inside Workspace's server or page.

**Lifecycle.**

1. An admin enters the app's manifest URL (or picks it in the directory). Workspace fetches
   it through the outbound address check (`outbound.rs`), validates it, and shows a preview:
   who publishes it, the account it will have, the scopes it asks for and why, the kinds,
   operations and events it adds, its panels, and the frame origins it uses.
2. The admin approves some or all scopes (a scope marked `required` can't be left out; then
   the install doesn't happen). Conversation-reading scopes follow the consent model from the
   end state: shared channels by an admin; private conversations and DMs only by their
   participants, per conversation, later.
3. Workspace creates the **app account** (K1, kind `app`) with a generated role holding exactly
   the approved permissions, a `modules` row (`kind = 'connected'`), a `module_installs` row,
   and a `module_apps` row (below). It sends the app a one-time install code to its `install`
   URL; the app exchanges it at Workspace's token endpoint for its credentials (K1's client
   credentials) and the per-install signing secrets. Choice: a code exchange rather than
   posting secrets to the app's URL, because the app proves it controls the manifest's
   address before it gets anything.
4. Re-fetching the manifest (on a schedule and on request) shows changes; new scopes need
   approval again before they apply. Revoke disables the account, deletes the secrets, turns the
   module off; its things stay, readable and archived.

```sql
CREATE TABLE module_apps (
    module_id UUID PRIMARY KEY REFERENCES modules(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id),    -- K1's app account
    manifest_url TEXT NOT NULL,
    base_url TEXT NOT NULL,
    frame_origins TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(frame_origins) <= 4),
    scopes_approved TEXT[] NOT NULL DEFAULT '{}',
    scopes_requested TEXT[] NOT NULL DEFAULT '{}',
    outbound_secret_id UUID NOT NULL,   -- in the secrets store, never in this row
    inbound_secret_id UUID NOT NULL,
    approved_by_actor_id UUID NOT NULL REFERENCES actors(id),
    approved_at TIMESTAMPTZ NOT NULL,
    manifest_fetched_at TIMESTAMPTZ NOT NULL,
    manifest_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'active', 'suspended', 'revoked'))
);
```

**The manifest** (served by the app, format `agent-workspace-module/1`; the description of
3.2 plus the parts only connected modules have):

```json
{
  "format": "agent-workspace-module/1",
  "id": "acme.builds",
  "name": "Builds",
  "version": "1.4.0",
  "description": "Shows each build of your game and its status, and links builds to tasks.",
  "publisher": { "name": "Acme", "url": "https://builds.example.com" },
  "baseUrl": "https://builds.example.com/workspace/",
  "install": "install",
  "account": { "name": "Builds", "avatar": "https://builds.example.com/icon.png" },
  "scopes": [
    { "scope": "tasks.view", "why": "To link builds to the tasks they fix.", "required": true },
    { "scope": "tasks.edit", "why": "To mark tasks fixed when a build ships." },
    { "scope": "events:task.updated", "why": "To notice tasks moved to Done." }
  ],
  "kinds": [{ "kind": "acme.build", "one": "build", "many": "builds",
              "fields": [{ "id": "status", "label": "Status", "type": "single_choice",
                           "options": ["queued", "running", "passed", "failed"] }] }],
  "operations": [{ "name": "acme_build_rerun", "verb": "change", "permission": "tasks.edit",
                   "input": { "type": "object", "properties": { "build": { "type": "string" } },
                              "required": ["build"] },
                   "endpoint": "ops/rerun" }],
  "events": { "emits": [{ "type": "acme.build.finished", "kind": "acme.build" }] },
  "panels": [{ "id": "builds", "slot": "space.tab", "title": "Builds", "entry": "panels/builds" }],
  "frameOrigins": ["https://builds.example.com"]
}
```

**Kinds.** A connected kind's things are stored by Workspace in K3's thing registry as rows
the app writes through `thing_upsert` (title, status, fields, space, `externalUrl`), so links,
search, embeds, comments and history work on day one. Choice: Workspace stores the thing
envelope rather than asking the app on every read, because search, links and access must
work when the app is slow or gone.

**Operations** are calls out. K2's registry gains one handler type, `RemoteOperation`: the
dispatcher checks permission and modules as for any operation, then `POST {baseUrl}{endpoint}`
with the input and the caller (account id, requested-by), signed with Standard Webhooks
headers (`webhook-id`, `webhook-timestamp`, `webhook-signature`) using the outbound secret,
timeout 30 s, the result validated against the declared schema. The app account acting in
Workspace uses ordinary operations with its own credentials, like any account.

**Events in.** `POST /api/v1/modules/{module_id}/events`, signed with the inbound secret
(Standard Webhooks; a timestamp older than five minutes or a reused `webhook-id` is refused).
Each event must be a type the manifest declares and about a thing of a kind it owns; it goes
into K3's log as any event does, so automations, webhooks, notifications and agents can follow
it. Events out to the app use K3's delivery (A2's subscription machinery), filtered to its
approved `events:` scopes.

**Panels: the block format.** A panel is a small document of blocks Workspace draws with its
own pieces. Format `aw-panel/1`:

```json
{
  "format": "aw-panel/1",
  "title": "Builds",
  "blocks": [
    { "type": "status", "state": "working", "label": "Build 214 is running", "progress": 0.6 },
    { "type": "fields", "items": [
        { "label": "Branch", "value": "main" },
        { "label": "Started", "value": "2026-11-14T09:12:00Z", "kind": "time" },
        { "label": "Task", "value": { "kind": "task", "id": "…" }, "kind": "thing" } ] },
    { "type": "table", "columns": [
        { "id": "build", "label": "Build" }, { "id": "status", "label": "Status", "kind": "badge" } ],
      "rows": [{ "id": "214", "cells": { "build": "214", "status": { "text": "running", "tone": "agent" } } }],
      "rowAction": { "id": "open_build" } },
    { "type": "chart", "kind": "bar", "x": ["Mon", "Tue", "Wed"],
      "series": [{ "name": "Failed", "values": [2, 0, 1] }] },
    { "type": "buttons", "items": [
        { "label": "Rerun", "action": { "id": "rerun" }, "style": "primary",
          "confirm": { "title": "Rerun build 214?" } } ] }
  ],
  "state": "opaque-string-up-to-8KB",
  "refresh": { "afterSeconds": 15 }
}
```

The blocks, all of them:

| Block | Fields | Wide | Narrow |
| --- | --- | --- | --- |
| `text` | `text` (plain text with `**bold**`, `*italic*`, `` `code` ``, links to `https:` or app paths), `style: body|heading|muted|callout`, `tone` | as written | the same |
| `fields` | `items[] {label, value, kind: text|time|date|number|badge|link|thing|person}` | two columns | stacked |
| `form` | `id`, `schema` (JSON Schema 2020-12 subset: object of string, number, integer, boolean, enum, array of enum; `title`, `description`, `default`, `minimum`, `maxLength`, `pattern`), `ui {order[], widgets {field: text|textarea|select|radio|checkbox|multiselect}}`, `values`, `submit {label, action}`, `cancel?` | labelled fields, errors under each | the same, stacked |
| `list` | `items[] {id, title, subtitle?, status?, meta?, action?}`, `empty`, `more {action}` | rows | rows, wrapping |
| `table` | `columns[] {id, label, kind}`, `rows[] {id, cells}`, `rowAction?` | columns | a card per row, values labelled (the table piece's existing behaviour) |
| `buttons` | `items[] {label, action, style: primary|secondary|danger, confirm?}` | a row | one menu when more than two |
| `status` | `state: ok|working|waiting|error|unknown`, `label`, `detail?`, `progress?` | a line with a dot | the same |
| `chart` | `kind: bar|line|number`, `x[]`, `series[] {name, values[]}` or `value`, `unit` | drawn | drawn, scrolls inside itself |
| `section` | `title?`, `blocks[]` (no nesting deeper than one) | a group | the same |

Limits: 200 blocks, 64 KB per document, 500 table rows, 50 form fields. A form may not ask for a
secret: the validator refuses `format: "password"`, `writeOnly`, and field names matching
`key|secret|token|password` (the app must use a frame for those). Unknown block types draw as
"This panel needs a newer Workspace" rather than failing the panel. Text is never HTML.

**Round trip.** Workspace asks for a panel with `POST {baseUrl}{entry}` and sends actions to
the same URL, signed as operations are:

```json
{ "install": "…", "workspace": { "id": "…" }, "space": { "id": "…", "name": "Game" },
  "viewer": { "account": "…", "locale": "en-GB", "timezone": "Europe/Berlin" },
  "context": { "slot": "space.tab", "thing": null },
  "state": "…as the last response gave it…",
  "action": { "id": "rerun", "values": { } },
  "requestId": "…" }
```

The app answers with one of: `{ "panel": {...} }` (draw this), `{ "errors": {field: message} }`
(show under the form), `{ "frame": {"url": "...", "title": "...", "height": 480} }` (open a frame,
below), `{ "done": { "message": "...", "result": {...} } }` (the flow is finished; `result` is
handed to whoever opened the panel, such as A2's creation flow), or `{ "close": true }`. Panel
requests time out after 10 s with "The app didn't answer", and the person can retry. Rate limit:
per install, 10 panel requests a second burst and 120 a minute per viewer (K5's shared
limiter).

**Slots** a panel names: `space.tab`, `thing.side:<kind>`, `home.card`, `settings.module`,
`conversation.tab` (a channel tab beside C1's document tabs, drawn in C2's header),
`agents.create`, `agents.detail`. Each panel becomes a layout part `panel.<module>.<id>`, so
layouts and roles hide it like anything else.

**Frames, the escape hatch.** For secrets (a model key, a host login) and anything blocks can't
draw. The rules:

- the frame shows only an origin listed in the manifest's `frameOrigins` and approved at
  install;
- the app's page loads inside a Workspace-served host page, `/panel-frame/{launch}`, served by
  the `api` role (it needs a header per launch, so it is not part of the static app; R31), whose
  response has its own CSP: `default-src 'none'; frame-src <that origin>; script-src 'none';
  style-src 'self'`. The outer app keeps `frame-src 'self'` (the global header at
  `lib.rs:256` grows only that). So no other origin can ever be framed, the host page runs no
  script, and the CSP lists one origin per response, never another workspace's apps;
- the inner `<iframe>` has `sandbox="allow-scripts allow-forms allow-same-origin
  allow-popups allow-popups-to-escape-sandbox"` (same-origin here means the app's own origin,
  never Workspace's), `referrerpolicy="no-referrer"`, and `allow=""` (no camera, microphone or
  other features). No `allow-top-navigation`;
- the URL carries a launch token: signed by Workspace, audience the app, single use, valid five
  minutes, naming the install, the viewer's account and the panel. The app redeems it with a
  signed server-to-server call. No Workspace cookie or token ever reaches the frame;
- messages go by `postMessage`, checked both ways (`event.source` is the frame's window and
  `event.origin` the approved origin):

| Direction | Message | Meaning |
| --- | --- | --- |
| app to Workspace | `{"aw": 1, "type": "ready"}` | loaded |
| app to Workspace | `{"aw": 1, "type": "resize", "height": 640}` | height in CSS pixels, capped at 2000 |
| app to Workspace | `{"aw": 1, "type": "done", "ref": "…"}` | finished; Workspace then asks the app server-to-server for the result by `ref`, so nothing the page says is trusted on its own |
| app to Workspace | `{"aw": 1, "type": "cancel"}` | the person backed out |
| Workspace to app | `{"aw": 1, "type": "context", "theme": "dark", "locale": "en-GB"}` | look and language |

**MCP Apps: bridge later, stay separate now.** MCP Apps (`io.modelcontextprotocol/ui`) let a
tool name a `ui://` HTML resource that a host draws in a sandboxed iframe, talking JSON-RPC
over `postMessage` (`ui/initialize`, then `tools/call` and `resources/read` from the view).
It is the right standard for an app's UI inside someone else's chat client, and it is the
frame model: the app's HTML and script run in the host's page. Panels choose blocks first
because blocks need no app code in Workspace's page, draw natively on a phone and in the
product's look, and can be read by agents and tests. Decision:

- **Blocks:** our own format. MCP Apps has no declarative block format to adopt.
- **Frames:** keep our small protocol, named so a bridge is cheap: a later slice can let a
  connected module declare `"mcpApp": {"server": "...", "resource": "ui://..."}` for a frame
  panel, and Workspace then acts as an MCP Apps host for that frame, behind the same frame
  host page and sandbox.
- **Outward:** Workspace's own MCP server may later serve its panels and forms as MCP Apps to
  other chat clients (R2's "later"), which is K2's protocol work.

**What a connected module can never do.** Run code in the server or in Workspace's page; read
or write beyond its approved scopes (every call is checked against the app account's current
role, as for anyone); act as a person (that is delegation, a separate OAuth grant the person
makes); see another workspace; receive any secret Workspace holds; frame an origin it didn't
declare; draw HTML; reach a private address on a hosted server (the outbound check; an
operator of a self-hosted server can allow private addresses as today); read private
conversations or DMs without their participants' consent; change its own scopes without a new
approval; or survive revocation (its credentials and secrets are deleted at once).

### 3.8 The first connected module: an agent host's creation flow

R21: a host makes agents; Workspace never does and never knows a host's steps. Workspace's whole
part in "create one" is (1) list what linked hosts **offer**, (2) draw the host's own steps,
(3) receive the result, (4) give the account a role here. M1 provides (1) to (3) generically:

- **Offers.** A host's `describe` returns `offers: [{id, title, summary, start}]`, where `start`
  is a panel (this block format, `entry`) or a framed page from an approved origin. Workspace
  does not know what an offer is ("new agent from a template", "copy of an agent", "describe it
  in a sentence"); the host names them. Each offer is drawn as a choice in the `agents.create`
  slot.
- **Steps.** Whatever the host answers, drawn by the round trip above; a builder, templates,
  backends, authority on the machine, proactivity and model keys are all the host's panels or
  frames. The blocks above are enough for these steps (form, list, fields, table, status,
  buttons, section, plus a frame for secrets); nothing here is specific to agents.
- **Result.** `done.result` is handed to A2's generic `agent_register` operation (the host
  proposes an account with a profile, an outside reference and how it is reached; a person
  with the permission approves it and picks its role and spaces).
- **Actions on an agent's page.** Instead of a fixed lifecycle list, a host publishes
  `actions: [{id, label, needs: confirm | panel, danger?}]` per agent (in `describe`, refreshed
  with the agent's profile notice). Workspace draws them in the `agents.detail` slot as buttons;
  `confirm` sends the action straight back; `panel` opens a panel round trip for it. Workspace
  stores nothing about what an action does.

A2 owns "Add an agent › create one" and the agent page. It asks M1 for every offer of every
enabled host (`offers_for_slot('agents.create')`), shows them as choices, opens the chosen one,
and when it returns `done`, calls `agent_register`. Workspace knows only the slot, the panel
format, and the shape of `result`. Nothing in Workspace names a vendor.

What any agent host would publish (this is the shape an Agent OS connector, a hosted agent
service or anyone else uses; no name of any of them appears in Workspace):

```json
{
  "format": "agent-workspace-module/1",
  "id": "examplehost.agents",
  "name": "Example agent host",
  "version": "1.0.0",
  "description": "Create and manage agents that run on this host.",
  "baseUrl": "https://agents.example.net/aw/",
  "install": "install",
  "account": { "name": "Example agent host" },
  "scopes": [
    { "scope": "agents.manage", "why": "To hand a new agent its Workspace connection.", "required": true },
    { "scope": "runs.report", "why": "To report what its agents are doing." }
  ],
  "panels": [
    { "id": "create", "slot": "agents.create", "title": "Create an agent here", "entry": "panels/create" },
    { "id": "model_access", "slot": "agents.create", "title": "Model access", "frameOnly": true },
    { "id": "agent", "slot": "agents.detail", "title": "On the host", "entry": "panels/agent" }
  ],
  "offers": [
    { "id": "new", "title": "A new agent", "summary": "Start from scratch or a template.", "start": { "panel": "create" } }
  ],
  "frameOrigins": ["https://agents.example.net"]
}
```

Per-agent actions come with the agent's profile, for example
`[{"id": "restart", "label": "Restart", "needs": "confirm"}, {"id": "retire", "label": "Retire",
"needs": "confirm", "danger": true}, {"id": "settings", "label": "Settings on the host", "needs":
"panel"}]`.

The `create` panel's steps, as one host might answer them (an example of the host's own content;
Workspace knows none of it; each step is a response to the previous action; `state` carries the
host's progress):

1. Start: `list` of "From scratch", "From a template" (the host's templates), "Copy an existing
   agent". `action: start`.
2. Basics: `form` with `name`, `purpose` (textarea), `backend` (enum of what the host has; the
   host may leave this out to keep the runtime hidden).
3. What it may do on the host: `form` with the host's authority options (enum or checkboxes).
4. How proactive: `form` with schedule and triggers the host supports.
5. Model access: `{ "frame": { "url": "https://agents.example.net/aw/keys?…", "title":
   "Model access" } }`. The key goes into the host's own page; Workspace never sees it.
6. Review: `fields` summarising, a `table` of what will change on the host, `status`
   "waiting for review" if the host requires its own review, then `buttons` "Create".
7. `{ "done": { "message": "Created.", "result": { "agentRef": { "issuer": "agents.example.net/ctx-7",
   "id": "agt_91", "lineage": [] }, "displayName": "Fixer", "avatar": "…",
   "suggested": { "purpose": "…", "skills": ["triage"] } } } }`.

A2 then registers the agent through `agent_register` with the outside reference (`agentRef`),
asks the person for its role and spaces (Workspace's own step), and the host receives the
account's credential through the fixed host protocol (A1). A host is an app account (R7); if the
host has enrolled through A1's host protocol, the enrolment installs this connected module in
the same step from the host's `describe` (A1 calls `connected_module_install` with the manifest
URL `describe` gives), so an account that may link hosts approves a host once (R45). Enrolment names the host's owner, a
person or a workspace (R19); a person's own host may offer its agents in any workspace where
that person may add agents.

### 3.8a File sources: a kind of connected module (R23)

A file source (an outside store such as a drive, a repository or a plain bucket) is a connected
module with a fixed contract; no store is named in the core. Its manifest declares
`"fileSource": {"put": true|false, "contentSearch": true|false}` and implements, as signed
remote operations (3.7):

| Call | Input | Result |
| --- | --- | --- |
| `list` | `folder` (null for the root), `cursor` | `{items: [{ref, name, kind: file|folder, size?, mime?, modifiedAt, webUrl?}], nextCursor}` |
| `get` | `ref` | one item as above |
| `content` | `ref`, `range?` (bytes) | the bytes, streamed, or a short-lived URL to them |
| `thumbnail` | `ref`, `size` | an image or `null` |
| `changes` | `cursor` | `{changes: [{ref, change: created|updated|removed}], cursor}` for name search and previews |
| `put` (optional) | `folder`, `name`, bytes | the new item |

The source is connected by a person (their grant, stored as their connection) or by an app (the
workspace's grant); tokens stay in the server's secret store, never in the browser or the log.
Installing one attaches it to a workspace or a space (`module_installs`). C1 owns the file thing
with `source_id` and `source_ref` and everything people do with source files (C1.36); M1 owns
the contract, its validation and a conformance test any source can run. Slice M1.18.

### 3.8b Shapes: packages chosen at first run (R22)

A workspace starts from a **shape**, which is a built-in package (3.6) with `target:
"workspace"`: modules on and off, the default layout, starter channels and pages, and the words.
M1 ships five in `modules/packages/shapes/`:

| Shape | Modules on | Notes |
| --- | --- | --- |
| Friends | Chat (channels, DMs, groups; the inbox is part of Chat), Files, Search | chat only: no Tasks, Pages, Agents, Runs; Home is the conversation list (C2.40); invitations by link |
| Team | everything | today's default |
| Just me and my agents | the same modules as Team (R29) | differs only in the starting layout: personal space first, no invitation prompts |
| Documents | Pages, Sheets, Whiteboards, Files, Chat | no Tasks or Agents |
| Blank | Chat only, with nothing created | for builders |

Agents is a module that can be off. Any account may join any workspace its role allows,
whichever modules are on (R31): adding an agent to a workspace with Agents off offers, and does
not require, switching Agents on; the account's kind always shows on its badge and profile. First run and sign-up ask what the workspace is for and
install the matching shape (H1 asks the question at sign-up). A shape can be changed later with
`workspace_shape_set {shape}` (a `package_install` with the shape's switches, previewed): it turns
modules on and off and adds starter parts, and **never deletes** anything; turning a module off
hides it, as switches do today. W1.39's shape walk test reads these packages. Slice M1.17.

### 3.9 The directory

What can be installed: packages and connected modules, from sources.

```sql
CREATE TABLE directory_sources (
    id UUID PRIMARY KEY,
    -- NULL: a server-wide source the operator set; else one workspace's own.
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('builtin', 'folder', 'url_list', 'catalogue')),
    location TEXT NOT NULL,           -- a path (folder), or an https URL
    trusted_key TEXT,                 -- ed25519 public key a catalogue's index must be signed with
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE directory_entries (
    source_id UUID NOT NULL REFERENCES directory_sources(id) ON DELETE CASCADE,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('package', 'connected_module')),
    entry_key TEXT NOT NULL,          -- "studio.game_studio", "acme.builds"
    version TEXT NOT NULL,
    name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
    location TEXT NOT NULL,           -- the package file or the manifest URL
    digest TEXT,
    fetched_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (source_id, entry_kind, entry_key, version)
);
```

- **Built in**: `modules/packages/*/` in the release (today's templates, plus starter packages
  such as Game studio). Always listed.
- **Self-hosted**: the operator sets `AGENT_WORKSPACE_DIRECTORY_DIR` (a folder of package files
  and manifest links) and `AGENT_WORKSPACE_DIRECTORY_URLS` (URL lists: a JSON index of entries).
  Read at start-up and hourly; a broken entry is listed with its error, not dropped silently.
- **Hosted, later**: a catalogue we publish is a URL list whose index is signed; the server
  checks it against a pinned key. Self-hosted servers can add it like any URL list. Self-hosting
  never needs it.
- Workspace-level sources: allowed on a self-hosted server; on a hosted server only if the
  operator allows (owner call, below).

Operations: `directory_list(query, kind)` (read), `directory_source_write` (operator for
server-wide; `modules.manage` for workspace sources). The Modules settings page gains
"Installed" and "Available" tabs drawn from `module_list`, `package_list` and
`directory_list`.

## 4. Migration path

1. **Registry beside the lists.** The registry is built from today's manifests and
   `CORE_SOURCES` is replaced by the generated list in one move (the guard "every file in
   `modules/` is listed", `mod.rs:90`, becomes "every manifest has a core folder"). The
   `manifest::of_*` functions keep their signatures and read the registry, so no caller moves.
2. **State to the table.** One migration creates `module_installs`, backfills switched-off
   modules from `settings`, and widens `workspace_roles.modules_off`. In the same pull request
   `write_switches` (`modules/http.rs:283`) writes the table, and every reader moves:
   `state::switches_of` (`state.rs:225`), the notification SQL (`:596`, which reads `settings`
   directly), the gate. During the deploy window the old binary could still write a `settings`
   row; a trigger on `settings` (insert or update of a `modules.%` key at workspace scope)
   mirrors it into `module_installs`, so nothing is lost whichever runs (R16: the only trigger
   M1 adds; it writes only `module_installs`; added in M1.2, removed in M1.11). The contract
   step (M1.11) drops the trigger, deletes the `modules.%` settings rows and removes the settings
   definitions (`settings.rs:487-500`) and `has_own_routes`'s `modules.` prefix.
3. **Hand lists to registrations, one owner at a time.** Routes, settings, notification kinds,
   triggers, actions and setup steps move in M1.3 (no neighbour needed). Kinds, events, outbox
   kinds and search move when K3's types exist; operations when K2's dispatcher exists (the 45
   per-tool checks are deleted as each tool becomes an operation, by K2's slices, with the
   ratchet in section 6 counting down); permissions when K1's catalogue is data.
4. **Web registrations beside the hand rows.** Each module's `module.ts` is added and the
   shell reads registrations; the hand rows are removed in the same pull request per area
   (routes, sidebar, palette, settings, embeds, live), so the app never draws a row twice. The
   existing guard tests keep passing throughout because claims still come from manifests.
5. **Built modules and packages are new tables**: nothing to copy. Templates are read through a
   converter, so saved templates need no data migration. The old template routes and tools
   remain as aliases for one release after M1.12 and are removed in M1.12's follow-up named
   there.
6. **Connected modules are new.** The `integration` actor kind and connected apps stay as they
   are; K1 decides how app accounts relate to them.

Nothing here deletes data. Every migration is additive until M1.12's contract step, which only
removes the duplicate switch rows after every reader has moved.

## 5. Slices

Order: self-registration for core modules first, then built modules and packages, then
connected modules. "∥" marks what can run beside what.

### M1.1 The registry (M)

The `CoreModule` trait, `Registrar`, `Registry::build`, `ModuleDescription` (manifest format 2,
all new fields optional), `build.rs` generating `core_modules.rs` from
`apps/server/src/modules/core/*/mod.rs`, one folder per existing core module holding only its
registration (manifest source and, for now, nothing else). `manifest::core()`, `get`, `of_*`
read the registry. `ModuleSet` becomes `[u64; CORE_WORDS]`; `MAX_CORE` goes.
`AppState` carries `Arc<Registry>`.

- Changes: `apps/server/build.rs`, `apps/server/src/modules/{registry.rs, core.rs, core/*/mod.rs,
  manifest.rs, state.rs, mod.rs}`.
- Depends on: nothing.
- Acceptance: the server starts and behaves the same (all existing tests pass); the guard
  tests in `modules/mod.rs` run against the registry; new unit tests in
  `modules/registry.rs`: two modules claiming one tool fail `build`, a `needs` cycle fails, an
  unknown `needs` fails, a 70-module synthetic set builds and `ModuleSet::all().contains` holds
  for the 70th. The hand-match ratchet (section 6, guard 1) lands here with today's sites in its
  allowlist.
- Size M. Can run beside: M1.4, any neighbour.

### M1.2 Module state in a table; the gate follows roles and id-routed kinds (M)

Migration `module_installs` with backfill and the `settings` mirror trigger; widen
`workspace_roles.modules_off`. `write_switches` writes the table; `state::switches_of`, the
notification SQL and the gate read it. `PUT …/modules` accepts `space` for space narrowing.
The gate resolves the space for id-routed routes from registered `id_routes` (pages, boards,
sheets, runs) and applies the caller's role narrowing (`state::for_project`), closing F5. If P0
fixes F5 first (P0.7 does: R10), this slice keeps P0.7's tests and only moves the lookup to the
registry.

- Changes: new migration; `modules/{state.rs, gate.rs, http.rs}`; `modules/core/{pages,
  whiteboards, sheets, runs}/mod.rs` (their `id_routes`); `tests/integration/modules.rs`.
- Depends on: M1.1, P0.7.
- Acceptance: P0.7's tests pass unchanged; integration tests in `tests/integration/modules.rs`: a switch written before the
  migration reads as off after it (backfill); a write through the old settings path is mirrored;
  a role with `modules_off: ["pages"]` gets `module_off` on `GET /api/v1/pages/{id}` and the page
  document routes, and the board list leaves boards of an off module out (one case per
  id-routed family); a space row turning `whiteboards` off hides it there and not elsewhere; a
  space row can't turn on what the workspace has off. Isolation: workspace B's switch never
  changes A's answer.
- Size M. ∥ M1.4.

### M1.3 Server registrations, no neighbour needed (M)

Each core module registers its routes (`lib.rs:153-215` becomes `registry.router()` plus the
non-module routes), settings (assembled `settings::REGISTRY`; module switches drop out of it as
generated entries), notification kinds, automation triggers and actions, setup steps, and the
Messages/Channels `ModuleFilter` (replacing `state.rs:148-154, 516, 548, 603`,
`automations.rs:645-655`, `mcp.rs:3922`). Export slots (`ExportPart`) are declared for H1 to
fill.

- Changes: `lib.rs`, `settings.rs`, `notifications.rs`, `automations.rs`, `onboarding.rs`,
  `modules/core/*/mod.rs`, `modules/state.rs`.
- Depends on: M1.1.
- Acceptance: `every_route_names_its_module` now compares the router's routes with registered
  ones (no source scan); removing a module's registration from the test registry removes its
  routes (unit test with a two-module registry); notification SQL test (`state.rs:665`) still
  passes with the special cases gone; the hand-match allowlist shrinks by the sites removed.
- Size M. ∥ M1.2, M1.4. Keep early: H1.8 and H1.9 (export) build on its `ExportPart` slots
  (H1 may use an in-code export list until then, R41).

### M1.3k Kinds, events and search from registrations (M, with K3)

`things.rs` `KINDS`, resolvers and `links_sql`, `events::EVENT_TYPES`, outbox resource kinds,
realtime visibility and `SearchKind` read registered kinds, events and search sources. The
reference URL regex in `thing_references` (`20261113060000_thing_references.sql:38-56`) is
generated from registered `href`s by a migration-free function K3 calls.

- Depends on: M1.1 and K3's `KindProvider`, event and search types. Done as part of, or right
  after, K3's slices; M1 supplies the registration calls.
- Acceptance: a kind registered by the scaffold dummy (section 6) opens through
  `GET /api/v1/things/{kind}/{id}`, appears in search, and its event reaches a webhook, with no
  edit outside its folder.
- Size M. ∥ M1.4 to M1.11.

### M1.3o Operations and permissions from registrations (S each, with K2 and K1)

Modules register K2 operations; the dispatcher checks `module_of_op` once. The per-tool
"module off" helpers and their call sites go as each tool becomes an operation (K2's slices;
the ratchet counts them down). Modules register K1 permission definitions; `PERMISSION_MODULES`
(`policy.rs:589`) and the web `PERMISSION_KEYS` (`roles.ts:8`) are read from the registry.

- Depends on: K2's operation registry and dispatcher; K1's permission catalogue as data.
- Acceptance: the scaffold dummy's operation is listed in `tools/list`, refused with
  `retryCategory: "module"` where it is off, and its permission shows in the role editor
  group for its module.
- Size S (M1's part). ∥ anything.

### M1.4 The web registry: routes, navigation, palette, settings (M)

`modules/define.ts`, `modules/registry.ts` (glob of `../*/module.ts`), a `module.ts` per core
module, with the shell fields W1 needs (3.4: route handles, `phone`, `badge`, `things`,
`capture`, `live` by kind). `App.tsx`'s module routes, `ProjectSidebar`'s rows,
`HomeSidebar`/`ProjectRail` rows, palette entries and module settings pages come from
registrations; the hand rows go. Manifest `parts` merge into the layout registry (Vite plugin
and `layouts.rs`); `parts.ts` is generated. This is the one registration contract; W1.13 draws
the persistent shell from it and adds no second one.

- Changes: `apps/web/src/modules/{define.ts, registry.ts}`, `apps/web/src/*/module.ts`,
  `app/App.tsx`, `shell/{ProjectSidebar,HomeSidebar,ProjectRail,CommandPalette}.tsx`,
  `settings/registry.ts`, `experience/parts.ts`, `vite.config.ts`, `apps/server/src/layouts.rs`.
- Depends on: M1.1 (for `web.folder` in manifests). W1's layout-route shell is welcome but not
  required: registrations carry `shell: 'project' | 'home' | 'none'` that today's per-page
  `AppShell` and W1's layout route both read.
- Acceptance: `slots.test.ts` and `registry.test.tsx` pass unchanged in what they assert; new
  `modules/registry.test.ts`: every `module.ts` id is a manifest, every manifest with `web`
  has a `module.ts`, no `module.ts` has a static non-type import, every registered sidebar row
  and tab names a part; `pnpm build`'s budget check holds the initial load.
- Size M. ∥ M1.2, M1.3.

### M1.5 The web registry: embeds, live updates, tabs, composer extras (M)

`PageEmbed.tsx` and `moduleEmbed.ts` dispatch on registered embeds (collections and sheets move
into their manifests and `module.ts`, F3); `LivePage.tsx`'s toolbar lists registered pickers;
`liveDocuments.ts` runs registered handlers keyed by event type; thing tabs, composer extras,
makers and slash entries (shape from C2) register. W1.26 replaces the embed dispatch with pieces
on top of these entries. Ends with the web scaffold test.

- Changes: `memory/embeds/*`, `memory/live/LivePage.tsx`, `realtime/{liveDocuments.ts,
  useRealtimeConnection.ts}`, `collections/module.ts`, `sheets/module.ts`,
  `modules/collections.json`, `modules/sheets.json`, `modules/scaffold.test.tsx`.
- Depends on: M1.4.
- Acceptance: existing embed and live tests (`embedModel.test.ts`, `moduleEmbed.test.ts`,
  `liveDocuments.test.ts`) pass; the web scaffold test (section 6) passes.
- Size M.

### M1.6 Built modules: tables and operations (M)

Migrations `modules`, `module_parts`, `module_nav`; operations `module_describe`,
`module_write`, `module_part_add`, `module_part_remove`, `module_remove`, `module_restore`;
`module_list` extended; permissions `modules.build`, `modules.manage`; `object_on_sql` used by
collection lists, record queries (added into T1's `RecordSource::scope`, or into
`collections/query.rs` if T1.5 hasn't merged), search and the dispatcher's scope check; events
`module.*`.

- Changes: new migrations; `apps/server/src/modules/built.rs`; `modules/core/modules/mod.rs`;
  the collection read paths (T1's code, by agreement); `mcp` tools via K2 (or `#[tool]` if K2
  hasn't landed, with `agent_parity` rows).
- Depends on: M1.2. T1's collections as records is not required.
- Acceptance: `tests/integration/built_modules.rs`: `module_write` with a new collection, a
  form, an automation and a nav entry creates them all in one call and again changes nothing
  (idempotent); switching the module off makes its collection's records vanish from
  `record_query`, search and `thing_open`, its form answer "turned off", its automation skip
  with the reason, and switching on brings them back with no data lost; `module_remove` with
  `keep` leaves a plain collection; isolation: a part from workspace B can't be added in A; an
  agent with `modules.build` by role can do all of it, a viewer can't (no account-kind
  difference).
- Size M. ∥ M1.8.

### M1.7 Built modules in the app (M)

Settings › Modules lists all sorts with Installed and Available tabs (Available fills in M1.11);
the builder (name, parts, navigation); `BuiltNav` draws module navigation in the sidebar as the
dynamic part `module.<key>`.

- Depends on: M1.4, M1.6.
- Acceptance: one vitest for the builder's happy path; `check-in-browser` light, dark and
  390 px screenshots in the pull request; `registry.test.tsx` covers the dynamic part.
- Size M.

### M1.8 Package format and export (M)

`apps/server/src/packages/{format.rs, export.rs, secrets_check.rs}`: the format, the inline JSON
form, canonical digest, `package_export` from a space, a module or a workspace through each
part's portable form, the no-secrets check. `modules/packages/package.schema.json` shared with
the web (a parity fixture, as `parseParity.json` is).

- Depends on: M1.1. ∥ M1.6.
- Acceptance: `tests/integration/packages.rs`: exporting a space with collections, a form, an
  automation with an outbound webhook, a role and a skill page gives a package with no token,
  secret, webhook signing secret, member or form token in it (asserted by scanning the output);
  export twice gives the same digest; the six built-in templates convert to packages that
  validate.
- Size M.

### M1.9 Package preview and install (L, split if it runs long: preview first, install second)

`package_preview`, `package_install`, `package_list`, `package_remove`; tables `packages`,
`package_installs`, `package_objects`; install into a new or existing space or the workspace;
`needsAdmin` checks; agent templates offered to A2 (stored, listed by `agent_template_list`).

- Depends on: M1.6, M1.8.
- Acceptance: install into an empty workspace then export gives an equivalent package (round
  trip, ignoring ids); install twice with the same `clientRequestId` changes nothing; a changed
  workspace between preview and install is refused by `planHash`; a member without
  `roles.manage` can't install a package that adds a role; an agent can draft a package inline,
  preview it and install it with the right role.
- Size L.

### M1.10 Package upgrades (M)

Three-way upgrade preview and apply, per-part choices, "no longer in the package" marking.

- Depends on: M1.9.
- Acceptance: tests for each rule in 3.6 (unchanged here applies; changed both is a conflict
  that keeps mine by default; removed upstream detaches; type change makes a new field).
- Size M.

### M1.11 Templates become packages; the directory (M)

Built-in templates move to `modules/packages/`; `from_template` reads saved workspace and
project templates as packages; the template routes and tools call the package operations;
`directory_sources`, `directory_entries`, folder and URL-list sources, the Available tab.
Contract step for switches (section 4, step 2): drop the mirror trigger and the `modules.%`
settings rows. The old template routes and tools are removed one release later, in a follow-up
pull request this slice files as a task.

- Depends on: M1.9, M1.2.
- Acceptance: `tests/integration/awd76_workspace_templates.rs` passes unchanged against the new
  path; a package dropped into the directory folder appears in `directory_list` and installs;
  a broken entry is listed with its error.
- Size M.

### M1.12 Connected modules: install, approve, revoke (L)

Manifest fetch through `outbound.rs`, validation, preview, approval, app account and generated
role (K1), `module_apps`, install-code exchange, signing secrets in the secrets store, re-fetch
and re-approval, revoke.

- Depends on: M1.2, M1.6 (the `modules` table), K1's app accounts and client credentials.
- Acceptance: `tests/integration/connected_modules.rs` with a test app server in the test
  tree: a manifest on a private address is refused on a hosted profile; scopes beyond the
  approved ones are refused on every call; a new scope in a re-fetched manifest doesn't apply
  until approved; revoke disables the account and the next call is refused; isolation across
  workspaces.
- Size L.

### M1.13 Remote operations and events in (M)

K2's `RemoteOperation` handler; Standard Webhooks signing out; the inbound events endpoint
with timestamp and replay checks; connected kinds through `thing_upsert`.

- Depends on: M1.12, K2's dispatcher, K3's event log and thing registry.
- Acceptance: a declared operation is listed as a tool for an account allowed to use it and
  calls the test app with a valid signature; a forged or replayed inbound event is refused; a
  valid one triggers an automation; a connected thing is found by search and can be linked
  from a task.
- Size M.

### M1.14 Panel blocks (M)

The `aw-panel/1` schema (shared fixture), the Rust validator, panel requests and actions
(signed, timed out, rate limited), `PanelHost.tsx` fetching panels and rendering through W1.25's
`PanelBlocks({blocks, onAction})` (W1 owns the block renderer, R17), wide and narrow; panel slots
as dynamic parts, including `conversation.tab`.

- Depends on: M1.12, M1.4, W1.25 (or a minimal renderer here that W1.25 replaces; named stopgap).
- Acceptance: validator tests (each block, limits, secret fields refused, unknown block
  fallback); `Pieces.test.tsx`-style test drawing every block with axe; the pieces gallery
  shows a sample panel at 1440 and 390 px in `e2e/pieces.spec.ts`; a form round trip with the
  test app returns `errors` then `done`.
- Size M.

### M1.15 Framed panels (M)

`/panel-frame/{launch}` host page with its own CSP, launch tokens and their redemption, the
message protocol, `FrameHost.tsx`.

- Depends on: M1.14.
- Acceptance: unit test on the CSP of the host page (one origin, no script); a token is single
  use and expires; an integration test that a frame for an origin not in `frameOrigins` is
  refused; a browser check (`check-in-browser`) that the frame can't read the parent and that
  `done` without the server-side result does nothing.
- Size M.

### M1.16 The agent-creation panels, end to end (S)

A reference agent-host app in the test tree publishing the manifest of 3.8 with `offers` and
per-agent `actions`; the `agents.create` slot exposed to A2 (`offers_for_slot`,
`panels_for_slot`); per-agent actions drawn in `agents.detail` and sent back; an end-to-end test
from "create one" to `done` with an `agentRef` handed to `agent_register`.

- Depends on: M1.15; A2's flow consumes it (A2 builds the screens and `agent_register`).
- Acceptance: the test finds no vendor name in Workspace code paths it exercises (a grep guard
  over `apps/server/src` and `apps/web/src` for the example host's id); A2's flow receives
  `result.agentRef`; an action with `needs: confirm` reaches the host once; one with `needs:
  panel` opens the host's panel; Workspace code has no list of action ids.
- Size S.

### M1.17 Shapes (M; R22)

The five shapes of 3.8b as built-in packages; the first-run question (and sign-up's, with H1);
`workspace_shape_set` (operation and agent tool) as a previewed package install that never
deletes; Agents switchable off; each shape's words.

- Depends on: M1.9, M1.11.
- Acceptance: `tests/integration/shapes.rs`: a "Friends" workspace has only chat modules on and
  its `module_list`, settings and command list name no task, project, agent or run; an agent can
  be added to it with Agents left off; "Just me and my agents" has the same switches as "Team"; changing
  "Friends" to "Team" and back loses no message, page or record; each shape validates as a
  package; W1.39's shape walk passes for each shape (with C2.40 for "Friends").
- Size M.

### M1.18 File sources: the contract (M; R23)

`fileSource` in the connected-module manifest; the six calls as signed remote operations with
schemas; a conformance suite any source can run (`tests/fixtures/file_source/`), a fake source
in the test tree, tokens through the secret store; attaching a source to a workspace or space.
C1.36 builds the file thing on it.

- Depends on: M1.12, M1.13.
- Acceptance: the fake source passes the conformance suite; a source missing `changes` or
  returning a malformed `list` fails install with the reason; no source name appears in
  `apps/server/src` or `apps/web/src` (grep guard); a token never appears in a response, log
  line or event.
- Size M.

## 6. Guard rails

1. **No module id matched by hand** (`apps/server/src/modules/guards.rs`, test
   `module_ids_are_matched_only_in_the_registry`): scans `apps/server/src/**/*.rs` outside
   `modules/registry.rs`, `modules/state.rs` and each module's own `modules/core/<id>/` folder
   for a core module id as a string literal in `contains("…")`, `with(…, "…")`, `== "…"`,
   `manifest::get("…")`, `matches!(…, "…")` or `module_off(…, "<Name>")`. Existing sites live in
   `modules/hand_matched.allowlist` as one `file: count` entry per file; the test fails
   when a file's count rises or a file not listed has one, and tells the author to register a
   `ModuleFilter` or use the operation's module. The allowlist only shrinks. The web twin in
   `modules/registry.test.ts` scans for `moduleOn('…')` literals outside `modules/` and
   `*/module.ts`.
2. **Every registered part names its module.** `Registry::build` attaches the module to every
   contribution; `modules/mod.rs` asserts every route in the router, tool in `tools/list`,
   kind, event, permission, setting, notification kind, search source, export part, trigger,
   action and layout part resolves to exactly one module, and every registered embed, palette
   entry, sidebar row, settings page and tab on the web names a module and a part
   (`modules/registry.test.ts`). Today's claim tests stay as they are until the registry tests
   cover them, then are folded in.
3. **The scaffold test, server** (`tests/integration/module_scaffold.rs` with
   `tests/fixtures/modules/scaffold/{manifest.json, mod.rs}`): a dummy module registered
   through `Registry::build(core::ALL, &[&SCAFFOLD])` with one table (created by the test),
   one kind, one operation, one event, one permission, one setting, one search source and one
   route. Asserts: the route answers; switching it off answers `module_off` on the route and
   refuses the operation; its kind opens as a thing and is found by search (after M1.3k); its
   event reaches a webhook; its permission groups under it. The test edits no file outside the
   test tree, so a future change that needs a central edit to add a module breaks it.
4. **The scaffold test, web** (`apps/web/src/modules/scaffold.test.tsx`): builds the registry
   with the discovered modules plus a dummy registration and a dummy manifest; renders the
   router and shell and asserts the dummy's route renders, its sidebar row and palette entry
   appear, hide with its part and with the module off, and its embed draws for its href.
5. **No static imports in `module.ts`** (guard in `modules/registry.test.ts`, section 3.4).
6. **Packages never carry secrets**: the export test scans output for secret-shaped values; a
   package with one is refused on install too.
7. **Panel parity**: `modules/packages/package.schema.json` and `modules/panel.schema.json` are
   read by Rust and TypeScript tests, as `parseParity.json` is.
8. **Connected modules stay outside**: a test that no route under `/api/v1/modules/{id}/…`
   accepts a request without a valid signature, and that the frame host page's CSP has no
   `script-src` other than `'none'`.
9. **AGENTS.md** (via P0): "A new module is a manifest, a server folder under
   `src/modules/core/<id>/` and a web `module.ts`. Never match a module id by hand; register a
   filter."

## 7. Risks and open questions

**Risks.**

1. **Neighbour timing.** The "zero edits" result for kinds, events, operations and permissions
   needs K1, K2 and K3. If they slip, M1 still delivers routes, settings, notifications, web
   navigation and the scaffold for those parts, and the ratchet keeps the old lists from
   growing. Notice: M1.3k and M1.3o stay open. Fallback: M1 registers adapters over today's
   static lists so the scaffold test covers what exists.
2. **Hidden data still reachable.** Built-module parts filtered by a SQL fragment can be missed
   by a read path someone forgets (the F5 class of bug again). Notice: guard 3 and the
   built-module integration tests check each read family. Fallback: put the check in K3's
   thing visibility function so every read through things inherits it.
3. **Connected-module security.** Phishing through frames, SSRF through manifest URLs, scope
   creep. Mitigations are in 3.7; the riskiest part is the frame. Notice: the frame tests and a
   security review before M1.15 merges (the `security-review` skill). Fallback: ship blocks
   only (M1.14) and leave frames behind a server setting until reviewed.
4. **Deploy-window drift** between `settings` and `module_installs` while two binaries run.
   The mirror trigger covers writes; reads move in one pull request.
5. **Panel blocks growing into a UI framework.** Each new block needs a written reason and a
   phone design; the set is closed by the schema version.
6. **Build-script surprises.** A generated module list is unfamiliar. Mitigation: the generated
   file is short and readable, and a test prints it on mismatch.

**Questions for the owner** (with what this plan assumes):

1. Built modules: can a built module live in one space and also be workspace-wide?
   Recommended and assumed: both, defaulting to the space where it's made.
2. On the hosted service, may workspace admins add their own directory sources and install
   connected modules from any URL, or only from the catalogue? Recommended: any URL, with the
   install preview and the outbound check; the catalogue is a convenience, not a gate.
3. Should installing a package that brings an agent template offer to create the agent right
   away? Recommended: no, it shows in "Add an agent" with one tap, because creating an agent
   picks a host and that is a person's choice.
4. Do connected modules count against the free plan? Recommended: no cap on how many; rate
   limits per install only.

## 8. Touch points

| Workstream | M1 needs | M1 provides |
| --- | --- | --- |
| K1 | App accounts (kind `app`) with client credentials; a generated role per connected module; module use in role data (replacing `modules_off`, keeping "a role only narrows"); permission catalogue as data so modules register permissions; `modules.build`, `modules.manage` in the catalogue | The module list and descriptions the role editor groups by; `PermissionDecl` from connected manifests |
| K2 | `OpDef` with a `module` field (or lookup through the registry); a dispatcher hook `modules::check(op, scope, account)`; a `RemoteOperation` handler type; the operations above as K2 operations with agent tools | `Registry::module_of_op`; module on/off per caller and space; the operations' descriptions for generated TypeScript |
| K3 | `things::KindProvider` (resolve, space_of, fields), event registration, search source type, `thing_upsert` for connected kinds, inbound events into the log, events to apps through delivery | Registered kinds, events and search sources; `object_on_sql` for built-module visibility; the `module` kind |
| K4 | Space kinds carrying `modules_allowed`; `space_id` meaning; personal and workspace spaces; space starting shapes in M1's package format (K4.20 depends on M1.17, soft) | `module_installs.space_id` narrowing; replaces `in_project` |
| K5 | Cross-process cache invalidation (`realtime::publish_ephemeral(Invalidate)` on a switch), shared rate limits for panel and remote-operation calls, file storage for package uploads | Events to invalidate on |
| T1 | Collections, fields, views, forms keep their portable definitions (`collection_describe`); read paths take `object_on_sql` (in `RecordSource::scope`); the Dashboards module (T1.32 builds `modules/core/dashboards` and its `module.ts`) | Built modules as bundles of T1 objects; packages carry T1 definitions; the registry Dashboards registers in |
| A1 | Host enrolment (hosts are app accounts, R7; A1.11 installs the module at enrolment through M1.12) calling `connected_module_install` with the manifest URL from `describe` | Panels for any host's steps; offers and actions as generic lists |
| A2 | Consumes `agents.create` offers (A2.20, through M1.12 and M1.14) and `agents.detail` panels and actions; `agent_register` (R21); defines `result.agentRef`'s schema; shows agent templates from packages | `offers_for_slot`, `panels_for_slot`, panel round trip, actions round trip, agent templates |
| C1 | Pages as parts (page templates as pages marked template; skill pages); `page_create`/`page_update` used by `module_write`; the file thing for source files (C1.36) | The file source contract (M1.18) |
| C2 | The maker trait and the slash entry shape; chat standing alone for "Friends" (C2.40) | `Registrar::maker`, slash entries, the `conversation.tab` slot, the shapes (M1.17) |
| W1 | The shell drawn from M1's registrations (W1.13); `PanelBlocks` (W1.25) that `PanelHost` renders through; the embed dispatch (W1.26); the shape walk test (W1.39) | `defineModule` with the shell fields, `modules/registry.ts`, generated `parts.ts` (M1.4), embed entries (M1.5), the shapes |
| H1 | Fills `ExportPart` per module for the workspace archive; the directory's hosted catalogue and signing key; operator config for directory sources | `ExportPart` slot; package format shares its archive conventions |
| P0 | F5 fix (P0.7, before M1.2); the AGENTS.md rule | — |
| H1 | (also) the sign-up question that picks a shape | the shapes |

## 9. Changes to the end state

1. **Spaces and modules.** END-STATE §3 gives a space "its own … modules". Proposed wording:
   "A workspace switches modules on and off. A space can switch a module off for itself, never
   on when the workspace has it off, and is where a built module made for it is installed.
   Roles narrow both."
2. **Agents in packages.** END-STATE §3 says a package bundles "agents' profiles and skills".
   Proposed: "agent templates and skills: a template is a starting point for 'create an agent',
   never an account, a connection or a credential, and names no host or model."
3. **Presets and templates list what is off.** Add under Modules: "Presets and packages list
   the modules they turn off, so a new module is on unless something says otherwise."
4. **Panels.** Add: "Panels are a small, fixed set of blocks; anything else is the app's own page
   in a frame, which never sees Workspace's credentials and is only shown from origins an admin
   approved."
5. Not a change, a correction for the record: the module switches are not stored as a
   bitmask; only the in-memory set is one. The migration is of `settings` rows.

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| M1.1 | The registry | M | — | M1.4, all neighbours |
| M1.2 | Module state in a table; gate follows roles and id routes | M | M1.1, P0.7 | M1.3, M1.4 |
| M1.3 | Server registrations without neighbours | M | M1.1 | M1.2, M1.4 |
| M1.3k | Kinds, events, search from registrations | M | M1.1, K3 | M1.4 to M1.11 |
| M1.3o | Operations and permissions from registrations | S | M1.1, K2, K1 | anything |
| M1.4 | Web registry (the one registration contract, with W1's shell fields) | M | M1.1 | M1.2, M1.3 |
| M1.5 | Web registry: embeds, live, tabs, composer; web scaffold | M | M1.4 | M1.6, M1.8 |
| M1.6 | Built modules: tables and operations | M | M1.2 | M1.8 |
| M1.7 | Built modules in the app | M | M1.4, M1.6 | M1.8, M1.9 |
| M1.8 | Package format and export | M | M1.1 | M1.6, M1.7 |
| M1.9 | Package preview and install | L | M1.6, M1.8 | M1.7 |
| M1.10 | Package upgrades | M | M1.9 | M1.11, M1.12 |
| M1.11 | Templates become packages; the directory; switch contract step | M | M1.9, M1.2 | M1.10, M1.12 |
| M1.12 | Connected modules: install, approve, revoke | L | M1.2, M1.6, K1 | M1.10, M1.11 |
| M1.13 | Remote operations and events in | M | M1.12, K2, K3 | M1.14 |
| M1.14 | Panel blocks (`PanelHost` through W1.25's `PanelBlocks`) | M | M1.12, M1.4, W1.25 (stopgap allowed) | M1.13 |
| M1.15 | Framed panels | M | M1.14 | M1.13 |
| M1.16 | Agent-creation offers, panels and actions end to end | S | M1.15, A2 | — |
| M1.17 | Shapes | M | M1.9, M1.11 | M1.12 to M1.16 |
| M1.18 | File sources: the contract | M | M1.12, M1.13 | M1.14 to M1.17 |

## Review changes

- 3.4 and M1.4: M1's `defineModule` is the one registration contract; it gains W1's shell fields (route handles with space and layout, `phone`, `badge`, `things`, `capture`, `live` handlers by event kind instead of cache keys); W1.13 draws the shell from it and drops `defineWebModule` (two builders of one thing).
- M1.5: embeds, makers and slash entries register here; W1.26 does the dispatch onto pieces.
- 3.4 and M1.14: `PanelHost` renders through W1.25's `PanelBlocks` (R17: W1 owns the block renderer); M1.14 depends on W1.25 with a named stopgap.
- 3.2: `Registrar::maker` and `Registrar::slash` (C2 defines the shapes).
- 3.7: new panel slot `conversation.tab`.
- 3.8: R21: offers `{id, title, summary, start}` and per-agent actions `{id, label, needs, danger?}` from `describe`, generic; result to A2's `agent_register`; the example steps marked as the host's own content; hosts are app accounts installed from `describe` at enrolment (R7), enrolment names an owner (R19). M1.16 tests offers and actions.
- New 3.8a and slice M1.18: the file source contract (`list`, `get`, `content`, `thumbnail`, `changes`, optional `put`) as a kind of connected module, with a conformance suite; no store named (R23).
- New 3.8b and slice M1.17: shapes as built-in packages ("Friends", "Team", "Just me and my agents", "Documents", "Blank"), chosen at first run, changeable with nothing deleted; Agents can be off (R22).
- Section 4 and R16: the `settings` mirror trigger is M1's only trigger, added in M1.2, removed in M1.11 (the text said M1.12, which is connected modules).
- M1.2 keeps P0.7's tests and depends on P0.7 (R10).
- M1.6: `object_on_sql` goes into T1's `RecordSource::scope` (or `collections/query.rs` before T1.5).
- Touch points: Dashboards in the T1 row; C2 row added; W1, A1, A2, C1, H1 rows updated.

### Second pass

- `Registrar::realtime` registers a default K3 `Audience` per event type instead of `LiveScope` (kernel review).
- Module switches call `realtime::publish_ephemeral(Invalidate)` (3.3, K5 row).
- 3.8b: "Just me and my agents" has Team's modules and differs only in starting layout; "Friends" includes the inbox as part of Chat (R29).
- 3.8b and M1.17: agents may join a chat-only workspace with Agents off; the join offers switching it on; the kind shows on badge and profile (R31).
- 3.7: the frame host page is served by `api` (R31).
- K4 row: space starting shapes use M1's package format (K4.20 soft on M1.17).
- M1.3 stays early: H1.8 and H1.9 depend on it (R41 allows an in-code export list until then); M1.12 and M1.14 serve A1.11 and A2.20 (agents and hosting review).
