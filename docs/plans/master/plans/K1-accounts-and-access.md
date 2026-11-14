# K1. Accounts and access: the detailed plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for K1, 2026-11-14. Code read at `fe95a74` in `/home/user/agent-workspace`. All
paths below are under `apps/server/src/` unless they start with `apps/`, `migrations/`
(meaning `apps/server/migrations/`) or `docs/`. Line numbers were re-read in the code, not
copied from the health check. Where a statement is inferred rather than read, it says so.

## 1. Goal

When K1 is done, every person, agent and app is one kind of thing, an account, and what any
account may do is decided by one function from the roles a workspace configured. An account
of any kind is global to the server and joins workspaces by membership (R19): an agent has
an owner (a person or a workspace), is tied to no one project or workspace, keeps its
account, roles and history when it moves host, and can be given exactly a person's access by picking a role: nothing in the code
asks "is this an agent?" to decide a permission. "Edits arrive as suggestions" is a role
setting anyone can have, agents get short-lived and task-scoped tokens instead of only a
365-day bearer, and every action records who acted, through which app, and who asked.

## 2. Where the code is today

### 2.1 Identity: five actor kinds, created in ten places

`actors` (`migrations/0002_walking_skeleton.sql:4-12`) has `id`, `kind IN ('human',
'external_agent', 'execution_worker', 'integration', 'system')`, `status`. It has no
workspace column. Rows are inserted in:

| Kind | Where created | What it is |
| --- | --- | --- |
| `human` | `bootstrap.rs:333`, `invitations.rs:723`, `workspace.rs:1738`, `auth/workos.rs:497`, `seed.rs:80` | A person; also a `users` row (global `login_name`) |
| `external_agent` | `external_agents.rs:384` (`insert_agent`, `:372-461`) | An agent reached over MCP; one connection row, one home project |
| `execution_worker` | `agent_profiles.rs:679` (profile agents), `execution_setup.rs:93` (one bare worker per project) | Server-run agents; project role `worker` |
| `integration` | `github.rs:4676` | The GitHub app, one per project, project role `member` |
| `system` | `seed.rs:117` | The server itself |

What each kind is tied to:

- **External agents** are anchored to a project. `external_agent_connections` has `project_id
  NOT NULL`, `UNIQUE (project_id, actor_id)` and foreign keys into
  `project_memberships(project_id, actor_id)` for the agent and its creators
  (`migrations/0033_m9_external_agent_connections.sql:3-50`). `agent_connection_can_access`
  (`migrations/0040_member_accounts.sql:72-88`) lets the agent act in a target project only
  while it is still an enabled member of its home ("anchor") project. `insert_agent`
  (`external_agents.rs:372-461`) creates the actor, an organization membership as `member`,
  one project membership, the connection and a 365-day credential
  (`external_agents.rs:28`).
- **Credentials** for MCP live in four places: `mcp_client_credentials`
  (`migrations/0027_m6_mcp_foundation.sql:6-39`, project-scoped, `aw_mcp_` prefix, digest
  only, scopes `read`/`write`); MCP OAuth grants and tokens (`migrations/0029_m6_mcp_oauth.sql:46-90`,
  `aw_oauth_` tokens), including "connect as an agent" grants backed by a credential row
  (`migrations/0039_oauth_agent_grants.sql`); WorkOS agent JWTs bound through
  `workos_agent_bindings` (`migrations/0041_workos_agent_identities.sql:9-24`), 24-hour sessions
  (`auth/workos/agents.rs:681`); and one-time setup links that hand out a fresh token
  (`migrations/20261113120000_agent_setup_links.sql:9-21`). Bearer dispatch is by token shape:
  `eyJ` goes to WorkOS, `aw_oauth_` to OAuth, the rest to `mcp_client_credentials`
  (`mcp.rs:4755-4872`). Connection scopes are a second, coarse permission system:
  `read`, `write`, `events`, `approval_respond`, `worker_request`
  (`external_agents.rs:32-38`).
- **The caller** on MCP is `McpPrincipal` (`mcp.rs:137-153`): `actor_kind`, a mandatory
  `project_id` (for people connected account-wide it is their Messages space,
  `mcp/oauth.rs:492-500`), `oauth_grant_id`, `external_agent_connection_id`, `scopes`,
  `account_wide`. On the web it is `AuthenticatedActor` (`auth/mod.rs:49-57`) with
  `actor_kind: String`. Sessions are for `kind = 'human'` only (`auth/mod.rs:216`, `:523`).
- **"Acted through which app"** is recorded as `via_oauth_grant_id` on about 15 tables, filled
  by `current_via_grant()` (`migrations/0042_agent_interface.sql:28-46`) from
  `set_config('aw.via_grant')`. `McpPrincipal::via_grant_id` returns it only for people
  (`mcp.rs:155-161`). `audit_events` has `channel`, `client_name`, `mcp_credential_id`
  (`migrations/0027_m6_mcp_foundation.sql:48-55`) and an OAuth grant and connection id
  (`roles.rs:940`). **"Requested by"** exists only on `launches.requested_by_actor_id`
  (`migrations/20261016220100_launches.sql:12`) and AI planning rows.
- **Names** are stored three times (`users.display_name`, `organization_actor_profiles`,
  `project_memberships.display_name/handle`), kept in step by the `canonical_member_profile`
  trigger (`migrations/0040_member_accounts.sql:29-48`), which exempts execution workers.

### 2.2 Roles: data for custom roles, code for built-in ones

- The catalogue: 30 permissions in 12 module groups (`policy.rs:304-579`, groups
  `:582-595`), 43 actions each mapped to one permission (`policy.rs:91-179`). Each
  `PermissionInfo` (`policy.rs:245-261`) has a key, module, scope (project, workspace, both),
  built-in defaults for owner/admin/member/viewer, `people_only` and `required`. Three are
  people-only: `agents.manage` (`:512`), `roles.manage` (`:561`), `settings.manage` (`:570`).
- `PermissionSet` is a `u64` bitset (`policy.rs:623-703`). `RoleGrant::stored` clamps a custom
  role to its base role (`policy.rs:729-743`). Built-in `worker` holds nothing
  (`policy.rs:636-643`).
- `workspace_roles` (`migrations/20261030020000_workspace_roles.sql:20-72`): built-in rows
  hold no permissions (theirs are in code); custom rows hold `permissions TEXT[]`, a
  `base_role`, `agent_limit TEXT[]` (really "what apps acting for holders may do"),
  `defaults`, `is_default`; `modules_off` was added by
  `migrations/20261105213000_role_modules.sql:11-19`. Memberships carry `role` (text) and
  `role_id` (NULL means "the built-in role named by `role`").
- `custom_role_allows(role_id, key)` is `role_id IS NULL OR key = ANY(...)`
  (`migrations/20261030020000_workspace_roles.sql:121-128`): it only says "a custom role, if
  any, didn't remove it". `roles::role_allows_sql` (`roles.rs:387-397`) and
  `search::projects_allowing` (`search.rs:467-490`) wrap it as if it were a full check, so
  a viewer or worker passes for anything. No caller passes a permission viewers lack yet, so
  no bug is live (health check A4, re-checked).
- `organization_memberships.role` is partly derived from project roles by
  `sync_organization_role` (`memberships.rs:756-800`).

### 2.3 Access: one sound core and seven ways around it

**Keep, unchanged in meaning:**

- `policy::authorize(Subject, Action, Resource)` (`policy.rs:1118-1244`), pure and well tested.
- `conversation_visible_to(conversation, viewer)` (`migrations/0037_direct_conversations.sql:160-181`).
- `actor_can_access_project(viewer, connection, project)` (`migrations/0042_agent_interface.sql:50-79`),
  used by MCP's `scoped_project` (`mcp.rs:5906-5960`), the one choke point that resolves
  every id argument to a project and checks it.
- The other visibility functions: `pull_request_visible_to`
  (`migrations/0050_pull_request_visibility.sql:12`), `whiteboard_visible_to`
  (`migrations/20261017070000_whiteboards.sql:120`), `ai_suggestion_visible_to`
  (`migrations/20261106073614_ai_suggestions_need_see_tasks.sql:10`),
  `external_event_visible_to` (`migrations/20261113100000_person_questions.sql:88`).
- `work_item_commands::lock_membership` plus `authorize_mutation` (`work_item_commands.rs:363-410`),
  the pattern newer modules already copy.
- The delegation ("agent limit") mechanism: a task-local `AgentLimits`
  (`policy.rs:777-855`) set per MCP call (`mcp.rs:4127-4130`) and carried into deferred work
  (`within_agent_limits`, `policy.rs:812-820`).

**The seven ways around it** (each with the evidence an implementer needs):

1. **Hand-built `Subject`s.** 31 `Subject::new` calls outside `policy.rs`; `authorize(` is
   called from 33 sites in 22 files (list in appendix F). Every module loads its own
   membership row, for example `conversations.rs:154-180`, `memory.rs:287`,
   `environments.rs:169`, `sheets/http.rs:95`, `collections/http.rs:118`,
   `whiteboards.rs:205`, `mcp/writes.rs:2131`, `mcp/common.rs:265`, `api.rs:2458`,
   `projects.rs:1563`. The actor-kind string parser is copied seven times
   (`auth/mod.rs:628`, `mcp.rs:5279`, and five more found by `fn parse_actor_kind`).
2. **A second role model.** `workspace::WorkspaceRole` and `manages()`
   (`workspace.rs:101-121`), `caller_role` (`:166`) read the plain role name and only
   `kind = 'human'` (`:189`, `:198`). It decides the audit log (`audit_log.rs:199-201`),
   mail (`mail/http.rs:69-70`), server status (`server_status.rs:224-225`), workspace rename
   and member capabilities (`workspace.rs:238-257`, `:360-364`, `:443`, `:1112`, `:1257`).
   `member_manager_role` and `caller_may` (`workspace.rs:125-163`) patch custom roles in for
   some of these only.
3. **Raw role checks in SQL and Rust.** 41 SQL sites and 39 Rust sites read role names
   (appendix A).
4. **`custom_role_allows` and its wrappers** at 29 SQL sites, plus 12 calls of
   `role_allows_sql`/`projects_allowing` (appendix B).
5. **Delegation limits by hand.** `agent_limit_hides` and `agent_limit_projects_allowing` at
   18 sites (appendix C). A raw-SQL check that forgets one ignores the limit.
6. **The MCP layers.** `TOOL_PERMISSIONS` (`mcp/tool_permissions.rs:24`), a coarse first gate
   over the union of the caller's permissions everywhere (`caller_reach`, `mcp.rs:3853-3867`);
   the module gate (`module_off_here`, `mcp.rs:3899-3940`), which checks the call's home
   project, so whiteboard, sheet and collection tools check modules again inside
   (`mcp/whiteboard_tools.rs:94-121`, `mcp/sheet_tools.rs:64`); connection scopes.
7. **Decisions by account kind.** 45 `!= ActorKind::Human` and `kind != "human"` refusals,
   and about 170 SQL or string comparisons of `actors.kind` in 52 files (appendices D and E).
   Inside the core itself, `authorize` denies people-only permissions by kind
   (`policy.rs:1194`), sends `worker` memberships to a `DelegatedScope` branch that production
   never constructs (`policy.rs:1198-1212`; every production `Subject::new` passes `None`),
   applies delegation limits only to `Human` (`:1218`) and checks `human_required` approvals by
   kind (`:1235`). `roles::effective` strips people-only permissions by kind (`roles.rs:105`).

**Hard-coded agent rules found** (each becomes a role setting; full list in appendix D):

- Pages: `must_suggest` forces an agent's edit to a page it didn't create into a suggestion
  (`page_docs/suggestions.rs:41-48`, called from `mcp/memory_tools.rs:550` and `:1272`), and
  agents may accept or reject suggestions only on pages they made
  (`page_docs/suggestions.rs:557-563`).
- Roles an agent may hold are capped at Member or Viewer in four places:
  `external_agents.rs:602-608` and `:1206`, `memberships.rs:531`, `roles.rs:1530-1537`.
- About 30 MCP tools and 15 HTTP handlers refuse agents outright, for example `run_start`
  (`mcp/runs.rs:422`), `task_launch` (`mcp/launch_tools.rs:249`), `invitation_list`
  (`mcp/invitation_tools.rs:116`), `role_write` (`mcp/role_tools.rs:334`),
  `project_create` (`mcp/admin.rs:687`), `reply_stop` (`mcp/replies.rs:1086`),
  `conversation_continue` (`mcp/groups.rs:434`), group member changes (`direct.rs:578`),
  environments (`environments.rs:463`, `:567`, `:688`, `:773`, `:871`), webhooks
  (`webhooks.rs:307`), moderation (`conversations.rs:4137`, `message_edits.rs:358`,
  `work_items/comments.rs:782`).
- Execution approvals require a person: `human_required` (`provider.rs:53-64`), checked in
  `policy.rs:1235`, `conversations.rs:3498` and `external_events.rs:229`.

### 2.4 Managing an agent: four rules that disagree

| Path | Rule today |
| --- | --- |
| Web (`external_agents.rs:1282-1319`) | A person, workspace owner or admin by role *name*, plus `ExternalAgentManage` in the agent's home project |
| MCP `agent_list` manage view (`mcp/agents.rs:442-480`) | A person, workspace `role IN ('owner','admin')` (`:466`), plus `role_allows` in the home project |
| OAuth "connect as agent" (`mcp/oauth.rs:1272-1297` `agent_choices`, `:1300-1335` `managed_agent`) | Project `role IN ('owner','admin') AND custom_role_allows(role_id,'agents.manage')` in the home project, plus any enabled workspace membership |
| Connected apps list and revoke (`connected_apps.rs:100-118`, `:140-160`) | Same as OAuth |
| Profile agents (`agent_profiles.rs:202-211`, `:562`) | A person with `SettingsManage` in the project |

The OAuth rule lets a workspace *member* who is a project admin mint a grant that acts as the
agent (`mcp/oauth.rs:482-520`) and so reaches every project the agent is in, including ones
the person is not in (health check A2; the reach is inferred from
`agent_connection_can_access`).

### 2.5 Modules and access

Role `modules_off` narrows modules for holders, but the HTTP gate follows only workspace
switches and only `/projects/{id}` and `/organizations/{id}` paths (`modules/gate.rs:1-14`,
`:72-91`); page routes by id check no module (`memory.rs:1353`, `:1374`;
`page_docs/http.rs`), and MCP checks the call's home project (`mcp.rs:3899-3940`). So whether
"modules off" is a permission depends on the path (health check F5).

### 2.6 Web

The session hides role permissions (`auth/mod.rs:123`). The web gates by role name in at least
14 places (appendix A, web part), for example `apps/web/src/settings/registry.ts:49-50`,
`apps/web/src/settings/workspace/MembersPage.tsx:67`,
`apps/web/src/modules/TurnedOffPage.tsx:19`, `apps/web/src/work-items/TaskActivity.tsx:128`.
Permission keys are copied by hand into `apps/web/src/settings/workspace/roles.ts:8-39`.

### 2.7 Guards that exist

`policy_inventory.rs` scans every `.route(` and requires each mutating route to carry an
`Action` (`policy_inventory.rs:1820-1883`), but nothing checks the handler uses it.
`mcp/tool_permissions.rs:271-287` requires every tool to name its permission once.
`agent_parity.rs` maps web writes to tools by name only. `tests/integration/workspace_isolation.rs`
and `write_isolation.rs` are the two-workspace nets; `roles_and_access.rs` (5,589 lines) holds
the role tests.

## 3. The target design

### 3.1 Accounts

One table of accounts, one kind column with four values, and mechanics, never permissions,
that differ by kind.

