# K4 Spaces: plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for workstream K4, 2026-11-14. Read-only review of `/home/user/agent-workspace` at
fe95a74. Line references are to that commit. "Inferred" marks anything I read but did not see
run. Counts from the migrations come from a small parse of the 130 migration files (CREATE
TABLE, ADD/DROP COLUMN, DROP and RENAME TABLE). The parse is approximate, so treat a count as
plus or minus two.

## 1. Goal

A space is its own concept. There is a `spaces` vocabulary in the database and the server, with
four kinds: the workspace's own space, projects, team spaces and each account's personal space.
No content lives in a hidden project any more. DMs and groups belong to the people in them (R20): they
sit in the server's direct scope, or in a workspace that chose kept DMs, never in a space, and
one visibility rule decides who sees every conversation.
Every web address names its workspace (`/w/acme/...`). Old links keep working, and a person in
several workspaces can work in all of them, with each browser tab in its own workspace. Today
their DMs, Home and settings quietly follow one "default" workspace.

## 2. Where the code is today

### 2.1 Projects that are not projects

`projects.kind` is `standard`, `direct` or `workspace`
(`apps/server/migrations/20261113203000_workspace_space.sql:18-20`; first added as
`standard`/`direct` in `0037_direct_conversations.sql:12-18`). There is one `direct` and one
`workspace` row per organization (unique partial indexes at `0037:16-18` and
`20261113203000:22-24`). Each is used as a container for things that aren't project content:

| Hidden row | What is filed under it | Where |
| --- | --- | --- |
| `direct` ("Messages space", key `DMSPACE`) | Every DM and group conversation, with all child rows: messages, mentions, reactions, revisions, read states, members, reply requests, link previews, files, person questions | `direct.rs:1-7`, `:42-43`, `:1042-1080` (`space_project`); the 23 tables with composite foreign keys into `conversations`/`messages` (section 2.4) |
| `direct` | Notices that belong to no project: a scheduled send that failed, capacity alerts, automation failures for a maker who lost the project, Later items people save | `scheduled_messages.rs:802-811`, `usage/alerts.rs:361-370`, `automations.rs:1913-1922`, `later.rs:574-581` (all through `direct::space_for_member`, `direct.rs:1002`) |
| `direct` | The live-update "carrier" for whiteboards that have no project. The outbox needs a project id, so it borrows this one | `whiteboards/store.rs:46-51` |
| `direct` | The anchor for a person's account-wide app grant (OAuth "act as me") | `mcp/oauth.rs:492-499` (`space_for_actor`, `direct.rs:879-931`) |
| `direct` | The home project of agents made by the host command `direct-agent-create`. The live crew agents were made this way | `main.rs:229-243`; `docs/plans/workspace-merge-plan.md:27-28` |
| `direct` | Agents that people DM, joined as members with role viewer or member | `direct.rs:1169-1279` (`reachable_agents`, `join_reachable_agents`) |
| `workspace` (key `WSPACE`) | Workspace pages (AWD-96), and the place MCP and the pages API put workspace-level pages | `workspace_space.rs:1-11`, `:38-96`; `mcp.rs:5590-5608`; `memory.rs:1265`; `projects.rs:1184` |

Code tests the hidden kinds by string 85 times in 36 server files (my grep for `kind = 'direct'`,
`'workspace'`, `'standard'`, `kind <> 'direct'` and `NOT IN ('direct'`). The most are in
`conversations.rs` (10), `search.rs` (7) and `modules/state.rs` (7). Rust matches on the strings
too: `modules/state.rs:149`, `:161-170` (`in_project`: the workspace space allows only Pages),
`modules/gate.rs:137-155`, `projects.rs:696`, `memory.rs:339`, `sheets/http.rs:110`,
`mcp/talk.rs:535`, `:841`. The web checks the kinds in 7 places, and it reads `workspaceSpace` in
`projects/api.ts:88` and `memory/MemoryPages.tsx:158`.

Membership of the hidden rows is copied from workspace membership by triggers:

- Messages space: `direct_space_follows_workspace_role` copies the workspace role onto people's
  memberships only (`20261106050216_direct_space_follows_workspace_role.sql:18-51`). Agents keep
  the role they were given (lines 15-16, 34-35). People join on their first visit
  (`direct.rs:1082-1167`).
- Workspace space: `workspace_space_follows_workspace_role` (`20261113203000:26-55`) and the
  deferred constraint trigger `organization_memberships_workspace_space_member`
  (`20261113203000:133-151`). These add every enabled workspace member, people and agents,
  through `workspace_space_add_member` (`:61-127`). That function also works out a name and a
  free handle for each member, because project memberships carry a name and handle.

### 2.2 Tables that carry `project_id`

185 tables in all. 121 have a `project_id` column, 56 have `organization_id` and 27 have both.
Grouped by what `project_id` means:

| Meaning | Count | Main tables |
| --- | --- | --- |
| Chat inside a space (and, today, DMs) | 14 | `conversations`, `messages`, `message_mentions`, `message_reactions`, `message_revisions`, `message_link_previews`, `message_work_item_references`, `conversation_members`, `conversation_read_states`, `conversation_notification_levels`, `reply_requests`, `person_questions`, `scheduled_messages`, `saved_items` |
| Tasks | 33 | `work_items` and 20 `work_item_*` tables, `project_counters`, `project_labels`, `project_milestones`, `project_task_settings`, `recurring_tasks`, `task_templates`, `task_imports`, `saved_views`, `field_definitions` |
| Pages and knowledge | 7 | `project_pages`, `project_page_documents`, `project_page_comments`, `project_page_suggestions`, `project_decisions`, `project_instructions`, `thing_references` |
| Collections, sheets, whiteboards, files | 10 | `collections`, `collection_records`, `collection_views`, `sheets`, `sheet_cells`, `sheet_tabs`, `whiteboards` (nullable), `files` |
| Code and runs | 29 | `repositories`, 13 `github_*` tables, `agent_runs`, `agent_sessions`, `agent_profiles`, `execution_profiles`, `execution_workers`, `provider_sessions`, `launches`, `project_environments` |
| Automations, suggestions, webhooks | 10 | `automation_rules`, `automation_runs`, `ai_*` (5), `webhook_subscriptions`, `webhook_deliveries`, `bug_report_links` |
| The space's own membership, scope and layout | 8 | `project_memberships`, `project_invitations`, `project_layout_rules`, `project_navigation_states`, `settings`, `onboarding_progress`, `workspace_template_uses`, `notification_project_mutes` |
| Not a container: an anchor or carrier | 10 | `external_agent_connections`, `external_agent_events`, `mcp_client_credentials`, `mcp_oauth_authorization_codes`, `mcp_oauth_grants`, `outbox_events`, `notifications`, `audit_events`, `background_jobs`, `secrets` |

So 103 tables hold the content of a space, 8 describe the space itself, and 10 use a project
id where they should use a workspace or an account. Integrity rides on `project_id`:

- 69 composite foreign keys point into `project_memberships(project_id, actor_id)` (an author,
  assignee or archiver must be a member of the same project).
- 19 point into `conversations(project_id, id)` and 10 into `messages(project_id, id)`.
- `projects` has `UNIQUE (organization_id, id)` (`0030_m7_project_invitations.sql:3-5`), so a
  table can prove that its project is in its workspace.
- About 30 trigger functions refuse a change to `project_id`, for example
  `0037_direct_conversations.sql:84-86` and `0047_message_edits.sql:112`, `:142`.

The server names `project_id` 8,876 times and `organization_id` 2,370 times. `FROM/JOIN
projects` appears 363 times in 117 files and `project_memberships` 382 times. No query uses the
sqlx compile-time macros (0 `query!`), so renaming a table behind a view needs no code change.

### 2.3 Workspace membership and project membership

- `organization_memberships` (`0004_m1_identity_projects.sql:54-63`): primary key
  `(organization_id, actor_id)`, role owner/admin/member, plus `role_explicit`
  (`20261016200000`), `role_id` and `agent_limit` (`20261030020000:129-133`), and `enabled`.
- `project_memberships` (`0002_walking_skeleton.sql:35-50`, widened later): role
  owner/admin/member/viewer/worker, `role_id`, `display_name`, `handle` (unique per project
  among enabled members), `enabled`.
- Access to a project needs both: `actor_can_access_project`
  (`0042_agent_interface.sql:50-74`) joins an enabled project membership, an enabled
  organization membership, an active actor and (for people) an active user. Agents go through
  `agent_connection_can_access` (`0040_member_accounts.sql:72-88`). That needs the agent to stay
  a member of its connection's home project as well as the target.
- The workspace role is derived upward: `memberships.rs:765` `sync_organization_role` promotes
  someone to workspace admin when they are a built-in owner or admin of any project, unless
  `role_explicit` is set.
- Names flow downward: the trigger `canonical_member_profile` overwrites project-membership
  names from `organization_actor_profiles` (`0040:29-47`).
- Module switches are workspace settings (`modules/state.rs:1-8`, `:214-245`). Roles narrow
  them, and the project kind adjusts them (`in_project`, `:161-170`). There are no per-project
  module switches yet. Layout rules are per project (`project_layout_rules`,
  `20261028080000_layouts.sql:54-68`).

### 2.4 Conversations

- One `conversations` table with kind channel, thread, dm or group, and `visibility IN
  ('project','members')` (`0037:22-62`). Threads can hang under a DM or group
  (`0042_agent_interface.sql:118`).
- The rule today: `conversation_visible_to(target, viewer)` (`0037:160-180`, never redefined)
  says only "public in its project, or the viewer is on the member list". It does not check
  project or workspace membership, so every caller pairs it with a membership join by hand
  (B report, finding 6).
