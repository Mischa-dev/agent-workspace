# Plan: make Agent Workspace the one app

September 24, 2026. Agent Workspace (this Rust repo) becomes the only app. The
TypeScript "Workspace" app at workspace.mischanelson.dev stays live until this
one can do everything Mischa uses it for. After that it is retired.

This plan replaces the milestone ceremony in `PROJECT_SOURCE_OF_TRUTH.md` for
new work. That document stays as history and as a reference for how existing
code behaves.

## Status (September 24, 2026)

- **Phase 0:** mostly done. Minecraft servers stopped (18 GB of RAM free), so
  there's no swapfile. The dev loop is rsync to `~bot/dev/agent-workspace`,
  with a separate test Postgres on port 55433 (`aw-dev-postgres`). `AGENTS.md`
  is trimmed. The PR #88 merge is waiting on the owner: GitHub Actions can't
  run, so branch protection blocks it.
- **Phase 1:** built. Migration 0037, `direct.rs`, the `/messages` pages, and
  the visibility rule in every conversation access path.
- **Phase 2:** built. Migration 0038 adds reply requests and webhooks
  (`replies.rs`, `mcp/replies.rs`, reply status in the conversation page).
  Migration 0039 adds OAuth sign-in as an agent.
- **Phase 3:** in progress. Release `phase3-20260924` is live on Red Hat at
  `https://redhat.tail6b11cc.ts.net:8447` (Tailscale only), with a database
  backup in `/var/backups/agent-workspace/`. Mara, Rook, Iona, Sable and Moss
  exist in the Messages space. They were created by `direct-agent-create`,
  and their tokens live only on Nyx. Nyx runs `aw-crew@<agent>.timer` (every
  5 minutes) plus `aw-crew-wakeup.service` on 100.119.56.3:8732, and signed
  wakeups were checked end to end. The old `workspace-crew@` bridge still
  serves the old app. Still to do: the public hostname (needs the owner's OK
  for a DNS record), Muse over OAuth, committing and pushing, and CI.

## What we're building toward

Mischa can open the app from anywhere, DM any agent, talk with several agents
in a group, and have them answer on their own. That works whether an
agent checks in on a schedule or can be woken instantly. The projects, tasks,
Codex runs and GitHub pull requests Agent Workspace already has keep working
alongside that.

The agents to move over: **Muse** (Meta's agent; MCP + heartbeat, no incoming
webhooks) and the **Nyx crew**: **Mara, Rook, Iona, Sable, Moss** (AgentOS
agents on the Nyx host, driven by a bridge script).

## How we work (less ceremony)

- **While building:** `cargo check`, plus only the tests for the code being
  changed.
- **Before each deploy:** clippy, the full test suite once, and the web build,
  all on Red Hat. Then one real end-to-end try on the live server. For
  example: "DM Mara and she answers."
- **Write tests only where bugs would be silent and costly.** That means two
  agents racing to claim the same request, private conversations leaking to
  non-members, and the rules for who answers. There are no test matrices for
  UI details.
- **No** acceptance-evidence files, milestone authorizations, or decision-log
  entries for ordinary changes. Keep this plan and the short `AGENTS.md` up to
  date. That's the paperwork.

## What the code already has (verified by reading it)

| Area | What exists | Where |
|---|---|---|
| Identity | One actor model: human, external agent, execution worker, integration, system. Organizations and projects both have memberships. | `migrations/0002`, `0004`; `policy.rs` |
| Permissions | One central `authorize(subject, action, resource)` | `policy.rs` |
| Conversations | Project channels, one level of threads, structured mentions (by actor ID, not text scanning), ordered timelines, read state, archive | `conversations.rs` |
| Live updates | One WebSocket per browser. A transactional outbox feeds the publisher, and temporary `message.delta` events stream Codex output. | `realtime.rs`, `outbox.rs` |
| Agent access | An MCP server with OAuth 2.1 (DCR, PKCE, refresh) and bearer credentials. Its 23 tools include listing and reading conversations, **sending messages as the agent**, polling an event inbox, work items, approvals and worker requests. | `mcp.rs`, `mcp/*` |
| Agent inbox | Per-connection event log with cursors. It fills when an agent is @mentioned, assigned a work item, or has an approval waiting. | `external_events.rs` |
| Coding | Codex App Server worker, worktrees, scheduler, recovery, GitHub App PRs/checks/reviews | `provider*`, `git_runtime.rs`, `github.rs` |
| Deploy | `bot` builds in `/home/bot/deployments/agent-workspace`, releases go to `/opt/agent-workspace/releases/<sha>`, and systemd runs `agent-workspace.service`. Postgres runs in bot's Podman on `127.0.0.1:55432`. Reachable only over Tailscale. | `deploy/redhat/` |
| Frontend redesign | PR #88 merges cleanly into `main` | `codex/frontend-overhaul` |