```sql
-- Expand on the existing table (renamed to accounts only in K1.31; see 4.6).
ALTER TABLE actors
    ADD COLUMN account_kind TEXT CHECK (account_kind IN ('person', 'agent', 'app', 'system')),
    -- Ruling R19: every account is global. Agents and apps have an owner: a person
    -- ("my agent") or a workspace ("the team's agent"). Structural (who is accountable,
    -- who may delete it), not a permission.
    ADD COLUMN owner_account_id UUID REFERENCES actors(id),
    ADD COLUMN owner_workspace_id UUID REFERENCES organizations(id),
    -- False for accounts that exist for plumbing and are not shown in directories
    -- (today's bare per-project execution workers). Not a permission.
    ADD COLUMN listed BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN created_by_actor_id UUID REFERENCES actors(id);

-- Kept in step while old code still inserts with `kind` only.
CREATE FUNCTION actors_account_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.account_kind IS NULL THEN
        NEW.account_kind := CASE NEW.kind
            WHEN 'human' THEN 'person'
            WHEN 'external_agent' THEN 'agent'
            WHEN 'execution_worker' THEN 'agent'
            WHEN 'integration' THEN 'app'
            ELSE 'system' END;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER actors_account_kind BEFORE INSERT OR UPDATE OF kind ON actors
    FOR EACH ROW EXECUTE FUNCTION actors_account_kind();

-- After the backfill (K1.5): agents and apps have exactly one owner, people
-- and the system none.
ALTER TABLE actors ADD CONSTRAINT actors_owner CHECK (
    CASE WHEN account_kind IN ('agent', 'app')
         THEN num_nonnulls(owner_account_id, owner_workspace_id) = 1
         ELSE num_nonnulls(owner_account_id, owner_workspace_id) = 0 END
) NOT VALID;   -- VALIDATE in K1.8, after worker accounts are backfilled
CREATE INDEX actors_owner_account ON actors (owner_account_id) WHERE owner_account_id IS NOT NULL;
CREATE INDEX actors_owner_workspace ON actors (owner_workspace_id) WHERE owner_workspace_id IS NOT NULL;
```

Design choices:

- **Keep `kind`, add `account_kind`.** About 170 string comparisons read `kind`; renaming its
  values in place would break every one at once and every open branch. A second column kept
  by trigger lets code move file by file, and K1.31 drops `kind`.
- **Every account is global; agents and apps have an owner** (ruling R19). `actors` already
  has no workspace column. An account joins any number of workspaces by membership, the same
  act for every kind (3.11). The owner is a person or a workspace; today's agents get
  `owner_workspace_id` from their one organization membership
  (`external_agents.rs:388-397`).
- **No merge of duplicate agents.** The same real agent connected to two projects is two
  actors today. Merging would change ids and move history, which the brief forbids. Both stay
  accounts; a manager can retire one (status `disabled`), and its history stays attached.

Rust (`accounts` module, new file `accounts.rs`):

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind { Person, Agent, App, System }

impl AccountKind {
    /// The one parser, replacing the seven copies (and `policy::ActorKind`'s
    /// readers) as files move. Accepts both old and new stored values.
    pub fn parse(stored: &str) -> Result<Self, AppError>;
}

pub enum Owner { Account(Uuid), Workspace(Uuid) }
pub struct Account { pub id: Uuid, pub kind: AccountKind, pub active: bool,
                     pub owner: Option<Owner> }        // Some for agents and apps only

/// Mechanics only. Each answers a "how", never a "may".
impl Account {
    pub fn signs_in_interactively(&self) -> bool;   // has a users row (password, SSO)
    pub fn has_inbox(&self) -> bool;                // people get inbox cards and push
    pub fn receives_deliveries(&self) -> bool;      // agents get events through A2
}
pub async fn load(conn: &mut PgConnection, id: Uuid) -> Result<Option<Account>, AppError>;
pub async fn create_person(tx, NewPerson) -> Result<Account, AppError>;   // K1.29
pub async fn create_agent(tx, NewAgent) -> Result<Account, AppError>;     // K1.6
```

`policy::ActorKind` stays as an alias of `AccountKind` until K1.31; `access` never reads it.

### 3.2 An agent account, its managers and its outside reference

```sql
-- Who is named on an agent (or app) account as able to manage it.
CREATE TABLE account_managers (
    account_id UUID NOT NULL REFERENCES actors(id),
    manager_account_id UUID NOT NULL REFERENCES actors(id),
    workspace_id UUID REFERENCES organizations(id),
    added_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- workspace_id records where the naming happened (audit); naming is global (R19),
    -- so it is nullable for managers the owning person names.
    PRIMARY KEY (account_id, manager_account_id),
    CHECK (account_id <> manager_account_id)
);
CREATE INDEX account_managers_manager ON account_managers (manager_account_id, workspace_id);

