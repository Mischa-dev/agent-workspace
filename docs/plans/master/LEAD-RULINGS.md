# Lead's rulings after the fourteen plans came back

Read with `END-STATE.md`, `PLAN-BRIEF.md` (the "who owns what" table) and `HANDBACKS.md`
(each planner's summary and assumptions). Where a plan disagrees with a ruling, the plan
changes. A ruling can be challenged in a review file with a reason; until the lead changes it,
it stands.

## Rulings

**R1. Column and table names until K4's last slice.** New tables and new columns are *named*
`workspace_id` and `space_id`. Their foreign keys point at `organizations(id)` and
`projects(id)` until K4.22, because a foreign key can't point at a view. That includes the
workspace column H1 adds to existing tables: name it `workspace_id`. Existing
`organization_id` and `project_id` columns are not renamed by anyone except in K4's plan.
Guards and generators (row rules, export, isolation tests) read the tenant column name from a
per-table list, so both names work.

**R2. One access interface, K1's.** K1's plan is canonical for the module name, function
names and types. The decision is three-way: allow, suggest only, deny. K2's runner, C1's
proposals and everyone else use K1's names exactly. If K1's plan has both a decision function
and a separate write-mode function, K1 says in one place how they relate, and other plans
call the one K1 names for their case. No plan defines its own access wrapper except as a
named stopgap with the slice that deletes it.

**R3. One visibility rule for conversations, changed in a fixed order.**
`conversation_visible_to(conversation, viewer)` stays the only gate. Its body is changed by
several workstreams, and two branches that each `CREATE OR REPLACE` it would overwrite each
other. So: the function body is owned by K1 (access). It is composed of named helper
functions, each owned by one workstream, and a migration only ever replaces the helper it
owns: `conversation_member_visible` (today's rule; K4 changes it when DMs leave the hidden
project and when space and workspace membership join the rule), `subject_visible_to` (C1
adds it for comments on a thing; K1 later replaces its body with the generated access
filter). P0.0 lands first and only adds the missing filter to listings. The first slice to
touch the function (whichever of K1, K4 or C1 merges first) splits it into the helpers
without changing behaviour; each plan's slice says "if the split has not landed, do it
first".

**R4. Addresses.** Things are addressed by global ids everywhere. The web app's addresses
are `/w/<slug>/...`. The generated operation API (K2's new surface) carries the workspace in
the path, `/api/v2/w/{workspace}/...`, where `{workspace}` is the workspace id or slug, so a
front router can send a request to the right group of servers without reading the body, and
so the per-request workspace scope for row rules is set from the path. Today's v1 routes keep
their paths until retired. MCP stays one endpoint where the credential decides the workspace;
a per-workspace alias `/w/<slug>/mcp` is also accepted and is refused if it disagrees with
the credential. A person's app that spans workspaces passes an optional `workspace` argument.

**R5. Existing agents at upgrade.** K1's migration preserves today's behaviour for existing
agents with a one-time role ("Member, suggests": edit own pages, suggest on others'), and it
is the only step that selects rows by account kind. A workspace owner switches agents to any
other role in one step afterwards. Newly added agents get the workspace's default role for
new agents, which starts as the same role a newly invited person gets. C1's slice that
deletes the hard-coded page rule depends on that K1 slice.

**R6. Runs.** A1's design stands: `runs` is a new table sharing its id with `agent_runs`,
which remains the built-in host's detail row. New operation names are A1's
(`run_usage_report`, `request_open`, and so on); the brief's working names give way.
A chat reply is not a run unless a launch created one or the agent opened one.

**R7. Hosts are app accounts.** A host signs in as an app account that may act only for the
agents bound to it. K1 models that; A1 uses it; M1 installs a host's connected module from
its `describe` at enrolment.

**R8. The event log has one schema and one sequencer, K3's.** K3 owns the `events` table,
the envelope and the code that gives committed rows their positions. K5 owns waking every
process and fanning out, and the shadow-mode switch of sockets onto the log. The two plans
must describe the same columns and helper tables, and exactly one slice builds the
sequencer (K3's; if K5 reaches it first, that K5 slice builds it to K3's schema and K3's
slice is marked done by it). The envelope must satisfy every consumer: workspace, nullable
space, thing (kind and id), event kind, actor, the app or credential acted through,
requested by, run, request id, the thing's new version, a small `changes` summary, and an
audience (space, conversation with its root and whether it is restricted, account, or
thing). Typing, presence, cursors and streaming text are never written to it.

**R9. One thread-creation function.** C1 owns it (its slice C1.2). K2's module slice for
conversations uses it. Nobody else writes one.

**R10. Bugs are fixed once, in P0.** Where another plan also fixes a bug on P0's list (the
agent task-comment mention, the lost event when notifications fail, reading through a tool
leaving notifications unread, the module gate skipped for pages and boards opened by id, the
scheduler loop stopping on a database error), that plan's slice says "done by P0.n; here
only what goes beyond it". New bugs planners found that are privacy or data-loss bugs move
to P0 as new slices at the end of P0's numbering: page comment notifications keeping 140
characters of a deleted comment (`page_docs/comments.rs:451`).

**R11. Containment is a parent, not a link** (K3). Plans that assumed a `part_of` link kind
(T1's sub-records and initiatives, C1) use the thing's parent instead. Mentions of people are
not links.

**R12. Status and presence.** One `account_statuses` record for every account, people and
agents (C2 owns the table; A2 writes an agent's line through the same operation).

**R13. Limits and budgets.** H1 owns plans, `spend_limits` and the one `admit` check. K5
owns the shared limiter store it counts in. A2's budgets are rows in H1's table, not a second
mechanism. K2's runner calls the check once per write.

**R14. Proposals.** C1 owns the `proposals` record and review. A1's `requests` has a
`proposal` kind for the acceptance card. K2 keeps "can be proposed" as a flag on any change
operation, separate from operations whose kind is "suggest". K1 adds an "edit own" scope
between suggest and edit.

**R15. Browsers keep a WebSocket; agents get server-sent events and polling.** Both resume
from the same log cursor.

**R16. Triggers on hot tables.** Several plans add triggers to the same busy tables (K3's
thing sync, T1's task and record sync, H1's workspace-column fill, A1's run mirror, C1's
freeze). Each plan lists, per table, the triggers it adds and when each is removed. No table
gets two triggers that write the same other table.

**R17. Shared web pieces.** W1 owns the generic pieces (list, table, board, calendar,
gallery, panel, form, the discussion panel shell, the request card, the run view, the block
renderer). T1 owns everything that knows fields and views, plus the timeline and chart
renderers. C1 owns document editors and the content of the discussion panel. C2 owns chat
and inbox screens.

**R18. An owner can be any account.** The invariant is on the workspace, not on a kind of
account: a workspace always has at least one owner whose sign-in can be recovered.

## Proposed end-state changes: accepted

All of these go into `END-STATE.md` draft 3 (the lead applies them): status for every
account; written unread rules; offline reading as an opt-in per person that a workspace can
switch off, cleared at sign-out; the browser WebSocket; a list may refetch itself when
membership can't be told from an event, never a whole space; account settings and public
forms outside `/w/`; a measurable speed bar on phones; a connection belongs to any account;
loop limits as chain depth plus a turn allowance, with "continue" a permission; pause holds
deliveries and refuses writes; chart as a view kind; triage, cycles, delegates and
initiatives in Tasks; version checks on changes, not creates; "part of" as a parent; the
audit trail as the log plus a sign-in record; restore for documents and records, with change
sets for sheets; log retention as a plan value with history never cut; a space may switch a
module off but not on; packages carry agent templates, never accounts; panels as fixed
blocks plus framed pages from approved origins; comments on a thing are one conversation
with anchored threads; "edit own"; every table carries its workspace except a published
list of global identity tables; staff never impersonate; first run protected by a one-time
code; delegation also covers apps acting for an agent; "act only within what the requester
could do" as a role setting; a thing may have no space; personal spaces made on first use;
old protocol revisions and wake formats are pinned by contract tests and retired only by
their own slice; hosts as app accounts; "interrupted" as a final outcome and "not
responding" as a mark; the built-in host keeps its own store; leases for single-holder jobs;
signed download links.

## Rulings after the owner read the end state (2026-11-14 02:08Z)

The owner agreed with most of draft 2 and changed six things. These rulings override draft 2
and any plan text that follows it. They go into `END-STATE.md` draft 3.

**R19. Every account is global to the server. A workspace admits accounts by membership.**
The owner: "I do disagree that an agent necessarily belongs to one workspace... they have
their one personal agent that they're going to want to add to their multiple projects."
Inside one workspace an agent already joins any number of projects (spaces). The change is
across workspaces: an agent or app account is no more tied to one workspace than a person is.

- `accounts` (today `actors`, which already has no workspace column) stays global for every
  kind. K1's `home_workspace_id` and its CHECK (`actors_home_workspace`) are replaced by an
  **owner**: `owner_account_id` (a person: "my agent") or `owner_workspace_id` (a team's
  agent). Exactly one is set for agent and app accounts, neither for people and the system.
  This is structural (who is accountable and who may delete it), not a permission.
- **Managing** an account that has an owner: the owning person, or whoever holds the
  manage-agents permission in the owning workspace, plus accounts the owner names. One rule,
  one function, in K1's access module.
- **Joining a workspace** is the same act for every kind of account: a membership row with a
  role. Adding an owned account needs two things: the adder may add members of that kind's
  starting role in the target workspace (a permission), and someone who manages the account
  agrees (the adder is a manager, or a manager accepts the invitation, exactly as a person
  accepts their own). A workspace setting says whether accounts owned outside it may join:
  yes, ask an admin, or no (default: ask an admin when hosted, yes when self-hosted).
- **Per workspace**: membership, role, space memberships, budgets, subscription scopes,
  runs, the work line of a status. **Global**: the account, its profile, its outside
  reference, its owner, its credentials, its DMs (R20), a status a person sets by hand.
  `account_statuses` (R12) is keyed by account and nullable workspace; a line an agent or
  host writes about work in a workspace is stored for that workspace and shown only there.
- **Connections** belong to the account. By default a connection reaches every workspace the
  account is in; it can be narrowed to one workspace or to spaces (K1's narrow tokens). The
  event stream of a connection merges the workspaces it reaches (K3's position is global, so
  one cursor still works), every event names its workspace, and every operation call names
  the workspace it acts in (R4's optional `workspace` argument becomes how any multi-workspace
  credential says where; a credential that reaches one workspace needs no argument).
- **Hosts** are app accounts (R7), so the same rule holds: a person links their own host
  once and may offer its agents in any workspace where they may add agents; a workspace
  links a host for everyone there. Enrolment names the owner (a person or a workspace).
- **What others see**: an account's profile and its membership here. Never the list of its
  other workspaces. People search returns only accounts that share a workspace with the
  caller. Isolation tests cover both.
- **Existing data**: nothing merges. Today's agents get `owner_workspace_id` from their one
  organization membership. Duplicate accounts for the same real agent stay separate.
- **Export and import**: a workspace export carries memberships and a profile stub for each
  member account of any kind; import matches by outside reference (agents, apps) or verified
  email (people), else creates.
- Safety: an account in two workspaces can carry what it learned from one to the other, as a
  person can. The scope screen shows "also a member of other workspaces" (a yes or no), and
  "accounts owned outside may join" is the control.

**R20. DMs and group conversations belong to the people in them, not to a workspace.**
The owner: "I kind of want a more Discord type feeling thing where with DMs, you can just DM
someone." With R19 this also gives a personal agent one DM with its person instead of one
per workspace.

- **The direct scope.** One reserved row per server in `organizations`, `kind = 'direct'`
  (new column, values `team` and `direct`), with a fixed id. It has no members, no roles, no
  spaces, no modules, is never listed, never appears in the switcher and has no address. New
  DMs and groups, and their messages, files, reactions and events, carry its id as their
  workspace. So every table keeps a non-null workspace column, and the log, search, storage,
  row rules and metering need no second code path. The direct scope may hold conversations of
  kind DM or group, what hangs off them, and the requests and runs raised from them. Nothing
  else: a task, page or record made from a DM message goes to a space the person picks.
- **Access** is participation and nothing else (R3's `conversation_member_visible`). No role,
  no admin, no workspace owner and no staff console reads a DM. Guards: no membership row may
  reference the direct scope; no listing of the direct scope exists that is not filtered by
  participant; the P0.0 class of leak has an isolation case here.
- **Who may DM whom.** An account may start a DM with any account it shares a workspace
  with, if its role in that shared workspace allows starting DMs (K4.12's permission becomes
  this), and subject to the other account's own setting (anyone I share a workspace with, or
  ask me first). The same rule for every kind of account. One DM per pair, server-wide:
  starting again returns it. A DM stays readable for good; sending needs the two to still
  share a workspace. Block and "ask me first" are C2's.
- **One list everywhere.** A person sees the same DMs in every workspace's sidebar. Web
  address `/w/<slug>/dm/<id>` (the DM shown inside the current workspace's shell); `/dm/<id>`
  resolves to it. Operations on direct-scope things use `/api/v2/direct/...`.
- **Kept DMs, for a team that needs them.** A workspace setting `direct_messages`:
  `personal` (default) or `kept`. In a `kept` workspace, DMs between its members that start
  inside it are created with that workspace's id, follow its retention and export, and end
  with membership, as in Slack. Same table, same visibility rule, different workspace id.
- **Existing DMs** keep the workspace id they have (no data moves) and are treated as
  personal: listed everywhere, one per pair. Where the same pair already has a DM in two
  workspaces, both stay and are labelled; nothing merges. K4's slices that take DMs out of
  the hidden project by `project_id = NULL` stand.
- **Export.** A workspace export includes only conversations that carry its id. A person can
  export their own DMs. The operator's full-server export includes the direct scope.
  (Corrected by R46: a workspace's export includes a DM only when it is a kept DM.)
- **Live and delivery.** A browser subscribes to the current workspace, the direct scope, and
  unread counts for its other workspaces (the switcher shows them). An agent's subscription
  says whether it includes DMs.

**R21. A host makes agents. Workspace never does, and never knows a host's steps.**
The owner: describing an agent in a sentence "is an Agent OS thing... make sure your plans
aren't writing it so it somehow includes that as something that Agent Workspace needs to
handle", and the create flow "shouldn't be truly hard-coded... not hard-coded for Agent OS,
but it works perfectly with Agent OS."

- Workspace's whole part in "create one": (1) list what linked hosts **offer**, (2) draw the
  host's own steps, (3) receive the result, (4) give the account a role here.
- **Offers.** `describe` returns a list of offers, each `{id, title, summary, start}` where
  `start` is a panel (M1's block format) or a page of the host's from an approved origin.
  Workspace does not know what an offer is ("new agent from a template", "copy of Nyx",
  "describe it in a sentence"): the host names them. A builder, templates, backends,
  authority on the machine, proactivity and model keys are all the host's panels.
- **Result.** One generic operation, `agent_register` (A2 names it): the host proposes an
  account with a profile, an outside reference and how it is reached. A person with the
  permission approves it and picks its role. Workspace's own step (the role, the spaces) is
  the only step Workspace owns. The same operation is how "pick one a host already has"
  works.
- **Actions.** Instead of a fixed lifecycle list (create, update, deploy, copy, move,
  retire), a host publishes **actions** on its agents: `{id, label, needs: confirm | panel,
  danger?}`. Workspace draws them on the agent's page and sends the chosen action back. The
  fixed part of the host protocol is only what Workspace's own records need: enrol,
  describe, start, cancel, steer, answer, report (runs, activities, usage, status), and
  three notices from the host: an agent's profile changed, its reference changed (moved or
  re-keyed, with lineage), it was retired.
- **Removed from Workspace's plans:** the agent builder and its `agent_blueprint` drafts
  (A2.21), the five-part review, and the eight `host_agent_*` operations as fixed names
  (A2.19 becomes offers, `agent_register`, actions and the three notices).
- **The Agent OS connector.** Workspace's job is the protocol, a host client library, the
  built-in host that uses it, a small example host, and a conformance suite any host can run.
  Taking the owner-specific bridge out of this repository (principle 6) stays in the plan as
  the generic *bridged* connector, because that code is here today. *Native* mode, identity
  across moves and creation from Workspace depend on Agent OS work that is not started: they
  move to an appendix "Integration, with Agent OS" that is outside the build order and is
  done in the later three-repository project. Nothing in Workspace's "done" waits on Agent OS.
- Test for done, item 2, becomes: "Add an agent: pick one from the directory, paste a link
  into one you already run, or start one on a host you've linked, where the host's own steps
  appear inside the app."

**R22. Chat only is a first-class shape.** The owner: "it should be equally possible for
someone to just set this up to be a fun place to message their friends... not even have the
tasks."

- A workspace starts from a **shape** (a package, M1): at least "Friends" (chat only),
  "Team" (everything), "Just me and my agents", "Documents", and "Blank". First run and
  sign-up ask what it's for. A shape can be changed later; nothing is deleted.
- With only Chat on: no project list, no task words, no agent pages (Agents is a module and
  can be off), channels live in the workspace's own space (K4), the home screen is the
  conversation list, invitations are a link. The words "project", "task", "agent" and "run"
  do not appear anywhere in the interface or in emails for that workspace.
- Guard: a browser test per shape that walks the shell, settings, search, the command list
  and the empty states and fails on a part or a word from a module that is off. W1 owns the
  test; M1 the shapes; C2 chat standing alone (the home screen, invite links, the phone);
  H1 the question at sign-up.
- Test for done gains: "A group of friends uses it only to chat and never sees a task, a
  project or an agent."

**R23. Files can live somewhere else.** The owner: "what if you could have your files inside
a Google Drive and just attach your Google Drive to your project."

- Two separate things. **Storage** (K5): where Workspace keeps the bytes of its own uploads:
  disk or any S3-compatible store, chosen by the operator, and on the hosted service a
  workspace may bring its own bucket. **File sources** (C1 for the file thing, M1 for the
  contract): an outside store attached to a space or a workspace, whose files appear as
  things **by reference**: browse, attach to anything, link, preview, search by name (and by
  content where the source can), open in the source. Bytes are fetched when asked, never
  copied unless a person says "keep a copy".
- A file source is a connected module with a fixed contract: `list(folder, cursor)`,
  `get(id)`, `content(id, range)`, `thumbnail(id)`, `changes(cursor)`, and optionally
  `put(folder, name, bytes)`. A source that can `put` may be chosen as where a space's new
  uploads go. Google Drive, Dropbox, OneDrive, a Git repository and a plain bucket are each
  one module; none is named in the core.
- Access: the source is connected by a person (their grant) or an app (the workspace's
  grant). Workspace shows a source's files to the members of the space it's attached to, by
  the space's roles, and says plainly that opening one in the source follows the source's
  own sharing. Tokens stay in the server's secret store, never in the browser or the log.
- `files` gains `source_id` and `source_ref` (both null for our own uploads). The file thing,
  links, comments and search treat both the same.

**R24. The hosted service has to be cheap to run, so the server is built for it now.**
The owner: "if we're doing this hosted, we're probably going to have to change some things if
we want this to be cheap... ready to be implemented inside Cloudflare or something. Not fully
implemented." No rewrite and no second database engine: the Rust server and PostgreSQL stay,
because self-hosted and hosted are the same code. What makes it cheap is that an idle
workspace costs only its stored bytes and that the expensive parts sit behind seams a cheap
platform can fill. Every plan keeps these rules:

1. **The web app is static files.** The server serves the API; it can also serve the web
   app for a self-hoster, but nothing requires it to. No server-rendered page (W1, H1).
2. **File bytes do not pass through the server when storage can sign links.** Upload and
   download go straight to object storage with short-lived signed links (K5). Disk storage
   keeps today's path.
3. **Database use is pooler-safe.** Settings only by `SET LOCAL` inside a transaction; no
   session advisory locks (K5's leases instead); no `PREPARE` by hand; transactions short,
   never held across a network call. `LISTEN`/`NOTIFY` is an optimisation on one direct
   connection per process and never the only way to learn of an event: a process without it
   polls the log position on a short interval. A lint and a test enforce the list (K5).
4. **Three process roles in one program** (K5): `api` (stateless; may be stopped when idle
   and must answer within a second of starting), `live` (holds browser sockets and agent
   streams, nothing else), `work` (jobs, deliveries, indexing, the scheduler). A self-hoster
   runs all three in one process. `work` can be driven by an outside timer calling a tick
   endpoint, so nothing has to stay awake for the scheduler.
5. **The live role is a published contract** (K5): `api` issues a short-lived signed ticket
   naming the audiences a connection may hear; the live role receives event envelopes for
   audiences and pushes them; a client that reconnects resumes from its cursor through
   `api`. The live role needs no database. The in-process implementation is the default, and
   a conformance suite lets an edge service that sleeps idle sockets replace it without the
   server changing. Live document rooms are audiences under the same contract.
6. **Nothing runs per workspace.** No timer, loop, connection, cache warm-up or in-memory
   state exists because a workspace exists. Schedulers query due rows. A guard test starts
   the server with 10,000 empty workspaces and asserts idle CPU and memory do not grow (H1).
7. **Cold data is cheap data.** The event log past its retention, old document update logs
   and old history payloads compact into snapshots or move to object storage. History is
   never cut; it may take longer to open (K3, C1, H1).
8. **Every cost has a counter** before the hosted service opens: stored bytes, rows, emails,
   pushes, deliveries, model calls we pay for, per workspace (H1). Email and push are the
   metered outbound costs: digests and plan limits.
9. **Search stays in PostgreSQL.** Meaning-based search uses a model the workspace brings.
10. **Agents never run on our machines** and model calls are brought by the workspace, as
    before. These two are most of what makes a free plan affordable.

H1 writes `docs/hosting/shapes.md` with three reference shapes and what each costs idle:
"one box" (self-host), "small hosted" (two small machines, managed PostgreSQL, object
storage, a CDN), and "edge" (static app on a CDN, files in zero-egress object storage, the
live role on edge objects that hibernate idle sockets, `api` in containers that stop when
idle, PostgreSQL through a transaction pooler). The edge shape is designed and its contracts
are tested, and it is built only when the numbers say so. Checked against one provider's
current documentation: containers there bill only while running and sleep after a timeout,
its pooler is transaction-mode with no `LISTEN`/`NOTIFY` and no session locks, and its edge
objects keep idle sockets open without billing duration. Rules 3 to 5 are exactly what that
needs. No provider is named in the code.

**R25. Licence.** Deferred. Assumed: one fully open project with no closed edition. No plan
may depend on a paid-only feature. H1's licence slice stays last and says "chosen at release".

## Rulings on the content and web review (`review/content-web.md`)

**R26. An accepted proposal is applied by C1.** `proposal_decide` applies the prepared change
through `Proposable::apply` inside a K2 dispatch (`Edge::Proposal{proposal_id, accepted_by}`),
so part of a proposal can be accepted and a stale part refused. K2 drops `proposal_accept`
(re-running stored input) and its `ProposalDraft` becomes the input to `Proposable::prepare`.
A1's `request_answer` on a `proposal` request calls `proposal_decide` and nothing else.

**R27. Channel tabs** are a C1 table plus an `embeds` link. K3's link kinds stay closed.

**R28. The run view.** A1.5 builds the run page first; W1.24 extracts the generic piece from
it without a rewrite.

**R29. Shapes.** M1 3.8b's table stands with one change: "Just me and my agents" switches on
the same modules as "Team"; what differs is the starting layout (personal space first, no
invitation prompts). "Friends" is Chat, Files and Search, with the inbox as part of Chat.

**R30. Homes.** `client-config` (what the static web app fetches at start) is H1's.
Invitation link tokens are K4's, with the other invitations.

**R31. Three corrections to R19 to R25.**
- *An agent in a chat-only workspace (R22 with R19).* R22's word rule is about a module's
  parts: pages, navigation, settings, commands, empty states, emails. What kind of account
  someone is talking to is account information, not the Agents module, and it always shows:
  the badge beside a name and the line on a profile say "agent" in every shape. Any account
  may join any workspace its role allows, whichever modules are on (refusing agents while
  Agents is off would be a rule by account kind). Adding an agent to a workspace with Agents
  off offers, and does not require, switching Agents on. W1.39's walk runs twice: with people
  only (no such word anywhere) and with an agent member (the badge and the profile only).
- *The frame host page (R24.1).* `/panel-frame/{launch}` is served by the `api` role, because
  it needs a response header per launch. It is not part of the app shell, so rule 1 holds.
- *One DM per pair (R20).* Precisely: one personal DM per pair on the server, and at most one
  kept DM per pair in each `kept` workspace. Inside a `kept` workspace, opening a DM with a
  member returns the workspace's DM, and the personal one stays in the list, labelled.
  Reaching someone you share no workspace with (by email or a link) does not exist and needs
  a ruling before anyone adds it.

The review's "Requests to other plans" are accepted as written and go to the owners of K1,
K2, K3, K4, K5, A1, A2, P0 and H1.

## Rulings on the kernel review (`review/kernel.md`)

**R32. `conversation_readable_by`** (P0.28, C2.1) stays as a thin wrapper whose body is one
call to `conversation_visible_to`, with a guard that it is nothing more.

**R33. One `/api/v2` route** serves browser sessions (session plus CSRF) and bearer tokens.
A request carrying both is refused. One API document, one rate-limit path.

**R34. Who may manage an owned account (R19, made precise).** The owning person; anyone with
`agents.manage_any` in the owning workspace; or a manager named on the account who holds
`agents.manage`. Holding `agents.manage` alone is not enough, which keeps the escalation fix
K1.4 tests.

**R35. Feeding a separate live service (R24.5).** In the edge shape only, one `work`
instance holding a short `live.publisher` lease tails the log and posts to the live service.
The default in-process shape uses neither.

**R36. Key rotation's session lock** (`secrets.rs:1105-1110`) is allowed only on the direct
database connection and rotation is refused when there is none. It moves to a lease if the
edge shape is built.

**R37. `paused` and `pending_review`** are values inside an `ok` result from A2's
operations, not outcomes in K2's envelope.

The review's "Requests to other plans" are accepted as written and go to the owners of C1,
A1, A2, H1, P0, M1, C2, T1 and W1.

## Rulings on the agents and hosting review (`review/agents-hosting.md`)

**R38. Approving `agent_register`.** The host's registration opens an A1 approval request
addressed to the person who chose the offer. Anyone who may add accounts with that role may
approve it. A host never adds an account to a workspace without a person seeing it.
(Reworded by R45: approval is by permission, not by kind of account.)

**R39. When a workspace that owns accounts is deleted.** Those accounts are retired and
their memberships elsewhere end, after notice and a 14-day window in which a person who
manages one can take ownership of it (it becomes their own).

**R40. Storage for DMs.** Files sent in the direct scope count against a personal allowance
per account, one server-wide plan value the operator sets (unlimited by default when
self-hosted, the free plan's file allowance when hosted). Never against a workspace.

**R41. H1.9's export** (wanted by the owner now) may use an in-code `ExportDecl` list in
H1.8 until M1.3 moves it into module registrations.

**R42. Scoping every database call (R24.3, H1.21, H1.22).** About 2,400 pool call sites move
into `db::scoped_tx` or `db::scoped_one`. That is done as one slice per server module
directory, mechanical and mergeable in any order, behind a ratchet test that counts unscoped
call sites and may only go down. Strict row rules switch on only when the count is zero.
K2's runner sets the tenancy scope the way H1 3.4 defines it; there is no `app.workspace_id`.

The review's "Requests to other plans" are accepted as written and go to the owners of K1,
K2, K3, K5, C1, C2, T1, M1, W1 and P0.

## Rulings on the build order (`BUILD-ORDER.md`)

**R43. The build order's section 9 stands.** Where a plan's slice table disagrees with
`BUILD-ORDER.md` section 9 (the six cycle breaks, the overlaps between C2.39 and K4.24 and
between C2.6 and K4.11, the vague dependencies section 8 resolves), the build order wins and
the session that takes the slice corrects the plan's table in its pull request. K1.14 is
split: **K1.14a** removes the role caps and `people_only` so any account may hold any role
(needs only K1.12 and K1.13; wave 5), and **K1.14b** carries the new capabilities and
sensitive requests (after A1.8 and A2.8; wave 8). That brings "the owner's own agents at
full permissions" from wave 8 to wave 5. K4.20 is called "space starting content", not
"shapes", which means a workspace's shape (R22, R29). Each of H1.45 to H1.62 names its
server module directory in its task.

## Rulings after the owner's second read and two outside reviews (2026-11-14 04:37Z)

The owner asked for the build to be cut into far larger pieces of work, and passed on two
reviews with the instruction to judge them, not obey them. R44 is the owner's change. R45 to
R50 are the review points that held up; what did not hold up is at the end.

**R44. The work is built as large builds, not one pull request per slice.** The owner: "you
need to massivly shrink down the plan, NOT the features... give them MUCH larger chucks of
work." No slice is removed. The 471 slices are grouped into 31 builds in six stages
(`BUILD-ORDER.md`). A build is one long session, one branch and one pull request; a slice is
a line in its work list and at least one commit. CI and review run on the whole build, with
three CI runs or fewer as the aim. A build starts when the builds it needs have merged;
stages exist for the gates. There are no session-day or calendar estimates anywhere in the
plan: what a build costs is measured on the first builds. `slices.json` is the one list of
slices, builds, needs and milestones, checked and turned into `SLICES.md` and the tables of
`BUILD-ORDER.md` by `tools/plan.py`. This replaces the waves of R43; R43's cycle breaks and
the K1.14 split stand. Where a plan says "one slice, one session, one pull request" or names
a wave, read it as "one commit or more inside its build" and the build in `slices.json`.

**R45. Approval is by permission, never by kind of account.** R38 said a host never adds an
account "without a person seeing it", and R21 said "a person with the permission approves".
Both contradict the owner's rule that agents are accounts and nothing is hard-coded by kind.
Corrected:

- `agent_register` opens an approval request addressed to the account that chose the offer.
  Any account that may add members with that role in that workspace may approve it and pick
  the role, whether a person or an agent.
- Two rules that hold for every kind of account: no account approves a request about itself,
  and a host never approves its own registration.
- A workspace may add review by a person as a setting, per request type
  (`requests.person_review`), off by default. It is a workspace's choice, shown on the
  request, not a rule in the code.
- The same wording applies wherever a plan says "a person approves", "an admin confirms" or
  "people whose role lets them": read "an account whose role lets it".
- An owned account's owner is any account or a workspace (R19 said a person or a workspace).
  The chain of owners must end at a person or a workspace and may not loop; the server
  setting `accounts.owner_depth` caps it (default 3). Whoever manages an owner manages what
  it owns (R34). If an owning account is deleted, what it owned passes to that account's
  owner. K1.5 carries the columns; nothing else changes.

**R46. What Workspace promises about privacy, said exactly.**

- **Access rules control what Workspace shows and delivers. They cannot make an outside agent
  forget.** Removing an agent from a conversation, space or workspace stops every later read
  and delivery at once (the journey's step 7 tests it). It does not erase what the agent's
  host already holds. The end state and the scope screen say so in those words.
- **One identity is not one memory.** An account in two workspaces is one identity; whether
  what it learned in one is available to it in the other is a property of its host. A host
  states it in `describe` as `memory`: `shared`, `per_workspace`, `per_space` or `none`
  (unknown if not stated). The scope screen shows it as the host's statement, not as
  something Workspace enforces. The workspace setting "accounts owned outside may join"
  (R19) may require a stated level, for example `per_workspace`. A2.15 and A2.19 carry it.
- **DM privacy is a guarantee about the application.** No role, admin, owner or staff
  console can read a DM they are not in, through the web app, the API, MCP, search, export
  or support tools, and isolation tests hold that. It is not a guarantee against whoever
  operates the server: someone with access to the database, its backups or a full-server
  export can read anything stored there. Self-hosted, that is the person running the
  server. Hosted, staff database access is restricted, needs a recorded reason and is
  logged (H1.35, H1.36), and the published privacy terms say this plainly. Messages are not
  end-to-end encrypted and the plan does not promise that they are.
- **Export, retention and "ends with membership" follow who a conversation belongs to, not
  the workspace id on its row.** R20 keeps existing DMs on their old workspace id but treats
  them as personal, and also said a workspace's export includes conversations carrying its
  id: together those would export personal DMs to an admin. Corrected: a conversation of
  kind DM or group is in a workspace's export, under its retention, and ended by loss of
  membership only when it is a kept DM (`kept = true`, made in a workspace whose
  `direct_messages` is `kept`). Personal DMs, old or new, whatever id they carry, are
  exported only by their participants and by the operator's full-server export. H1.9 and
  H1.10 read `kept`, and an isolation case holds an old DM out of its old workspace's export.

**R47. The foundations are proved by one journey, and contract tests arrive with the code
they test.** Stage 2 is done when `tests/integration/journey.rs` passes: one outside agent
with one identity in two workspaces receives events, edits through an operation with the
same result and audit line from the browser and from MCP, raises a request that an
authorised account answers, retries without duplicating (including a run start whose answer
was lost), loses access the moment its membership is revoked (with a stream open, a delivery
queued and an operation waiting), and has a request refused when it is answered after it
changed or after the requester lost access (`BUILD-ORDER.md` section 5). Build 1G creates
the file; build 2G owns making it pass. To make that possible: A1.4 (connected runners get
runs) moves to stage 1; A1.5, A1.6, A1.23 (the example host and conformance suite), A2.7
(the contract suite) and A2.8 move into build 2G beside the protocol they exercise. A2.19
adds its offer and action cases to A1.23's suite when it lands. If the journey shows an
abstraction is wrong, that is fixed before any stage 3 build merges.

**R48. A seam ships with a real user.** A new module, trait, registry or table is done only
when something that already exists in the product runs on it in the same build: the ops
kernel with the template slice and messages, the access module with real checks in compare
mode, the event log with webhooks and automations reading it, pieces with collections moved
onto them. A seam with no real user at its stage's gate is removed or deferred by the lead,
in writing. What is deliberately not generalised stays as the plans have it: the task core
keeps its key (T1 Q6), and the page editor, the sheet grid and the board canvas
stay specialised editors inside the shared shell.

**R49. Hosting numbers are targets until measured.** The free plan being the whole product,
history that is never cut, and a quiet workspace costing almost nothing are design targets
the rules of R24 aim at. They become public promises only when the meters (H1.25, H1.26)
and the idle-cost guard (H1.42) have run against real hosted use and the numbers hold; H1.40
publishes what was measured. Until then the end state calls them targets.

**R50. Later stages are a forecast, and changes are written down.** Stages 0 to 2 are fixed.
Stages 3 to 5 are cut again by the lead at the stage 2 gate, from what the first builds
showed. Every gate needs a working demonstration (a passing test or a flow walked in a
browser). Any later change to a build is an edit to `slices.json` and a dated line with the
reason in `PLAN-CHANGES.md`. The owner's go covers the whole order; the lead reports at
each gate and carries on unless a gate fails.

**Milestones corrected.** "Add an agent three ways" is a stage 3 milestone and every slice
it names is in stage 3 (A2.18, A2.19, A2.20, M1.14); a host's framed pages and the
end-to-end panel check (M1.15, M1.16) are a separate stage 4 milestone. The stage a
milestone lands in is computed from its slices, so the two can't disagree again.

**Review points not adopted.** "Keep the task core" as an argument against moving tasks into
record storage: the owner already chose the full move (option B, AWD-110), and R48 keeps
what should stay specialised. Holding the later stages back for a separate authorisation:
the owner asked for less waiting, not more, so the gates report and the work carries on
(R50).

## Ruling after the owner said the app need not be deployed until the plan is done (2026-11-14 07:27Z)

**R51. One deploy at the end, rehearsed at every gate.** The owner: "i am probobl fine with
not deploying until its done." The live server stays on the commit it runs today, and the
whole plan reaches it in one deploy after build 5X.

- **What it removes.** Steps that exist only to keep a running server working between
  releases: waiting "one release" before a contract slice, a quiet window for K4.22's flip,
  and shims between the web app and the server (they ship together). A contract slice may
  run as soon as its replacement has merged; build 5X needs no release before it. A build
  may also do its contract step itself when nothing outside the repository uses the old
  path, and say so in its pull request.
- **What it keeps.** Anything that talks to the live server from outside the repository
  keeps working through the deploy: old MCP tool names, the old wake format, MCP revision
  2025-06-18, the public v1 API. Their aliases and sunset dates count from the deploy.
  Every migration must carry the live database's existing data across, so expand and
  backfill steps stay; only the wait between them goes.
- **What it adds: a rehearsal.** One deploy of this size is a risk of its own. At each stage
  gate from stage 1 on, the lead restores the latest live backup into a scratch database
  on the server (never the live one), runs every migration and the new server against it,
  and checks row counts, the isolation tests' read checks and a walk of the main pages.
  Anything that fails is fixed before the next stage merges. The first rehearsal asks the
  owner once, in the thread, because it runs on the owner's server; the deploy itself
  still waits for the owner's "go" in words.