## What's missing (the actual work)

1. **No DMs or group chats.** Every conversation is a project channel or
   thread, and anyone in the project can see all of them. There is no
   per-conversation member list.
2. **Agent names are per project.** Handle and display name live on
   `project_memberships`, and actors have no profile of their own.
3. **Agent connections are Hermes-only and tied to one project.**
   `external_agent_connections` requires `runtime_kind = 'hermes'`,
   `runtime_version = '0.20.0'` and a single project.
4. **OAuth can only connect *you*.** The consent screen gives the connecting app
   your own identity, so Muse can't connect as "Muse".
5. **Agents only hear about @mentions.** There's no "please answer",
   claim/lease, pass, or limit on how long agents can reply to each other.
6. **No push wakeups.** Agents can only poll.
7. **Not public.** Tailscale-only, and no automatic backups (manual scripts
   only).
8. **No notes/files.** Attachments were deliberately left off in version one
   (decision D-053).

## Key design choices

**DMs and groups live in a hidden "Direct" space per organization.** In the
database that space is a project row with `kind = 'direct'`, and the UI never
shows it as a project. `project_id` runs through about 160 queries in
`conversations.rs` plus the outbox, realtime and policy code. Reusing it means
none of that has to be rewritten. It also fits the org-as-tenant model a SaaS
version would use (Slack's DMs work the same way). Every project list, Global
Home count and search must leave the Direct space out.

**Per-conversation membership, used everywhere.** Add
`conversations.visibility` (`project` | `members`) and a `conversation_members`
table. Access becomes: member of the project, *and* either the conversation is
project-wide or you're on its member list. Threads inherit their parent's
visibility. This goes into the few central access functions
(`authorized_conversation`, `fetch/lock_conversation_access`,
`lock_message_conversation`, realtime `event_is_authorized`, the sidebar,
timeline and MCP list queries), not into each endpoint. Private project
channels come for free.

**One profile per agent at the organization level.** Add display name, handle
and description at the org level (on `organization_memberships` or a small
`actor_profiles` table), with handles unique per org. `project_memberships`
keeps its copies so the existing joins still work. Renaming an agent updates
every copy in one transaction.

**One connection per agent, reached in stages.** Stage 1: the connection
covers the Direct space, which is where the crew and Muse talk. Stage 2 (after
cutover): credentials are scoped to the organization, and tools find the
project from the conversation or work item they're given. This defers an MCP
refactor until agents actually work inside projects.

**The rules for who answers are ported from Workspace, not reinvented.** They
are already designed and were live-tested there (`group-participation.ts`,
`participants.ts`):

- A human DM to an agent asks that agent for a **required** reply.
- A human message in a group asks **every** agent member for an **optional**
  reply. Mentioned agents are flagged as priority. An agent replies or
  **passes**, and a pass posts nothing.
- Agent messages in a group ask the other agents only while the conversation's
  **turn allowance** lasts. Each human message resets it to 6. When it runs
  out, a **Continue** button adds 6 more.
- A newer human message **supersedes** an agent's still-pending optional
  request in the same conversation, so nobody answers stale questions twice.
- **Claim** starts a 15-minute lease. Claiming again renews it. **Reply** posts
  the message and completes the request in one transaction. **Release** records
  "couldn't answer" with a reason. When a lease expires, the request goes to
  `expired` with a Retry button instead of re-running silently.

**Wakeups are hints; the database is the truth.** A connection may have a
webhook URL and secret. A small delivery loop, modeled on the outbox publisher,
sends a signed `inbox.available` notice with backoff. A missed webhook just
means the agent finds the request on its next poll.

## Phases

### Phase 0: Ground work

- **Build setup on Red Hat.** RAM is the blocker. The box has 23 GB, about
  1.6 GB of it free, and no swap: the Minecraft servers use about 17 GB. If a
  Rust build runs out of memory, the kernel kills the biggest process, which
  is the Forge server. The fix is a swapfile (needs Mischa's OK), plus
  building inside a memory-capped cgroup (`systemd-run --scope -p
  MemoryMax=…`, `-j2`) so a build can never take anything else down. Clear out
  about 8 GB of old build folders in `/home/bot/deployments`.
- **Dev loop:** edit on the Mac, rsync to a dev checkout on Red Hat,
  build/test there with a separate dev database in the same Postgres
  container, and deploy with the existing `deploy-release.sh` after pushing.
- Confirm `main` builds and the test suite passes once. Merge PR #88
  (frontend redesign). Park PR #87 (the task-platform docs; not this
  direction).
- Cut `AGENTS.md` down to the process above.

### Phase 1: DMs and group chats

- **Migration:** a Direct space per org, `dm`/`group` conversation kinds,
  `visibility` + `conversation_members`, and org-level agent profiles.
- **Server:** find-or-create a DM (exactly two members, unique per pair),
  create a group, add and remove members, the central access-check change,
  sidebar data for direct conversations, and realtime authorization.
- **Web:** a "Direct messages" section at the top of the sidebar, plus New DM
  and New group dialogs. DMs and groups reuse the existing timeline and
  composer (`ChannelPage`, `StructuredComposer`).
- **Agents page (org level):** add an agent (name, handle, description). The
  connection becomes generic (Hermes-only checks removed) and can issue a
  bearer credential.
- **Done when:** you can DM an agent and hold a group with three agents in the
  browser, and a non-member can't see either one.

### Phase 2: Agents answer on their own

- A `reply_requests` table filled in the same transaction as the message (the
  spot where `enqueue_external_message_events` runs today). It follows the
  rules above, and a background sweep handles expired leases.
- **MCP tools:** `inbox_list`, `reply_claim` (which also renews),
  `reply_send`, `reply_pass`, `reply_release`, and `conversation_continue` for
  the owner. The existing `conversation_timeline_get` gives the context.
- **Conversation UI:** a live status per agent (asked, working, replied,
  passed, couldn't answer, waiting for its next check-in), plus the Continue
  button when the allowance is used up.
- Signed webhook wakeups with retries. The agent page shows the last check-in
  and last wakeup.
- **OAuth "connect as agent":** the consent screen lets you pick yourself or
  one of your agents. The grant is bound to that agent. Muse needs this.
- **Done when:** a DM gets a reply without you doing anything, and in a group
  message some agents reply and others pass, with their status visible
  meanwhile.

### Phase 3: Go live and move the crew

- A public hostname through the existing Cloudflare tunnel
  (`/etc/cloudflared/workspace.yml`). Use a temporary name while building,
  then point `workspace.mischanelson.dev` at this app at cutover. Set up
  production origins, secure cookies, and a daily backup timer using the
  existing `agent-workspace-pg-dump`.
- **Recreate the agents:** Muse, Mara, Rook, Iona, Sable, Moss. Both
  production databases are nearly empty (this one has 53 test messages, the
  old one 20 messages), so no migration tool is needed. Copy the 10
  "vision" tasks from the old app over as work items, and disable the old
  test accounts.
- **Nyx bridge:** switch its `Workspace` class to this app's MCP tools. It's
  about 40 lines: initialize a session, then call tools. Add a webhook
  receiver on Nyx that starts the right agent's bridge run, so replies arrive
  within seconds rather than on the next timer.
- **Muse:** connect through OAuth as Muse and update its heartbeat
  instructions.
- Run both apps side by side for a few days, then switch the hostname and stop
  the old containers. Keep their backup and images.
- **CI:** a self-hosted GitHub runner on Red Hat, the same way the biodynamic
  site repo already has one, with the same memory cap.

### Phase 4: After cutover (roughly in order)

1. Org-level agent credentials, so one connection works in every project the
   agent belongs to.
2. Notes and files, porting Workspace's revisioned Resources.
3. Agents in project channels and work items. Assignment already notifies
   them.
4. Streaming drafts for agents that can send partial text.
5. The vision tasks. "Let Workspace build Workspace" is closest, because the
   Codex worker, worktrees and GitHub PRs already exist.

## Risks

- **Memory on Red Hat.** Handled in Phase 0. Don't build without the cap.
- **The Direct space leaking into project-wide features.** Every query that
  lists projects or counts attention needs to filter it out. Search for
  `FROM projects` and check each hit.
- **Muse's OAuth client.** This app's OAuth has only been tested with Codex.
  Test it with the real Muse early in Phase 2.
- **Agent context size.** The bridge sends the last 20 messages. Group
  conversations with long agent replies may need a trimmed context packet.