-- Which agent elsewhere this account is. Optional; at most one active per account.
CREATE TABLE agent_outside_refs (
    id UUID PRIMARY KEY,
    -- The outside reference is global (R19). Set only on references copied from today's
    -- per-workspace duplicates ("nothing merges"), so two old accounts for one real agent
    -- can both keep theirs; NULL for every new reference, which is unique server-wide.
    workspace_id UUID REFERENCES organizations(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    -- Who issued the outside id: an opaque issuer string the host declares when it
    -- enrols (A1), e.g. a host installation id plus a context. Never a machine name.
    issuer TEXT NOT NULL CHECK (char_length(issuer) BETWEEN 1 AND 200),
    external_id TEXT NOT NULL CHECK (char_length(external_id) BETWEEN 1 AND 200),
    -- Earlier (issuer, external_id) pairs this agent was moved or re-keyed from,
    -- newest first: [{"issuer": "...", "externalId": "...", "until": "..."}].
    lineage JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(lineage) = 'array' AND octet_length(lineage::text) <= 16000),
    status TEXT NOT NULL CHECK (status IN ('active', 'pending_rebind', 'detached')),
    host_id UUID,                     -- A1's hosts(id) when known; not identity
    attached_by_actor_id UUID REFERENCES actors(id),
    attached_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    detached_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX agent_outside_refs_one_active
    ON agent_outside_refs (account_id) WHERE status = 'active';
CREATE UNIQUE INDEX agent_outside_refs_identity
    ON agent_outside_refs (COALESCE(workspace_id, '00000000-0000-0000-0000-000000000000'), issuer, external_id)
    WHERE status <> 'detached';
```

Operations (each an operation with an agent tool from day one, names per K2's
`<thing>_<verb>`):

| Operation | Input | Permission | Effect |
| --- | --- | --- | --- |
| `agent_create` | name, handle, profile, owner (the caller, or this workspace), role id (default: the workspace setting), spaces to join, managers | `agents.create` (workspace) and the role within what the caller holds | Account with its owner, membership in this workspace, chosen space memberships, caller named as manager; event `agent.created` |
| `account_workspace_add` | an owned account (agent or app), role, spaces | `members.invite` for that role in the target workspace (`may_grant`), **and** a manager's agreement: the adder manages the account, or a manager accepts an invitation, as a person accepts their own; refused or held per the target's `access.outside_accounts` setting (3.11) | Membership row with the role; event `workspace.member.added` |
| `agent_update` | profile fields, expected version | manage rule (3.6) | Profile change; event |
| `agent_role_set` | role id, scope (workspace or a space) | manage rule plus `members.manage` there; grant within caller's | Role change |
| `agent_managers_set` | add or remove manager accounts | manage rule | Rows in `account_managers`; a manager must hold `agents.manage` |
| `agent_retire` / `agent_restore` | | manage rule | `actors.status`; credentials revoked on retire |
| `agent_ref_attach` | issuer, external id, lineage | manage rule | `agent_outside_refs` row `active` |
| `agent_ref_rebind` | new issuer and external id, the old pair as lineage proof | manage rule (from a person or app) **or** the host that holds the old reference (A1), which creates `pending_rebind` | Host-initiated: a request (A1) to a manager; on accept, the old pair moves into `lineage`, the new pair becomes active, the account, roles, name and history stay |
| `agent_ref_detach` | | manage rule | `status = 'detached'` |
| `agent_copy` | source account, new name, "copy memberships" | `agents.create` and manage rule on the source | A **new** account; memberships copied if asked; no history copied |

A rebind found by lineage (a host says "agent X was moved from Y") never applies silently: it
always waits for a manager's confirmation, because a forged lineage would hand one agent's
account to another (R1 section 2.5).

### 3.3 Roles, completely as data

#### Permission shape

Every catalogue entry is either a **level on a kind of thing** or a **named capability**.

- Kinds of thing: `space`, `conversation`, `task`, `page`, `whiteboard`, `sheet`,
  `collection` (its structure), `record`, `run`, `code`, `agent`, `member`, `role`,
  `settings`, `automation`, `webhook`, `file`. A module adds its own kinds (M1).
- Levels, ordered: `see` < `comment` < `suggest` < `edit_own` < `edit` < `manage`. Holding a
  level implies every lower level on the same kind. `edit_own` means "edit things this
  account created" and exists because today's page rule is "an agent edits its own pages
  directly"; it is equally useful for a junior teammate.
- Capabilities are verbs that are not levels: `projects.create`, `tasks.create`,
  `tasks.archive`, `tasks.workflow`, `runs.launch`, `runs.approve`, `requests.answer_sensitive`,
  `replies.steer`, `conversations.members`, `members.invite`, `directory.view`, and so on.
- **Modules off is a permission.** Every entry names its module. A module that is off for the
  workspace, the space or the holder's role (`modules_off`) removes every entry of that module
  from the holder's effective set, so the HTTP routes, MCP tools, lists and search all refuse
  the same way (closes F5's question in favour of "permission").

Stored keys keep their meaning forever: the existing 30 keys stay, so custom roles saved
today mean the same tomorrow. Each gets `kind`/`level` or `capability` metadata:

| Existing key | Becomes | New keys added beside it |
| --- | --- | --- |
| `projects.view` (required) | `space` `see` | |
| `projects.create` | capability | |
| `projects.manage` | `space` `manage` | |
| `tasks.view` / `tasks.edit` | `task` `see` / `edit` | `tasks.comment`, `tasks.suggest`, `tasks.edit_own` |
| `tasks.create`, `tasks.delete`, `tasks.workflow` | capabilities | |
| `channels.post` | `conversation` `comment` | `conversations.see` (required, implied today) |
| `channels.create`, `channels.moderate` | capabilities | `conversations.members` (change group members) |
| `pages.edit` | `page` `edit` | `pages.see` (required), `pages.comment`, `pages.suggest`, `pages.edit_own` |
| `whiteboards.edit` | `whiteboard` `edit` | `whiteboards.comment`, `whiteboards.suggest`, `whiteboards.edit_own` |
| `collections.edit` / `collections.build` | `record` `edit` / `collection` `manage` | `collections.comment`, `collections.suggest` |
| `sheets.edit` | `sheet` `edit` | `sheets.comment`, `sheets.suggest`, `sheets.edit_own` |
| `runs.launch`, `runs.approve`, `runs.interrupt`, `runs.accounts` | capabilities | `requests.answer_sensitive`, `replies.steer`, `runs.environments` |
| `code.view`, `code.publish`, `code.manage` | `code` `see`, capability, `code` `manage` | |
| `agents.manage` | `agent` `manage` | `agents.create`, `agents.manage_any` |
| `members.*`, `directory.view`, `roles.manage`, `settings.manage` | as today | `webhooks.manage`, `automations.manage`, `spaces.browse` (see open spaces you're not in; guests lack it) |

`people_only` is removed from the catalogue (K1.14). `required` stays.

#### Back-compatibility of new keys

A custom role made before a key existed does not hold it (`policy.rs:185-189`). Each slice
that adds a key adds, in the same migration, the key to every custom role that holds its
"parent" (for example `pages.comment` to every role holding `pages.edit`, because page
comments need `PageEdit` today, `page_docs/discussion.rs:85-90`). The backfill is one
`UPDATE workspace_roles SET permissions = array_append(...) WHERE ... AND NOT (key = ANY(permissions))`,
idempotent, so an interrupted run resumes by running again. Built-in roles get the new key's
defaults from the catalogue, so for people nothing changes.

#### Tables

```sql
-- The catalogue as rows, so SQL and the web read the same list as Rust.
-- Written by migration for the core catalogue; M1 adds module rows.
CREATE TABLE permission_catalogue (
    key TEXT PRIMARY KEY CHECK (key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    module TEXT NOT NULL,
    thing_kind TEXT,                    -- NULL for a capability
    level TEXT CHECK (level IN ('see','comment','suggest','edit_own','edit','manage')),
    scope TEXT NOT NULL CHECK (scope IN ('space', 'workspace', 'both')),
    required BOOLEAN NOT NULL DEFAULT false,
    -- Built-in starting defaults, for seeding starting roles in new workspaces.
    default_for TEXT[] NOT NULL DEFAULT '{}',
    CHECK ((thing_kind IS NULL) = (level IS NULL))
);

ALTER TABLE workspace_roles
    -- Starting roles hold their permissions as data too (K1.10).
    -- `permissions` is what an admin ticked; `effective_permissions` is that set
    -- closed under level implication and `required`, written by the server on
    -- every save so SQL checks are one `= ANY`.
    ADD COLUMN effective_permissions TEXT[] NOT NULL DEFAULT '{}',
    -- The tier used for "only owners give admin-tier roles" and for ownership.
    -- Replaces base_role's other job (clamping) once built-in rows hold data.
    ADD COLUMN tier TEXT NOT NULL DEFAULT 'member'
        CHECK (tier IN ('owner', 'admin', 'member', 'viewer', 'guest', 'worker')),
    -- When a holder acts on someone's request, intersect with the requester's
    -- access (the end state's "limited to what that person could have done").
    ADD COLUMN act_within_requester BOOLEAN NOT NULL DEFAULT false,
    -- Starting roles track catalogue defaults until an admin edits them.
    ADD COLUMN customized BOOLEAN NOT NULL DEFAULT false;
-- `agent_limit` keeps its column name until K1.31 and is shown as
-- "What apps acting for holders may do": it is a delegation limit, not about agents.
```

Rules, all data:

- **Starting roles** in every workspace: owner, admin, member, viewer, guest (new), and a
  hidden `worker` (tier `worker`, no permissions, for today's worker memberships until A1
  retires them). Owner always holds every key (enforced in code and by
  `workspace_roles_owner_keeps_modules`, already there). Starting roles are editable except
  owner; `customized = false` rows receive new catalogue defaults automatically.
- **Custom roles** are what they are today, minus the clamp to a base role's code-defined
  set: a custom role holds what its creator ticked, within what its creator holds
  (`roles.rs:1011-1029` already does that check).
- **The default role for a newly added agent** is a workspace setting
  `access.default_agent_role_id` in the existing settings registry
  (`migrations/20261016070200_settings.sql`), with the default invitation role
  (`workspace_roles.is_default`) beside it. This is the only place "agent" and "role" meet.
  The default is the same role as a newly invited person (Member) in new and existing
  workspaces alike (ruling R5). "Member, suggests" (K1.12, 4.5) is given once to the agents
  that exist at the switch and is never a default.
- **Per-space roles**: a space membership names a role (`project_memberships.role_id`, then
  K4's `space_members`). In a space, the space's grants count; the workspace role applies to
  workspace-scope permissions. That is today's rule (`roles.rs:13-15`), kept.
- **Grants** (K1.27): `role_grants` lets a user group hold a role in a workspace or space, and
  an account holds the union of its direct and group grants there.

```sql
CREATE TABLE role_grants (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),            -- NULL: the workspace scope
    grantee_account_id UUID REFERENCES actors(id),
    grantee_group_id UUID REFERENCES account_groups(id),
    role_id UUID NOT NULL REFERENCES workspace_roles(id),
    granted_by_actor_id UUID REFERENCES actors(id),
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((grantee_account_id IS NULL) <> (grantee_group_id IS NULL))
);
CREATE UNIQUE INDEX role_grants_account
    ON role_grants (workspace_id, COALESCE(space_id, workspace_id), grantee_account_id)
    WHERE grantee_account_id IS NOT NULL;
CREATE INDEX role_grants_group ON role_grants (grantee_group_id) WHERE grantee_group_id IS NOT NULL;
```

#### Permissions requested by other workstreams

The catalogue is K1's; these keys are added by the slice named, with the defaults given, so
other plans don't invent their own names. Back-compatibility as above (a custom role holding
the parent key gets the new one).

| Key | Kind or capability | Requested by | Default holders | Parent for back-fill | Added in |
| --- | --- | --- | --- | --- | --- |
| `channels.browse` | capability | C2 | owner, admin, member | `channels.post` | K1.27 |
| `channels.create_private` | capability | C2 | owner, admin, member | `channels.create` | K1.27 |
| `channels.mention_everyone` | capability | C2 | owner, admin | `channels.moderate` | K1.27 |
| `direct.start` (start a DM or group; checked in a workspace the two accounts share, ruling R20; K4 names it) | capability | K4, C2 | owner, admin, member | `channels.post` | K1.9 |
| `views.share` | capability | T1 | owner, admin, member | `tasks.edit` | K1.9 |
| `dashboards.view`, `dashboards.edit` | kind `dashboard` see / edit | T1 | see: all but guest; edit: owner, admin, member | `tasks.view` / `tasks.edit` | K1.9 |
| `data.connect`, `data.share` | capabilities | T1 | owner, admin | `collections.build` | K1.9 |
| `runs.report`, `requests.raise`, `requests.answer` | capabilities | A1 | `runs.report`: none by default (granted to hosts' app accounts); others: owner, admin, member | `runs.launch` | K1.14 |
| `hosts.manage` | capability | A1 | owner, admin | `settings.manage` | K1.14 |
| `modules.build`, `modules.manage` | capabilities | M1 | owner, admin | `settings.manage` | K1.11 |
| `workspace.export`, `workspace.delete` | capabilities | H1 | export: owner, admin; delete: owner | `settings.manage` | K1.19 |
| `credentials.manage`, `secrets.manage` | capabilities | K2 | owner, admin | `settings.manage` | K1.23 |
| `conversations.continue_agents` (replaces `replies.steer` for the "continue" rows of appendix D), `conversations.stop_agents`, `conversations.agent_settings` | capabilities | A2 | owner, admin, member | `channels.post` | K1.14 |
| `budgets.manage` | capability | A2 | owner, admin | `settings.manage` | K1.14 |
| `scopes.conversations.read` (a consent scope: an app may read conversations for its account) | capability | A2 | owner, admin, member | `channels.post` | K1.14 |

Cycle-breakers: A1.8 and A2.8 add their permission variants to `policy.rs` (the interim
`Permission` enum) so they can merge before K1; K1.14 moves those variants into the catalogue
with the keys above.

`channels.moderate` also covers `comments` conversations (C1.4's mapping of
`work_items/comments.rs:782`): moderating a thing's discussion is moderating a conversation.
`account_groups.handle` (3.4) is unique per workspace and is what `@design` mentions resolve
(C2.18).

K4's earlier names `spaces.create`, `spaces.manage` and `workspace.settings` are the existing
keys `projects.create`, `projects.manage` and `settings.manage` (stored keys keep their
meaning forever); K4 uses those.

### 3.4 Guests, user groups and narrowed delegation (data here, UI in C2)

- **Guests.** A person whose workspace grant is the starting role `guest` (tier `guest`):
  `space.see` and `conversation.comment` only in spaces they are explicitly a member of; no
  `directory.view`, no `spaces.browse`, so open channels and the directory are hidden. A
  membership may carry `expires_at` (new column on `organization_memberships` and
  `project_memberships`); expiry is a scheduled job that disables the membership. A guest is
  never different by kind: an agent can be a guest too.
- **User groups.**

  ```sql
  CREATE TABLE account_groups (
      id UUID PRIMARY KEY,
      workspace_id UUID NOT NULL REFERENCES organizations(id),
      handle TEXT NOT NULL CHECK (handle ~ '^[a-z][a-z0-9_-]{1,31}$'),
      name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
      description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 300),
      created_by_actor_id UUID REFERENCES actors(id),
      version BIGINT NOT NULL DEFAULT 1,
      created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
      UNIQUE (workspace_id, handle)
  );
  CREATE TABLE account_group_members (
      group_id UUID NOT NULL REFERENCES account_groups(id) ON DELETE CASCADE,
      account_id UUID NOT NULL REFERENCES actors(id),
      added_by_actor_id UUID REFERENCES actors(id),
      created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
      PRIMARY KEY (group_id, account_id)
  );
  ```

  Uses: a role grant to a group (3.3), `@group` mentions (C2), "who may answer this request"
  (A1), conversation membership by group (C2). Operations `group_create`, `group_update`,
  `group_members_set`, `group_archive`, permission `members.groups` (capability, admins by
  default).
- **Delegation narrowed to part of a person's access.** A delegation credential (3.7, kind
  `delegation`: today's OAuth grant) carries `limit_permissions TEXT[]` and
  `limit_spaces UUID[]`. The effective set when acting through it is
  `holder's grants ∩ role.agent_limit ∩ membership.agent_limit ∩ credential.limit_permissions`,
  and only in `limit_spaces` if set. This is today's `AgentLimits` computation
  (`roles.rs:155-221`) moved into `access` and keyed on "acting through a delegation", not on
  "is a person". It works the same when an app acts for an agent (today's "connect as agent"
  grant, `migrations/0039_oauth_agent_grants.sql`).

### 3.5 The access module

One module, `access/` (`access/mod.rs`, `access/holding.rs`, `access/sql.rs`,
`access/tokens.rs`, `access/compare.rs`, `access/guard.rs`), is the only place that reads
roles, memberships, delegation limits, token scopes, module switches and account status to
make a decision. It builds on `policy::authorize`: the pure core moves in as
`access::decide` with the same exhaustive tests, minus the kind branches.

```rust
/// Who is calling, resolved server-side (never from client input).
#[derive(Clone, Debug)]
pub struct Caller {
    pub account_id: Uuid,
    /// Mechanics only (inbox, sign-in). `access` never branches on it.
    pub account_kind: AccountKind,
    /// The credential this call came through and what it says about the call.
    pub via: Via,
    /// The account whose request this action serves, if any (3.8).
    pub requested_by: Option<Uuid>,
    /// The run this call is part of (a run's scoped token, A1), for the event envelope.
    pub run_id: Option<Uuid>,
    /// Which workspaces this credential reaches (ruling R19): every workspace the account
    /// is in, or one, or listed spaces. A call from a credential that reaches more than one
    /// names its workspace (K2); `access` then checks that workspace is within `reach`.
    pub reach: Reach,
    /// Per-request memo of holdings, so a request loads each scope once.
    memo: Arc<Mutex<HashMap<Scope, Holding>>>,
}

#[derive(Clone, Debug)]
pub enum Via {
    Session { session_id: Uuid },                         // the browser
    Secret { credential_id: Uuid },                       // long-lived secret
    AccessToken { token_id: Uuid, credential_id: Uuid, scope: TokenScope },
    Delegation { credential_id: Uuid, app_client_id: Uuid, limit: DelegationLimit },
    System { job: &'static str },                         // jobs, automations
    /// A host's app account acting for an agent bound to it in A1's `host_agents`
    /// (ruling R7). Decisions are the agent's holding; the app is recorded as "via".
    ActingFor { app_account_id: Uuid, credential_id: Uuid },
}

#[derive(Clone, Debug)]
pub enum Reach { AllMemberships, Workspace(Uuid), Spaces { workspace: Uuid, spaces: Vec<Uuid> } }

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Scope { Workspace(Uuid), Space(Uuid) }

/// What the caller holds in one scope after every narrowing.
#[derive(Clone, Copy, Debug)]
pub struct Holding { pub scope: Scope, pub permissions: PermissionSet, pub tier: Tier }

/// The thing an operation acts on. K3's thing registry later supplies
/// `space_of` and `visible_sql` for every kind; until then `access::things`
/// holds them for the kinds that exist.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    Workspace(Uuid),
    Space(Uuid),
    Thing { kind: ThingKind, id: Uuid },
}

/// Proof that a check passed, for one permission on one target. K2's
/// operation runner requires one whose permission equals the operation's
/// declared permission (guard 6.2).
#[must_use]
pub struct Grant { pub permission: Permission, pub target: Target, pub mode: WriteMode }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode { Direct, Propose }

#[derive(Debug)]
pub enum Denied {
    /// The caller can't see the thing: answer 404, as today.
    NotFound,
    /// Visible, but the role, a delegation limit or a token scope leaves this out.
    Permission { permission: Permission, because: DenyReason },
    ModuleOff { module: &'static str },
    Disabled,                 // account, membership or credential disabled
}

/// The one question: may this caller do this to this thing?
pub async fn check(conn: &mut PgConnection, caller: &Caller, need: Permission, target: Target)
    -> Result<Result<Grant, Denied>, AppError>;

/// For writes on kinds with levels: Direct with `edit` (or `edit_own` on the
/// caller's own thing), Propose with only `suggest`, else Denied. C1 calls
/// this; nothing else decides suggest-versus-edit.
pub async fn write_mode(conn: &mut PgConnection, caller: &Caller, kind: ThingKind, target: Target)
    -> Result<Result<Grant, Denied>, AppError>;
/// Same, locking membership rows, inside K2's write transaction.
pub async fn write_mode_locked(tx: &mut Transaction<'_, Postgres>, caller: &Caller, kind: ThingKind,
                               target: Target) -> Result<Result<Grant, Denied>, AppError>;

/// What the caller holds in a scope (for "my permissions", tool lists, the web).
pub async fn holding(conn: &mut PgConnection, caller: &Caller, scope: Scope)
    -> Result<Option<Holding>, AppError>;

/// Same as `check`, locking the membership rows (`FOR SHARE`) like
/// `work_item_commands::lock_membership`, inside a write transaction.
pub async fn check_locked(tx: &mut Transaction<'_, Postgres>, caller: &Caller,
                          need: Permission, target: Target) -> Result<Result<Grant, Denied>, AppError>;

/// The rule for lists and search, from the same catalogue: a SQL fragment and
/// its binds that keep only rows the caller may `need`.
pub fn sql_filter(caller: &Caller, need: Permission, space_column: &str) -> SqlFilter;
pub fn sql_filter_thing(caller: &Caller, need: Permission, kind: ThingKind, alias: &str) -> SqlFilter;

pub struct SqlFilter { pub sql: String, pub binds: Vec<SqlBind> }

/// Managing an account (3.6).
pub async fn may_manage_account(conn: &mut PgConnection, caller: &Caller, account_id: Uuid)
    -> Result<Result<Grant, Denied>, AppError>;
/// Nobody grants more than they hold: `role` (or token scope, or delegation
/// limit) is within the caller's holding in `scope`.
pub async fn may_grant(conn: &mut PgConnection, caller: &Caller, scope: Scope, grants: PermissionSet)
    -> Result<bool, AppError>;
```

**How the functions relate (ruling R2).** `access::decide` is the pure core (no database: a
holding, a need, a thing's facts in, a decision out); only `access` calls it. `check` and
`check_locked` answer allow or deny. `write_mode` and `write_mode_locked` answer the three-way
question: `Ok(Grant { mode: Direct })` is allow, `Ok(Grant { mode: Propose })` is suggest
only, `Err(Denied)` is deny. K2's dispatcher calls `check_locked` for an operation that is not
marked proposable and `write_mode_locked` for one that is (K2's "can be proposed" flag, ruling
R14); C1 calls `write_mode` to draw the editor. Stopgaps until K1.2 lands, each with the slice
that removes it: K2's `ops/access_shim.rs` (deleted by K2.17), K4's `workspace_member_role`
(deleted by K1.15), K3's `things::visible_ids` body (replaced by K1.20).

How a decision composes, in this order (the same order in Rust and SQL):

1. Account active (`actors.status`), and if it has a login, the login active
   (`users.status`). Replaces the `users` join in `actor_can_access_project`
   (`migrations/0042_agent_interface.sql:66-74`), which today shuts out any account without a
   `users` row unless it has a connection.
2. Credential active (session, secret, token or delegation not revoked or expired).
3. Workspace membership enabled, and the workspace within the credential's `reach`; for a
   space-scope permission, an enabled space membership or a role grant (direct or through a
   group) there, read through K4's `space_membership` / `space_readable_by` helpers so open
   spaces follow K4's rule. The agent's connection project no longer matters (3.7). The
   direct scope (K4, ruling R20) has no memberships, so no role grants anything there.
4. Effective permissions = union of the role(s)' `effective_permissions` in that scope.
5. Minus modules off: the workspace's switches, the space's, and the role's `modules_off`.
6. Narrowed by the credential: a delegation's limit (3.4), a token's `permissions` and
   `spaces` (3.7).
7. Narrowed by the requester when the role says `act_within_requester` and `requested_by` is
   set: intersect with the requester's holding in the same scope.
8. The thing's own visibility: for a thing, `conversation_visible_to` (composed as in 3.10;
   a DM or group is visible to its participants only, and no role, admin or owner passes),
   `whiteboard_visible_to`, `pull_request_visible_to`, `ai_suggestion_visible_to` or the
   kind's registry entry; a token scoped to things must name it. Not visible is `NotFound`.
9. The permission is in the set, or `Denied::Permission`. For levels, a higher held level
   satisfies a lower need; `edit_own` satisfies `edit` when the thing's creator is the caller.

The SQL side is two functions generated from the same rules, written in one migration and
replaced (`CREATE OR REPLACE`) whenever the rules change:

```sql
-- Spaces in which `account` (through `credential`, NULL for a session) holds
-- `permission`, after steps 1 to 7. Called once per statement as an InitPlan:
--   WHERE t.project_id = ANY(ARRAY(SELECT access_spaces($1, $2, 'tasks.see')))
CREATE FUNCTION access_spaces(account UUID, credential UUID, permission TEXT)
RETURNS SETOF UUID LANGUAGE sql STABLE AS $$ ... $$;

-- Single-space form, for row-level use in joins.
CREATE FUNCTION access_allows(account UUID, credential UUID, space UUID, permission TEXT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$ ... $$;
```

`access_spaces` is built over K4's `spaces_readable_by(account)` (K4.2), adding steps 4 to 7.
Both read `permission_catalogue` (for the module of a key), `workspace_roles.effective_permissions`,
memberships or grants, module switches in `settings`, and the credential's limits. The
`requested_by` narrowing (step 7) is applied in Rust only, because lists are shown to the
caller, not acted on for someone else; the SQL comment says so and a test pins it.
`sql_filter_thing` adds the kind's visibility function (step 8). `sql_filter` emits the
`= ANY(ARRAY(SELECT ...))` form so Postgres computes the space list once per statement.

**Performance.** A single check is one indexed query that joins account, credential,
workspace membership, space membership and role rows (all primary-key lookups), memoized on
`Caller` for the rest of the request. No process-wide cache of decisions is kept, because
roles change in another process (K5) and today's 15-second socket revalidation
(`realtime.rs:30`) already bounds staleness for long-lived connections. The only process cache
is the immutable catalogue. Lists use `access_spaces` once per statement. K1.2 adds a
`tracing` span per check with its duration so the compare runs (4.3) also measure cost.

**What moves onto it, one path at a time** (each is a slice in section 5):

| Path today | Moves to |
| --- | --- |
| Per-module `Subject::new` builders (appendix F) | `access::check` / `check_locked` |
| `roles::standing`, `roles::project_holding`, `roles::app_access`, `roles::agent_union` | `access::holding` (they become thin wrappers, then go) |
| `workspace::WorkspaceRole`, `caller_role`, `member_manager_role`, `caller_may` | `access::check` with the workspace permissions |
| Raw `role IN (...)` SQL (appendix A) | `access_spaces` / `access_allows`, or `tier` for ownership |
| `custom_role_allows`, `role_allows_sql`, `projects_allowing` (appendix B) | `access::sql_filter` |
| `agent_limit_hides`, `agent_limit_projects_allowing` (appendix C) | gone: the credential's limit is inside `access_spaces` |
| MCP `caller_reach` + `TOOL_PERMISSIONS` + `module_off_here` + `scoped_project` | `access::holding` for the tool list; `access::check` against the resolved target for the call (with K2's argument targets) |
| `actor_can_access_project`, `agent_connection_can_access` | kept as SQL names, re-implemented as `access_allows(..., 'projects.view')` |

### 3.6 Managing an agent: one rule

> A caller may manage an agent (or app) account when they are its owning person; or hold
> `agents.manage_any` in its owning workspace; or are named in `account_managers` for it and
> hold `agents.manage` in a workspace the account is a member of. What they give it (a role, a
> token scope, a space) is within what they hold there. (Ruling R19: "the owning person, or
> whoever holds the manage-agents permission in the owning workspace, plus accounts the owner
> names". Ruling R34 made it precise exactly as written here: holding `agents.manage` alone is
> not enough, which keeps the escalation fix K1.4 tests.)

- One function: `access::may_manage_account`. Managing the account (profile, credentials,
  outside reference, retire) is global. A workspace the account merely belongs to controls only
  its own membership: removing it, its role and spaces there (`members.manage`), never the
  account.
- `agents.manage_any` is held by the owner starting role only, so a workspace can't lose
  control of an agent whose managers left.
- Everything goes through `access::may_manage_account` and `access::may_grant`: the web
  (`external_agents.rs:1282-1319`), MCP `agent_list` (`mcp/agents.rs:442-480`), OAuth
  (`mcp/oauth.rs:1272-1335`), connected apps (`connected_apps.rs:100-160`), agent profiles
  (`agent_profiles.rs:202-211`, `:562`), setup links and WorkOS bindings
  (`auth/workos/agents.rs:337-600`).
- Backfill: each existing agent gets as managers its connection's `created_by_actor_id` and
  every enabled workspace owner and admin today (people who pass the web rule now). That keeps
  who can manage which agent the same, except that the OAuth gap closes.

### 3.7 Credentials for accounts

```sql
CREATE TABLE account_credentials (
    id UUID PRIMARY KEY,
    -- Connections belong to the account (ruling R19). NULL: reaches every workspace the
    -- account is a member of; set: narrowed to that one workspace (`limit_spaces` narrows
    -- further). Legacy copies keep their workspace.
    workspace_id UUID REFERENCES organizations(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    -- issuer: no secret; owns server-issued scoped tokens for one account (K1.24)
    kind TEXT NOT NULL CHECK (kind IN ('secret', 'federated', 'delegation', 'setup_link', 'issuer')),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    -- secret and setup_link: opaque, digest only (aw_sk_...)
    token_prefix TEXT UNIQUE,
    token_digest BYTEA UNIQUE CHECK (token_digest IS NULL OR octet_length(token_digest) = 32),
    -- federated: trust an outside issuer's signed assertion about `subject`
    trusted_issuer_id UUID REFERENCES trusted_issuers(id),
    subject TEXT,
    -- delegation: an app (OAuth client) acting for this account
    app_client_id UUID REFERENCES mcp_oauth_clients(id),
    -- optional: the agent connection (A2's `agent_connections`) this credential belongs to,
    -- so a credential resolves to its connection; NULL for people's and apps' credentials
    connection_id UUID,
    show_via BOOLEAN NOT NULL DEFAULT true,
    -- narrowing, NULL = none
    limit_permissions TEXT[],
    limit_spaces UUID[],
    max_token_ttl INTERVAL NOT NULL DEFAULT interval '1 hour'
        CHECK (max_token_ttl <= interval '24 hours'),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    rotated_from_id UUID UNIQUE REFERENCES account_credentials(id),
    old_secret_valid_until TIMESTAMPTZ,       -- rotation overlap, at most 24 hours
    created_by_actor_id UUID REFERENCES actors(id),
    revoked_by_actor_id UUID REFERENCES actors(id),
    revoked_at TIMESTAMPTZ,
    revoked_reason TEXT,
    -- provenance while legacy tables are copied (K1.22); dropped in K1.31
    legacy_table TEXT, legacy_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (legacy_table, legacy_id),
    CHECK ((kind IN ('secret', 'setup_link')) = (token_digest IS NOT NULL)),
    CHECK ((kind = 'federated') = (trusted_issuer_id IS NOT NULL AND subject IS NOT NULL)),
    CHECK ((kind = 'delegation') = (app_client_id IS NOT NULL))
);
CREATE UNIQUE INDEX account_credentials_federated
    ON account_credentials (trusted_issuer_id, subject) WHERE status = 'active';

-- Outside issuers a workspace trusts for token exchange (an identity provider's
-- agent tokens, a host's workload identity). The WorkOS agent path becomes one row.
CREATE TABLE trusted_issuers (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    issuer TEXT NOT NULL,            -- the `iss` claim
    jwks_uri TEXT NOT NULL,
    audience TEXT NOT NULL,          -- what `aud` must be
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, issuer, audience)
);

-- Short-lived tokens, issued by exchange or by the server for a run or task.
CREATE TABLE access_tokens (
    id UUID PRIMARY KEY,
    credential_id UUID NOT NULL REFERENCES account_credentials(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    token_prefix TEXT NOT NULL UNIQUE,        -- aw_at_...
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    -- {"permissions": [...] | null, "spaces": [...] | null,
    --  "things": [{"kind": "task", "id": "..."}] | null}
    scope JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(scope::text) <= 8000),
    requested_by_actor_id UUID REFERENCES actors(id),
    run_id UUID,                               -- A1's runs(id)
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at <= created_at + interval '24 hours')
);
CREATE INDEX access_tokens_expiry ON access_tokens (expires_at) WHERE revoked_at IS NULL;
CREATE INDEX access_tokens_run ON access_tokens (run_id) WHERE run_id IS NOT NULL;
```

Design choices:

- **Opaque tokens checked against the database**, not self-contained signed tokens. A revoked
  credential stops every token at once, with no revocation list to share between processes,
  and the lookup is the one indexed query the bearer path already does
  (`mcp.rs:4775-4810`).
- **Where the agent's side can exchange, it does; otherwise a secret.** Three ways to get an
  access token, all at the existing OAuth token endpoint (`mcp/oauth.rs`), standard grant
  types only:
  - `grant_type=client_credentials` with `client_id` = the credential id and
    `client_secret` = the `aw_sk_` secret: returns a token of at most `max_token_ttl`.
  - `grant_type=urn:ietf:params:oauth:grant-type:token-exchange` with a `subject_token`
    signed by a `trusted_issuers` row; matched to a `federated` credential by `(issuer,
    sub)`. The WorkOS agent path (`auth/workos/agents.rs:236-335`) becomes this, and a host
    with workload identity later uses the same door.
  - The plain secret as a bearer, the simple option. It keeps working forever for agents that
    can't exchange (R1: today's Agent OS slots hold only long-lived bearers).
- **Narrow tokens for runs and tasks**, for A1 and A2:

  ```rust
  pub struct ScopedTokenRequest {
      pub account_id: Uuid,
      pub workspace_id: Uuid,
      pub permissions: Option<PermissionSet>,  // within the account's holding
      pub spaces: Option<Vec<Uuid>>,
      pub things: Option<Vec<(ThingKind, Uuid)>>,
      pub ttl: Duration,                       // at most 24h; default 1h
      pub requested_by: Option<Uuid>,
      pub run_id: Option<Uuid>,
  }
  /// Called by A1's run service when a run starts, and by A2 for a delivery
  /// that hands work to an agent. Refuses a scope wider than the account holds.
  pub async fn issue_scoped(tx: &mut Transaction<'_, Postgres>, issuer: &Caller,
                            request: ScopedTokenRequest) -> Result<IssuedToken, AppError>;
  pub async fn revoke_for_run(tx: &mut Transaction<'_, Postgres>, run_id: Uuid) -> Result<u64, AppError>;
  ```

  The `credential_id` of a scoped token is the account's `issuer` credential: a row with no
  secret whose only job is to own server-issued tokens and be revocable as a whole.
- **Legacy credential scopes map onto permissions.** `read` → every `see` level; `write` → no
  narrowing; `events` → delivery (A2); `approval_respond` → `runs.approve`; `worker_request`
  → `runs.launch`. They are copied into `limit_permissions` and stop being checked separately.
- **Reach and the workspace of a call** (ruling R19). A credential with `workspace_id` NULL
  reaches every workspace its account is in; every operation call through it names its
  workspace (K2's `{workspace}` path part or the MCP `workspace` argument), and `access`
  refuses a workspace outside `reach` as `NotFound`. A credential narrowed to one workspace
  needs no argument. Its event stream merges every workspace it reaches under K3's one global
  position (A2). A federated credential matched through a workspace's `trusted_issuers` row is
  narrowed to that workspace.
- **Rotation**: `credential_rotate` issues a new secret, links `rotated_from_id`, and keeps the
  old one valid until `old_secret_valid_until` (default now, at most 24 hours), so an
  unattended agent can be switched without a gap. **Revocation**: `credential_revoke`,
  immediate, cascades to its access tokens; retiring an agent revokes all its credentials.
- **Setup links** (`agent_setup_links.rs`) become `setup_link` credentials whose first fetch
  returns a `secret` credential, as today.
- Operations: `credential_list`, `credential_create` (secret or federated), `credential_rotate`,
  `credential_revoke`, `trusted_issuer_write`; permission: the manage rule (3.6) for another
  account's credentials, nothing extra for one's own delegations.

The resolver becomes one function used by MCP, the public API and the web's token routes:

```rust
/// Bearer token → Caller. Accepts aw_sk_, aw_at_, and during the window the
/// legacy aw_mcp_, aw_oauth_ and identity-provider JWT forms.
pub async fn resolve_bearer(state: &AppState, token: &str) -> Result<Option<Caller>, AppError>;
```

### 3.8 "Acted through which app, requested by whom"

Every write records three ids: the acting account, the credential it came through (from which
the app's name is read, so a renamed app shows its current name), and the requester.

- `Caller::via` and `Caller::requested_by` are set once at the edge. `requested_by` comes from
  the server, never the client: a scoped token's `requested_by` (a run's requester), a reply
  claim's asker (A2), an automation's rule owner, a scheduled message's author.
- In SQL, the existing `current_via_grant()` pattern is generalized: the edge runs
  `set_config('aw.via_credential', ...)` and `set_config('aw.requested_by', ...)` in the
  transaction, and new columns default from `current_via_credential()` and
  `current_requested_by()`. The existing `via_oauth_grant_id` columns keep their values; a
  view `acting_via` maps a legacy grant id to its copied credential id.
- `audit_events` gains `via_credential_id` and `requested_by_actor_id`. K3's event log carries
  the same three fields on every event (touch point). The audit page shows "Ada, through
  Claude Code, asked by Ben".

### 3.9 Web

- The session payload adds, per workspace, `permissions: string[]` (effective, keys); the
  project payload adds the caller's effective keys there. One hook `useCan(key, scope)`.
- `PERMISSION_KEYS` (`apps/web/src/settings/workspace/roles.ts:8-39`) is generated from the
  catalogue (by K2's generator, or until then checked against `/permissions` by a test).
- Role-name gates (appendix A, web) move to `useCan`. Pages that manage roles show levels per
  kind as a grid (kinds as rows, see/comment/suggest/edit own/edit/manage as columns) and
  capabilities as a list; that page registers in the layout registry with the Members module.
  The agent page shows "Managed by", "Outside reference", and credentials. New parts are lazy
  (W1 builds the shared pieces; K1 builds these settings pages).

### 3.10 Conversation visibility, split into helpers (ruling R3)

K1 owns the body of `conversation_visible_to(conversation, viewer)`; it is a composer of
helpers, each owned by one workstream, so several plans can change conversation access
without editing one function body at once:

```sql
-- K1's body, as C1 3.2 shows it: one branch per helper.
CREATE OR REPLACE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT COALESCE((
        SELECT CASE COALESCE(parent.visibility, c.visibility)
                 -- C1 owns: a thing's discussion is as visible as the thing (C1.3 adds the
                 -- 'subject' visibility and the helper; K1.20 replaces the helper's body
                 -- with access::sql_filter_thing's rule).
                 WHEN 'subject' THEN subject_visible_to(
                     COALESCE(parent.subject_kind, c.subject_kind),
                     COALESCE(parent.subject_id, c.subject_id), viewer)
                 -- K4 owns: channels by space and private membership; DMs and groups by
                 -- participation only (ruling R20).
                 ELSE conversation_member_visible(target, viewer)
               END
        FROM conversations c
        LEFT JOIN conversations parent ON parent.id = c.parent_channel_id
        WHERE c.id = target), false)
$$;
```

The split is one migration that moves today's body into `conversation_member_visible`
unchanged and makes `conversation_visible_to` the composer (before C1.3 there is no
`subject` visibility, so the `WHEN 'subject'` arm is added by C1.3 with its helper). **If the split has not landed, do it first**:
K1.15 and K1.20 (and K4.4, C1.3) start with it when they are the first to touch the function;
whichever merges first does it, and the others find it done. P0.0's sidebar fix and P0.28's
name `conversation_readable_by` are folded in as callers of `conversation_visible_to`.

**DMs and groups (ruling R20).** Visibility of a DM or group is participation and nothing
else: no role, admin, workspace owner or server role passes `conversation_member_visible` for
one, and `access::decide` never turns a permission into DM visibility. Starting a DM needs
`direct.start` in a workspace the two accounts share (any one suffices), and the other
account's own setting (C2). Sending in an existing DM needs the two to still share a
workspace; reading it does not. Guards (6.7) and isolation cases (K1.33) pin both.

### 3.11 One account, many workspaces (ruling R19)

- **Per workspace**: membership, role, space memberships, budgets, subscription scopes, runs,
  the work line of a status (`account_statuses` keyed by account and nullable workspace, R12).
  **Global**: the account, its profile, its outside reference, its owner, its credentials, its
  DMs, a status set by hand.
- **Joining** is a membership row with a role, the same act for every kind (`account_workspace_add`
  in 3.2 for owned accounts; invitations for people). A workspace setting
  `access.outside_accounts`: `yes`, `ask_admin` (an admin approves the join as a request),
  `no`; default `ask_admin` on the hosted service and `yes` self-hosted (H1 sets the hosted
  default). Accounts owned by this workspace or by one of its members always may join.
- **What others see**: an account's profile and its membership here; never the list of its
  other workspaces. People search, mention lookup, the directory and profile reads return only
  accounts that share a workspace with the caller, and say nothing of where else they are. The
  one exception is the scope screen when adding an account, which shows "also a member of other
  workspaces" as yes or no.
- **Hosts** (R7, R21) are app accounts with the same rule: a person's own host is owned by
  that person and may offer its agents in any workspace where the person may add agents; a
  workspace's host is owned by the workspace.
- **Export and import** (H1): a workspace export carries memberships and a profile stub per
  member account of any kind; import matches agents and apps by outside reference and people
  by verified email, else creates.
- **Existing data**: nothing merges; duplicate accounts for one real agent stay separate.
- **When an owning workspace is deleted** (ruling R39): the accounts it owns are retired and
  their memberships elsewhere end, after notice to their managers and a 14-day window in which
  a person who manages one can take ownership of it (`account_owner_take`: it becomes their
  own, `owner_account_id` set and `owner_workspace_id` cleared, permission
  `may_manage_account`). H1's workspace deletion job calls `accounts::retire_owned_by(workspace)`
  when the window ends.
- **Hosts act only for their bound agents** (ruling R7): a host's app account acting through
  `Via::ActingFor` is allowed only for agent accounts bound to it in A1's `host_agents`
  (K1.23); until K1.23, A1 runs its own `hosts::acting_for` check.

## 4. Migration path

### 4.1 Rules for this workstream's migrations

- Timestamp-named, expand then copy then switch then contract. Every backfill is a single
  idempotent statement (`... WHERE new_column IS NULL` or `ON CONFLICT DO NOTHING`), so a run
  that stops part-way resumes by running again. Tables touched are small (accounts,
  memberships, roles, credentials: thousands of rows on the live server, inferred from one
  team's use), so no batching job is needed.
- No migration assumes another pending K1 migration ran with data. Each backfill re-derives
  from the old columns, which stay in step by trigger until K1.31.

### 4.2 Accounts (K1.5 to K1.8)

1. Add `account_kind`, `owner_account_id`, `owner_workspace_id`, `listed`,
   `created_by_actor_id` and the trigger (3.1). Backfill `account_kind` from `kind`. Backfill
   `owner_workspace_id` (ruling R19: today's agents are their workspace's; nothing merges):
   `external_agent` from `organization_memberships` (each has exactly one, inserted by
   `insert_agent`; if more than one exists, the connection's project's organization wins);
   profile `execution_worker` from `agent_profiles.organization_id`; bare `execution_worker`
   from their project's organization, with `listed = false`; `integration` from their
   project's organization.
2. Profile agents get a workspace membership if they lack one, with the workspace's
   "Member, suggests" role (K1.12, given once to agents existing at the switch, 4.5) or,
   before that slice, `member`. Their project `worker`
   memberships keep role `worker` until A1 moves runs off them; access gives `worker` tier
   nothing, as today.
3. Ids never change. All history (messages, runs, audit rows) points at `actors.id` and stays
   attached.

### 4.3 Old and new checks side by side

Each area switches in three steps, controlled by a server config value read at start
(`AGENT_WORKSPACE_ACCESS_<AREA>=old|compare|new`, default set by the slice that introduces the
area):

- **compare**: the old check decides. Right after it, `access::compare(area, operation,
  caller, need, target, old_allowed)` runs the new check and, if they differ, logs
  `tracing::warn!(target = "access.disagreement", ...)` and records one row:

  ```sql
  CREATE TABLE access_disagreements (
      id UUID PRIMARY KEY,
      area TEXT NOT NULL, operation TEXT NOT NULL,
      workspace_id UUID, space_id UUID, account_id UUID NOT NULL,
      account_kind TEXT NOT NULL,        -- for analysis only; never read by a decision
      permission TEXT NOT NULL,
      target_kind TEXT, target_id UUID,
      old_allowed BOOLEAN NOT NULL, new_allowed BOOLEAN NOT NULL, new_reason TEXT,
      first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
      last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
      count BIGINT NOT NULL DEFAULT 1,
      fingerprint BYTEA NOT NULL UNIQUE  -- hash of (area, operation, account, permission, target_kind, old, new)
  );
  ```

  Recording is `INSERT ... ON CONFLICT (fingerprint) DO UPDATE SET count = count + 1,
  last_seen_at = now()`, on its own pooled connection after the response is decided, and a
  failure to record is logged, never surfaced. Server operators read them at
  `GET /api/v1/server/access-disagreements` (and an operator-only MCP tool). The retention job
  drops rows older than 30 days.
- **new**: the new check decides; the old code is unreachable and removed in the area's
  slice or K1.31.
- An area moves from compare to new in its own small pull request after a week with no
  unexplained disagreements on the live server. Expected disagreements (the fixes: the OAuth
  agent rule, F5 module narrowing, finding 4) are listed in that pull request.

Compare mode is cheap enough to run on every call in the area because the new check is one
query; if a hot list shows cost, compare samples 1 in N there (config).

### 4.4 Roles (K1.9, K1.10, K1.12)

1. K1.9 adds the catalogue rows and new keys, backfilling custom roles (3.3).
2. K1.10: built-in role rows get their permissions written (from `PERMISSIONS` defaults),
   `effective_permissions` is computed for every role row by the server at start-up (a
   one-shot reconcile that only writes rows whose value differs, so it's safe to run on every
   start), `tier` is set from `base_role`. Then every membership with `role_id IS NULL` is
   pointed at its built-in row (`worker` memberships at the hidden `worker` row). A trigger on
   both membership tables keeps `role` and `role_id` in step either way while old code
   remains: setting `role` alone points `role_id` at that built-in row; setting `role_id`
   sets `role` to the row's tier. After this, `role_id` is `NOT NULL` (added `NOT VALID`,
   validated in K1.31) and `custom_role_allows` is re-written as a full check
   (`effective_permissions`), which also closes finding 4 for any caller not yet moved.
3. K1.12 makes "Member, suggests" (3.3 and 4.5) and the default-agent-role setting.

### 4.5 Keeping today's agent behaviour at the switch

Removing every hard-coded agent rule must not silently change what live agents can do. So
K1.12 creates, in each workspace that has agent accounts holding a role with `pages.edit`, a
custom role per such role R named "R, suggests" whose permissions are R's, minus `pages.edit`,
plus `pages.suggest` and `pages.edit_own`, minus the capabilities that were refused to agents
by hand and that R holds (`runs.launch`, `runs.accounts`, `replies.steer`,
`conversations.members`, `requests.answer_sensitive`, `webhooks.manage`; see appendix D), and
moves those agent memberships to it. In practice that is one role per workspace, "Member,
suggests", because agents are capped at Member or Viewer today. The workspace's
`access.default_agent_role_id` stays the invitation default (Member, ruling R5): agents
added after the switch get the same role as a newly invited person. The owner then gives any
existing agent plain Member, or Admin, to give it a person's access. This is the only
migration step that selects rows by account kind, and it runs once.

### 4.6 Credentials (K1.22 to K1.24)

1. Create the tables. Copy `mcp_client_credentials` (secret kind; keep prefix and digest, so
   existing tokens keep working unchanged), OAuth grants (delegation kind; access and refresh
   tokens stay in their tables until they expire), WorkOS bindings (federated kind, with a
   `trusted_issuers` row per configured client), setup links. `legacy_table`/`legacy_id` make
   the copy idempotent.
2. A trigger on each legacy table mirrors inserts, revocations and rotations into
   `account_credentials` while old code writes there.
3. The one resolver reads `account_credentials` first and the legacy tables second (compare
   mode logs disagreements, area `credentials`). Then it reads only the new table; the legacy
   tables are dropped in K1.31 after one release.

### 4.7 Per-project external agents become workspace agents (K1.6)

- `external_agent_connections` gets `organization_id` (backfilled from its project);
  `project_id` becomes nullable and means "where it was first added"; the foreign keys into
  `project_memberships` and `UNIQUE (project_id, actor_id)` are dropped and replaced by
  `UNIQUE (organization_id, connection_name)`. `mcp_client_credentials` likewise gets
  `organization_id` and loses its membership foreign key.
- `agent_connection_can_access` is re-written: active account, active connection, enabled
  workspace membership, enabled membership in the target. The anchor-membership join goes.
  This is the visible change: removing an agent from its first project no longer cuts it off
  elsewhere.
- Routes: workspace-level `/api/v1/organizations/{id}/agents[/{account_id}...]` are added; the
  project routes (`external_agents.rs:40-65`) stay as aliases that resolve the agent and call
  the same operations, removed in K1.31 once the web uses the new ones.
- The table itself is A2's from here (renamed `agent_connections` by A2). K1 changes only its
  anchor and its credentials.

### 4.8 What is deleted, and when

| Old thing | Deleted in |
| --- | --- |
| `must_suggest`, `is_agent`, agents-decide-only-their-pages (`page_docs/suggestions.rs:41-48`, `:557-563`) | K1.12 |
| Agent role caps (`external_agents.rs:602-608`, `:1206`; `memberships.rs:531`; `roles.rs:1530-1537`) and `people_only` | K1.14 |
| The four manage-agent rules | K1.4 |
| `WorkspaceRole`, `caller_role`, `member_manager_role`, `caller_may` | K1.19 |
| `custom_role_allows`, `role_allows_sql`, `projects_allowing`, `agent_limit_hides`, `agent_limit_projects_allowing` | K1.20 |
| `DelegatedScope`, the worker branch and kind branches of `authorize`, per-module `Subject` builders | K1.31 |
| `mcp_client_credentials`, `workos_agent_bindings`, the legacy bearer forms | K1.31 (after one release of mirroring) |
| `actors.kind`, `memberships.role` text, the project routes for agents, the compare config | K1.31 |

## 5. Slices

Sizes: S about a day, M a few days, L about a week. "Beside" means it can run in parallel.

### Phase A: stop the drift, close the hole

**K1.1 Guard rails with a ratchet.** Adds the source-scan tests of section 6 with an
allowlist seeded from today's counts (appendices A to E), so nothing new adds a raw role check
or kind check while the rest of K1 runs. Also adds the `AGENTS.md` rule (with P0). Builds on
P0.24's scanner and P0.26's rules (`raw_role_checks`, `custom_role_allows`,
`workspace_role_enum`, `account_kind_checks`): no second scanner; K1.1 adds only the patterns
and allowlist entries P0 lacks and the ratchet's "slice that removes it" column.
- Changes: new `access/guard.rs` (test-only module) and `access/allowlist.rs`; `AGENTS.md`.
- Depends on: P0.24, P0.26. Beside: everything.
- Acceptance: adding `if principal.actor_kind != ActorKind::Human` to any non-allowlisted
  file fails `access::guard::no_new_kind_checks`; removing one without lowering the count
  fails `allowlist_is_exact` with a message saying which line to delete.
- Size: S.

**K1.2 The access module and compare mode.** Creates `access/` with `Caller`, `Via`,
`Target`, `Grant`, `Denied`, `check`, `check_locked`, `holding`, `sql_filter`, built on
today's data (memberships, `RoleGrant::stored`, `AgentLimits` as a `Via::Delegation` limit,
modules from `modules::state::for_member`). `policy::authorize`'s pure core is called inside;
its kind branches are bypassed by passing `ActorKind::Human` with the delegation limit set
only for delegations (the decision no longer depends on kind). Adds `access_disagreements`,
`access::compare`, the config switch and the operator route. `McpPrincipal` and
`AuthenticatedActor` each get `fn caller(&self) -> Caller`.
- Changes: `access/*.rs`, `lib.rs`, a migration `..._access_disagreements.sql`, `mcp.rs`
  (`caller()`), `auth/mod.rs` (`caller()`), `server_status.rs` (route).
- Depends on: nothing. Beside: K1.1, K1.3, K1.5.
- Acceptance: unit tests port `policy.rs`'s exhaustive matrix to `access::decide` minus kind;
  `tests/integration/access_compare.rs` forces a disagreement (a role that differs from the
  old SQL) and asserts one row with `count = 2` after two calls; the isolation net calls the
  new operator route from the other workspace and gets 404.
- Size: M.

**K1.3 A complete SQL role check.** The fix (finding 4: SQL helpers ignoring the base role) is
done by P0.8; here only what goes beyond it: the catalogue table and one `role_allows`. Adds `permission_catalogue` (rows written by the
migration) and replaces `custom_role_allows` callers' meaning with `role_allows(role TEXT,
role_id UUID, key TEXT)`, which also knows the built-in defaults. Points `role_allows_sql`
(`roles.rs:387`) and `projects_allowing` (`search.rs:467`) at it.
- Changes: migration `..._permission_catalogue.sql`; `roles.rs`, `search.rs`.
- Depends on: P0.8. Beside: K1.1, K1.2.
- Acceptance: `policy::tests::catalogue_matches_sql` compares `PERMISSIONS` with the rows;
  `roles_and_access.rs` adds "a viewer asking `tasks.edit` through `role_allows_sql` gets
  false" and the same for a `worker` membership.
- Size: S.

**K1.4 One rule for managing an agent.** P0.6 makes the four rules one interim rule; here only
what goes beyond it. Adds `account_managers` with the backfill (3.6),
`agents.manage_any` (owner), `access::may_manage_account` and `access::may_grant`, and moves
all five paths in 2.4 onto them. Fixes the OAuth and connected-apps gap.
- Changes: migration `..._account_managers.sql`; `access/manage.rs`; `external_agents.rs`,
  `mcp/agents.rs`, `mcp/oauth.rs`, `connected_apps.rs`, `agent_profiles.rs`,
  `auth/workos/agents.rs`; tool `agent_managers_set` (+`tool_permissions.rs`, parity,
  isolation case).
- Depends on: K1.2, P0.6. Beside: K1.3, K1.5.
- Acceptance: `tests/integration/oauth_agent.rs` adds the escalation case: a workspace
  member who is a project admin with a custom role keeping `agents.manage` cannot see the
  agent on the consent screen nor complete "connect as agent"; a named manager with
  `agents.manage` can; an owner not named can (`manage_any`); a manager removed from
  `account_managers` loses the connected-apps listing. `m9_external_agents.rs` keeps passing.
- Size: M.

### Phase B: one identity

**K1.5 Account kinds and one parser.** Adds the `actors` columns and trigger (3.1), the
`accounts` module with `AccountKind::parse`, and replaces the seven parsers. No behaviour
change.
- Changes: migration `..._account_kinds.sql`; `accounts.rs`; `auth/mod.rs`, `mcp.rs`, and the
  five other parser sites.
- Depends on: nothing. Beside: K1.2 to K1.4.
- Acceptance: `m1_identity_projects_database.rs` asserts every row has `account_kind`, and
  that agents and apps have exactly one owner (`owner_workspace_id` for every existing one); inserting with `kind` only (old code) sets
  `account_kind` by trigger.
- Size: M.

**K1.6 Agents belong to the workspace, not a project.** Section 4.7: the connection and
credential anchor moves to the workspace, `agent_connection_can_access` stops requiring the
home project, `agent_create` (with an owner) and the workspace agent routes and tools land,
project routes become aliases. Also lifts the `runtime_kind = 'hermes'` CHECK in
`migrations/0033` that A1 asked for, and drops the trigger `validate_external_agent_connection`
(`0033:60-101`) explicitly. (Joining further workspaces is K1.33.)
- Changes: migration `..._workspace_agents.sql`; `external_agents.rs` (split into
  `agents/` operations and `agents/http.rs`), `mcp/agents.rs`, `mcp.rs` (bearer SQL drops the
  `connection.project_id = c.project_id` join), `agent_setup_links.rs`; web
  `apps/web/src/settings/workspace/AgentsPage.tsx` (lazy).
- Depends on: K1.4, K1.5. A2 is told (touch points). Beside: K1.7.
- Acceptance: `m9_external_agents.rs`: an agent removed from its first project still acts in
  a second project it belongs to; an agent created at workspace level with no space can read
  only workspace-level things; old project routes still create and rotate.
  `workspace_isolation.rs`: the new routes and tools with the other workspace's ids get 404.
- Size: L.

**K1.7 Outside reference and rebind.** `agent_outside_refs` and the four `agent_ref_*`
operations (3.2). Host-initiated rebind raises a request through A1's requests when A1 has
them; until then it is listed on the agent's page with Accept and Reject.
- Changes: migration `..._agent_outside_refs.sql`; `agents/refs.rs`; tools.
- Depends on: K1.6; A1.7 (the issuer a host declares); A1.11 (soft: rebind requests, listed on
  the agent's page until then). Beside: K1.8.
- Acceptance: `tests/integration/agent_refs.rs`: attach, then a rebind proposing lineage leaves
  the account id, roles and message authorship unchanged after a manager accepts; a second
  account cannot attach an issuer and id already active; a rebind is never applied without a
  manager.
- Size: M.

**K1.8 Profile and worker agents become agent accounts.** Backfills profile agents' workspace
memberships and owner, marks bare workers `listed = false`, validates the `actors_owner`
constraint. Profile management uses the manage rule.
- Changes: migration `..._worker_accounts.sql`; `agent_profiles.rs`.
- Depends on: K1.4, K1.5; A1.1 (the runs table A1 owns, so profile agents' runs keep working).
  Beside: K1.7.
- Acceptance: `w22_agent_kit.rs` or `agent_interface.rs`: a profile agent appears in the
  workspace directory with its role; a bare worker does not; tagging a profile agent still
  launches (A1's tests unchanged).
- Size: M.

### Phase C: roles as data

**K1.9 Catalogue shape.** Adds `kind`, `level`, `capability` metadata to `PermissionInfo`,
level implication, the new keys of 3.3 with defaults that change nothing for people, and the
custom-role backfill. `permission_catalogue` rows follow.
- Changes: `policy.rs` (catalogue moves to `access/catalogue.rs`, `policy` re-exports),
  migration `..._catalogue_levels.sql`, `apps/web/src/settings/workspace/roles.ts`.
- Depends on: K1.3. Beside: Phase B.
- Acceptance: `access::catalogue::tests`: every existing key keeps its built-in defaults; every
  level implies the lower ones; `roles_and_access.rs`: a custom role holding `pages.edit`
  before the migration holds `pages.comment` after it.
- Size: M.

**K1.10 Every membership names a role row.** Section 4.4 step 2: built-in permissions stored,
`effective_permissions`, `tier`, the hidden `worker` role, the `guest` starting role, the
start-up reconcile, `role_id` backfill and step-keeping trigger, `custom_role_allows` as a
full check. Starting roles become editable (except owner) on the Roles page.
- Changes: migration `..._roles_as_data.sql`; `roles.rs`, `access/holding.rs`, `memberships.rs`
  (trigger interplay), web Roles page.
- Depends on: K1.9. Beside: Phase B.
- Acceptance: `roles_and_access.rs`: no membership has `role_id NULL`; old-style insert with
  `role = 'viewer'` gets the viewer row; editing the Member role's `tasks.delete` off changes
  what members may do (one integration case per scope); owner can't be edited.
- Size: M.

**K1.11 Modules off is a permission.** The module gate by id (finding F5) is done by P0.7;
here only what goes beyond it. `access` applies workspace, space and role module
switches (step 5 in 3.5). The HTTP gate keeps its typed error but resolves id-routed pages and
whiteboards through `access::check`.
- Changes: `access/holding.rs`, `modules/gate.rs`, `memory.rs`, `page_docs/http.rs`,
  `whiteboards/http.rs:112`.
- Depends on: K1.2, K1.10, P0.7, M1.2 (the registry that names each permission's module).
  Beside: K1.12.
- Acceptance: `modules.rs`: a role with `modules_off: ["pages"]` gets `module_off` on
  `GET /api/v1/pages/{id}`, the live editor sync and `page_get`; whiteboard list leaves out
  boards where Whiteboards is off for the role.
- Size: M.

**K1.12 Suggest instead of edit is a permission.** `access::write_mode` and
`write_mode_locked` (3.5), `pages.suggest` and `pages.edit_own` live; deciding a suggestion
needs `edit` on the page (or `edit_own` on one's own); "Member, suggests" roles for existing
agents and the default-agent-role setting (Member, ruling R5) (4.5); and deletes
`must_suggest`, `is_agent` and the agents-decide-only-their-pages rule
(`page_docs/suggestions.rs:42-48`, `:558-565`), as the content review asked (C1.8 no longer
deletes them). C1 builds the proposal record for other kinds on `write_mode`.
- Changes: `page_docs/suggestions.rs`, `mcp/memory_tools.rs:540-560`, `:1262-1280`,
  `access/write_mode.rs`, migration `..._suggest_roles.sql`, settings registry
  (`access.default_agent_role_id`), web Members and Roles pages.
- Depends on: K1.10. Beside: K1.11, K1.13.
- Acceptance (plus `write_mode`'s three answers in `access` unit tests, and a new agent gets
  Member): `awd53_discussion.rs` and `awd53_page_blocks.rs`: (a) an existing agent's edit to
  someone else's page is still a suggestion after the migration; (b) the same agent given plain
  Member edits directly; (c) a **person** given "Member, suggests" gets a suggestion; (d) an
  agent with `edit` decides suggestions on any page; (e) the server has no reference to
  `ActorKind` in `page_docs/` (guard count drops to zero there).
- Size: M.

**K1.13 Refusals by kind become permissions, part 1 (managers' permissions).** Removes the
kind checks whose permission only owners and admins hold by default, so no live agent gains
anything (agents are Member or Viewer today): settings, roles, members, invitations, projects
create/update/archive, layouts, templates, code manage, workflow, model catalogue, agent
profiles, environments. Each tool keeps its permission in `TOOL_PERMISSIONS`. People-only
flags stay until K1.14.
- Changes: the files in appendix D rows marked "1".
- Depends on: K1.2. Beside: K1.12.
- Acceptance: `mcp_admin.rs`: an agent given a custom role holding `projects.manage` can
  `project_update` (today refused); an agent with Member still can't (role, not kind);
  `agent_parity.rs` unchanged.
- Size: M.

**K1.14a Agents may hold any role.** (Split from K1.14 by ruling R43, so the owner's own
agents reach full permissions without waiting for A1 and A2.) Removes `people_only` and the
four role caps on agents, so any account can be given any role, Admin and Owner included
(R18). Live agents keep today's behaviour because they hold "Member, suggests" from K1.12
until someone changes it.
- Changes: `roles.rs:105`, `:505-525`, `:1002`, `:1487`, `:1530`, `:1793`;
  `external_agents.rs:602-608`, `:1206`; `memberships.rs:531`; the catalogue's `people_only`.
- Depends on: K1.12, K1.13. Beside: K1.20, K1.22.
- Acceptance: `roles_and_access.rs`: an agent can be given Admin by an owner and then
  `role_write` works; nobody can give an agent a role they could not give a person (K1.4's
  escalation test still passes); a workspace keeps at least one owner whose sign-in can be
  recovered.
- Size: S.

**K1.14b Refusals by kind become permissions, part 2.** Removes the remaining kind refusals
(appendix D rows marked "2") behind the new capabilities, which the "Member, suggests" role
from K1.12 doesn't hold, so live agents keep today's behaviour; `human_required` approvals
become `requests.answer_sensitive` (A1 carries the required permission on the request). The
catalogue rows that this plan lists under "K1.14" land here.
- Changes: appendix D rows "2"; `policy.rs:1194`, `:1235`; `conversations.rs:3498`;
  `external_events.rs:229`.
- Depends on: K1.14a; A1.8 (requests carry the required permission); A2.8 (reply
  continuation on the new capabilities). Beside: Phase D.
- Acceptance: `roles_and_access.rs`: with "Member, suggests" `run_start` is refused naming
  `runs.launch`; a person with "Member, suggests" is refused the same; an approval marked
  sensitive can be answered by an agent holding `requests.answer_sensitive` and not without.
- Size: M.

### Phase D: every area onto `access`

Each of these runs the area in compare mode first (4.3), then flips to new in a one-line
follow-up pull request. They are independent of each other and can run side by side once
K1.2 and K1.10 have landed.

**K1.15 Chat.** `conversations.rs` (`allows`, `:154-180`, moderation `:4243-4268`,
`:3344`), `direct.rs`, `message_edits.rs`, `reactions.rs`, `mcp/talk.rs`, `mcp/groups.rs`,
`realtime.rs:588`. Visibility stays `conversation_visible_to`, composed as in 3.10: if the
split has not landed, do it first. Deletes K4's interim `workspace_member_role`. Size M.
Acceptance:
`m2_conversations_http.rs`, `direct_conversations.rs`, `message_edits.rs` pass in both modes;
the moderation rule exists once (`work_items/comments.rs:785-806` calls it).

**K1.16 Tasks and task comments.** `work_item_commands.rs:392-410` (`authorize_mutation`
becomes a wrapper), `work_items*.rs`, `work_item_*.rs`, `mcp/writes.rs:2131`, `mcp/tasks.rs`.
Fixes E-F2's permission mismatch by making the web and MCP comment paths both need
`tasks.comment`. Size M. Acceptance: `w3_tasks_http.rs`, `mcp_tasks.rs`; a viewer role given
`tasks.comment` can comment through both surfaces.

**K1.17 Documents and records.** `memory.rs`, `page_docs/*`, `whiteboards*`, `sheets/http.rs`,
`collections/http.rs`, `collection_forms.rs`, `files.rs`. Size M. Acceptance: `awd53_*`,
`w16_whiteboards.rs`, `awd54_sheets.rs`, `awd90_collections.rs` in both modes.

**K1.18 Runs, approvals and code.** `api.rs:2458`, `runs.rs`, `launch.rs`, `execution.rs`,
`external_execution.rs`, `repositories.rs`, `github.rs`, `github/connect.rs`,
`mcp/approvals.rs`, `mcp/runs.rs`. Size M. Acceptance: `m3_execution_http.rs`, `w7_launch.rs`,
`m5_github_foundation.rs` in both modes.

**K1.19 Members, roles and workspace administration.** Retires `WorkspaceRole`,
`caller_role`, `member_manager_role`, `caller_may` (`workspace.rs:101-215`) for permissions:
audit log needs `settings.audit` (new capability, owners and admins), mail `settings.manage`,
server status the server role (K1.30), rename `settings.manage`. `invitations.rs:1010-1040`
and `workspace.rs:1472` use `roles::issuer_still_grants` through `access::may_grant`.
`sync_organization_role` (`memberships.rs:756-800`) stops deriving workspace roles from
project roles (current values kept, `role_explicit` set on all rows once). Size M.
Acceptance: `roles_and_access.rs`: a custom admin-tier role without `settings.audit` can't
read the audit log (today it can); `m7_memberships.rs` passes.

**K1.20 Lists, search and notifications in SQL.** Every appendix B and C site uses
`access::sql_filter`; `agent_limit_hides` and `custom_role_allows` callers reach zero. Files:
`search.rs`, `later.rs`, `notifications.rs`, `home.rs`, `ai/*.rs`, `questions.rs`,
`webhooks.rs`, `external_events.rs`, `work_items/imports.rs`, `work_item_planning.rs`,
`run_views.rs`, `mcp/briefing.rs`, `mcp/history.rs`, `mcp/question_tools.rs`. Replaces the
bodies of K3's `things::visible_ids` and C1's `subject_visible_to` with
`access::sql_filter_thing`'s rule (3.10: if the split has not landed, do it first). Depends
on K4.2 (`spaces_readable_by`) and, soft, K3.10. Size L, split
into two pull requests by file if needed. Acceptance: a property test
(`access::sql::tests::rust_and_sql_agree`, against real PostgreSQL) builds roles × delegation
limits × module switches × memberships and asserts `access::check` equals `access_allows` for
every catalogue key; `notifications.rs` and `search` integration tests pass in both modes.

**K1.21 The MCP edge.** `run_tool` takes the tool list from `access::holding` and checks the
resolved target with `access::check` (using K2's declared targets where present, else
`scoped_project`); `module_off_here` and the per-tool module re-checks go
(`mcp/whiteboard_tools.rs:94-121`, `mcp/sheet_tools.rs:64`); `confined_project`
(`mcp/common.rs:159`) reads the credential's `limit_spaces`. Size M. Acceptance:
`agent_interface.rs`, `modules_targets.rs`: a whiteboard tool naming a board in another
project checks that project's modules once. Depends on K2.3 (declared targets).

### Phase E: credentials and acting context

**K1.22 One credentials table and one resolver.** Tables of 3.7 (with the optional
`connection_id`, so A2 can resolve a credential to its connection; until then A2 takes
`connectionId` as a parameter), the copy and mirror
triggers (4.6), `access::tokens::resolve_bearer` used by MCP, the public API
(`public_api.rs:79`) and the WorkOS path; credentials area in compare mode.
- Depends on: K1.6. Beside: Phase D.
- Acceptance: `m6_mcp_foundation.rs`, `oauth_agent.rs`, `awd63_setup_links.rs` pass unchanged
  (existing tokens work); revoking a legacy credential through the old route revokes the new
  row by trigger. Size M.

**K1.23 Exchange and short-lived tokens, rotation and revocation.** `client_credentials` and
`token-exchange` grant types at the OAuth token endpoint, `trusted_issuers`,
`credential_create/rotate/revoke` operations and tools, `aw_sk_`/`aw_at_` formats, the
`connect.md` and `auth.md` guides updated. WorkOS agent tokens become a trusted issuer.
Adds `Via::ActingFor` for host app accounts acting for agents bound in A1's `host_agents`
(ruling R7), and credentials that reach every workspace of their account (ruling R19).
- Depends on: K1.22; A1.11 (soft: `host_agents`). Beside: K1.24.
- Acceptance: `tests/integration/credentials.rs`: a credential reaching two workspaces is
  refused a call that names neither and names a workspace the account left as `not_found`; a
  host acting for an agent not bound to it is refused; a secret exchanges for a 1-hour token that
  works for MCP and the public API; a revoked credential's token fails at once; a rotated
  secret's old value works until `old_secret_valid_until` and not after; a federated
  assertion with a wrong audience is refused. Size M.

**K1.24 Narrow tokens for runs and tasks.** `issue_scoped`, `revoke_for_run`, kind `issuer`
credentials, `Via::AccessToken` scope applied in `access` (permissions, spaces, things).
- Depends on: K1.22. Used by A1 and A2. Beside: K1.23.
- Acceptance: a token scoped to one task can `task_comment` on it and gets `not_found` for
  another task in the same space; a token cannot be issued wider than the account holds;
  `revoke_for_run` cuts it off. Size M.

**K1.25 Acting context on every action.** `set_config('aw.via_credential')` and
`'aw.requested_by'` at the edge, `current_via_credential()`, `current_requested_by()`,
`audit_events` columns, the "acting_via" view over legacy grant ids, `McpPrincipal::via_grant_id`
no longer person-only, the audit page shows all three; `act_within_requester` applied in
`access`.
- Depends on: K1.22, K3.2 (the event fields `via_credential_id`, `requested_by_account_id`).
  Beside: K1.23.
- Acceptance: `awd53_attribution.rs`: an agent's message posted with a run token records the
  run's requester; an app acting for an agent shows "via"; a role with
  `act_within_requester` cannot edit a page the requester can't. Size M.

### Phase F: the rest of the account model

**K1.26 Permissions in the web.** Session and project payloads carry effective keys;
`useCan`; generated or checked `PERMISSION_KEYS`; the role-name gates of appendix A (web) move
to `useCan`; the Roles page shows levels as a grid. Depends on: K1.10; K2.4 (soft: the
generator) and W1 (the shell's `useCan` slot). Size M. Acceptance: `apps/web/src/settings/registry.test.ts` (new): a custom
admin-tier role without `settings.manage` doesn't see Settings › Workspace › General; a test
compares `PERMISSION_KEYS` with the server's `/permissions`.

**K1.27 Groups, guests and role grants.** `account_groups`, `account_group_members`,
`role_grants` (copied from memberships, kept in step by trigger), the `guest` role's
semantics (no `spaces.browse`, no `directory.view`), membership `expires_at` and its job,
operations `group_*`; the C2 channel keys of 3.3's requested table. The membership row's
`role_id` stays the written source; a trigger mirrors it into `role_grants`, and `access`
reads `role_grants` from this slice on. UI is C2's. Depends on: K1.10, K1.20; C2 (its screens,
not a blocker). Size M. Acceptance:
`roles_and_access.rs`: a group granted Member in a space gives its members that space; a
guest doesn't see open channels or the directory over web, MCP or search; an expired guest
membership is disabled by the job.

**K1.28 One name per account per workspace.** `organization_actor_profiles` becomes the one
source; reads join it; the membership copy and `canonical_member_profile` trigger go (health
check A7). Depends on: K1.5. Size M. Acceptance: renaming an agent once renames it in every
space; a write to `project_memberships.display_name` is no longer silently reverted because
the column is no longer read.

**K1.29 One way to create a person and one invitation engine.** `accounts::create_person`
used by bootstrap, invitations, workspace invitations and the identity provider sign-up
(health check A10); one invitation engine targeting a workspace with optional spaces; both URL
families kept as aliases; issuer checks through `access::may_grant`. Coordinate with H1
(sign-up). Also unclaimed accounts and claim links (H1 asked: an account made for an
imported or invited person who has not signed in yet, claimed by a signed one-time link).
Depends on: K1.5. Size M.

**K1.30 Server roles and the identity provider seam.** A `server_roles` table (operator)
replacing "whoever has the bootstrap audit row" (`server_status.rs:45-68`); an
`IdentityProvider` trait (start, callback, session check, token verification) with the WorkOS
code as one implementation registered only when configured, and the provider session stored
generically (health check A8, A9). Depends on: K1.22; H1.18 (the staff console that reads
server roles). Coordinate with K5 (sign-in state). Size M. Acceptance: deleting the bootstrap audit row doesn't change who sees Server;
with WorkOS off, `load_authenticated_actor` makes no WorkOS query.

**K1.31 Contract.** Deletes what 4.8 lists for K1.31: `DelegatedScope` and the kind branches
in `policy.rs`, `Subject` builders, `actors.kind` (after the guard count of kind reads is
zero outside `accounts`), `memberships.role` text, legacy credential tables and bearer forms
after one release, the agent project-route aliases, the compare switches; validates the
`NOT VALID` constraints; renames `actors` to `accounts` with an updatable `actors` view for one
release (only if K2/K3 agree; otherwise left to the repository-wide rename).
Depends on: everything above. Size M.

**K1.32 Per-thing grants and link tokens.** C1.27 asked for sharing one thing (a page, a
board, a sheet) with an account or by link: `thing_grants (thing_id, grantee_account_id |
link_token_digest, level, expires_at, granted_by)`, read by `access::decide` step 8 and
`sql_filter_thing`; link tokens are `access_tokens` scoped to one thing with a `link` kind
credential. Operations `thing_share`, `thing_unshare`, `thing_link_create`. Depends on: K1.20,
K3.10. Size M. Acceptance: a page shared with a person outside its space opens for them and
nothing else in the space does; a link token opens only that page and stops at expiry or
revoke; isolation cases for the three operations.

**K1.33 Accounts across workspaces (ruling R19, R20, R39).** `account_owner_take` and
`accounts::retire_owned_by` for a deleted owning workspace (R39), `account_workspace_add` and the
manager's agreement (an invitation to the account's managers, accepted like a person's),
the `access.outside_accounts` setting, `may_manage_account` per 3.6's R19 wording,
credentials' `reach` enforced at every edge, `direct.start` checked in a shared
workspace, and the isolation cases of 3.11. Depends on: K1.6, K1.23, K4.23 (the direct scope)
for the DM cases. Size M. Acceptance, in `workspace_isolation.rs` and
`tests/integration/accounts_across_workspaces.rs`: an agent owned by Ada joins workspace B by
an admin of B with Ada's agreement, and not without it; with the setting `no` it can't; people
search and profile reads in B never show that the agent or any person is also in A; a DM
between two accounts sharing only A is started in A and listed in B too, and no owner or admin
of A or B can read it; sending in it after they share no workspace is refused while reading
still works; a credential narrowed to A gets `not_found` for a call naming B; when workspace A
that owns an agent is deleted, a manager who takes ownership within 14 days keeps it with its
memberships elsewhere, and an untaken one is retired with its memberships ended.

## 6. Guard rails

1. **No new role-name or kind checks** (`access/guard.rs`, K1.1). Scans `src/**/*.rs` (tests
   cut off at `#[cfg(test)]`, as `policy_inventory.rs:1836-1838` does) for these patterns,
   counting per file:
   - role names: `role IN ('owner'`, `role IN ('admin'`, `role <> 'viewer'`, `role = 'owner'`,
     `role = 'admin'`, `role = 'viewer'`, `role = 'worker'`, `"owner" | "admin"`,
     `== "owner"`, `== "admin"`, `MembershipRole::Owner | MembershipRole::Admin`,
     `WorkspaceRole::`;
   - account kinds: `ActorKind::Human`, `ActorKind::ExternalAgent`,
     `ActorKind::ExecutionWorker`, `kind = 'human'`, `kind IN ('human'`,
     `kind = 'external_agent'`, `kind = 'execution_worker'`, `kind == "human"`,
     `kind != "human"`, `AccountKind::` comparisons;
   - bypasses: `custom_role_allows(`, `agent_limit_hides(`, `agent_limit_projects_allowing(`,
     `Subject::new(`, `policy::authorize(`.

   Allowed without counting: `access/`, `accounts.rs` (mechanics helpers), migrations.
   Everything else must match `access/allowlist.rs` exactly: `(path, pattern, count, reason,
   slice that removes it)`. A count above the allowlist fails ("use `access::check`"); a
   count below fails too ("lower the allowlist"), so the list only shrinks. The web gets the
   same test for `role === 'owner'`, `role === 'admin'` and `kind === 'external_agent'` used
   in a condition (`apps/web/src/access.guard.test.ts`).
2. **Every operation names its permission and checks it.** Until K2's registry: a test joins
   `policy_inventory::MUTATING_ENTRY_POINTS` and `TOOL_PERMISSIONS` with the catalogue (every
   named permission exists; none is people-only). With K2: each operation declares
   `permission: Permission | Own | Public`; the operation runner requires the handler to
   return a `Grant` whose `permission` equals the declared one (a debug assertion that fails
   tests, and a log line in release), so the declared permission and the checked one can't
   drift (fixes the wrong label E-F7 found at `policy_inventory.rs:1444-1449`).
3. **One catalogue everywhere.** `catalogue_matches_sql` (Rust list equals
   `permission_catalogue` rows), `rust_and_sql_agree` (property test, K1.20), and the web key
   list test (K1.26).
4. **No permission by kind in the core.** `access::decide` takes no `AccountKind`; a
   compile-time check is the type itself, plus a unit test that for every catalogue key and
   every kind, the same holding gives the same decision.
5. **Isolation.** Every new route and tool gets a case in `workspace_isolation.rs` or
   `write_isolation.rs` (their source scans already force it).
6. **Parity means behaviour.** `agent_parity.rs` gains a check that no tool returns a
   refusal whose reason is "not for agents" (the refusal strings in appendix D are deleted, and
   the guard in 1 stops new ones).
7. **DMs are participants' only** (ruling R20): a source scan refuses any SQL that reads
   `conversations` of kind DM or group (or the direct scope's id) outside
   `conversation_member_visible` and the participant-filtered list; a test asserts an owner,
   an admin and the server operator each get `not_found` for a DM they are not in.
8. **No account's other workspaces leak** (ruling R19): people search, profile and directory
   reads are tested from a workspace that shares only one of a person's two workspaces.

## 7. Risks and open questions

### Risks

1. **A switch silently widens or narrows access.** Noticed by the compare rows (4.3) and by
   the area's integration tests run in both modes. Fallback: set the area back to `compare`
   or `old` by config, no deploy needed.
2. **Live agents change behaviour at the switch** (they start editing pages directly, or
   launching runs). Noticed by K1.12 and K1.14 acceptance tests (a) and by compare rows with
   `account_kind = 'agent'`. Fallback: the "Member, suggests" migration (4.5) is data; an
   owner can move an agent back in one click.
3. **The SQL and Rust rules drift.** Noticed by `rust_and_sql_agree`. Fallback: lists call the
   Rust check on the page of rows they return (slower, correct) until fixed.
4. **List queries get slower** with `access_spaces`. Noticed by the check span durations and
   slow-query logs. Fallback: a per-request temporary set of allowed spaces passed as a bound
   array, computed once in Rust.
5. **Existing tokens break during the credentials copy.** Noticed by the credentials compare
   area and `m6_mcp_foundation.rs`. Fallback: the resolver keeps reading legacy tables
   (they are only dropped one release after mirroring).
6. **Many branches touch the same files** (`mcp.rs`, `roles.rs`, `policy.rs`). The ratchet
   allowlist conflicts are one line per file; areas are split so two slices rarely touch the
   same file.

### Questions only the owner can answer

1. **Keep today's agent behaviour at the switch?** Recommendation: yes, through the
   "Member, suggests" role made for existing agents (4.5); you then give chosen agents plain
   Member or Admin.
2. **The default role for a newly added agent.** Decided (ruling R5): the same as a newly
   invited person (Member) in every workspace; "Member, suggests" only for agents that exist
   at the switch.
3. **Can an agent be a workspace owner?** Decided (ruling R18): an owner can be any account;
   a workspace must keep at least one owner with a recoverable sign-in (password, single
   sign-on or passkey), so removing or demoting the last such owner, or transferring away from
   them, is refused. Today's `transfer_ownership` requires a login (`workspace.rs:950-959`);
   K1.14 replaces that with the "at least one" rule.
4. **Who manages an agent nobody is named on?** Recommendation: owners, through
   `agents.manage_any`; admins only when named.
5. **Should an agent that was copied keep its memberships?** Recommendation: offered as a
   choice when copying, off by default.

## 8. Touch points

| Workstream | K1 needs | K1 provides |
| --- | --- | --- |
| P0 | The F5 module-gate fix may land in P0 first; the `AGENTS.md` rule "decisions go through `access`, never by role name or account kind" | The guard (K1.1) and its allowlist |
| K2 | Operations declare `permission` and a target; the runner calls `access::check` (or requires a `Grant`); the error list includes `permission_denied {permission, reason}` and `module_off {module}`; a generator for `PERMISSION_KEYS` | `access::check`, `Grant`, `Denied`, the catalogue, `Caller` built at the edge |
| K3 | Thing registry entries give each kind's space and visibility predicate (`access::things` until then; `things::locate`); events record actor, `via_credential_id` and `requested_by_account_id`; search calls `access::sql_filter_thing` | The three acting fields and `sql_filter`; the events K1 emits (`workspace.member.*`, `role.*`, `account.disabled`, `agent.created`, `group.*`, `credential.*`), in K3's catalogue |
| K4 | `spaces` and `space_members` replace projects and project memberships for access; `space_membership`, `space_readable_by`, `spaces_readable_by` (K4.2); `conversation_member_visible` (3.10); the direct scope (K4.23); personal spaces for every account (removes the kind check at `direct.rs:986`); the organization-to-workspace rename | `role_grants` keyed on space id; access reads whatever K4's membership table is through one view; `access::holding(conn, caller, Scope::Workspace(w))` in place of K4's interim `workspace_member_role` (deleted by K1.15); `direct.start`; the events `workspace.member.added/removed/role_changed` |
| K5 | Nothing cached across processes; sign-in state; rate limits per credential and per workspace | Credential ids to key limits on; `Caller` |
| T1 | Field-level permissions, if any, are expressed as catalogue entries | Levels per kind for records and tasks |
| A1 | Runs carry `requested_by`; requests carry the required permission (replacing `human_required`); a host's issuer string for outside references; retirement of bare workers | `issue_scoped` and `revoke_for_run`; rebind requests; `requests.answer_sensitive` |
| A2 | Takes over `external_agent_connections` as `agent_connections` after K1.6; delivery rechecks with `access::check`; agent-to-agent reply requests (`replies.rs:105`, `:171`, `launch.rs:2777`) as loop-limit settings, not kind | Global agents with an owner (ruling R19) and their credentials; `act_within_requester`; `conversations.continue_agents` and `conversations.stop_agents` |
| M1 | The module registry says each permission's module; installing a module adds catalogue rows; connected modules are app accounts | `permission_catalogue` accepting module rows; app accounts with credentials and scopes |
| C1 | Proposal records and review UI for every kind | `access::write_mode` and the `suggest`/`edit_own` levels; decide-rule is `edit` |
| C2 | Guests, groups and their screens, `@group` mentions | The data and operations of 3.4 |
| W1 | `useCan` in the shell; generated types | Effective permissions in the session and project payloads |
| H1 | Sign-up through `accounts::create_person`; the staff console reads server roles; row rules can reuse `access_allows` | `create_person`, `server_roles` |

Assumptions about other workstreams: K2's operation runner can carry a `Grant` and a declared
target; K3 events have room for `via_credential_id` and `requested_by_actor_id`; K4's
`space_members` keeps one row per account per space with an enabled flag (role moves to
`role_grants`); A1's requests can carry a permission key; A2 keeps the connection id stable
when renaming the table.

## 9. Changes to the end state

1. **Levels.** Section 3, "Role": "for each kind of thing, what they may do: see, comment,
   suggest, edit, manage" → "see, comment, suggest, edit their own, edit, manage". Reason:
   today's page rule ("agents edit their own pages directly") needs "edit their own" to be
   expressible as a role, and it is useful for people too.
2. **Delegation is not only for people.** Section 3, "Acting for a person": add "An app can
   also act for an agent account its managers allow, narrowed the same way." Today's
   "connect as agent" OAuth grant does exactly this.
3. **Owners.** Section 3, "Role": add "Any account can be an owner; a workspace always keeps
   at least one owner whose sign-in can be recovered, so it can't be locked out." (Ruling R18.)
6. **Accounts are global** (ruling R19, carried by draft 3): replace "an agent belongs to one
   workspace" with "every account is global and joins workspaces by membership; agents and
   apps have an owner, a person or a workspace".
4. **Acting on someone's request.** Section 4 "Safety in shared spaces" says this is a role
   setting; make it explicit in section 3: "A role can say that when its holder acts on
   someone's request, it may do only what that person could have done."
5. **The agent default role at upgrade.** Section 10, call 3: add "Existing workspaces keep
   their agents' current behaviour through a role made at upgrade."

## Triggers (ruling R16)

| Trigger | On table | Writes | Added by | Removed by |
| --- | --- | --- | --- | --- |
| `actors_account_kind` | `actors` | `actors.account_kind` from `kind` | K1.5 | K1.31 (with `kind`) |
| role and `role_id` step-keeping | `organization_memberships`, `project_memberships` | the other of `role` / `role_id` | K1.10 | K1.31 (with `memberships.role`) |
| `validate_external_agent_connection` (existing, `0033:60-101`) | `external_agent_connections` | refuses connections whose agent lacks the project membership | today | K1.6 (dropped explicitly) |
| legacy credential mirrors | `mcp_client_credentials`, `mcp_oauth_grants`, `workos_agent_bindings`, `agent_setup_links` | `account_credentials` | K1.22 | K1.31 (with the legacy tables) |
| `role_grants` mirror | `organization_memberships`, `project_memberships` (then K4's `space_members`) | `role_grants` | K1.27 | K1.31, once memberships stop carrying roles |
| `canonical_member_profile` (existing) | `project_memberships` | `organization_actor_profiles` copy | today | K1.28 |

## 10. Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| K1.1 | Guard rails with a ratchet | S | P0.24, P0.26 | all |
| K1.2 | The access module and compare mode | M | none | K1.1, K1.3, K1.5 |
| K1.3 | A complete SQL role check (fix done by P0.8) | S | P0.8 | K1.1, K1.2 |
| K1.4 | One rule for managing an agent | M | K1.2, P0.6 | K1.3, K1.5 |
| K1.5 | Account kinds and one parser | M | none | K1.2, K1.3, K1.4 |
| K1.6 | Agents belong to the workspace, not a project | L | K1.4, K1.5 | K1.7, Phase C |
| K1.7 | Outside reference and rebind | M | K1.6, A1.7, A1.11 (soft) | K1.8 |
| K1.8 | Profile and worker agents become agent accounts | M | K1.4, K1.5, A1.1 | K1.7 |
| K1.9 | Catalogue shape: kinds, levels, capabilities | M | K1.3 | Phase B |
| K1.10 | Every membership names a role row | M | K1.9 | Phase B |
| K1.11 | Modules off is a permission (gate fix done by P0.7) | M | K1.2, K1.10, P0.7, M1.2 | K1.12 |
| K1.12 | Suggest instead of edit is a permission | M | K1.10 | K1.11, K1.13 |
| K1.13 | Kind refusals become permissions, part 1 | M | K1.2 | K1.12 |
| K1.14a | Agents may hold any role (role caps and `people_only` removed) | S | K1.12, K1.13 | K1.20, K1.22 |
| K1.14b | Kind refusals, part 2 (new capabilities, sensitive requests) | M | K1.14a, A1.8, A2.8 | Phase D |
| K1.15 | Chat onto access | M | K1.2, K1.10 | K1.16 to K1.21 |
| K1.16 | Tasks and task comments onto access | M | K1.2, K1.10 | K1.15, K1.17 to K1.21 |
| K1.17 | Documents and records onto access | M | K1.2, K1.10 | K1.15, K1.16, K1.18 to K1.21 |
| K1.18 | Runs, approvals and code onto access | M | K1.2, K1.10 | K1.15 to K1.17, K1.19 to K1.21 |
| K1.19 | Members, roles and workspace admin onto access | M | K1.2, K1.10 | K1.15 to K1.18, K1.20, K1.21 |
| K1.20 | Lists, search and notifications in SQL | L | K1.2, K1.10, K4.2, K3.10 (soft) | K1.15 to K1.19, K1.21 |
| K1.21 | The MCP edge onto access | M | K1.2, K1.11, K2.3 | K1.15 to K1.20 |
| K1.22 | One credentials table and one resolver | M | K1.6 | Phase D |
| K1.23 | Exchange, short-lived tokens, rotation, revocation; acting for bound agents; reach | M | K1.22, A1.11 (soft) | K1.24, K1.25 |
| K1.24 | Narrow tokens for runs and tasks | M | K1.22 | K1.23, K1.25 |
| K1.25 | Acting context on every action | M | K1.22, K3.2 | K1.23, K1.24 |
| K1.26 | Permissions in the web | M | K1.10, K2.4 (soft), W1 | Phase D, E |
| K1.27 | Groups, guests and role grants | M | K1.10, K1.20 | K1.28, K1.29 |
| K1.28 | One name per account per workspace | M | K1.5 | K1.27, K1.29 |
| K1.29 | One way to create a person; one invitation engine; unclaimed accounts | M | K1.5 | K1.27, K1.28 |
| K1.30 | Server roles and the identity provider seam | M | K1.22, H1.18 | K1.27 to K1.29 |
| K1.31 | Contract: delete the old paths | M | all above | none |
| K1.32 | Per-thing grants and link tokens | M | K1.20, K3.10 | K1.26 to K1.30 |
| K1.33 | Accounts across workspaces | M | K1.6, K1.23, K4.23 | K1.26 to K1.30 |

## Appendix A. Raw role-name checks

SQL (41): `connected_apps.rs:113`, `:154`; `conversations.rs:3344` (`<> 'viewer'`), `:4264`;
`bootstrap.rs:377`, `:544`, `:565`; `memberships.rs:735`, `:741` (last-owner locks), `:778`,
`:788`; `mail/mod.rs:317`; `seed.rs:95`; `notifications.rs:574`, `:1129`;
`work_items/imports.rs:261` (`<> 'viewer'`); `work_items/comments.rs:799`;
`workspace.rs:339`, `:833`, `:837`, `:1011`, `:1472`; `migration_preflight.rs:255`;
`automations.rs:1933`, `:2652`; `execution.rs:321` (worker); `external_events.rs:221`;
`account_security.rs:1254`, `:1267`; `invitations.rs:897`, `:1010`, `:1011`, `:1039`, `:1040`;
`api.rs:1020`, `:1116` (worker); `mcp/agents.rs:466`; `mcp/oauth.rs:1287`, `:1323`;
`mcp/admin.rs:847`; `workspace_templates.rs:1063`.

Rust (39): `modules/http.rs:158`; `audit_log.rs:199-201`; `memberships.rs:321`, `:464`,
`:549-550`; `layouts.rs:854`; `mail/http.rs:69-70`; `workspace.rs:125-163`, `:238-257`,
`:307-309`, `:360-364`, `:375`, `:443`, `:559`, `:577`, `:595-621`, `:692-706`, `:947-948`,
`:985`, `:1004-1005`, `:1112`, `:1165-1166`, `:1257`, `:1639-1670`, `:1921`; `onboarding.rs:245`;
`external_agents.rs:1289`; `invitations.rs:746`, `:890`, `:1239`; `roles.rs:504`, `:524`,
`:1005`, `:1127`, `:1265`, `:1279`, `:1289`, `:1524`, `:1527`, `:1805-1806`;
`workspace_templates.rs:1335`; `server_status.rs:224-225`.

Which stay: last-owner protection and ownership transfer read the `owner` tier
(`memberships.rs:735`, `:741`; `workspace.rs:339`, `:833-837`, `:1011`; `mcp/admin.rs:847`;
`workspace_templates.rs:1063`), moved behind `access::owners_of(workspace)`. Bootstrap and seed
writes set the owner role and stay as writes. Every other site becomes a permission check.

Web (14): `apps/web/src/modules/TurnedOffPage.tsx:19`; `auth/session.ts:98`;
`team/TeamSettings.tsx:388`; `settings/account/NotificationsPage.tsx:73`;
`settings/account/ProfilePage.tsx:95`; `settings/workspace/GeneralPage.tsx:86`, `:310`;
`settings/workspace/RunsPage.tsx:33`; `settings/workspace/MembersPage.tsx:67`, `:187`, `:384-385`;
`settings/workspace/RolesPage.tsx:95`; `settings/workspace/TemplatesPage.tsx:16`;
`settings/registry.ts:49-50`; `work-items/TaskActivity.tsx:128` (all under `apps/web/src/`).

## Appendix B. `custom_role_allows` and its wrappers

`custom_role_allows` (29): `ai/ready.rs:192`, `:611`; `connected_apps.rs:114`, `:155`;
`conversations.rs:4229`, `:4265`; `work_item_planning.rs:129`; `search.rs:482`; `later.rs:922`;
`notifications.rs:575`, `:1130`, `:1331`, `:1336`; `work_items/imports.rs:187`;
`work_items/comments.rs:800`; `webhooks.rs:1017`, `:1022`; `files.rs:169`;
`external_events.rs:222`; `home.rs:127`; `work_items.rs:4091`, `:4113`, `:4137`;
`mcp/question_tools.rs:307`, `:480`; `mcp/oauth.rs:1288`, `:1324`; `roles.rs:395`;
`questions.rs:237`.

`role_allows_sql` / `projects_allowing` callers (12): `ai/suggestions.rs:387`;
`conversations.rs:4563`; `search.rs:467`; `work_item_move.rs:575`; `later.rs:140`, `:338`,
`:353`; `run_views.rs:359`; `api.rs:2220`; `mcp/briefing.rs:724`; `mcp/history.rs:349`;
`mcp/runs.rs:392`.

All move to `access::sql_filter` in K1.20 (K1.4 for the OAuth and connected-apps ones).

## Appendix C. Delegation limits applied by hand

`agent_limit_hides` / `agent_limit_projects_allowing` (18): `ai/ready.rs:228`, `:626`;
`ai/suggestions.rs:387`; `conversations.rs:4221`, `:4252`; `work_item_planning.rs:228`;
`search.rs:491`; `later.rs:325`, `:353`; `notifications.rs:1630`, `:1631`, `:1633`;
`work_items/imports.rs:196`; `work_items/comments.rs:787`; `mcp/planning.rs:187`;
`roles.rs:385-386` (doc); `launch.rs:2715`. All disappear into `access_spaces` (K1.20).

## Appendix D. Refusals and rules by account kind that decide a permission

"Part" says which slice removes it: 1 = K1.13 (permission held only by owners and admins by
default, so no live agent changes), 2 = K1.14 (new capability the "Member, suggests" role
lacks), S = K1.12, M = K1.4, C = core (K1.2/K1.14).

| Where | What it refuses or decides | Replaced by | Part |
| --- | --- | --- | --- |
| `policy.rs:1194` | People-only permissions | Remove `people_only`; roles decide | C |
| `policy.rs:1198-1212` | Worker branch (dead) | `worker` tier holds nothing | C |
| `policy.rs:1218` | Delegation limit only for people | Limit applies to `Via::Delegation` | C |
| `policy.rs:1235`, `conversations.rs:3498`, `external_events.rs:229`, `provider.rs:53-64` | `human_required` approvals | `requests.answer_sensitive` | 2 |
| `roles.rs:105-107` | Strip people-only for agents | Removed | C |
| `roles.rs:1002`, `:505` | Define roles | `roles.manage` | 1 |
| `roles.rs:1487`, `:518` | Assign roles | `members.manage` | 1 |
| `roles.rs:523` | Invite | `members.invite` | 1 |
| `roles.rs:1530-1537`, `external_agents.rs:602-608`, `:1206`, `memberships.rs:531` | Agents capped at Member or Viewer | `may_grant` only | 2 |
| `roles.rs:1793`, `:1797` | Delegation limits only on people | Limits apply to any account's delegations | 2 |
| `page_docs/suggestions.rs:41-48` (`mcp/memory_tools.rs:550`, `:1272`) | Agent edits to others' pages forced to suggestions | `write_mode` (`pages.suggest`, `pages.edit_own`) | S |
| `page_docs/suggestions.rs:557-563` | Agents decide suggestions only on their pages | `pages.edit` (or `edit_own`) | S |
| `external_agents.rs:1287`, `mcp/agents.rs:447`, `agent_profiles.rs:202`, `:562`, `mcp/agent_profile_tools.rs:129` | Manage agents | Manage rule (3.6) | M |
| `mcp/invitation_tools.rs:116`, `:202` | Invitations | `members.invite` | 1 |
| `mcp/role_tools.rs:334`, `:523` | Role write, assign | `roles.manage`, `members.manage` | 1 |
| `mcp/admin.rs:264` | Workflow update | `tasks.workflow` | 1 |
| `mcp/admin.rs:687`, `:886`, `:978` | Project create, update, archive | `projects.create`, `projects.manage` | 1 |
| `mcp/admin.rs:1185`, `mcp/layout_preset_tools.rs:143` | Layouts | `settings.manage` | 1 |
| `mcp/admin.rs:1499` | Project members | `members.manage` | 1 |
| `mcp/settings_tools.rs:191`, `settings.rs:1133` | Settings (workspace and personal) | `settings.manage`; personal settings need nothing | 1 |
| `mcp/workspace_template_tools.rs:125`, `:208`, `workspace_templates.rs:645` | Templates | `settings.manage` + `roles.manage` | 1 |
| `mcp/project_template_tools.rs:191` | Project templates | `tasks.workflow` | 1 |
| `mcp/model_catalog_tools.rs:132` | Model catalogue | `settings.manage` | 1 |
| `mcp/github_tools.rs:72`, `mcp/repository_tools.rs:73` | GitHub, repositories | `code.manage` | 1 |
| `mcp/environment_tools.rs:122`, `environments.rs:463`, `:567`, `:688`, `:773`, `:871` | Environments | `runs.environments` (default owners, admins) | 1 |
| `bug_reports.rs:379` | Manage form links | `collections.build` | 1 |
| `automations.rs:797` | Automation admin view | `automations.manage` | 1 |
| `modules/http.rs:158`, `onboarding.rs:245` | Manage modules, onboarding | `settings.manage` | 1 |
| `workspace.rs:958` | Ownership transfer target | Any account (ruling R18); the transfer is refused only if it would leave the workspace with no owner who has a recoverable sign-in | 2 |
| `mcp/runs.rs:422`, `:573` | Run start, retry | `runs.launch` | 2 |
| `mcp/launch_tools.rs:249` | Task launch | `runs.launch` | 2 |
| `mcp/batch_tools.rs:161`, `:209` | Usage batches | `runs.accounts`, `runs.launch` | 2 |
| `mcp/replies.rs:1086`, `:1147`, `replies.rs:1152` | Stop or retry a reply | `conversations.stop_agents` (A2's name; `replies.steer` kept as its parent key) | 2 |
| `mcp/groups.rs:434`, `replies.rs:860` | Let agents continue a conversation | `conversations.continue_agents` (A2's name) | 2 |
| `mcp/groups.rs:263`, `direct.rs:578` | Change group members (agents may only leave) | `conversations.members` | 2 |
| `webhooks.rs:307` | Outgoing webhooks | `webhooks.manage` | 2 |
| `mcp/approvals.rs:817` | Approval list | `runs.approve` | 2 |
| `mcp/approvals.rs:487`, `:719` | Two approval paths by kind | One path; scope `approval_respond` becomes `runs.approve` in the credential limit | 2 |
| `conversations.rs:4137`, `message_edits.rs:358-364`, `work_items/comments.rs:782` | Moderation | `channels.moderate` | 2 |
| `later.rs:1086`, `mcp/later_tools.rs:258` | Later list | Per account, no kind (C2 owns the list) | 2 |

## Appendix E. Kind checks that are mechanics (allowlisted, then moved behind `accounts`)

These read the kind to decide *how*, not *may*. K1.1 allowlists them with a reason; each moves
behind an `Account` helper (`signs_in_interactively`, `has_inbox`, `receives_deliveries`) or a
credential test (`Via::Delegation`) in the area slice that touches the file.

- Sign-in and sessions: `auth/mod.rs:216`, `:523`; `password_reset.rs:559`, `:701`;
  `account_security.rs:754`; `mcp/oauth.rs:736`, `:739`, `:894`, `:897`, `:1190-1198`.
- "Acting through an app" written as "is a person": `mcp.rs:155-161`, `:3858`, `:5935-5943`;
  `mcp/common.rs:159`; `events.rs:306`, `:315`; `webhooks.rs:962`; `automations.rs:1291`,
  `:2535`, `:2852`; `work_items/imports.rs:1527`; `launch.rs:1643`, `:2285`; `api.rs:454`, `:712`;
  `scheduler.rs:171`; `bug_reports.rs:238`, `:321`, `:404`; `mcp/role_tools.rs:263`;
  `roles.rs:1958`; `mcp/later_tools.rs:137`, `:531`, `:632`; `mcp/project_template_tools.rs:80`.
  All become `caller.via` tests.
- Inbox and notifications for people: `scheduled_messages.rs:799`; `notifications.rs:551`,
  `:643`; `page_docs/suggestions.rs:458`; `push/actions.rs:274`.
- Agent-to-agent replies and tag launches (A2 makes these loop-limit settings):
  `replies.rs:105`, `:129`, `:171`, `:208`, `:231`, `:263`, `:896`; `launch.rs:2692`, `:2777`;
  `mcp/talk.rs:1756-1770`, `:2036-2047`.
- Directories and lists that hide plumbing accounts (become `listed`): `memberships.rs:386`,
  `:682`, `:716`, `:957-958`, `:1051-1052`; `workspace.rs:189`, `:198`, `:325-333`, `:545`,
  `:565`, `:707`, `:785`, `:1466`, `:1819`; `roles.rs:297`, `:333`, `:341`, `:566`, `:1431`,
  `:1942`; `conversations.rs:2240-2308`, `:3338`, `:3703-3759`, `:3833-3837`, `:3953-3957`;
  `direct.rs:261-263`, `:986`, `:1190`, `:1230`, `:1295`; and the rest of the 52 files counted
  by the K1.1 allowlist (`account.rs:1482`, `ai/detect.rs:591`, `ai/plans.rs:1075`,
  `api.rs:1024`, `:1120`, `audit_log.rs:411`, `bootstrap.rs:455`, `:542`, `:567`,
  `domain.rs:322`, `execution.rs:692`, `external_agents.rs:1007`, `external_events.rs:111`,
  `:216`, `external_execution.rs:279`, `:338`, `:601`, `:623`, `harnesses.rs:254`, `:361`,
  `invitations.rs:852`, `:999`, `:1028`, `later.rs:947`, `layouts.rs:932`, `:1548`,
  `mcp.rs:4806`, `:4810`, `:5213`, `:5828`, `mcp/admin.rs:530`, `mcp/agents.rs:245`,
  `mcp/approvals.rs:519`, `mcp/collaboration.rs:73`, `:329`, `mcp/common.rs:528`,
  `mcp/groups.rs:57`, `mcp/message_edits.rs:150`, `mcp/question_tools.rs:199`,
  `mcp/reads.rs:377`, `onboarding.rs:198`, `settings.rs:1294`, `work_item_commands.rs:2641`,
  `:2665`, `:2718`, `work_items.rs:2069`, `:2093`, `work_items/comments.rs:672`,
  `work_items/imports.rs:1545`, `work_items/list.rs:1706`, `work_items/project_templates.rs:273`).
  Each of these was not read individually; the allowlist entry for each must state which
  bucket it is in, and any that turns out to decide a permission is moved to appendix D's
  treatment in the slice that touches it.

## Appendix F. Hand-built `authorize` calls (33 sites)

`conversations.rs:180`, `:2341`, `:4101`, `:4762`; `auth/mod.rs:593`; `memberships.rs:516`;
`layouts.rs:923`; `work_item_commands.rs:402`; `mcp.rs:5264`; `repositories.rs:759`;
`account.rs:1484`; `github.rs:5348`; `direct.rs:1453`; `memory.rs:330`; `execution.rs:1185`;
`projects.rs:1366`, `:1367`, `:1369`, `:1556`, `:1575`; `github/connect.rs:635`;
`settings.rs:1044`, `:1094`; `external_agents.rs:1311`; `realtime.rs:588`; `invitations.rs:1241`;
`api.rs:2478`; `mcp/talk.rs:1886`; `mcp/approvals.rs:592`; `mcp/reads.rs:403`;
`roles.rs:1641`; `external_execution.rs:655`, `:1070`; `scheduler.rs:206`. Each moves to
`access::check` in its area's slice (Phase D).

## Review changes

- §3.1: `home_workspace_id` and `actors_home_workspace` replaced by `owner_account_id` / `owner_workspace_id` and `actors_owner`; `Account.owner` (ruling R19).
- §3.2: `account_managers.workspace_id` and `agent_outside_refs.workspace_id` nullable, outside references unique server-wide for new rows; `agent_create` takes an owner; new operation `account_workspace_add` (R19).
- §3.3: capability example `spaces.create` corrected to the stored key `projects.create`; default agent role is Member everywhere (R5).
- §3.3: added "Permissions requested by other workstreams" (C2, T1, A1, M1, K4, H1, K2, A2), with defaults and the adding slice (review check 2).
- §3.5: `Caller` gained `run_id` and `reach`; `Via::ActingFor` for hosts (R7); `write_mode_locked`; "How the functions relate" (R2) naming the stopgaps and their removing slices; steps 3 and 8 use K4's helpers and the composed visibility; `access_spaces` over `spaces_readable_by`.
- §3.6: the manage rule follows R19 (owning person, `agents.manage_any` in the owning workspace, named managers), one function; a non-owning workspace controls only its membership.
- §3.7: credentials belong to the account and reach every workspace unless narrowed; multi-workspace calls name their workspace (R19).
- New §3.10: `conversation_visible_to` as K1's composer of C1's `subject_visible_to` and K4's `conversation_member_visible`, the body as C1 3.2 shows it, with "if the split has not landed, do it first"; DMs are participants' only; `direct.start` (K4's name) checked in a shared workspace (R3, R20, content review).
- §3.3: `channels.moderate` covers `comments` conversations; group `handle` noted for C2.18 (content review).
- New §3.11: one account, many workspaces: what is per workspace and global, joining, `access.outside_accounts`, no leak of other workspaces, hosts, export (R19).
- §4.2 backfills `owner_workspace_id`; §4.5 default role stays Member (R5); §4.8 and K1.12: K1.12 deletes `must_suggest`, `is_agent` and the decide-own-pages rule (content review request, accepted).
- Duplicates (R10): K1.1 builds on P0.24/P0.26, K1.3 on P0.8, K1.4 on P0.6, K1.11 on P0.7: "here only what goes beyond it".
- K1.6 also lifts `runtime_kind='hermes'` (A1's ask); K1.12 keeps `write_mode`, keys, roles, migration and deletes the old rule; K1.15 and K1.20 carry the split wording and delete or replace the stopgaps; K1.23 adds `Via::ActingFor` and reach; K1.27 states the role source; K1.29 adds unclaimed accounts and claim links (H1).
- New slices K1.32 (per-thing grants and link tokens, C1.27's assumption) and K1.33 (accounts across workspaces, R19, R20).
- Depends named with slice ids: K1.1, K1.3, K1.4, K1.7, K1.8, K1.11, K1.14, K1.20, K1.21, K1.23, K1.25, K1.26, K1.30; fake ones dropped (K1.27's K4, C2; K1.29's H1).
- Owner questions 2 and 3, end-state change 3 and appendix D's `workspace.rs:958` row follow R5 and R18; end-state change 6 added (R19); appendix D's steer rows use A2's keys.
- Guards 7 (DMs participants' only) and 8 (no leak of other workspaces) added.
- Touch-point rows for K3 and K4 updated to the names they use.
- Added the trigger table (R16).

### Second pass (agents and hosting review, R32 to R42)

- §3.3: `scopes.conversations.read` added (A2, K1.14); cycle-breakers for A1.8 and A2.8 noted.
- §3.6: the manage rule cites R34 (unchanged in substance).
- §3.7 and K1.22: optional `connection_id` on `account_credentials` (A2).
- §3.11: hosts act only for agents bound in `host_agents` (R7; A1's interim check until K1.23); accounts owned by a deleted workspace are retired after a 14-day take-ownership window (R39), built in K1.33.
- K1.6 drops `validate_external_agent_connection` explicitly; added to the trigger table.
- §8 A2 row: "global agents with an owner" and A2's keys replace the stale wording.
- Lead, R43: K1.14 split into K1.14a (role caps, no A1 or A2 dependency) and K1.14b (the rest). Other mentions of "K1.14" in this plan mean K1.14b unless they are about role caps or `people_only`.
