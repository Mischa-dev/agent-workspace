# Plan for the first deployed release

September 21, 2026. Expanded launch scope authorized by the owner: implement and verify the app, including channels, planning views, and a shared board for artifacts/files in spaces. This replaces the previous requirement to complete all 28 packages before launch. Implementation is now authorized. See BUILD_CONTRACT.md for coordinated interfaces. Deployment target and client setup are being qualified; do not describe them as active before verification.

## What we are shipping

One deployed private workspace where Mischa and the initial set of agents can exchange messages, share Resources, use projects and tasks, and hand work to one another. Each agent can check in periodically or receive a supported immediate wakeup. Connection behavior is configurable; ship editable defaults so using the app does not require designing an integration.

Keep the working local core. The existing DMs/groups, deterministic Auto routing, versioned Resources, planning proposals, task dependencies, bounded local execution, and GitHub links remain useful. Existing local execution only stays available where its runtime is actually configured; moving the web server does not move a Mac's Codex session or credentials.

The full vision remains in [LONG_TERM_ROADMAP.md](LONG_TERM_ROADMAP.md). Its completion gates are deferred. The active launch work is the six L-series packages in [NEXT_BACKLOG.json](NEXT_BACKLOG.json); W-series packages describe later work.

## Keep for launch

- Independently authenticated agents with scoped access, a small setup form, and reusable editable connection presets.
- One checked MCP/HTTP interface for inbox reads, claims, conversation context, replies/handoffs, Resources/files, task capture, task assignment, planning proposals, and results.
- Durable pending requests, current-context checks, cancellation, duplicate protection, and truthful uncertain-run recovery.
- DMs, groups, named channels and focused threads, with improved navigation and Auto behavior; retain the six-turn burst and ordinary owner renewal rules.
- Spaces with shared boards for notes, pastes, files and saved outputs; pin/filter material, open it, and attach exact revisions to messages/tasks. Keep Resources as the underlying object.
- Projects with usable outline, board and dependency views, reviewed plans, assignment, blockers and linked PRs. Expose them to external agents through the same records.
- A small availability display: waiting for check-in, working, blocked, connection problem. Surface questions and results in the existing conversation/task views.
- A deployed reachable service, appropriate authentication/TLS, persistent storage, startup/restart checks, and a tested backup/restore.
- Real connection tests for the initial agents and a complete handoff that returns to its originating conversation.

Preserve access boundaries across private messages, group context, Resource retrieval, files, and task attempts. Reading, being asked to reply, claiming work, and being authorized to execute remain different operations. A missed wakeup leaves the request pending. A vanished worker does not make duplicate execution safe.

## Simplify the implementation

Use one application and database. Add only the schema and indexes needed for agent grants, connection settings, durable claims, and bounded inbox/context queries. Keep existing safe transaction serialization initially. A full store rewrite or fine-grained lock redesign is not a launch prerequisite; measure ordinary use and fix actual bottlenecks.

Use a small connector registry and editable stored presets. Generic polling through MCP/HTTP is the common path. Add a bounded configurable wakeup adapter for a runtime that supports it. A new brand using an existing protocol requires configuration, not core code. Defer a general plugin system, visual automation builder, custom-template publishing, and elaborate subscription rules.

Keep one default connection per agent initially. Preserve a stable optional external identity reference for AgentOS. AgentOS continues to own the configuration and lifecycle of its managed agents; launch does not duplicate that authority. An existing Hermes agent can participate through supported collaboration tools and wakeup configuration while deeper AgentOS management remains later work.

Reuse existing client heartbeats and schedulers. The workspace supplies connection details and short setup instructions; it does not need its own schedule designer. Use authentication the actual launch clients support, with maintained components where OAuth is needed. Do not build a general identity platform.

Use existing Resources for durable shared notes and decisions. A formal memory service can follow actual use.

## Six bounded work packages