- The direct-space role rule ("a person's role in Messages follows their workspace role; an
  agent keeps its own") is pasted as a `CASE` into the access queries:
  `conversations.rs:3697-3708`, `:3757-3764`, `:2276-2287` and two more (B report, finding 6).
- 23 tables can hold rows of a DM or group, through composite keys into `conversations` or
  `messages`: `conversations`, `messages`, `message_mentions`, `message_reactions`,
  `message_revisions`, `message_link_previews`, `message_work_item_references`,
  `conversation_read_states`, `conversation_members`, `reply_requests`, `person_questions`,
  `files`, `outbox_events`, `project_navigation_states`, `agent_runs`, `agent_sessions`,
  `launches`, `provider_sessions`, `repository_workspaces`, `external_execution_requests`,
  `work_item_conversation_links`, `work_item_worker_invocations`,
  `github_pull_request_conversation_links`. Add `notifications`, `saved_items`,
  `scheduled_messages` and `conversation_notification_levels`, which reference conversations
  by id.
- Read state has primary key `(project_id, conversation_id, actor_id)`
  (`0005_m2_conversations.sql:452`). Code upserts on that key in 3 places.
- Private channels are half built. `insert_channel` never sets visibility
  (`conversations.rs:952-970`). Roles can name "private channels they join on arriving"
  (`roles.rs:1842`), but nothing can create one.
- DMs follow the default workspace. `ensure_direct_space` uses `workspace::default_workspace`
  (`direct.rs:982-994`), so a person in two workspaces sees only one workspace's DMs until they
  change "default" in settings (inferred from that code and `settings/workspace/GeneralPage.tsx`).
  Agents can't open the Messages overview at all (`direct.rs:986-988`).

### 2.5 Addresses and the current workspace

- Web routes have no workspace segment: `/home`, `/inbox`, `/messages/:id`, `/whiteboards/:id`,
  `/projects/:projectId/...`, `/:taskKey` (`apps/web/src/app/App.tsx:143-380`). Home, workspace
  pages and layouts mean "the default workspace": `useHomeWorkspaceId`
  (`apps/web/src/experience/layout.ts:233-241`).
- API: `/api/v1/projects/{id}/...` appears 445 times, `/api/v1/organizations/{id}/...` 110
  times, and the rest are addressed by id (`/conversations/{id}`, `/pages/...`,
  `/whiteboards/{id}`). There is one MCP endpoint, `/api/v1/mcp` (`mcp.rs:124`).
- The server finds the current workspace in four ways:
  1. From the path: `/organizations/{id}`.
  2. From a project id: `projects.organization_id`.
  3. From an optional `organizationId` argument: `later.rs:568-571`, `webhooks.rs:464`.
  4. From `users.default_organization_id`, else the earliest membership
     (`workspace.rs:1933-1951`). This last one is used for Messages, notification settings
     (`notification_settings.rs:384-406`), webhooks, MCP automation tools
     (`mcp/automation_tools.rs:229`) and the session payload (`auth/mod.rs:606-618`).
  Sessions carry no workspace. MCP principals carry a mandatory `project_id` anchor
  (`mcp.rs:138-157`).
- Slugs: `organizations.slug` is globally unique, kebab-case, with no length limit
  (`0002_walking_skeleton.sql:17`). It can't be renamed: only the name can
  (`workspace.rs:467`). WorkOS sign-up makes `workspace-<32 hex>` (`auth/workos.rs:508-511`).
- The server builds web links in about 150 places across about 40 files, for notifications,
  search, Later, things, MCP results and mail. For example `notifications.rs:1745`,
  `search.rs:225`, `search.rs:259` (in SQL), `later.rs:204`, `modules/things.rs:85-110`,
  `replies.rs:544`. Tools accept a link and take its last UUID segment
  (`mcp/common.rs:440-472`), so a `/w/<slug>` prefix needs no parser change.

### 2.6 Keep these

- Composite tenancy keys (`UNIQUE (organization_id, id)` plus `(project_id, …)` keys). They
  make cross-workspace mixing impossible in SQL.
- Whiteboards' pattern: `organization_id NOT NULL` with a nullable `project_id`, and their own
  visibility function (`20261017070000_whiteboards.sql:16-31`, `:120-147`).
- One SQL visibility function for conversations, used by HTTP, MCP and realtime.
- The lock order in `lock_conversation_scope` (`conversations.rs:3881-3918`).
- The isolation nets: `tests/integration/workspace_isolation.rs` (its column scan at
  `:2197-2216`) and `write_isolation.rs`.
- Idempotent template application (`workspace_templates.rs:1-23`).
- The settings registry's scopes (workspace, project, user).

### 2.7 Permission differences by account kind in this area

Each is listed with the role setting that replaces it (K1 builds the setting; K4 removes the
branch when it moves the code):

1. Direct-space role: people follow the workspace role, agents keep a per-space role
   (`conversations.rs:3697-3708` and four copies; `20261106050216:15-16`, `:34-35`).
   Replaced by the workspace role for every account (K4.12).
2. Only people can open the Messages overview or get a direct space (`direct.rs:986-988`,
   `:1002-1018`). Replaced by "may start conversations" in the workspace role (K4.12).
3. Only people can pull an agent into Messages (`direct.rs:1224-1236`). Replaced by the same
   role permission plus the existing "shares a space with them" reach rule (K4.12).
4. Account-wide app grants exist only for people (`mcp/oauth.rs:492-499`). That is delegation,
   a mechanical difference, and stays. K1 re-anchors it to the account (K4.17 depends on that).

## 3. The target design

### 3.1 Concepts

| Concept | Rows | Membership | Created |
| --- | --- | --- | --- |
| Workspace [organization] | `workspaces` | `workspace_members` | sign-up, bootstrap, "add workspace" (H1, K1) |
| Workspace space (kind `workspace`) | one `spaces` row per workspace | implicit: every enabled workspace member, with their workspace role | lazily, by `spaces::workspace_space` (as today) |
| Project (kind `project`) | `spaces` | explicit `space_members` | `space_create` |
| Team space (kind `team`) | `spaces` | explicit; open ones can be joined by any workspace member | `space_create` |
| Personal space (kind `personal`) | one per account per workspace | the owner only | lazily, on first use |
| DM, group | `conversations` with `space_id` NULL | `conversation_members` (participants), within the workspace | `conversation_start` |

Project and team differ only in their defaults (starting shape, visibility, how they're listed).
They are the same table and the same rules. Kind decides four things, and nothing else:

- whether membership is implicit (workspace), single-owner (personal) or explicit;
- whether the space can be archived (not workspace or personal);
- how it is listed;
- which starting shape it gets.

### 3.2 Decision: rename `projects`, don't add a parent table

Two real options:

- **(a) A new parent `spaces` table**, with `projects` kept as the one-to-one detail row for the
  project kind (the pattern tasks-as-records uses). Before any non-project space could hold a
  page or a channel, all 121 foreign keys to `projects(id)` would have to be repointed at
  `spaces(id)`, and the detail row would hold almost nothing. `projects` today is only id,
  organization, key, name, kind, archive, version and timestamps (`0002:22-30`, `0004:10-14`).
  Project-specific settings already live in their own tables: `project_task_settings`,
  `project_github_automation_settings`, `project_counters`.
- **(b) `projects` becomes `spaces`.** Every row already has the same id, so all 121 foreign
  keys, the 69 membership keys and every trigger stay valid unchanged. Kind is just a column.

**Chosen: (b).** It is the better abstraction, not only the cheaper one. Everything a project
has that other spaces lack is already a module's setting in its own table. Every space has a
`key` (short code), because any space with Tasks switched on numbers its tasks. The workspace
space already has one (`WSPACE`).

How it is renamed matters more than whether. With 14 workstreams merging at once, and new
tables in most of their migrations saying `REFERENCES projects(id)` or `REFERENCES
organizations(id)`, the base tables keep their old names until the very end. A foreign key
can't point at a view, so renaming early would break every branch that adds one. The new
names arrive as **views** first:

```sql
-- K4.1. Plain single-table views, so PostgreSQL can write through them
-- (INSERT, UPDATE, DELETE, ON CONFLICT, row locks).
CREATE VIEW workspaces AS
  SELECT id, name, slug, version, created_at, updated_at FROM organizations;
CREATE VIEW workspace_members AS
  SELECT organization_id AS workspace_id, actor_id, role, role_id, role_explicit,
         agent_limit, enabled, version, created_at, updated_at
  FROM organization_memberships;
CREATE VIEW spaces AS
  SELECT id, organization_id AS workspace_id, kind, project_key AS key, name, description,
         visibility, owner_actor_id, created_by_actor_id, shape, archived_at,
         archived_by_actor_id, version, created_at, updated_at
  FROM projects;
CREATE VIEW space_members AS
  SELECT id, project_id AS space_id, actor_id, role, role_id, display_name, handle, enabled,
         invited_by_actor_id, disabled_by_actor_id, disabled_at, version, created_at, updated_at
  FROM project_memberships;
```

(The `space_members` column list is written from the `project_memberships` columns as they
stand at merge. The view-parity guard, G3, checks it.)

New Rust code reads and writes the new names. Old code keeps working untouched. The physical
flip (`ALTER TABLE projects RENAME TO spaces` and the others, recreating the old names as
views) is the last slice, K4.22, run in a quiet window. After it, foreign keys that already
exist follow the table automatically.

Columns: the 8,876 `project_id` references are **not** renamed in one sweep. The rule for
everyone:

- **New tables** use `workspace_id` and `space_id`. Until K4.22 they reference the base tables:
  `workspace_id UUID NOT NULL REFERENCES organizations(id)`, `space_id UUID REFERENCES
  projects(id)`, and `FOREIGN KEY (workspace_id, space_id) REFERENCES projects(organization_id,
  id)`.
- **Existing tables** keep `project_id` and `organization_id`, documented as meaning "space"
  and "workspace". A workstream that rebuilds a module's tables (T1 tasks, C1 comments, A1 runs)
  renames them in its new tables.
- **Whether to sweep the rest** before the public release is an owner question (section 7).

### 3.3 Data model additions

```sql
-- K4.1, on the base table (the views above expose these columns).
ALTER TABLE projects
  ADD COLUMN description TEXT NOT NULL DEFAULT ''
      CHECK (char_length(description) <= 500 AND description !~ '[[:cntrl:]]'),
  -- members: only members see it. workspace: every workspace member can find
  -- and read it, and join it to take part.
  ADD COLUMN visibility TEXT NOT NULL DEFAULT 'members'
      CHECK (visibility IN ('members', 'workspace')),
  ADD COLUMN owner_actor_id UUID REFERENCES actors(id),        -- personal spaces only
  ADD COLUMN created_by_actor_id UUID REFERENCES actors(id),
  ADD COLUMN shape JSONB CHECK (shape IS NULL OR jsonb_typeof(shape) = 'object'); -- K4.20
ALTER TABLE projects DROP CONSTRAINT projects_kind_check,
  ADD CONSTRAINT projects_kind_check
      CHECK (kind IN ('standard', 'direct', 'workspace', 'team', 'personal')),
  ADD CONSTRAINT projects_personal_owner_check
      CHECK ((kind = 'personal') = (owner_actor_id IS NOT NULL));
CREATE UNIQUE INDEX projects_one_personal_space
  ON projects (organization_id, owner_actor_id) WHERE kind = 'personal';
UPDATE projects SET visibility = 'workspace' WHERE kind = 'workspace';
```

- The stored value `standard` stays until K4.22, which changes it to `project` and drops
  `direct`. Rust maps it in one place, `SpaceKind::from_stored`.
- Existing projects get `visibility = 'members'`, so nobody sees more than before.
- Workspace slugs (K4.5):

```sql
CREATE TABLE workspace_slugs (
  slug TEXT PRIMARY KEY CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND char_length(slug) <= 63),
  workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  current BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  retired_at TIMESTAMPTZ,
  CHECK (current = (retired_at IS NULL))
);
CREATE UNIQUE INDEX workspace_slugs_one_current ON workspace_slugs (workspace_id) WHERE current;
INSERT INTO workspace_slugs (slug, workspace_id) SELECT slug, id FROM organizations;
```

  `organizations.slug` stays the current slug, written in the same transaction. Rules for new
  slugs, checked in Rust: 3 to 40 characters, and none of the reserved words (`api`, `w`,
  `app`, `admin`, `settings`, `sign-in`, `join`, `forms`, `new`, `static`, `assets`, `sw`,
  `www`, `help`, `status`, plus anything the web serves at the root). Existing longer slugs,
  such as WorkOS's 42-character ones, stay valid. A retired slug stays pointing at its workspace
  and can't be taken by another one, so an old link never opens someone else's workspace.
- **The direct scope** (ruling R20, K4.23). DMs and groups belong to the people in them, not to a
  workspace. One reserved `organizations` row per server holds them, so every table keeps a
  non-null workspace column and the log, search, storage, row rules and metering need no
  second path:

```sql
ALTER TABLE organizations ADD COLUMN kind TEXT NOT NULL DEFAULT 'team'
    CHECK (kind IN ('team', 'direct'));
-- The fixed id is a constant in Rust (`spaces::DIRECT_SCOPE_ID`) and in SQL (`direct_scope_id()`).
INSERT INTO organizations (id, kind, name, slug)
VALUES ('00000000-0000-7000-8000-00000000d1ec', 'direct', 'Direct messages', 'aw-direct-scope')
ON CONFLICT (id) DO NOTHING;
CREATE UNIQUE INDEX organizations_one_direct ON organizations (kind) WHERE kind = 'direct';
-- No member, no space, no role in it, ever.
ALTER TABLE organization_memberships ADD CONSTRAINT organization_memberships_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
ALTER TABLE projects ADD CONSTRAINT projects_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
ALTER TABLE workspace_roles ADD CONSTRAINT workspace_roles_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
-- Only DMs and groups (and threads under them) live there.
ALTER TABLE conversations ADD CONSTRAINT conversations_direct_scope_kinds
    CHECK (workspace_id <> '00000000-0000-7000-8000-00000000d1ec'
           OR kind IN ('dm', 'group', 'thread'));   -- a thread's parent is checked by the parent trigger
```

  The row is never listed, never in the switcher, has no slug a person can reach (`aw-direct-scope` is added to the reserved words, so no
  workspace can already hold it once K4.5 lands, and K4.5's backfill and slug lookup skip
  `kind = 'direct'`; if a workspace already holds that slug when K4.23 runs, the migration
  stops with a message naming it), no
  modules and no settings. Its id is never
  accepted as `{workspace}` in an address. What hangs off a DM or group (messages, reactions,
  read states, files, events, and the requests and runs raised from them) carries the same id;
  nothing else does (guard G7). A task, page or record made from a DM message goes to a space
  the person picks.
- **The `direct_messages` workspace setting** (settings registry): `personal` (default) or
  `kept`. In a `kept` workspace, a DM or group between its members started inside it carries
  that workspace's id, follows its retention and export, and ends with membership, as in Slack.
  Same table, same visibility rule, different workspace id; `conversations.kept BOOLEAN NOT NULL
  DEFAULT false` marks them, because existing DMs also carry a team workspace's id but are
  personal (below).

### 3.4 The access helpers (what other workstreams call)

K4 owns "which spaces exist and who is in them". K1 owns "what a role allows". The helpers
answer membership and readability only, and K1's access function composes on top of them.

```sql
-- The account's membership of a space: explicit, or implicit for the
-- workspace space. No row when they aren't a member.
CREATE FUNCTION space_membership(target UUID, account UUID)
RETURNS TABLE (role TEXT, role_id UUID, implicit BOOLEAN)
LANGUAGE sql STABLE AS $$
  SELECT m.role, m.role_id, false
  FROM projects s
  JOIN project_memberships m ON m.project_id = s.id AND m.actor_id = account AND m.enabled
  JOIN organization_memberships w
    ON w.organization_id = s.organization_id AND w.actor_id = account AND w.enabled
  JOIN actors a ON a.id = account AND a.status = 'active'
  WHERE s.id = target AND s.archived_at IS NULL AND s.kind <> 'workspace'
  UNION ALL
  SELECT CASE w.role WHEN 'owner' THEN 'owner' WHEN 'admin' THEN 'admin' ELSE 'member' END,
         w.role_id, true
  FROM projects s
  JOIN organization_memberships w
    ON w.organization_id = s.organization_id AND w.actor_id = account AND w.enabled
  JOIN actors a ON a.id = account AND a.status = 'active'
  WHERE s.id = target AND s.kind = 'workspace' AND s.archived_at IS NULL
$$;

-- May the account read the space: a member, or the space is open to the
-- workspace and they're an enabled member of it.
CREATE FUNCTION space_readable_by(target UUID, account UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
  SELECT EXISTS (SELECT 1 FROM space_membership(target, account))
      OR EXISTS (
        SELECT 1 FROM projects s
        JOIN organization_memberships w
          ON w.organization_id = s.organization_id AND w.actor_id = account AND w.enabled
        JOIN actors a ON a.id = account AND a.status = 'active'
        WHERE s.id = target AND s.archived_at IS NULL AND s.visibility = 'workspace'
          AND s.kind IN ('team', 'standard'))
$$;

-- Every space in a workspace the account may read, for list queries:
--   WHERE s.id IN (SELECT spaces_readable_by($account, $workspace))
CREATE FUNCTION spaces_readable_by(account UUID, workspace UUID)
RETURNS SETOF UUID LANGUAGE sql STABLE AS $$ … same rule, set-returning … $$;
```

Notes on the helpers:

- People still need an active `users` row to sign in. That is K1's credential check (a
  mechanical difference), so it is not repeated here.
- Narrowing by an agent's credential (today `agent_connection_can_access`) stays K1's. K1's
  `access` filter is `space_readable_by(...) AND credential_reaches(...)`.

Rust module `apps/server/src/spaces/` (new; it absorbs `workspace_space.rs` and the space
parts of `direct.rs`):

```rust
pub enum SpaceKind { Workspace, Project, Team, Personal }
impl SpaceKind {
    pub fn from_stored(value: &str) -> Option<Self>; // "standard" -> Project until K4.22
    pub fn stored(self) -> &'static str;
}
pub enum SpaceVisibility { Members, Workspace }
pub struct Space {
    pub id: Uuid, pub workspace_id: Uuid, pub kind: SpaceKind, pub key: String,
    pub name: String, pub description: String, pub visibility: SpaceVisibility,
    pub owner_actor_id: Option<Uuid>, pub archived_at: Option<DateTime<Utc>>, pub version: i64,
}
pub struct SpaceMembership { pub role: String, pub role_id: Option<Uuid>, pub implicit: bool }

/// The space if `account` may read it; None otherwise (callers answer 404, never 403).
pub async fn get(conn: &mut PgConnection, account: Uuid, space: Uuid) -> Result<Option<Space>, AppError>;
pub async fn readable(conn: &mut PgConnection, account: Uuid, workspace: Uuid, kinds: &[SpaceKind]) -> Result<Vec<Space>, AppError>;
pub async fn membership(conn: &mut PgConnection, account: Uuid, space: Uuid) -> Result<Option<SpaceMembership>, AppError>;
/// Get or create, safe under concurrency (ON CONFLICT on the partial unique index).
pub async fn workspace_space(tx: &mut Transaction<'_, Postgres>, workspace: Uuid) -> Result<Uuid, AppError>;
pub async fn personal(tx: &mut Transaction<'_, Postgres>, workspace: Uuid, account: Uuid) -> Result<Uuid, AppError>;
pub async fn create(tx: &mut Transaction<'_, Postgres>, actor: &Actor, new: NewSpace) -> Result<Space, AppError>;
/// SQL fragment for list queries, so nobody rewrites the rule: "space_readable_by(s.id, $1)".
pub fn readable_sql(space_column: &str, account_param: &str) -> String;
```

`spaces::create` is the only place that inserts a space row (guard G1). `projects.rs` create,
`workspace_space::ensure`, `direct::space_project`, `bootstrap.rs`, `seed.rs` and
`workspace_templates.rs` call it.

### 3.5 Space operations

Each is a K2 operation (web route, MCP tool, public API, automation action), with an isolation
case in `workspace_isolation.rs` or `write_isolation.rs`. Permission keys are K1's. The names
below are the working ones I assume.

| Operation | Kind | Input | Permission | Effect |
| --- | --- | --- | --- | --- |
| `space_list` | query | `workspace?`, `kinds?`, `includeArchived?`, page | workspace member | readable spaces; open ones the caller isn't in are marked `joinable` |
| `space_open` | describe | `space` (id, key or link) | `space_readable_by` | the space, the caller's membership, modules on, guide page, skills (today's `project_open`) |
| `space_create` | create | `workspace`, `kind` (`project` or `team`), `name`, `key?`, `description?`, `visibility?`, `shape?`, `clientRequestId` | `projects.create` (K1's stored key) | row, creator as owner, starting shape applied (K4.20), `space.created` |
| `space_update` | change | `space`, `expectedVersion`, `name?`, `description?`, `visibility?`, `key?` | `projects.manage` | `space.updated` with the changed field names; not the kind |
| `space_archive` / `space_restore` | change | `space`, `expectedVersion` | `projects.manage` | not for workspace or personal kinds; `space.archived` / `space.restored` |
| `space_member_add` / `space_member_remove` / `space_member_set_role` | change | `space`, `account`, `role?` | `members.manage` in the space | refused on workspace (implicit) and personal spaces; `space.member.*` |
| `space_join` / `space_leave` | change | `space` | open space and workspace member / any member | joins with the space's default role; `space.member.added` / `removed` |
| `space_personal_open` | describe | `workspace?` | workspace member | the caller's personal space, made on first use |
| `workspace_update` | change | `workspace`, `expectedVersion`, `name?`, `slug?`, `directMessages?` | `settings.manage` | slug history row; `workspace.updated` |

The old tools stay as aliases (K2): `projects_list`, `project_open`, `project_create` (kind
`project`), `project_update`, `project_archive`, `project_members`, `project_member_set`,
`project_layout_set` and `project_export` map to their `space_*` names.

### 3.6 Events (what K5's subscriptions and A2's delivery need)

Written through K3's log (today the outbox). Every event carries `workspaceId`. A space event
also carries `spaceId`, and a membership event carries the `accountId` whose access changed, so
a subscriber recomputes only that account's scopes.

```json
{ "type": "space.member.added", "workspaceId": "0193…", "spaceId": "0193…",
  "accountId": "0193…", "role": "member", "implicit": false,
  "actorId": "0193…", "at": "2026-11-20T09:12:03Z" }
```

| Event | When |
| --- | --- |
| `space.created`, `space.updated`, `space.archived`, `space.restored` | as named; `space.updated` lists `fields` (and says whether `visibility` changed) |
| `space.member.added`, `space.member.removed`, `space.member.role_changed` | explicit membership, join, leave |
| `workspace.member.added`, `workspace.member.removed`, `workspace.member.role_changed` | emitted by K1. Implies the same change in the workspace space. K4 sends no separate space event for implicit membership |
| `conversation.member.added` (new), `conversation.member.removed` (exists, `outbox.rs:153`, `:302`) | DM, group and private-channel participants |
| `conversation.visibility.changed` | a channel made private or public (if C2 allows converting) |
| `workspace.updated` | name or slug changed (the web replaces the address) |

Until K3's log lands, `project.membership.updated` (`outbox.rs:196`, `:305`) stays, and the
space events are added beside it. The names are K3's catalogue names (K3 §3.3.3). Events of
DMs and groups carry the direct scope's id as their workspace and a conversation audience
(K3, ruling R20).

### 3.7 Conversations

**Schema target.** `conversations.workspace_id UUID NOT NULL`: a team workspace for channels
and kept DMs, the direct scope for new personal DMs and groups (ruling R20); existing DMs keep
the team workspace id they have and are personal (`kept = false`), so no data moves for R20.
The space is `project_id` (renamed `space_id` in K4.22). It is NULL exactly for DMs, groups and
threads under them.
Children of a DM keep `conversation_id` and have `project_id` NULL. Composite foreign keys use
PostgreSQL's default match rule, which skips the check when any column is NULL. So the 19
conversation keys and the membership keys simply stop applying to DM rows, and need no rebuild.
In their place:

- `conversation_members` gets `workspace_id` (the conversation's) and a generated
  `member_workspace_id UUID GENERATED ALWAYS AS (NULLIF(workspace_id, direct_scope_id)) STORED`
  with `FOREIGN KEY (member_workspace_id, actor_id) REFERENCES
  organization_memberships(organization_id, actor_id)`. For channels and kept DMs participants
  must be in the workspace; for direct-scope rows the key is NULL and skipped (no memberships
  exist there). Personal DMs from before R20 keep their team workspace id, so the key still
  applies to them; a participant who left that workspace keeps the member row through
  `ON DELETE` doing nothing (memberships are disabled, never deleted).
- `conversation_read_states` gets the key `(conversation_id, actor_id)`: a unique index first,
  then the primary key is swapped once the three upserts use the new key.
- One personal DM per pair on the server, and at most one kept DM per pair in each `kept`
  workspace (rulings R20, R31): `UNIQUE (dm_key) WHERE kind = 'dm' AND workspace_id =
  direct_scope_id` for personal DMs, `UNIQUE (workspace_id, dm_key) WHERE kind = 'dm' AND kept`
  for kept ones; they replace the `(project_id, dm_key)` index. Inside a `kept` workspace,
  opening a DM with a member returns the workspace's kept DM, and the personal one stays in the
  list, labelled. Elsewhere, starting a DM again returns the direct-scope one, else the newest
  pre-R20 one. Pairs that already have a DM in two workspaces keep both (labelled with the
  workspace's name); nothing merges. Reaching an account you share no workspace with does not
  exist (R31: it needs a ruling first).
- `CHECK ((kind IN ('dm','group')) = (project_id IS NULL))` for non-threads. The
  `validate_conversation_parent` trigger checks that threads share their parent's space, or
  both have none and share a workspace. Added in K4.17, once no DM row points at a hidden
  project.

Conversation ids, message ids, read-state rows and members are all kept. Only `project_id`
changes, to NULL.

**The one rule.** `conversation_visible_to(target, viewer)` keeps its name and signature, and
is K1's composer (ruling R3, K1 §3.10, the body C1 3.2 shows): a `subject` conversation goes to
C1's `subject_visible_to`, every other one to `conversation_member_visible(target, viewer)`.
K4 owns `conversation_member_visible`, which is today's body made whole. If the split has not
landed, do it first: the first of K4.4, K1.15 and C1.3 to touch the function moves today's
body into `conversation_member_visible` unchanged, makes `conversation_visible_to` the
composer, and then changes the body. The body branches
on the conversation's kind, not on whether `project_id` is NULL, so it is right both before
and after the move:

```sql
CREATE OR REPLACE FUNCTION conversation_member_visible(target UUID, viewer UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
  SELECT EXISTS (
    SELECT 1
    FROM conversations c
    LEFT JOIN conversations parent ON parent.id = c.parent_channel_id
    JOIN actors a ON a.id = viewer AND a.status = 'active'
    WHERE c.id = target
      AND CASE
        -- DMs, groups and their threads: participants only (ruling R20). No role, admin,
        -- owner or server role passes. A kept DM also ends with workspace membership.
        WHEN COALESCE(parent.kind, c.kind) IN ('dm', 'group') THEN EXISTS (
          SELECT 1 FROM conversation_members m
          WHERE m.conversation_id = COALESCE(c.parent_channel_id, c.id) AND m.actor_id = viewer)
          AND (NOT COALESCE(parent.kept, c.kept) OR EXISTS (
            SELECT 1 FROM organization_memberships w
            WHERE w.organization_id = c.workspace_id AND w.actor_id = viewer AND w.enabled))
        -- Channels and their threads: readers of the space; private ones, members only.
        ELSE space_readable_by(c.project_id, viewer)
         AND (COALESCE(parent.visibility, c.visibility) = 'project'
              OR EXISTS (
                SELECT 1 FROM conversation_members m
                WHERE m.conversation_id = COALESCE(c.parent_channel_id, c.id)
                  AND m.actor_id = viewer))
      END
  )
$$;
```

For channels this only narrows: every caller already paired the function with a membership
check (B report, finding 6). For personal DMs it widens one thing on purpose (ruling R20): a
DM stays readable after its participants stop sharing a workspace; sending then needs a shared
workspace again (C2's write rule, K1 §3.10). From here on, the function alone is enough. AGENTS.md's rule ("Conversation access
goes through `conversation_visible_to`") becomes literally true, and guard G2 enforces it.
Posting, moderating and the rest stay role checks (K1) on top of visibility. Reading is
visibility alone.

**Where channels belong.** A channel belongs to exactly one space (`project_id NOT NULL` for
kind channel, as today). The workspace space can hold channels too (a workspace-wide
`#general`) once its starting shape switches Channels on there. Today `in_project("workspace")`
allows only Pages (`modules/state.rs:164-167`). That becomes data in the workspace space's
module set (M1). **Chat only is whole** (ruling R22): a workspace with no project anywhere has
its workspace space with Channels on, personal spaces, and the direct scope, and every chat
feature (channels, private channels, threads, DMs, groups, search, notifications, invitations
by link) works there; nothing in the chat path requires a project row or says "project"
(K4.25 tests it; W1 owns the words in the web).

**Private channels, finished (server side).**

- `channel_create` takes `visibility: "space" | "members"` (stored `project` / `members` until
  K4.22) and adds the creator as a member.
- `conversation_member_add` / `conversation_member_remove` work for groups and private
  channels. They generalise `direct.rs:425-460`, which already serves
  `/api/v1/conversations/{id}/members`.
- A private channel's member must be able to read its space.
- The role default "private channels they join on arriving" (`roles.rs:1842`) starts taking
  effect.
- Threads already follow their parent's visibility.
- C2 owns browsing, invitations and converting a channel in the web app.

**DM permissions** (ruling R20). An account may start a DM or group with any account it shares
a workspace with, if its role in that shared workspace holds `direct.start` (K1's key;
`access::holding(conn, caller, Scope::Workspace(w))`, or until K1.2 the interim SQL function
`workspace_member_role`, deleted by K1.15), and subject to the other account's own setting
(anyone I share a workspace with, or ask me first; C2). The same rule for every kind of
account. Sending in a DM needs the two to still share a workspace; reading does not. The five
pasted `CASE` blocks and the people-only trigger go away (section 2.7, items 1 to 3). "An
agent that shares a readable space with you" (`direct.rs:1169-1222`) becomes "shares a
workspace with you", the same as for people.

**One DM list everywhere** (ruling R20). A person sees the same personal DMs and groups in
every workspace's sidebar (the direct scope's and the pre-R20 ones), plus the kept DMs of the
current workspace. The Messages overview takes the current workspace only to add its kept DMs
(`/api/v1/workspaces/{workspace}/direct`, the old path an alias using the default workspace).
DM and group operations are direct-scope operations, `POST /api/v2/direct/ops/{name}` (K2);
a `kept` workspace's `conversation_start` names the workspace and gets a kept DM when both
are members there. Web address `/w/<slug>/dm/<id>` shows the DM inside the current
workspace's shell; `/dm/<id>` resolves to the last-used workspace's form.

### 3.8 Things that rode on hidden projects

| Today | Target |
| --- | --- |
| Account notices filed under the Messages space (`space_for_member` callers) | `notifications.workspace_id NOT NULL` (backfilled from `projects`) and `project_id` nullable. Account notices have no space |
| Later items people save, filed under the Messages space (`later.rs:574`) | `saved_items` already has `organization_id`; `project_id` NULL for workspace-level saves |
| Whiteboards without a project; outbox carrier (`whiteboards/store.rs:46-51`) | The board moves into the workspace space (`project_id` set to the workspace space). Visibility is unchanged: everyone in the workspace, exactly what `whiteboard_visible_to` gives a NULL project (`20261017070000:133-145`) |
| Outbox needs a project (`0005:525`) | K3's event log carries `workspace_id` and a nullable `space_id`. If K3 hasn't landed, K4.13 makes `outbox_events.project_id` nullable, adds `workspace_id`, and realtime authorizes events without a space by `conversation_visible_to` or workspace membership |
| OAuth account-wide grants anchored to the Messages space | K1 anchors them to the account. A person's grant spans workspaces |
| Agents homed in the Messages space (host command; the crew) | K1 anchors agent connections to the account and its workspaces (A report, finding 5; ruling R19). The host command takes a workspace |
| Agents joined to the Messages space | Not needed: DM access is participation (ruling R20) |
| Workspace pages in the workspace space | Stay. The workspace space is a real kind now, not a hidden project |

### 3.9 Personal spaces

- **One per account per workspace.** A person in three workspaces has three; so does an agent
  in three workspaces (ruling R19: every account is global and joins by membership).
- **Created lazily**, by `spaces::personal`, the first time something needs it: "My space" is
  opened, a page is saved there, or an agent writes a page with no space given and its role
  allows. Eager creation would mean touching the five user-creation paths (A report,
  finding 10) and the agent-creation paths, and making rows for accounts that never use one.
  Get-or-create on the unique index `projects_one_personal_space` is safe when two requests
  race.
- **Key** `U` plus six characters from the account id (`^[A-Z][A-Z0-9]{1,9}$`, with a suffix
  retry on a clash).
- **Name**: the account's name. Shown to its owner as "My space".
- **Membership**: the owner, as `owner`, explicit. Private: `visibility = 'members'`, and
  `space_member_add` refuses. Sharing one thing out of it is a per-thing share (K1) or a move to
  another space (C1).
- **What lives there**: draft pages, notes, sheets, whiteboards, personal collections and, if
  the owner switches Tasks on, personal tasks. For an agent: its own notes and pages, including
  the memory page A2 designs.
- **Who else can read it**: by default nobody, admins included. Workspace export (H1) includes
  it. Whether an agent's managers can read its personal space is an owner question; the
  recommendation is yes, read-only, through K1's "manages this agent" rule.
- **Lifecycle**: leaving the workspace disables the membership, so the space is hidden.
  Coming back restores it. Retiring an agent archives its space.

### 3.10 Workspace in the address (server side)

Decisions:

1. **Web addresses**: `/w/{slug}/...` for everything inside a workspace. Account-level pages
   (profile, security, sign-in) have no workspace. `/w/{workspace-id}/...` is accepted and
   replaced by the slug, so the server can link before it knows the slug.
2. **API addresses** (ruling R4): the generated operation API is
   `POST /api/v2/w/{workspace}/ops/{name}` (K2), where `{workspace}` is an id or a slug; account
   operations are `/api/v2/ops/{name}` and direct-scope operations (DMs, groups) are
   `/api/v2/direct/ops/{name}` (ruling R20). The generated client uses ids, so a slug rename
   never breaks an integration. v1 keeps its paths: `/api/v1/conversations/{id}` stays, and
   K4.6's `/api/v1/workspaces/{workspace}/...` aliases are the stopgap for workspace collections
   until each module is wrapped as operations; they are deleted in K2.22 with the other
   hand-written routes. `/api/v1/organizations/{id}/...` stays as an alias for K2's window.
3. **MCP** (rulings R4, R19): one endpoint, plus the alias `/w/<slug>/mcp`, which is refused if
   it disagrees with the `workspace` argument. Every tool takes an optional `workspace`
   argument (id or slug); a credential that reaches one workspace needs none, and any
   credential that reaches more than one (a person's app, an agent in several workspaces, a
   host) names it on each call. A tool finds the workspace from the ids it is given and refuses
   a mismatch as not found. Per-workspace endpoints as the only form would mean one OAuth
   registration and protected-resource record per workspace, and would break connected agents
   on a slug rename.
4. **The current workspace is never inferred from the session** for an operation. Every
   workspace-level route takes an explicit workspace. `default_workspace` survives only to
   choose where `/` lands and as the target of the old-path aliases. The column
   `users.default_organization_id` is updated when a person switches (`PUT
   /api/v1/account/default-workspace` exists, `workspace.rs:94`), so it means "last used".
5. **Addresses**: one module, `apps/server/src/addresses.rs` (named so it doesn't collide with
   K3's `links` module, the link store), builds every web link:

```rust
pub struct WorkspaceAddress { pub id: Uuid, pub slug: String }
pub enum Target {
    Home, Inbox, Later, Messages,
    Conversation { id: Uuid, space: Option<Uuid>, kind: ConversationKind, message: Option<Uuid> },
    Direct { id: Uuid, message: Option<Uuid> },          // /w/<slug>/dm/<id> (ruling R20)
    Space { id: Uuid }, Task { space: Uuid, id: Uuid }, Page { space: Uuid, id: Uuid },
    Whiteboard { space: Uuid, id: Uuid }, Sheet { space: Uuid, id: Uuid },
    Collection { space: Uuid, id: Uuid, record: Option<Uuid> }, Run { id: Uuid },
    WorkspaceSettings(&'static str),
}
pub async fn address(conn: &mut PgConnection, workspace: Uuid) -> Result<WorkspaceAddress, AppError>;
pub fn path(workspace: &WorkspaceAddress, target: &Target) -> String;
```

   While the server setting `web.workspace_addresses` is off, `path` returns today's shapes.
   When W1 serves the new routes, it is switched on and every link in notifications, mail,
   push, search and tool results changes at once. Links built inside SQL (`search.rs:225`,
   `:259`, `:418`) move into Rust.
6. **Address grammar** (a contract with W1, who may rename segments; `addresses.rs` is the one
   place to change):

```
/w/{ws}/home | inbox | later | my-work | search | runs | runs/{id}
/w/{ws}/messages                                        the DM list (the same in every workspace)
/w/{ws}/dm/{conversationId}                             a DM or group and its threads (ruling R20)
/dm/{conversationId}                                    resolves to /w/<last used>/dm/{id}
/w/{ws}/me                                              the caller's personal space
/w/{ws}/s/{spaceId}                                     a space's home
/w/{ws}/s/{spaceId}/channels/{id} | threads/{id} | tasks | tasks/{id} | pages | pages/{id}
                   | collections/... | sheets/... | whiteboards/{id} | settings/...
/w/{ws}/settings/...                                    workspace settings
/w/{ws}/{TASK-KEY}                                      task by key, unique within the workspace
/account/...                                            no workspace
```

7. **Old links**: `GET /api/v1/links/resolve?path=/projects/<id>/work-items/<id>` answers
   `200 {"path": "/w/acme/s/<id>/tasks/<id>"}` when the caller may see the target, and 404
   (the HTTP path keeps its name; the Rust module is `addresses`)
   otherwise (unknown and forbidden look the same). It knows every old shape in
   `App.tsx:143-380`, plus `/w/<retired-slug>/...`. The web's catch-all for old paths calls it
   (W1). The server doesn't redirect old paths itself: it would need the session to find the
   workspace, and a redirect would show a stranger the workspace's slug.
8. **Slug lookup**: `GET /api/v1/workspaces/by-slug/{slug}` returns the id for members,
   `{"redirect": "<current slug>"}` for a retired slug, and 404 for everyone else.

**What W1 must do** (web side):

- Mount every workspace page under `/w/:ws/*`, from one layout route that resolves the slug
  and provides the workspace to the pages.
- Include the workspace id in every workspace-scoped query key.
- Make `/` go to `/w/<last used>/home`.
- Make every old path a redirect through `links/resolve`.
- Add a workspace switcher that navigates, and calls `default-workspace` so "last used" follows.
- Let tabs differ: nothing in local storage may assume one workspace.
- Add "My space", team spaces in the sidebar, and a browser for open spaces, each registered
  in the layout registry.
- Gate "switch on `web.workspace_addresses`" on all of this.

### 3.11 Starting shapes (for templates and M1's packages)

A space's starting shape is data in M1's package format (the content review's request, accepted:
M1.17's workspace shapes are packages, and a space shape is the same format, a package whose
target is one space). `space_create` applies it, idempotently, the way workspace templates
already do (`workspace_templates.rs:16-19`). The built-in shapes are files in
`modules/spaces/*.json`. A workspace template's `"project"` block (for example
`modules/templates/software-team.json:46-60`) becomes a `spaces: [SpaceShape]` list. The object
below is the space part of a package; M1's schema is the one schema, and K4 adds no second
one.

```json
{
  "kind": "project",
  "name": "Engineering",
  "key": "ENG",
  "description": "",
  "visibility": "members",
  "modules": { "tasks": true, "channels": true, "pages": true, "code": true, "runs": true },
  "layout": "startup",
  "channels": [ { "name": "general" }, { "name": "incidents", "private": true } ],
  "pages": [ { "title": "Guide", "role": "guide", "markdown": "…" } ],
  "collections": [ { "…": "collection_describe definition, as today" } ],
  "tasks": { "statuses": ["Backlog", "Todo", "In progress", "Done"], "labels": ["bug"] },
  "views": [ { "name": "My open tasks", "type": "list", "filter": { "assignee": "me" } } ],
  "members": { "workspaceRoles": ["member"], "role": "member" }
}
```

- Built-in shapes: the workspace space (Pages; a "Welcome" guide; Channels with `#general`,
  for new workspaces only), a personal space for a person (Pages, "Notes"), a personal space
  for an agent (Pages, "Notes", the memory page from A2), a project (as today's default) and a
  team (Channels, Pages).
- The shape used is stored in `spaces.shape`, so M1 can later show "made from" and offer
  updates.
- Applying a shape never deletes anything. Modules a shape leaves off are hidden, which M1's
  per-space module switch does.

## 4. Migration path

Order of the moves, with what coexists at each step:

1. **Vocabulary first.** Views give the new names, and the base tables gain the new columns
   (K4.1). Old code is untouched, and new code reads `spaces` and `space_members`. Nothing is
   copied.
2. **One door.** The `spaces` module and the SQL helpers land (K4.2). Callers move to them in
   small slices. Ratchet guards (G1, G2) stop new kind checks and new conversation queries
   without the rule.
3. **Conversations learn their workspace** (K4.4). `conversations.workspace_id` is added
   nullable and backfilled in the same migration from `projects.organization_id` (one UPDATE;
   if interrupted, the migration rolls back and runs again on the next start). A `BEFORE
   INSERT` trigger fills it for writers that don't set it yet. Then it is made `NOT NULL`. The
   new `conversation_visible_to` body lands in the same slice. It works with DMs both in and
   out of the Messages project.
4. **Spaces become real kinds** (K4.7 to K4.10, K4.20). Team and personal spaces are created
   through the new operations. Workspace-level whiteboards move into the workspace space with
   one UPDATE. Its derived membership rows (the trigger copies) stay as a cache that only the
   database writes, because 69 composite keys need author rows. Readers use
   `space_membership`, which ignores the cached role. The cache goes when the composite
   membership keys are retired table by table (T1, C1, A1 rebuilds), not before.
5. **DMs move out** (K4.12 to K4.17):
   - (a) DM permissions come from the workspace role.
   - (b) Carriers (notifications, saved items, outbox, account notices) stop needing a project.
   - (c) Schema expand: `project_id` becomes nullable on the DM-capable tables; the read-state
     key and the member keys are added; the immutability triggers are rewritten to allow
     exactly one change, a hidden Messages project id to NULL.
   - (d) Writers create new DMs and groups with `project_id` NULL, and readers find DMs by
     workspace and kind.
   - (e) One migration moves the existing rows (below).
   - (f) A census confirms that nothing references the hidden rows. Then they are deleted with
     their triggers and functions, and `direct` leaves the kind check.
6. **Addresses** (K4.5, K4.6, K4.18, K4.19). Slugs and explicit-workspace routes land at any
   time. Links switch when W1's routes ship.
7. **The flip** (K4.22, last, in a quiet window). Base tables are renamed, old names become
   views, and the kind value `standard` becomes `project`.

**The DM row move (step 5e).** One migration, one statement per table. Children go first, so
no composite key is left pointing at a parent row that has already changed:

```sql
-- Idempotent: each statement touches only rows still under a Messages project.
WITH direct AS (SELECT id FROM projects WHERE kind = 'direct')
UPDATE message_reactions SET project_id = NULL WHERE project_id IN (SELECT id FROM direct);
-- … the same for message_mentions, message_revisions, message_link_previews,
-- message_work_item_references, reply_requests, person_questions, files,
-- conversation_read_states, conversation_members, conversation_notification_levels,
-- notifications, saved_items, scheduled_messages, project_navigation_states, then messages,
-- then threads under DMs, then conversations.
```

- Before this, K4.14 rewrites the trigger functions that refuse a `project_id` change on these
  tables (for example `0037:84-86` and `0047:112`, `:142`), to allow exactly
  "old value is a `direct` project, new value is NULL".
- It runs in the migration's transaction: if interrupted, it rolls back and the next start
  runs it again. The data is one team's chat history, which is minutes at most (inferred; the
  census gives the real numbers).
- Run-related tables (`agent_runs`, `agent_sessions`, `launches`, `provider_sessions`,
  `repository_workspaces`, `external_execution_requests`, `work_item_*` links) can hold DM rows
  only if runs were started in DM threads. The census shows whether any exist. If so, those
  rows move in A1's run-record rebuild (or in K4.16 with A1's agreement), and K4.17 waits.

**Census (read-only, run by the lead on the live server before K4.16 and K4.17):**

```sql
SELECT format(
  'SELECT %L AS table_name, count(*) FROM %I WHERE project_id IN (SELECT id FROM projects WHERE kind IN (''direct''))',
  table_name, table_name)
FROM information_schema.columns
WHERE table_schema = current_schema() AND column_name = 'project_id'
  AND table_name IN (SELECT table_name FROM information_schema.tables
                     WHERE table_schema = current_schema() AND table_type = 'BASE TABLE')
\gexec
```

**When the old things are deleted:**

| Old thing | Deleted in |
| --- | --- |
| `workspace_space.rs`, `direct::space_project`, `space_for_member`, `space_for_actor` | K4.10 and K4.15 (they become thin calls into `spaces`, then go) |
| `direct_space_follows_workspace_role` trigger and function | K4.12 |
| Hidden `direct` rows, their memberships, `projects_one_direct_space_per_organization`, `direct` in the kind check | K4.17 |
| Workspace-space mirror triggers | not in K4: after the composite membership keys are retired (T1, C1, A1) |
| `/api/v1/organizations/...` and `/api/v1/direct` aliases, `organizationId` fields | K2's deprecation window, after W1 and the generated client move |
| K4.6's `/api/v1/workspaces/{workspace}/...` aliases | K2.22, per module, once wrapped as operations (ruling R4) |
| Old web paths | never (they redirect forever) |
| Old tool names | K2's alias window |
| `standard` kind value, base-table names | K4.22 |

## 5. Slices

Sizes: S is about a day, M a few days, L about a week. "Beside" lists the slices that can run in
parallel with this one.

### K4.1 Space words in the database (M)

- **What**: the views `workspaces`, `workspace_members`, `spaces`, `space_members`; the new
  columns on `projects` (section 3.3); the `team` and `personal` kinds; the personal-space
  unique index; workspace spaces set to `visibility = 'workspace'`. No reader changes.
- **Changes**: `apps/server/migrations/<ts>_spaces_vocabulary.sql`; new test file
  `tests/integration/spaces.rs`.
- **Depends on**: nothing.
- **Acceptance**:
  - `spaces.rs::views_write_through` does through each view every statement form the code uses
    on the base tables: INSERT, INSERT ON CONFLICT DO NOTHING, ON CONFLICT with a column list
    on renamed columns, UPDATE … FROM, DELETE, and `FOR UPDATE OF` / `FOR SHARE OF` an alias of
    the view. Each is checked against the base table.
  - `spaces.rs::kinds` asserts that a personal space needs an owner and that a second personal
    space for the same account and workspace is refused.
  - `m0_database.rs` still passes (timestamp name).
- **If a statement form fails through a view**, drop the column renames from that view (keep
  the base column names), and record it in the plan.

### K4.2 The `spaces` module and access helpers (M)

- **What**: the SQL functions `space_membership`, `space_readable_by`, `spaces_readable_by`;
  the Rust module `apps/server/src/spaces/{mod.rs,kind.rs,access.rs}` with the API of
  section 3.4; `SpaceKind::from_stored`. `workspace_space::find/join` become calls into
  `spaces`. `projects.rs` list and get use `spaces::readable` and `readable_sql`. Behaviour is
  unchanged: existing spaces are `members`, and the workspace space is implicit as before.
- **Changes**: migration; `spaces/`; `workspace_space.rs`; `projects.rs` (`:317`, `:825`,
  `:1098`); `lib.rs`.
- **Depends on**: K4.1.
- **Acceptance**:
  - `spaces.rs::membership_matrix`: explicit member, disabled member, member of a disabled
    workspace membership, inactive account, implicit workspace member, archived space, other
    workspace. Each gives the expected `space_membership` and `space_readable_by`.
  - `spaces.rs::open_team_space_is_readable_not_joined` (a team space with `visibility =
    'workspace'` made in the test).
  - `workspace_isolation.rs` unchanged and green.
- **Beside**: K4.3, K4.5.

### K4.3 Guard rails, first set (S)

- **What**: G1 (kind and insert ratchets), G2 (conversation queries carry the rule: built on
  P0.0's `conversation_listings` guard and P0.26's `conversation_visible_to_raw` rule, extended
  to every `conversations`/`messages` read, starting from an allowlist of today's sites), G4 (new tables use `workspace_id`/`space_id`), G6
  (isolation column list). See section 6.
- **Changes**: new test `apps/server/src/spaces/guards.rs` (unit tests that read the source
  tree, like the existing source-scanning guards); `tests/integration/workspace_isolation.rs:2212-2215`;
  `AGENTS.md` (one paragraph on spaces and the naming rule; coordinate with P0).
- **Depends on**: K4.1, K4.2, P0.0 (its guard is the base of G2), P0.26.
- **Acceptance**: each guard fails on a planted violation in its own unit test. The allowlists
  contain only today's sites, each with a one-line reason.
- **Beside**: K4.4, K4.5.

### K4.4 Conversations carry their workspace; one full visibility rule (M)

- **What**: `conversations.workspace_id`: added, backfilled, filled by trigger for old writers,
  then NOT NULL, with an index `(workspace_id, kind)`. The new body of
  `conversation_member_visible` (section 3.7); if the split of `conversation_visible_to` (K1
  §3.10, ruling R3) has not landed, do it first. Writers that insert conversations (`conversations.rs:962`, `direct.rs`, the six
  thread creators: B report, finding 4) set `workspace_id` explicitly.
- **Changes**: migration; `conversations.rs`, `direct.rs`, `mcp/talk.rs`, `mcp/replies.rs`,
  `work_items.rs`, `launch.rs` (one bind each).
- **Depends on**: K4.2, P0.0 (the sidebar leak fix lands first; this slice keeps its test).
- **Acceptance**:
  - `direct_conversations.rs`: a new test that a participant of a **kept** DM whose workspace
    membership is disabled can no longer read it through any path (HTTP timeline, MCP
    `conversation_read`, search, realtime authorization); a personal DM stays readable to its
    participants (ruling R20), and no owner or admin who is not a participant reads either.
  - `m2_conversations_database.rs`: the function returns false for a project-wide channel when
    the viewer isn't a member of the space. Today it returns true.
  - A comment in the migration records EXPLAIN timings for the conversation list query and
    search on a seeded 50k-message dataset, before and after. The lead reviews them.
- **Beside**: K4.5, K4.6, K4.7.

### K4.5 Workspace slugs (S)

- **What**: the `workspace_slugs` table and backfill; slug rules and the reserved list
  (`apps/server/src/workspace/slug.rs`); `workspace_update` with `slug`;
  `GET /api/v1/workspaces/by-slug/{slug}`; WorkOS sign-up and bootstrap make a readable slug
  from the name with a numeric suffix (H1 owns sign-up, so coordinate).
- **Changes**: migration; `workspace.rs:467`; `auth/workos.rs:508-511`; `bootstrap.rs:627-640`.
- **Depends on**: nothing (K2.3 for the operation wrapper, soft). `aw-direct-scope` joins the
  reserved words and the backfill skips `kind = 'direct'`, so the direct scope can never be addressed.
- **Acceptance**:
  - `spaces.rs::slug_rename_keeps_old_links`: the old slug resolves to a redirect.
  - Another workspace can't take a retired slug.
  - A non-member gets 404 for an existing slug.
  - Reserved words are refused.
  - Isolation cases are added for the new route.
- **Beside**: everything.

### K4.6 An explicit workspace on every workspace-level request (M)

- **What**:
  - `/api/v1/workspaces/{workspace}/...` aliases for every `/api/v1/organizations/{id}/...`
    route, where `{workspace}` is an id or a slug, through one extractor
    `WorkspacePath` (shared with K2's `/api/v2/w/{workspace}/...`). These v1 aliases are the
    stopgap until each module is wrapped as operations (ruling R4); K2.22 deletes them.
  - Workspace-level routes that used `default_workspace` take an explicit workspace: the
    Messages overview, DMs and groups (`direct.rs:48-56`), notification settings
    (`notification_settings.rs:384`), webhooks (`webhooks.rs:464`), Later saves
    (`later.rs:568`), Home (`home.rs`), and MCP automation tools
    (`mcp/automation_tools.rs:229`). The old paths keep using the default workspace.
  - The session payload adds `slug` (already present) and `lastWorkspaceId`.
- **Changes**: `workspace.rs`, `direct.rs`, `notification_settings.rs`, `webhooks.rs`,
  `later.rs`, `home.rs`, `auth/mod.rs`, `policy_inventory.rs` (new rows placed next to the
  related ones), `workspace_isolation.rs` cases.
- **Depends on**: K4.5.
- **Acceptance**:
  - `direct_conversations.rs::dms_of_each_workspace` (reworked for ruling R20): a person in
    two workspaces gets the same personal DM list in both without changing their default; a
    kept DM of workspace A appears only in A's list; a pre-R20 DM that carries A's id appears
    in both.
  - The isolation suite covers the new routes with the other workspace's slug and id.
- **Beside**: K4.7, K4.11.

### K4.7 Space operations: list, open, create, update, archive (M)

- **What**: the operations `space_list`, `space_open`, `space_create` (`project` and `team`),
  `space_update`, `space_archive`, `space_restore` through K2's registry, with tools and
  aliases (section 3.5). The events `space.created`, `space.updated`, `space.archived` and
  `space.restored` are written beside today's project events. `projects.rs` create becomes a
  call to `spaces::create`.
- **Changes**: `spaces/ops.rs`; `projects.rs`; `mcp` tool registration (next to the project
  tools); `outbox.rs` payloads; `agent_parity.rs` entries.
- **Depends on**: K4.2; K2.3, K2.5 (the operation registry and its template). Until K2 lands,
  use HTTP plus an MCP tool pair, as today's pattern does.
- **Acceptance**:
  - `spaces.rs::team_space_lifecycle`: create, rename, archive, restore, list.
  - A person and an agent holding the same role get the same results: one test that runs both
    through the same calls.
  - Creating a workspace or personal space through `space_create` is refused.
  - Isolation and write-isolation cases are added.
- **Beside**: K4.8 (after the operations file exists), K4.11.

### K4.8 Space membership operations and events (M)

- **What**: `space_member_add`, `space_member_remove`, `space_member_set_role`, `space_join`,
  `space_leave`; the `space.member.*` events with `accountId`; refusals on the workspace and
  personal kinds. `memberships.rs` add and remove call into `spaces`.
- **Changes**: `spaces/members.rs`; `memberships.rs`; `outbox.rs`; MCP tool aliases
  `project_members` and `project_member_set`.
- **Depends on**: K4.7; K1.10 (soft: role rows; until then, today's role names).
- **Acceptance**:
  - `spaces.rs::join_open_space`: a workspace member joins an open team space, a closed one
    refuses, and the event is emitted with `accountId`.
  - `m7_memberships.rs` passes unchanged.
- **Beside**: K4.9, K4.10.

### K4.9 Personal spaces (M)

- **What**: `spaces::personal` (lazy, race-safe); the `space_personal_open` operation and tool;
  the person and agent starting shapes (Pages and Notes). `page_write` with no space and no
  project resolves to the caller's personal space when their role allows. C1 and K2 agree on
  the default; until then the argument is explicit (`space: "me"`). Retiring an agent archives
  its space.
- **Changes**: `spaces/personal.rs`; `memory.rs` (where the page's space is resolved);
  `mcp/memory_tools.rs`.
- **Depends on**: K4.7; K1.4 (the "manages this agent" rule, for managers' read access if the
  owner says yes to question 1).
- **Acceptance**:
  - `spaces.rs::personal_space_is_private`: the owner reads and writes; another person, a
    workspace admin and an agent get 404 through HTTP, MCP and search.
  - Two concurrent first calls make one space.
  - A page in it appears in the owner's search only.
  - Isolation cases are added.
- **Beside**: K4.10, K4.11.

### K4.10 The workspace space as a plain kind (M)

- **What**:
  - `workspace_space.rs` folds into `spaces` (`workspace_space`, implicit membership).
  - Whiteboards with `project_id IS NULL` move into the workspace space (one UPDATE; new boards
    without a space go there). The Messages-space carrier in `whiteboards/store.rs:46-51`
    goes, and `whiteboard_visible_to` is unchanged in effect.
  - `ModuleSet::in_project(&str)` takes `SpaceKind`. The kind's module restriction moves into
    the workspace-space starting shape, ready for M1's per-space switches.
  - The web `workspaceSpace` flag becomes `kind` (coordinate with W1).
- **Changes**: `workspace_space.rs` (deleted), `spaces/`, `whiteboards/store.rs`,
  `modules/state.rs:149-170`, `modules/gate.rs:137-155`, `projects.rs:1184`, `mcp.rs:5605`,
  `memory.rs:1265`.
- **Depends on**: K4.2; M1.2 (per-space module switches; until then the kind's restriction is
  data in this slice).
- **Acceptance**:
  - `awd96_workspace_pages.rs` passes.
  - A channel created in the workspace space of a workspace with no project works end to end
    (ruling R22; K4.25 tests the whole chat path).
  - New test: a workspace board's events reach every workspace member's socket and nobody
    else's (`m1_realtime_websocket.rs`).
  - The G1 ratchet count for `'workspace'` kind literals drops.
- **Beside**: K4.8, K4.9.

### K4.11 Private channels: rule and operations (M)

- **What**:
  - `channel_create` (HTTP and MCP) takes `visibility`.
  - The `conversation_member_add/remove` operations cover private channels (generalised from
    `direct.rs:425-460`); members must be able to read the space.
  - The role default "private channels they join on arriving" (`roles.rs:1842-1870`) works.
  - `conversation.member.added` is emitted.
- **Changes**: `conversations.rs:952-970`, `direct.rs` (member code moves to
  `conversations/members.rs`), `mcp/talk.rs` (`channel_create`), `roles.rs`, `outbox.rs`.
- **Depends on**: K4.4.
- **Acceptance**:
  - `m2_conversations_http.rs::private_channel`: a non-member of the channel who is in the
    project doesn't see it in the list, timeline, search, mentions autocomplete, notifications
    or realtime.
  - Adding a member gives access; removing withdraws their reply requests.
  - A thread under it follows it.
  - Write-isolation cases are added.
- **Beside**: K4.6 to K4.10.
- **Touch**: C2 builds the browse and invite UI.

### K4.12 DM permissions from a shared workspace's role (M)

- **What** (reworked for ruling R20):
  - The five pasted direct-space `CASE` blocks (`conversations.rs:3697-3708` and copies) and
    `direct_space_follows_workspace_role` are replaced by the holding in a workspace the
    accounts share: K1's `access::holding(conn, caller, Scope::Workspace(w))`, or an interim
    SQL function `workspace_member_role(workspace, account)` that K1.15 deletes.
  - Starting a DM or group needs `direct.start` in any workspace the starter shares
    with each other participant; sending needs a shared workspace still; reading needs only
    participation. Agents and people alike (section 2.7, items 1 to 3).
  - `reachable_agents` becomes "accounts that share a workspace with you", for every kind.
- **Changes**: `conversations.rs`, `direct.rs`, a migration dropping the trigger.
- **Depends on**: K4.4; K1.2 (`access::holding`) or the interim function; K1.9 (soft: the
  `direct.start` key; `channels.post` until then).
- **Acceptance**:
  - `direct_conversations.rs::agent_dm_permissions_follow_role`: an agent and a person with
    the same custom workspace role get the same allow and deny results for start, send, edit
    and member add.
  - Two accounts that share only workspace B: a role in B without `direct.start` can't
    start a DM; with it, it can; after one leaves B, sending is refused and reading works.
  - A role change takes effect on the next request with no copied row.
- **Beside**: K4.13.

### K4.13 Workspace-level carriers off the Messages space (M)

- **What**:
  - `notifications.workspace_id` (backfilled, NOT NULL) and `project_id` nullable.
  - Account notices (`scheduled_messages.rs:802`, `usage/alerts.rs:361`, `automations.rs:1913`)
    and people's Later saves (`later.rs:574`) are written with no project.
  - Notification reads and push and mail filter by `workspace_id`.
  - If K3's event log hasn't landed: `outbox_events.workspace_id`, `project_id` nullable, and
    realtime authorizes events without a space by conversation visibility or workspace
    membership (`realtime.rs:506-560`).
- **Changes**: migration; `notifications.rs`, `later.rs`, `usage/alerts.rs`, `automations.rs`,
  `scheduled_messages.rs`, `outbox.rs`, `realtime.rs`.
- **Depends on**: K4.4; K3.2 and K5.5 (the log and the live tail; if they have not landed, the
  outbox stopgap above, removed by K3.22).
- **Acceptance**:
  - `notifications.rs` tests: a capacity alert and a failed scheduled send reach the person
    with no Messages project existing for that workspace (the test deletes nothing; it creates
    a fresh workspace where none was ever made).
  - The isolation suite covers notification lists across two workspaces.
- **Beside**: K4.12.

### K4.14 DM storage, expand (S)

- **What**: `project_id` DROP NOT NULL on the DM-capable tables; `conversation_members.workspace_id`
  with its key into `organization_memberships` (made skippable for the direct scope by K4.23);
  the unique index on read states `(conversation_id, actor_id)` (the primary key swaps in
  K4.17); `UNIQUE (workspace_id, dm_key) WHERE kind='dm'` (narrowed by K4.23 and K4.24 to the
  two forms of section 3.7); the trigger functions rewritten to allow only "direct project to
  NULL". No code change.
- **Changes**: one migration.
- **Depends on**: K4.4.
- **Acceptance**:
  - `direct_conversations.rs::schema_allows_only_the_move`: updating a DM message's
    `project_id` from the Messages project to NULL succeeds; to another project fails; from
    NULL back fails.
  - The suite is otherwise unchanged.
- **Beside**: K4.12, K4.13.

### K4.15 DMs are written and read without a project (M)

- **What**:
  - New DMs and groups (and threads under them, and all their child rows) are inserted with
    `project_id` NULL. `open_dm` finds a pair by `dm_key` (personal) or `(workspace_id, dm_key)`
    (kept), per K4.24.
  - The sidebar, timeline, read state (upserts on `(conversation_id, actor_id)`), search,
    Later, forwarding, link previews, files, reply requests and realtime select DMs by
    `workspace_id` and kind, not by the Messages project.
  - Names in DMs come from `organization_actor_profiles` (K1's one profile source), not from
    Messages-space memberships.
  - `direct::space_for_member` and `space_for_actor` are gone; `space_project` is used only by
    the census.
- **Changes**: `direct.rs`, `conversations.rs`, `search.rs`, `later.rs`, `forwarding.rs`,
  `link_previews.rs`, `files.rs`, `replies.rs`, `realtime.rs`, `mcp/talk.rs`,
  `mcp/briefing.rs`.
- **Depends on**: K4.12, K4.13, K4.14; K1.28 (soft: the one profile source; read names from
  `organization_actor_profiles` directly until then).
- **Acceptance**:
  - The full `direct_conversations.rs`, `message_replies.rs`, `message_edits.rs`,
    `message_reactions.rs` and `notifications.rs` suites pass with DMs created after the
    switch, and with DMs created before it (rows still under the Messages project). The tests
    seed both.
  - `workspace_isolation.rs` and `write_isolation.rs` pass.
- **Beside**: K4.18.

### K4.16 Move existing DM rows (S)

- **What**: the migration of section 4 (children first; idempotent predicates). The lead runs
  the census before and after on a copy of the live database.
- **Changes**: one migration; a test fixture that builds pre-move data.
- **Depends on**: K4.15; A1.1 (only if the census finds run rows: they move in A1's rebuild).
- **Acceptance**:
  - `direct_conversations.rs::move_keeps_everything`: conversation and message ids, read
    positions (`highest_seen_sequence`), members, reactions, reply-request state and unread
    counts are the same before and after the migration.
  - Running the statements twice changes nothing.
- **Beside**: K4.18, K4.19.

### K4.17 Retire the hidden Messages projects (S)

- **What**: delete the `direct` rows (memberships, counters, instructions) once the census is
  zero. Drop `projects_one_direct_space_per_organization` and `direct` from the kind check. Add
  the `(kind IN ('dm','group')) = (project_id IS NULL)` check and the thread trigger rule.
  Swap the read-state primary key. The host command `direct-agent-create` takes a workspace
  (`main.rs:229-243`).
- **Changes**: migration; `direct.rs`; `main.rs`.
- **Depends on**: K4.16; K1.6 and K1.22 (agent connections and account-wide grants anchored off
  projects); A1.1 if run rows were found.
- **Acceptance**:
  - The G1 ratchet for `'direct'` reaches zero.
  - `spaces.rs::no_hidden_kinds`: no row of kind `direct` exists and the check refuses one.
  - The live crew agents (after K1's move) still receive and answer DMs. The lead checks this
    once on the live server.
- **Beside**: K4.20.

### K4.18 The address module and resolver (M)

- **What**:
  - `apps/server/src/addresses.rs` (section 3.10; not `links.rs`, which is K3's link store),
    with the server setting `web.workspace_addresses` (off).
  - Every server-built web link moves to it (about 150 sites in about 40 files). Links built
    in SQL move to Rust.
  - `GET /api/v1/links/resolve`.
  - G5 (address ratchet, in `addresses::guards`).
- **Changes**: `addresses.rs`; `notifications.rs`, `search.rs`, `later.rs`, `modules/things.rs`,
  `replies.rs`, `scheduled_messages.rs`, `conversations.rs`, `mcp/*` (common, runs, talk,
  reads, tasks, briefing, …), `work_items.rs`, `projects.rs`, `whiteboards.rs`, mail
  templates. Split by area across two PRs if review gets heavy.
- **Depends on**: K4.5.
- **Acceptance**:
  - `addresses.rs` unit tests: every `Target` gives the old shape with the setting off and the
    grammar shape with it on.
  - `spaces.rs::resolve_old_paths`: each old shape in `App.tsx:143-380` resolves for a member
    and 404s for a non-member and across workspaces.
  - A resolve isolation case is added.
- **Beside**: K4.15, K4.16.

### K4.19 Links switch to workspace addresses (S)

- **What**: turn `web.workspace_addresses` on by default. Mail and push carry `/w/` links.
  `/w/<id>` is accepted.
- **Changes**: `config.rs` default; tests.
- **Depends on**: K4.18; W1.14a (the `/w/:ws` routes and old-path redirects) merged.
- **Acceptance**: `notifications.rs` and mail tests assert `/w/<slug>/` links. A link from
  before the switch, stored in a message body, still opens through resolve (W1 test).
- **Beside**: K4.20, K4.21.

### K4.20 Space starting shapes (M)

- **What**: the `SpaceShape` format (section 3.11) as the space part of M1's package schema
  (no separate schema); the built-in shapes in `modules/spaces/*.json`; `space_create { shape }`. Workspace
  templates' `project` block reads as `spaces: [...]`, and the old key is still accepted.
  `spaces.shape` is recorded.
- **Changes**: `spaces/shape.rs`, `workspace_templates.rs`, `modules/templates/*.json`,
  `modules/spaces/*.json`.
- **Depends on**: K4.7; M1.2 (soft: per-space module switches; until then, `modules` in a shape
  applies only where the workspace allows and is recorded for M1); M1.17 (soft: the package
  schema; until then the shape is written to its draft and checked against it when it lands).
- **Acceptance**:
  - `awd76_workspace_templates.rs` passes unchanged with the old template files.
  - A new test applies a shape twice and gets no second channel, page or collection.
  - A team shape makes its channels and guide page.
- **Beside**: K4.17, K4.19, K4.21.

### K4.21 The rename at the interface (M)

- **What**:
  - Responses carry `workspaceId` beside `organizationId`, and `spaceId` beside `projectId`
    where it means the general concept. Requests accept either; a request that gives both,
    with different values, is a validation error.
  - The tool alias map (old name to new name) is handed to K2.
  - Rust types in K4's modules are named Workspace and Space (no mass rename elsewhere).
  - The user-visible copy in server messages ("project" where any space is meant) changes.
- **Changes**: response structs in `projects.rs`, `workspace.rs`, `direct.rs` and
  `conversations.rs` views; `mcp` argument parsing (one helper, `common::space_arg`, that reads
  `spaceId` or `projectId`).
- **Depends on**: K2.6 (envelopes and aliases); K4.7.
- **Acceptance**:
  - `mcp_groundwork.rs`: every tool that takes `projectId` also accepts `spaceId`.
  - The generated TypeScript types (K2) include both during the window.
- **Beside**: K4.19, K4.20.

### K4.22 The flip (M, late, in a quiet window)

- **What**: in one migration:
  - drop the K4.1 views;
  - rename `projects` to `spaces`, `project_memberships` to `space_members`, `organizations`
    to `workspaces` and `organization_memberships` to `workspace_members`, with their
    `organization_id`/`project_id`/`project_key` columns;
  - create views with the old names and old column names;
  - set the kind value `standard` to `project`, and `visibility` `project` to `space` on
    conversations, with the check constraints updated.
  
  Guard G3 now protects the old-name views.
- **Changes**: one migration; `SpaceKind::from_stored`; the few remaining `'standard'`
  literals (the G1 ratchet should have brought them near zero).
- **Depends on**: everything above. The lead announces a window in which no open branch adds
  a migration that references the old names. A branch that still does fails CI loudly (a
  foreign key can't reference a view), and the fix is to rename the reference.
- **Acceptance**:
  - The full CI is green.
  - `spaces.rs::old_names_still_work` repeats K4.1's statement-form test against the old-name
    views.
  - A copy of the live database migrates in under a minute (renames only touch the catalogue;
    the two value UPDATEs are small).
- **Beside**: nothing.

### K4.23 The direct scope (S)

- **What** (ruling R20): `organizations.kind`, the reserved direct-scope row and its fixed id
  (`spaces::DIRECT_SCOPE_ID`, SQL `direct_scope_id()`), the CHECKs of section 3.3 (no
  membership, space or role row in it; only DM, group and thread conversations), the
  `direct_messages` workspace setting (`personal` default, `kept`), `conversations.kept`, and
  `conversation_members.member_workspace_id` with its key (section 3.7). The switcher, slug
  lookup, workspace lists and `{workspace}` path parts refuse the direct scope.
- **Changes**: migration `<ts>_direct_scope.sql`; `spaces/direct.rs`; `workspace.rs` (lists skip
  `kind = 'direct'`); the settings registry; `WorkspacePath`.
- **Depends on**: K4.14 (DM storage expand: `project_id` nullable, member keys).
- **Acceptance**:
  - `spaces.rs::direct_scope_is_closed`: inserting a membership, a space or a role in the direct
    scope is refused; it is in no workspace list, no switcher payload, no slug lookup; a request
    naming its id as `{workspace}` is 404.
  - Guard G7 fails on a planted row of another kind carrying its id.
- **Beside**: K4.15 to K4.21.

### K4.24 Personal DMs: one per pair, one list everywhere (M)

- **What** (rulings R20, R31): new DMs and groups carry the direct scope's id (or, in a `kept`
  workspace between its members, that workspace's id with `kept = true`); one personal DM per
  pair on the server and at most one kept DM per pair in each `kept` workspace (starting again
  returns it; inside a `kept` workspace the kept one is returned and the personal one stays
  listed, labelled; pre-R20 duplicates stay, labelled); the DM list is
  the same in every workspace plus that workspace's kept DMs; DM and group operations on
  `/api/v2/direct/ops/...` (K2); addresses `/w/<slug>/dm/<id>` and `/dm/<id>` (K4.18's module);
  existing DMs keep their workspace id and are personal, with no data move; workspace export
  includes only conversations that carry its id, and a person can export their own DMs (H1).
- **Changes**: `direct.rs`, `conversations.rs`, `search.rs` (DM hits filtered by participation,
  not workspace), `notifications.rs`, `realtime.rs` (the direct scope is a subscription, K5),
  `addresses.rs`, `mcp/talk.rs`.
- **Depends on**: K4.23, K4.15, K4.12; K2.3 (soft: the direct route; hand-written until then).
- **Acceptance**, in `direct_conversations.rs`:
  - `one_dm_per_pair`: two people who share workspaces A and B start a DM from A and from B and
    get the same conversation; it shows in both sidebars.
  - `kept_dms_stay_in_their_workspace`: in a `kept` workspace a new DM between members carries
    its id, is listed only there, follows its export, and ends when one leaves; opening it
    again there returns the same kept DM, and the pair's personal DM is still listed,
    labelled (R31).
  - `pre_r20_dms_are_personal`: a DM created before the change is listed in both workspaces and
    keeps its id and workspace column.
  - No owner, admin or server operator who is not a participant reads any of them through HTTP,
    MCP, search, export or realtime (the P0.0 class of leak, an isolation case).
  - `workspace_isolation.rs` cases for the direct routes.
- **Beside**: K4.16 to K4.21.

### K4.25 A chat-only workspace is whole (S)

- **What** (ruling R22): an integration test, and the server fixes it finds, that a workspace
  with only Chat on and no project row anywhere supports the whole chat path: channels and
  private channels in the workspace space, threads, DMs and groups, mentions, search,
  notifications, invitations by link, Later, and that no server-built text (mail, push,
  notification, error message) for it says "project", "task", "agent" or "run", except that
  an agent member is still labelled "agent" as account information (ruling R31). An agent may
  join such a workspace like any account; the test runs once with people only and once with an
  agent member.
- **Changes**: `tests/integration/chat_only.rs` (new); server copy found by it.
- **Depends on**: K4.10, K4.24; M1's "Friends" shape (soft: the test switches modules off
  directly until then). W1 owns the browser walk per shape.
- **Acceptance**: the test above passes on a fresh workspace made without any project.
- **Beside**: K4.17 onward.

### K4.26 Invitation link tokens (S)

- **What** (ruling R30: invitation link tokens are K4's, with the other invitations; C2.40 and
  R22's "invitations are a link" need them): `invite_links (id, workspace_id, space_id NULL,
  role_id, token_digest, created_by, max_uses, uses, expires_at, revoked_at)`; operations
  `invite_link_create`, `invite_link_revoke`, `invite_link_list` (K2 registry, agent tools) and
  the accept route that turns a valid link into K1's invitation flow
  (`accounts::create_person` or a membership for a signed-in account). Permission
  `members.invite` for the link's role, within the creator's holding (`access::may_grant`).
  The token is shown once; only its digest is stored.
- **Changes**: migration `<ts>_invite_links.sql`; `spaces/invite_links.rs`; `invitations.rs`
  (accept path); isolation and write-isolation cases.
- **Depends on**: K1.29 (one invitation engine); K2.3 (soft: hand-written route and tool until
  the generator).
- **Acceptance**: a link admits up to `max_uses` people with its role and no more; a revoked or
  expired link is refused with the same answer as an unknown one; a link made by a member
  cannot grant Admin; the token never appears in a response after creation, a log line or a
  row.
- **Beside**: K4.18 onward.

## 6. Guard rails

All are unit tests next to the code they protect, or in `tests/integration/spaces.rs`.

- **G1, no new hidden kinds and one way to make a space.**
  - `spaces::guards::kind_check_is_the_list` reads `pg_constraint` for `projects_kind_check`
    (later `spaces_kind_check`) and asserts the kinds are exactly the ones `SpaceKind` knows,
    plus `direct` until K4.17. Adding a kind needs `SpaceKind`, the guard list and a sentence
    of rationale in the test.
  - `spaces::guards::kind_literals_ratchet` counts `kind = 'direct'`, `'workspace'`,
    `'standard'`, `NOT IN ('direct'` and Rust `== "direct"` per file. The count may only go
    down. `direct` must reach zero in K4.17.
  - `spaces::guards::one_space_creator` fails on `INSERT INTO projects` or `INSERT INTO spaces`
    outside `spaces/` (allowlist: `seed.rs`, test support, and migrations).
- **G2, no conversation query without the rule.** `spaces::guards::conversation_queries_check_visibility`
  extracts every raw SQL string in `apps/server/src/**/*.rs` that reads `conversations` or
  `messages` (`FROM` or `JOIN`). It requires `conversation_visible_to(` in the same string,
  unless the file and function are in `conversation_query_allowlist.txt` with a reason.
  Reasons are, for example: a write by id after a visibility-checked lock; an internal job that
  re-checks per recipient; the visibility module itself. The allowlist starts at today's sites
  and may only shrink, except by review. This is the check the B report asked for (finding 6).
- **G3, the compatibility views match their tables.** `spaces.rs::views_match_tables` compares
  each view's columns with its base table's, through the name map. It catches a branch that
  adds a column to `projects` but not to `spaces`, or the other way round.
- **G4, new tables follow the convention.** `m0_database.rs` gains
  `new_tables_use_workspace_and_space_ids`: in migrations named after K4.1's timestamp, a
  `CREATE TABLE` may not declare `organization_id` or `project_id`. It must declare
  `workspace_id`, and `space_id` where it has one. After K4.22, the same test refuses
  `REFERENCES projects(`, `REFERENCES organizations(`, `ALTER TABLE projects`,
  `ALTER TABLE organizations` and `CREATE TRIGGER … ON projects` in new migrations, because
  those names are views from then on.
- **G5, addresses in one place.** `addresses::guards::no_hand_built_links` counts `"/projects/`,
  `"/messages/`, `'/messages/'`, `"/whiteboards/`, `"/dm/` and `"/runs/` outside `addresses.rs`
  (`/api/` paths excluded). The count is a ratchet that ends at zero in K4.18.
- **G6, the isolation nets know the new columns.** `workspace_isolation.rs:2212-2215` adds
  `workspace_id` and `space_id` to the column list it scans, so new tables are covered by the
  two-workspace net from their first migration.
- **G7, the direct scope stays closed** (ruling R20). `spaces.rs::direct_scope_holds_only_dms`:
  for every base table with an `organization_id` or `workspace_id` column, rows carrying the
  direct scope's id are allowed only in an allowlist (conversations of kind dm, group or
  thread under them, messages and their revisions, mentions, reactions, link previews, read
  states, members, notification levels, files attached to them, reply requests, person
  questions, events, and A1's requests and runs raised from them); the CHECKs of section 3.3
  refuse memberships, spaces and roles. A second test asserts no listing route returns
  direct-scope conversations except the participant-filtered DM list.

## 7. Risks and open questions

**Risks**

1. **Writing through views may not support a statement form the code uses.** The likeliest
   are `ON CONFLICT (col, …)` with renamed columns, and `FOR SHARE OF <view alias>`. K4.1's
   test exercises every form found in the code first. Fallback: views without column renames
   (only new table names), or no views and the new names only in Rust. Nothing else in the
   plan depends on the views, only on the helpers.
2. **The DM move runs through the chat core**, which C1 and C2 edit at the same time
   (`conversations.rs` is 5,331 lines). A mistake there is a privacy bug.
   - The rule branches on kind, so it is correct in both states, and the switch (K4.15) is
     separate from the data move (K4.16).
   - The acceptance tests seed DMs from before and after the switch.
   - G2 stops new unguarded queries.
   - Each step is small and can be reverted alone until K4.16.
   - The sign of trouble is a failing isolation or direct-conversation test, or a DM missing
     from someone's sidebar after deploy (the lead checks the census and one person's sidebar
     on live).
3. **The stricter `conversation_visible_to` costs time on lists and search.** It adds two
   indexed lookups per row. K4.4 records EXPLAIN timings. Fallback: a workspace-filtered list
   query that applies `spaces_readable_by` once, with the function per row only for private
   channels.
4. **Retiring the Messages projects too early cuts agents off.** The live crew is homed there.
   K4.17 is gated on K1's anchor move and a zero census. It is the only step that deletes
   anything, and it deletes only rows the census shows are no longer referenced.
5. **Links switch before the web can serve them.** That is why K4.19 is a separate slice gated
   on W1. Fallback: switch the setting off; `resolve` makes both shapes work.
6. **The workspace-space membership cache stays for a while.** It is derived rows kept by a
   deferred trigger. It is honest about being a cache (readers use `space_membership`), but it
   is still a copy until the composite membership keys go. Noticed by: a G2-style ratchet on
   reads of `project_memberships` joined to a workspace-kind space.

**Questions for the owner** (each with my recommendation)

1. May an agent's managers read the agent's personal space? *Yes, read-only*, so a person can
   see the notes and pages their agent keeps. People's personal spaces stay private, even from
   admins, except in a full export.
2. Can every workspace member read an open team space without joining, like a public channel?
   *Yes*, with `visibility` per space. Projects stay members-only by default.
3. Should a retired workspace slug stay reserved to its workspace for ever? *Yes*, so an old
   link never opens someone else's workspace.
4. Rename the remaining `project_id` and `organization_id` columns (121 and 56 tables, about
   11,000 references in the server) in one sweep before the public release? *Not as one
   sweep.* New tables use the new names now, rebuilt modules rename theirs, and the rest stay,
   documented. Do a scripted sweep only in a freeze window if contributors find the old names
   confusing.
5. Should new workspaces get workspace-wide channels (`#general` in the workspace space)?
   *Yes*: the Slack feel needs channels that aren't in a project.

## 8. Touch points

**What K4 needs**

| From | What | Used by |
| --- | --- | --- |
| K1 | `access::holding(conn, caller, Scope::Workspace(w))` (K1.2; K1.15 deletes the interim `workspace_member_role`); the existing keys `projects.create`, `projects.manage`, `members.manage`, `settings.manage`, and the new `direct.start` (K1's requested-permissions table); the composer of `conversation_visible_to` (K1 §3.10, ruling R3); accounts that join many workspaces (K1.33, ruling R19) | K4.4, K4.7, K4.8, K4.12, K4.24 |
| K1 | Agent connections and account-wide OAuth grants anchored to the workspace or account, not a project | K4.17 |
| K1 | `organization_actor_profiles` as the one name and handle source | K4.15 |
| K1 | A "manages this agent" rule, for the personal-space read (if the owner says yes) | K4.9 |
| K1 | Roles per space live in `space_members.role`/`role_id` (or `role_grants` keyed by space) | K4.8 |
| K2 | The operation registry; tool aliases; `ctx.workspace_id` on every operation; `/api/v2/w/{workspace}/ops/...`, `/api/v2/direct/ops/...` and the optional `workspace` argument (rulings R4, R20) | K4.6, K4.7, K4.21, K4.24 |
| K3 | An event log row with `workspace_id` (the direct scope's for DMs) and a nullable `space_id`; a thing's `space` nullable for DMs and their messages; K3's event names | K4.13, events |
| K5 | Realtime authorization that calls `space_readable_by` and `conversation_visible_to`, and recomputes a connection's scopes on `space.member.*`, `workspace.member.*` and `conversation.member.*` | K4.13, K4.15 |
| M1 | Per-space module switches (replacing `in_project`); packages embedding `SpaceShape` | K4.10, K4.20 |
| A1 | Run rows in DM threads, if the census finds any, move in the run rebuild | K4.16, K4.17 |
| W1 | `/w/:ws` routes, old-path redirects via `links/resolve`, the switcher, workspace in query keys, "My space", team spaces | K4.19 |
| C2 | Private-channel UI; the one DM list in every sidebar; the "ask me first" setting and block; chat standing alone (ruling R22) | K4.11, K4.24, K4.25 |
| C1 | Page move between spaces (publishing a draft from a personal space) | K4.9 |
| H1 | Sign-up and bootstrap make slugs through `workspace::slug`; export and import include spaces and the slug history | K4.5 |
| P0 | One paragraph in `AGENTS.md`: the space rule, the naming convention, G2 | K4.3 |

**What K4 provides**

| To | What |
| --- | --- |
| Everyone | `spaces` and `space_members` (views now, tables after K4.22); the column convention (`workspace_id` and `space_id`, referencing `organizations` and `projects` until K4.22); `spaces::{get, readable, membership, personal, workspace_space, create, readable_sql}`; the SQL functions `space_membership`, `space_readable_by`, `spaces_readable_by` |
| K1 | The membership half of access, with no role semantics; the list of account-kind branches in this area (section 2.7) |
| K3, K5, A2 | The events `space.*`, `space.member.*`, `conversation.member.added`, `conversation.visibility.changed` and `workspace.updated`, each carrying `workspaceId` and, for membership events, `accountId` |
| A2 | Personal spaces for agents' notes and memory pages; "these spaces" as a subscription scope |
| C1, C2 | `conversation_member_visible`, K4's helper inside K1's composer; private channels on the server; DMs in the direct scope, or in the workspace for `kept` |
| T1 | Any space can hold tasks (every space has a key) |
| M1 | `SpaceShape` and `space_create { shape }` |
| W1 | The address grammar, `links/resolve`, `workspaces/by-slug`, explicit-workspace routes |
| H1 | Slug rules, reserved words and slug history |

## 9. Changes to the end state

1. Section 3, "Thing": "Every thing has an id, a kind, a title, a status, an owner, a space
   and a version." DMs, groups, their messages and account-level things have no space.
   Proposed: "…an owner, a workspace, a space (none for DMs, groups and things that belong to
   an account) and a version."
2. Section 5, "The web app": "One shell, mounted once, with the workspace in the address
   (`/w/acme/...`)." Add (ruling R4): "The operation API is `/api/v2/w/{workspace}/...` (id or
   slug); one tool endpoint serves every workspace, with an optional `workspace` argument that
   a credential reaching several workspaces must give." Draft 3 carries R4; this is the wording
   K4 assumes.
5. DMs (ruling R20, carried by draft 3): "DMs and groups belong to the people in them; the same
   list shows in every workspace; a workspace can choose to keep its members' DMs."
3. Section 9 and section 10, item 4 (renaming reaches the database last). Keep it, made
   precise: "New tables use the new names from now on; the core tables get their new names as
   views at once and physically last; existing column names are renamed only when a module's
   tables are rebuilt."
4. Section 3, "Space": add "A personal space is made the first time it's used. Projects and
   team spaces differ only in their starting shape and whether everyone in the workspace can
   find them."

## Triggers (ruling R16)

| Trigger | On table | Writes | Added by | Removed by |
| --- | --- | --- | --- | --- |
| `conversations` workspace fill (`BEFORE INSERT`) | `conversations` | `conversations.workspace_id` for old writers | K4.4 | K4.17 (every writer sets it by then) |
| immutability triggers, rewritten to allow only "direct project to NULL" | the DM-capable tables (`0037:84-86`, `0047:112`, `:142`, …) | refuse other `project_id` changes | K4.14 | permanent (they guard immutability; the exception is dropped in K4.17) |
| `direct_space_follows_workspace_role` (existing) | `organization_memberships` | `project_memberships` of the hidden Messages project | today | K4.12. Today it and `workspace_space_follows_workspace_role` both write `project_memberships` from `organization_memberships`, two writers of one table, which ruling R16 forbids; K4.12 removes the second writer |
| workspace-space mirror (`workspace_space_follows_workspace_role`, existing) | `organization_memberships` | `project_memberships` of the workspace space (a cache the composite keys need) | today | after the composite membership keys are retired (T1, C1, A1), not in K4 |
| `projects_one_personal_space` and other K4 constraints | — | none (indexes and CHECKs, not triggers) | K4.1, K4.23 | — |

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| K4.1 | Space words in the database (views, columns, kinds) | M | none | K4.5 |
| K4.2 | `spaces` module and access helpers | M | K4.1 | K4.3, K4.5 |
| K4.3 | Guard rails, first set | S | K4.1, K4.2, P0.0, P0.26 | K4.4, K4.5 |
| K4.4 | Conversations carry their workspace; one full visibility rule | M | K4.2, P0.0 | K4.5, K4.6, K4.7 |
| K4.5 | Workspace slugs | S | none (K2.3 soft) | all |
| K4.6 | Explicit workspace on workspace-level requests | M | K4.5 | K4.7, K4.11 |
| K4.7 | Space operations: list, open, create, update, archive | M | K4.2, K2.3, K2.5 | K4.8, K4.11 |
| K4.8 | Space membership operations and events | M | K4.7, K1.10 (soft) | K4.9, K4.10 |
| K4.9 | Personal spaces | M | K4.7, K1.4 | K4.10, K4.11 |
| K4.10 | The workspace space as a plain kind | M | K4.2, M1.2 (soft) | K4.8, K4.9 |
| K4.11 | Private channels: rule and operations | M | K4.4 | K4.6 to K4.10 |
| K4.12 | DM permissions from a shared workspace's role | M | K4.4, K1.2 (or interim), K1.9 (soft) | K4.13, K4.14 |
| K4.13 | Workspace-level carriers off the Messages space | M | K4.4, K3.2 and K5.5 (else outbox stopgap) | K4.12, K4.14 |
| K4.14 | DM storage, expand | S | K4.4 | K4.12, K4.13 |
| K4.15 | DMs written and read without a project | M | K4.12, K4.13, K4.14, K1.28 (soft) | K4.18 |
| K4.16 | Move existing DM rows | S | K4.15, A1.1 (if census) | K4.18, K4.19 |
| K4.17 | Retire the hidden Messages projects | S | K4.16, K1.6, K1.22, A1.1 | K4.20 |
| K4.18 | The address module and resolver | M | K4.5 | K4.15, K4.16 |
| K4.19 | Links switch to workspace addresses | S | K4.18, W1.14a | K4.20, K4.21 |
| K4.20 | Space starting shapes (M1's package format) | M | K4.7, M1.2 (soft), M1.17 (soft) | K4.17, K4.19, K4.21 |
| K4.21 | The rename at the interface | M | K4.7, K2.6 | K4.19, K4.20 |
| K4.22 | The flip (base tables renamed, old names as views) | M | all of the above | none |
| K4.23 | The direct scope | S | K4.14 | K4.15 to K4.21 |
| K4.24 | Personal DMs: one per pair, one list everywhere | M | K4.23, K4.15, K4.12, K2.3 (soft) | K4.16 to K4.21 |
| K4.25 | A chat-only workspace is whole | S | K4.10, K4.24, M1 shape (soft) | K4.17 onward |
| K4.26 | Invitation link tokens | S | K1.29, K2.3 (soft) | K4.18 onward |

## Review changes

- §3.3: added the direct scope (reserved `organizations` row, `kind`, CHECKs) and the `direct_messages` setting with `conversations.kept` (ruling R20).
- §3.5: permission keys are K1's stored keys (`projects.create`, `projects.manage`, `settings.manage`), since K1 keeps stored keys forever; `workspace_update` takes `directMessages`.
- §3.6: event names are K3's catalogue names; DM events carry the direct scope (R8, R20).
- §3.7: the rule is K4's `conversation_member_visible` inside K1's composer, with "if the split has not landed, do it first" (R3); DMs are participants only, personal DMs stay readable, kept DMs end with membership (R20); member key skippable for the direct scope; one DM per pair server-wide.
- §3.7: DM permissions come from `direct.start` in a shared workspace, the same for every kind; one DM list everywhere; `/w/<slug>/dm/<id>`, `/dm/<id>`, `/api/v2/direct/...` (R20).
- §3.7: chat only is whole: channels in the workspace space with no project anywhere (R22).
- §3.9: personal spaces per workspace for agents too (R19).
- §3.10 decisions 2 and 3: `/api/v2/w/{workspace}/ops/...`, K4.6's v1 aliases as a stopgap deleted in K2.22, MCP alias `/w/<slug>/mcp` and the `workspace` argument for any multi-workspace credential (R4, R19).
- §3.10: Rust module renamed `addresses.rs` (K3 owns `links.rs`); HTTP `GET /api/v1/links/resolve` keeps its name; G5 moves to `addresses::guards`; K4.18 renamed.
- K4.3's G2 builds on P0.0's guard and P0.26's rule (R10); K4.4 depends on P0.0.
- K4.6's acceptance test `dms_of_each_workspace` reworked for R20; K4.12 reworked for R20 (shared-workspace rule, `access::holding`, interim function deleted by K1.15).
- New slices K4.23 (the direct scope), K4.24 (personal DMs, one per pair, one list), K4.25 (a chat-only workspace is whole); guard G7 (direct scope holds only DMs).
- Depends named with slice ids (K4.3, K4.4, K4.7, K4.8, K4.9, K4.10, K4.12, K4.13, K4.15, K4.16, K4.17, K4.19, K4.20, K4.21); fake ones dropped (K4.11's C2, K4.5's H1).
- §4 deletion table gains K4.6's aliases; touch points and end-state changes 2 and 5 updated.
- Added the trigger table (R16), recording that `direct_space_follows_workspace_role` is a second writer of `project_memberships` until K4.12.
- Content review requests (accepted): the DM permission is named `direct.start`; the C1/C2 touch point reads "DMs in the direct scope, or in the workspace for `kept`"; the space starting shape is M1's package format (K4.20 depends on M1.17, soft); the composer is described as C1 3.2 shows it.
- R30: new slice K4.26 (invitation link tokens). R31: one personal DM per pair on the server and at most one kept DM per pair per `kept` workspace; reaching someone you share no workspace with does not exist.