| Package | Work | Finish when |
| --- | --- | --- |
| L01. Scoped agent access | Add agent/connection grants and narrowly required schema changes; preserve current owner access and records | Different agents have distinct access; private context stays private; migration does not replay work |
| L02. Inbox and collaboration tools | Add shared MCP/HTTP tools, claims, replies, handoffs, files, and existing task operations | An unfamiliar client can claim work, retrieve context, save a Resource, and reply without product-specific code |
| L03. Defaults and pickup | Add a compact setup flow, heartbeat instructions, connection selection, and supported immediate wakeups | Two agents can use one preset independently; polling and wakeups cannot duplicate work |
| L04. Deployment and recovery | Prepare and activate one suitable reachable host when authorized; persistent database/files, auth/TLS, health, backup/restore | Real remote requests work; restart/restore preserve records and do not restart old work |
| L05. Connect the launch agents | Use the same tools for Muse, ChatGPT Work, Codex, and the intended existing Hermes connection; include OpenClaw if it is in the actual launch roster | Each named connection has a real round trip and honest documented limitations; no new domain behavior by brand |
| L06. Daily-use check and release | Complete space boards, channels/threads and project planning views; verify a cross-agent handoff, cancellation, and recovery | Mischa can use the deployed app without carrying context between agents; no test runs remain active |

Implement L01-L03, prepare deployment in parallel after the shared interface is stable, then connect and verify. Launch smoke testing can begin before every client is configured. Keep each client's readiness explicit; never describe all agents as connected while one still needs setup.

For Codex, qualify either its existing managed connector on the chosen execution host or participation through the generic MCP interface. A deployed control app is not proof of remote coding capability. For AgentOS-managed agents, use only the interfaces and authority available in that installation; no AgentOS code or deployment change is implied by this release plan.

## Deliberately deferred

| Work | When it becomes useful |
| --- | --- |
| Full AgentOS configuration UI, declaration synchronization, deployment/lifecycle controls | After shared participation is dependable |
| General project build/test profiles, automatic draft PR publication, checks/review synchronization, combined-branch verification | First substantial follow-up; complete the original unattended coding workflow |
| Model-assisted invitations and generated conversation briefs | After reliable channels/threads and agent participation |
| Elaborate graph editing, richer alternatives and scheduling constraints | After the launch outline, board and dependency views |
| Typed shared memory, automatic extraction, semantic retrieval | Once agents are sharing real Resources and retrieval needs are known |
| Large synthetic benchmarks, global store/locking redesign, multiple human administrators | When measured workload requires them |

These are retained roadmap items. None blocks the initial shared-workspace release unless a concrete defect makes it necessary.

## Deployment and acceptance boundaries

Choose one host that the actual cloud clients can reach. A private-network-only address or localhost is insufficient unless their connection path supports it. Prepare the exact host, domain/TLS, authentication, storage, migration, and rollback steps before activation. Do not provision paid services, expose the app, or alter other projects based solely on this planning document.

Before calling the deployed release ready, verify actual agent authentication and file/Resource operations; one scheduled check-in; a supported immediate pickup; handoff and return to the original thread; revoked access; duplicate requests/results; cancellation or correction before delayed pickup; and restart/restore without replay. Reuse the current tests and add coverage for changed boundaries. Do not require completion of the entire old A/B/C vision to pass this narrower release.

If a third-party connection is blocked, finish the other agents and the deployment, identify the exact remaining connection step, and retain an honest partial status for that client. Never substitute a fixture for its live qualification.

Current implementation evidence remains in [ACCEPTANCE.md](../ACCEPTANCE.md). Success for this release is reliable daily collaboration with the launch agents. Completion of the advanced coding and AgentOS roadmap is tracked separately.

## Implementation checkpoint — September 21

L01–L03 and the expanded L06 interface are implemented and tested. Local HTTP/MCP, cross-agent file handoff, and isolated backup/restore pass. L04 is now deployed on Red Hat at https://workspace.mischanelson.dev with public OAuth/MCP, daily backups and a live isolated restore verified. L05 actual third-party client setup remains pending. See [CONNECTED_RELEASE.md](CONNECTED_RELEASE.md) for evidence and the remaining concrete steps.
