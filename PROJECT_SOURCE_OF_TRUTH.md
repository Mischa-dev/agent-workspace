# Agent Workspace
## Project Source of Truth and Implementation Plan

**Working title:** Agent Workspace  
**Status:** M4 implementation complete; acceptance pending; M5 entry backlog v0.15

**Owner:** Mischa Nelson  
**Date:** July 31, 2026
**Document role:** This file is the canonical product and engineering plan until a later version explicitly replaces it.  
**Implementation authorization:** Milestones M0 through M3 are complete. M4 is implemented and remains subject to its exact-commit acceptance gate. M5 is elaborated but is not authorized until M4 is accepted; every later milestone remains subject to the same entry gate.

> The product name, visual identity, and public branding are intentionally unresolved. "Agent Workspace" is a neutral working title, not a proposed final name.

## Revision history

### v0.15 — M5 GitHub delivery entry backlog

- elaborated every M5 task with acceptance criteria, dependencies, expected
  tests, and explicit exclusions without authorizing implementation before M4
  acceptance
- fixed a host-owned GitHub App boundary: private key and webhook secret stay
  outside PostgreSQL and the repository, while short-lived installation tokens
  exist only for one child API or Git operation
- fixed verified, deduplicated webhook receipt plus durable at-least-once jobs
  and outbox/refetch convergence as the GitHub state synchronization contract
- required an explicit human publish action, a clean branch with commits ahead
  of its recorded base, backend-owned credential injection, and uncertain-push
  reconciliation; M5 never stages or commits arbitrary dirty worktree content
- fixed Project-scoped pull-request, check, review, and explicit Work Item link
  projections while retaining detailed code review on GitHub
- fixed visible, configurable pull-request automation that never reopens closed
  work or bypasses a Project's allowed workflow transitions
- retained dirty worktrees, provider secret isolation, repository trust, and
  reversible cleanup as hard M5 boundaries

### v0.14 — M4 Work Items entry

- elaborated every M4 task with acceptance criteria, dependencies, expected
  tests, and explicit exclusions
- fixed one canonical optimistic-versioned Work Item, Project-owned workflow
  definitions, stable semantic status categories, and list/Kanban projections
- fixed M4 description, acceptance-criterion, comment, activity, assignment,
  Conversation-link, Repository-link, and structured-reference boundaries
- required all Work Item authorship and assignment to use server-resolved Actor
  identity and the existing centralized Project policy
- specified an idempotent Work-with-Worker handoff that creates and links a new
  Thread before invoking the accepted M3 run path
- authorized M4 implementation while retaining GitHub delivery, MCP exposure,
  final workflow customization, and external agent connections for later gates

### v0.13 — M3 acceptance

- accepted M3 against exact green commit
  `0179ee6b2c60815b647b2312c9bd3509cd83aad5`, GitHub CI run
  `30681247396`, and its immutable Red Hat release
- recorded the browser-driven hosted run, two durable approvals, one
  consolidated Worker Message, known-file-only dirty state, and exact
  repository/base/branch/provider identity
- recorded conservative systemd restart reconciliation of a real working turn
  into an actionable terminal interruption with its worktree preserved
- confirmed zero application-secret keys in the provider environment and zero
  credential patterns, runtime paths, or disallowed control characters in the
  inspected logs, durable rows, and browser output
- recorded that the pinned hosted execution mode did not elect to expose the
  experimental `request_user_input` tool while live command approvals worked
- left M4 unauthorized until its Work Items backlog is elaborated

### v0.12 — M3 execution foundation hardening

- recorded the additive M3 ownership, Project-scope, optimistic-version,
  repository-preparation, provider-session, and Agent Run admission foundation
- retained the already-applied `0009` migration byte-for-byte and moved every
  review correction into additive `0010`
- made an authenticated personal Execution Profile and a prepared trusted
  Repository explicit fail-closed admission requirements for M3 hosted runs
- recorded descriptor-relative, no-follow creation of profile directories
  beneath the configured data root and uniqueness of provider/Git runtime paths
- added a bounded pre-`0009` repair for only the known legacy seed paths;
  unknown layouts fail closed for operator repair
- kept team-owned/shared Execution Profile policy open for M7 while fixing M3
  personal-profile execution to the authenticated owning Actor

### v0.11 — M2 acceptance and M3 authorization

- accepted M2 against exact green commit
  `d4b08d025d82d1b83a5aaae677499b23b405f840`, GitHub CI run
  `30652464974`, and its immutable Red Hat release
- recorded browser-driven two-Actor Channel, semantic mention, linked Thread,
  pagination, independent read-cursor, one-session realtime, archive fallback,
  and explicit read-only history evidence
- recorded controlled systemd restart and exact outbox-event replay recovery
  without duplicate Messages, consumed peer cursors, or manual repair
- retained the immutable M2 migration checksums and the M0/M1 host, provider,
  worktree, and security boundaries
- declared M2 satisfied and authorized the elaborated M3 hosted Execution
  Worker backlog

### v0.10 — M2 implementation record and M3 entry backlog

- recorded the implemented M2 Conversation, Message, read-state, outbox,
  realtime, sidebar, archive, and structured-mention boundaries without
  declaring M2 accepted before exact-commit host and browser evidence exists
- fixed the version-one Message AST limits and the Conversation-specific
  not-found non-disclosure response in the Decision Log
- recorded at-least-once outbox publication plus reconnect/refetch convergence
  as the realtime correctness contract
- preserved the immutable applied M2 migration checksums and recorded the
  additive `0006` distinct-Actor mention projection plus the `0007`/`0008`
  archived-parent navigation and transaction-lock boundaries
- expanded every M3 hosted Execution Worker backlog item with acceptance
  criteria, dependencies, expected tests, and explicit exclusions
- retained M3 as unauthorized until the remaining M2 CI, Red Hat deployment,
  restart, and browser-driven two-Actor gates are accepted

### v0.9 — M1 acceptance record and M2 authorization

- recorded successful host-local bootstrap and private browser-session
  acceptance against the exact green Red Hat release
- recorded authenticated Project creation, switching, archive, active-only
  navigation, and per-Actor route restoration across reload and relogin
- confirmed that the M0 timeline and hosted Codex run remain functional through
  M1 session authentication and centralized Project policy
- retained redacted runtime and automated evidence in `docs/acceptance/m1.md`
- confirmed the elaborated M2 Conversations backlog and authorized M2

### v0.8 — M2 Conversations entry

- expanded every M2 backlog item with acceptance criteria, dependencies,
  expected tests, and explicit exclusions
- fixed the M2 boundary around durable Channels, Threads, immutable
  Actor-authored Messages, read cursors, and structured Actor mentions
- specified the transactional outbox, authenticated realtime envelope,
  reconnect/refetch behavior, and compact Thread-sidebar projection
- kept Work Items, attachments, reactions, hosted-worker ownership, and inline
  Channel execution outside M2

### v0.7 — M1 identity and Projects entry

- confirmed the accepted M0 exit before entering M1
- expanded every M1 backlog item with acceptance criteria, dependencies,
  expected tests, and explicit exclusions
- scoped host-local bootstrap, private-build password sessions, centralized
  Actor policy, Project lifecycle, the Project rail, and per-Actor navigation
- kept final team authentication, invitations, Conversations, Work Items, and
  later integration behavior outside M1

### v0.6 — M0 acceptance record

- recorded completion of the repository, CI, Red Hat host, Codex App Server,
  MCP, Git worktree, GitHub App, and browser-driven walking-skeleton gates
- recorded the exact MCP protocol negotiated by Codex `0.146.0`
- recorded the proven provider-Thread resume and conservative startup
  reconciliation contract

### v0.5 — PostgreSQL 18 container layout correction

- corrected the PostgreSQL 18 container volume target from
  `/var/lib/postgresql/data` to `/var/lib/postgresql`
- recorded the versioned cluster-directory behavior required by the pinned
  PostgreSQL 18 image

### v0.4 — verified M0 toolchain correction

- corrected the initial Rust pin from the unpublished `1.98.0` assumption to
  the verified stable `1.97.1` toolchain available for both target
  architectures
- required bootstrap pins to be checked against their official distribution
  artifacts before CI and host installation

### v0.3 — actor and execution-worker architecture

- clarified the product as a Slack-like project workspace joined to Work Items, list and Kanban views, GitHub delivery, and agent execution
- introduced one actor and authorization model for humans, externally operated agent members, execution workers, integrations, and system actions
- separated external agent members such as Hermes or OpenClaw from bounded execution workers such as hosted Codex
- made actor attribution, delegated execution, approval routing, messages, and audit boundaries explicit
- established that external systems own their agents' memory, schedules, autonomy, and runtime behavior
- reserved external agent connection protocols and product integration for a post-version-one milestone
- preserved Codex App Server as the only version-one hosted execution provider

### v0.2 — implementation-ready baseline

- authorized M0 and added milestone entry rules
- added a composed walking-skeleton gate after the external spikes
- moved managed repository workspaces into the hosted Agent Run phase
- made technical-alpha invocation Thread-based
- defined ordered conversation entries, read cursors, outbox semantics, and exhaustive Agent Run states
- defined deterministic repository selection and collision-safe branch ownership
- tightened credential, Git, subprocess, session, and backup boundaries
- assigned remaining open decisions to explicit future gates

### v0.1 — planning baseline

- established the product thesis, domain model, architecture, phases, and initial backlog

---

# 0. How to use this document

This file is the single source of truth for the initial build. It records:

- what the product is
- what it is not
- the core user experience
- the domain model
- the backend and frontend architecture
- the Codex, MCP, Git, and GitHub integration model
- the solo and team model
- the state machines
- the security boundaries
- the build order
- the initial backlog
- the decisions that are final for now
- the decisions that are still deliberately open

When implementation reveals that a decision is wrong, update this document in the same pull request that changes the architecture. Do not let the real system silently diverge from the plan.

This is not an immutable constitution. It is a living plan with explicit revisions.

## Source-of-truth rules

1. Product behavior documented here wins over assumptions made in code comments or chat history.
2. Architecture changes require a short decision entry in the Decision Log.
3. A future feature must not distort the version-one architecture unless the extension point is cheap and clearly useful now.
4. Do not add infrastructure merely because a large company might eventually need it.
5. Do not make solo use feel like operating a fake corporation.
6. Do not make team support an afterthought that requires changing the ownership and permission model later.

## Implementation readiness rules

This document is detailed enough to authorize repository bootstrap, technical spikes, and the first end-to-end walking skeleton.

It is not permission to implement every roadmap heading from title alone.

Before a milestone after M0 begins:

1. resolve every open decision whose `Decision required before` field names that milestone
2. expand that milestone's backlog items with acceptance criteria, dependencies, expected tests, and explicit exclusions
3. confirm that the preceding milestone exit criteria are satisfied
4. record any changed external assumption in the compatibility notes and Decision Log

Only one milestone needs task-level elaboration ahead of active implementation. Do not turn the entire roadmap into speculative ticket detail.

An implementation agent should receive one backlog item or one explicitly bounded vertical slice at a time. The source of truth supplies context; it does not replace a scoped task.

Milestone authorization does not by itself authorize creating external accounts or applications, publishing a service, exposing a host, spending money, or deleting remote artifacts. Those actions still require explicit operator scope at execution time.

---

# 1. Executive decision summary

The project will begin with the following decisions:

| Area | Decision |
|---|---|
| Product form | Desktop-first responsive web application |
| Deployment | Self-hostable, single always-on Project Host for version one |
| Backend language | Rust |
| Backend architecture | Modular monolith |
| Frontend | React and TypeScript with Vite |
| Database | PostgreSQL |
| Realtime | One authenticated WebSocket per client, durable state over HTTP and PostgreSQL |
| Background work | PostgreSQL-backed job queue, no Redis initially |
| Collaborative shape | Slack-like project workspaces with Channels, Threads, Messages, members, and durable history |
| Planning shape | Work Items with list, detail, and Kanban projections |
| Identity and authorization | One actor model and one policy engine for people, external agent members, execution workers, integrations, and system actions |
| Hosted execution worker | Codex App Server running on the Project Host |
| User-facing worker name | Execution Worker, default provisional handle `@agent` |
| Worker provider name | Hidden from ordinary messaging; version one provider is `codex_app_server` |
| External agent members | Architecture-compatible now; Hermes, OpenClaw, and similar connections are deferred to a later integration milestone |
| Local Codex workflow | Open work item in local Codex and let Codex use the app's MCP server |
| Local workflow thread | None is created automatically |
| Hosted invocation for technical alpha | Thread-based execution only; inline Channel execution is milestone-gated |
| Future remote provider | Codex cloud provider, deferred until a suitable public integration exists |
| Code hosting | GitHub remains source of truth for repositories, pull requests, checks, and reviews |
| Worker isolation | Git worktree per substantial hosted execution Thread or Agent Run |
| Planning object | Flexible Work Item |
| Kanban | One visualization of Work Items, not the underlying model |
| Conversations | Channels and first-class one-level Threads |
| Nested threads | Never supported |
| Solo and teams | Personal organization hidden in solo mode; project memberships and permissions exist from the start |
| Commercial posture | Self-hosted private product first; preserve commercial options and require a provider-authentication and service-terms gate before paid public distribution |
| Microservices | Explicitly rejected for the initial product |
| Generic all-agent framework | Explicitly rejected; define narrow actor and worker boundaries instead |

---

# 2. Product thesis

## 2.1 One-sentence description

Agent Workspace is a Slack-like project environment where people and, later, externally operated agent members communicate in Channels and Threads, organize Work Items through lists and Kanban views, delegate bounded implementation work to execution workers, and review GitHub artifacts without losing the relationship between discussion, planning, execution, and outcome.

## 2.2 The central loop

```text
Discuss an idea
      ↓
Create or link structured work when useful
      ↓
A human or external agent member coordinates the work
      ↓
An authorized actor performs it directly or delegates a bounded run to an execution worker
      ↓
Blockers and approvals surface clearly
      ↓
A branch, pull request, report, or other artifact is produced
      ↓
The result is reviewed
      ↓
The project preserves the entire relationship and history
```

## 2.3 Inspiration without cloning

The product borrows interaction ideas from several categories:

- Discord and Slack for projects, channels, mentions, and threads
- Linear for structured work and fast project navigation
- GitHub for durable code collaboration and review
- Codex for real coding execution, tool activity, approvals, and long-running agent work

The product is not a clone of any one of them. Its everyday collaboration model should nevertheless be immediately legible to someone who understands Slack: projects contain members, Channels, Messages, mentions, and Threads. Work Items, list and Kanban views, execution workers, and GitHub artifacts extend that collaboration model rather than forming separate disconnected applications.

Its center is the coordination layer connecting conversation, planned work, execution, and delivery.

## 2.4 Product principle

**Work is project-local. Attention is global.**

Each project should feel like its own contained world. Cross-project navigation should surface only things that need the user's attention and link directly back to the correct project location.

---

# 3. Product principles

These are non-negotiable unless a later decision explicitly replaces them.

## 3.1 Solo-first, team-native, actor-safe

The product must feel excellent with one person and one execution worker. It must not display empty enterprise ceremony, unnecessary ownership fields, or fake team workflows to a solo user.

At the same time, the database, permission model, read states, audit model, and identity model must support multiple actors from the beginning. Human users ship first. Externally operated agent members join through a later integration milestone without requiring messages, Work Items, permissions, approvals, or audit records to be redesigned.

## 3.2 Projects are real environments

A project is not a tag or filter. It has its own:

- channels
- threads
- members
- Work Items
- statuses and views
- repositories
- execution workers
- project instructions
- artifacts
- permissions
- remembered navigation state

Switching projects should feel like switching Discord servers or persistent workspaces.

## 3.3 Conversations and Work Items remain separate

A Thread is not a Work Item. A Work Item is not a conversation.

They can be linked many-to-many:

- a Thread can have no linked Work Item
- a Thread can link one or several Work Items
- a Work Item can link one or several Threads
- a Work Item can exist without any Thread

## 3.4 Threads are sub-channels

A Thread has almost all the behavior of a Channel:

- Messages from human, external-agent, and worker Actors
- attachments
- mentions
- tool activity
- linked Work Items
- linked artifacts

The differences are:

- a Thread belongs to exactly one parent Channel
- a Thread cannot contain another Thread
- a Thread is normally narrower and more temporary than a Channel
- a Thread surfaces in navigation based on activity and attention

## 3.5 Every attributable participant is an actor

An Actor is the security and attribution identity behind a project action. Human users, external agent members, execution workers, integrations, and the application system can be actors. Every Message, Work Item mutation, approval decision, worker invocation, and security-sensitive action records its initiating actor.

Project membership and centralized policy determine what an actor may do. Authentication method does not determine authorization: a browser session, external agent credential, internal worker capability, or system job must all resolve to an actor and pass the same action-on-resource policy checks.

Human members and external agent members should feel like peers in ordinary collaboration UI. Both may have a visible identity, handle, avatar, Project role, Messages, mentions, assignments, permissions, and activity history.

## 3.6 Agent members and execution workers are different roles

An external agent member is a persistent Project participant operated by a separate system such as Hermes, OpenClaw, or an OpenAI-based agent runtime. It may communicate, plan, create and update Work Items, and invoke an execution worker when authorized.

An execution worker performs a bounded run. The version-one worker is backed by Codex App Server and is optimized for repository work, streamed activity, questions, approvals, tests, Git changes, and artifacts. An execution worker may have a visible handle and author output, but it is not the same product concept as a persistent external agent teammate.

```text
Project actors
├── Human member
├── External agent member
│   └── externally operated runtime such as Hermes or OpenClaw
└── Execution worker
    ├── display name and handle
    ├── bounded delegated authority
    └── worker provider
        └── Codex App Server on the Project Host
```

External agent systems own their own models, memory, prompts, schedules, automations, loop behavior, budgets, and session duration. Agent Workspace does not host or supervise those internals. It owns the connected actor identity, Project membership, permissions, approval requirements, project data, execution-worker invocation, and audit history.

## 3.7 Worker providers remain implementation details

The visible execution worker and the system that executes it are separate concepts. Later, another worker or the same worker may use Codex Cloud or a different provider. The product should not expose provider jargon in every normal conversation.

Do not force external agent-member runtimes and execution-worker providers through one universal interface. They have different contracts. External agent connections consume authorized project APIs; worker providers perform bounded execution runs.

## 3.8 Preserve native provider strengths

The provider abstraction must not flatten Codex into a generic chatbot. Codex-specific features remain first-class:

- streamed Worker messages
- commands and command output
- file changes and diffs
- approvals
- questions and blockers
- test results
- Git activity
- resumable Codex sessions

Common lifecycle events are normalized, but raw provider events may also be retained for debugging and future UI improvements.

## 3.9 GitHub stays GitHub

The app does not become a Git hosting service or a complete pull request review replacement.

GitHub owns:

- Git repositories
- branches
- commits
- pull requests
- reviews
- checks
- merge state

Agent Workspace owns the relationships between those artifacts and the surrounding project work.

## 3.10 Responsibility and execution remain attributable

A Work Item may be assigned to an authorized human or external agent member. Project policy may also require a human owner, sponsor, reviewer, or approver for particular work without pretending that the human performed an agent's actions.

Worker runs are visible in activity and conversation history. Audit and activity distinguish the requesting actor, execution worker, provider connection, approval actors, and resulting artifacts.

## 3.11 Predictability over fake intelligence

The app must not spend model calls guessing whether every message deserves a Thread.

Thread behavior is determined by explicit UI context and simple rules. Users retain an override.

## 3.12 The core must remain light

Agent execution and repository builds may be expensive. The coordination application itself should remain efficient.

Do not add Redis, Kafka, Elasticsearch, a vector database, Kubernetes, or microservices until a measured problem requires them.

---

# 4. Explicit non-goals for version one

Do not build these during the initial product:

- Hermes, OpenClaw, or another external agent-member connection
- hosting or supervising an external agent member's model runtime, memory, schedules, automations, or autonomy loop
- a universal all-agent framework
- nested Threads
- custom Git hosting
- a complete GitHub pull request review replacement
- hosted disposable cloud runners
- local runner daemons on every computer
- perfect live synchronization with the official Codex desktop app
- a visual workflow automation builder
- Gantt charts
- advanced resource scheduling
- a plugin marketplace
- vector memory
- autonomous cross-project swarms
- native iOS or Android apps
- native desktop shell before the web experience proves itself
- editing or deleting sent Messages; version one uses follow-up corrections
- billing
- public multi-tenant SaaS isolation
- arbitrary untrusted user code execution

The initial deployment is for a trusted solo user or trusted team on a controlled Project Host.

---

# 5. Core terminology

## Organization

The top-level tenancy and ownership boundary. A solo user receives a personal Organization automatically, but the UI can hide it until team or multi-organization features are needed.

An Organization owns Projects, members, and shared integration settings.

## Project

A strongly separated environment containing Channels, Threads, Work Items, repositories, members, Execution Workers, instructions, and artifacts.

## Project Member

A human or external agent Actor with a role and permission set inside a Project. Human membership ships first. External agent membership is a later integration milestone.

## Actor

The identity used for attribution and authorization. Actor kinds include:

```text
human
external_agent
execution_worker
integration
system
```

An Actor is not an authentication credential. Credentials and sessions authenticate a client as an Actor; centralized policy determines what that Actor may do.

## External Agent Member

An externally operated agent, such as a Hermes, OpenClaw, or OpenAI-based agent runtime, connected as a Project Member.

It behaves like a teammate in ordinary project collaboration. Its external system owns its runtime behavior, memory, schedules, automations, model selection, and autonomy. Agent Workspace owns only its connection identity, membership, authorized project actions, approval obligations, messages, and audit trail.

External Agent Members are architecturally supported but are not implemented in version one.

## Execution Worker

A project-visible worker that performs bounded delegated execution through a Worker Provider. It may author messages and activity, ask questions, wait for approval, and produce artifacts.

The default Execution Worker is provisionally:

```text
Name: Agent
Handle: @agent
Provider: Codex App Server on Project Host
```

## Worker Provider

The bounded execution integration behind an Execution Worker.

Version one:

```text
codex_app_server
```

Future possibilities:

```text
codex_cloud
other provider-specific integrations
```

## External Agent Connection

A later integration record that authenticates an externally operated agent runtime as one Actor. It does not transfer ownership of that runtime to Agent Workspace.

The future connection contract must support revocation, credential rotation, scoped Project access, idempotent writes, actor attribution, and an authorized event or polling model. The exact protocol is deliberately deferred.

## Execution Profile

The credentials, configuration, provider state, and filesystem identity used to run a provider.

An Execution Profile may be:

- personal
- project service-owned
- organization service-owned

The initial solo build uses one personal Codex Execution Profile on the Project Host.

## Conversation

The shared conceptual base for a Channel or Thread.

## Channel

A durable top-level conversation inside a Project.

## Thread

A focused child conversation attached to one Channel. It cannot have child Threads.

## Message

A versioned rich-content object containing text and structured references such as Actor mentions and Work Item references.

## Work Item

The canonical structured planning object. Types include Feature, Bug, Task, Request, Research, and Improvement.

## Work Item View

A projection of Work Items, such as:

- list
- Kanban board
- review queue
- assigned to me
- blocked work
- roadmap later

## Agent Run

One bounded Execution Worker attempt requested by an authorized Actor and connected to an application Thread. The name remains `Agent Run` in the version-one implementation vocabulary because it is already pervasive, but it never means an external agent member's autonomous session. A future inline invocation may instead attach a run to a Channel invocation Message after O-002 is resolved.

## Provider Session

The provider-specific durable conversation or session, such as a Codex App Server thread ID.

## Artifact

A durable output linked to work, such as a pull request, commit, report, test result, generated file, or deployment.

## Project Host

The always-on machine running the application and the version-one hosted Codex provider.

---

# 6. User experience and navigation

## 6.1 Desktop layout

```text
┌──────┬──────────────────────┬────────────────────────────┬──────────────────┐
│ Rail │ Project sidebar      │ Main workspace             │ Context drawer   │
│      │                      │                            │                  │
│ Home │ Project overview     │ Channel                    │ Linked work      │
│ A    │ Work                 │ Thread                     │ Participants     │
│ T    │                      │ Work Item detail           │ Repository       │
│ B    │ Channels             │ Kanban or list             │ Pull request     │
│      │ # general            │ Review view                │ Worker status    │
│      │ # development        │ Settings                   │ Details          │
│      │   active Threads     │                            │                  │
│      │ # design             │                            │                  │
└──────┴──────────────────────┴────────────────────────────┴──────────────────┘
```

The right Context Drawer is collapsible. The project rail is narrow, approximately icon width rather than a second full sidebar.

## 6.2 Project rail

The project rail contains:

- Global Home
- pinned Projects
- recent Projects
- attention badges
- Project search
- create Project

The rail does not show every archived Project.

## 6.3 Global Home

Global Home is the one intentionally cross-project area. It contains:

- blockers requiring the current user
- newly completed unseen Threads
- failed runs requiring acknowledgement
- mentions
- review requests
- recently opened locations

It does not merge all project conversations into a global feed.

## 6.4 Remembered Project state

Each Project remembers, per user:

- last open route
- last open Channel or Thread
- selected Work Item view
- filters
- open Context Drawer
- draft messages where practical
- scroll location where practical

Switching Projects should be cheap and should not destroy mental context.

## 6.5 Channel sidebar Thread visibility

Threads appear beneath their parent Channel according to these rules.

Always show a Thread when it is:

- newest Agent Run is active
- newest Agent Run is blocked for the current user
- newest Agent Run failed and is not acknowledged
- newest Agent Run completed and is unseen by the current user

When the parent Channel is selected, also show the five most recently active Threads, even if they are already seen.

Then provide:

- Show more
- View all Threads
- archived Thread filtering

A seen, inactive Thread should disappear from the compact sidebar when the user leaves the parent Channel.

## 6.6 Thread colors

Colors are a visual projection, not stored business state.

- **Blue:** the newest Agent Run is created, queued, starting, working, or recovering
- **Orange:** the newest Agent Run is blocked and needs this user's input or approval
- **White:** the newest Agent Run completed successfully after this user's last view
- **Gray:** no run is active and the latest outcome has already been seen
- **Red:** the newest Agent Run failed and is not yet acknowledged

Color must never be the only signal. Every state also has text and an icon.

There is no visible purple queued state. Queueing may exist internally, but it does not deserve permanent navigation meaning.

## 6.7 Work Item experience

A Work Item detail view includes:

- stable readable key
- title
- description
- type
- status
- priority
- assignee
- acceptance criteria
- primary and related repositories
- linked Threads
- linked pull requests and commits
- dependencies and relations
- activity and comments
- actions such as Work with Worker and Open in Codex

The key is shown but rarely typed manually. Autocomplete inserts structured Work Item references.

---

# 7. Actor, external agent, and execution-worker interaction model

## 7.1 The default Execution Worker

Every new Project may optionally create one default Execution Worker:

```text
Name: Agent
Handle: @agent
```

The handle is provisional and customizable later. Handles must be unique inside a Project.

The Worker Provider is shown in settings and diagnostic UI, not treated as the worker's identity.

## 7.2 Structured mentions

`@agent` is a structured Actor mention. It is not implemented by scanning plain text for a magic string.

A Message stores an Actor ID in its content AST. Rendering produces `@agent`, but the backend receives an unambiguous Actor reference. The same representation supports human members and future external agent members.

## 7.3 Channel invocation behavior

The complete hosted execution loop is Thread-first.

For the technical alpha, mentioning an Execution Worker in a Channel offers:

```text
Run in a Thread
```

The application creates or selects the Thread before starting an Agent Run. This keeps execution ownership, blockers, approvals, worktrees, recovery, and attention state unambiguous while the core loop is being proven.

Version one may add an inline invocation mode:

- Reply here
- Run in a Thread

When inline invocation is implemented, deterministic defaults are:

1. A Work Item action such as Work with Worker defaults to a new Thread.
2. An Execution Worker mention inside an existing Thread continues in that Thread.
3. A Channel message referencing a Work Item defaults to Run in a Thread.
4. A normal Channel question defaults to Reply here.
5. The user can change the mode before sending.

No model call decides this.

Inline invocation must not ship until its run ownership, provider-session mapping, concurrency, repository context, approval behavior, recovery behavior, and durable rendering contract are specified and tested.

If an inline invocation unexpectedly requires a long or state-changing execution, the app may offer a Continue in Thread action. It must not silently move the conversation after the fact.

## 7.4 Hosted Execution Worker Thread flow

```text
Authorized actor starts work with @agent
        ↓
Application creates or reuses a Thread
        ↓
Thread links optional Work Items
        ↓
Backend creates an Agent Run
        ↓
Scheduler admits the run under host and profile limits
        ↓
Git manager creates an isolated worktree
        ↓
Codex provider starts or resumes a provider session
        ↓
Events stream into the Thread
        ↓
Question or approval makes the Thread orange
        ↓
Addressed actor answers, Thread returns blue
        ↓
Codex completes and optionally creates a pull request
        ↓
Thread becomes white for users who have not reviewed it
```

Queued runs do not create worktrees until the scheduler moves them to `starting`.

The requesting Actor may be a human in version one or an external agent member after the later integration milestone. The permission, approval, run, and audit models must not require that requester to be a `user`.

## 7.5 One active run per Thread initially

Version one permits multiple historical Agent Runs in a Thread, but at most one non-terminal run at a time.

This prevents ambiguous Thread state and keeps blue, orange, red, and white behavior understandable.

Later parallel execution can be added as an explicit feature, not accidentally created through race conditions.

Enforce this with a database constraint or another transactionally safe uniqueness mechanism over non-terminal Agent Runs, not only with an application-level preflight check.

## 7.6 Local Codex flow

A local Codex workflow is separate from hosted Threads.

From a Work Item, the user selects:

```text
Open in Codex
```

The web app opens a local Codex deep link with:

- the Git remote URL for workspace matching
- a short prefilled prompt
- the Work Item display key and immutable ID or canonical URL
- instructions to use the connected Agent Workspace MCP server

No application Thread is created merely because the user clicked the button.

No "opened locally" activity entry is required.

No blue, orange, white, gray, or red Thread state exists because there is no Thread.

Local Codex reads and updates project state through MCP. GitHub later confirms branches, pull requests, checks, and merges.

## 7.7 Future Codex cloud provider

The architecture reserves a provider kind named conceptually `codex_cloud`, but version one does not implement it.

A future cloud provider must be able to support enough of the following to be useful:

- authenticated per-user or service execution
- task creation
- run status
- result retrieval
- conversation or task linkage
- pull request linkage
- errors
- ideally streamed or pollable output

A trigger-only API that cannot return the Worker response is not sufficient for a first-class Thread provider.

## 7.8 Future external agent members

Hermes, OpenClaw, and similar systems connect later as externally operated Agent Members, not Worker Providers.

The target interaction is:

```text
External automation wakes Hermes outside Agent Workspace
        ↓
Hermes authenticates as its Actor
        ↓
Hermes reads and writes permitted Project resources
        ↓
Hermes posts Messages or creates and updates Work Items
        ↓
Hermes may invoke an Execution Worker when authorized
        ↓
Agent Workspace records requester, worker, approvals, results, and artifacts
```

Agent Workspace does not schedule Hermes, own its memory, choose its model, meter its reasoning loop, or decide when its autonomous session ends.

Version one must preserve compatibility by using Actor IDs for authorship, assignment, requests, approvals, and audit; centralized action-on-resource authorization; and client-neutral domain operations. It does not implement external-agent credentials, event delivery, polling, presence, or runtime-specific adapters.

---

# 8. Deployment and system architecture

## 8.1 Version-one deployment model

One always-on Project Host runs the shared application and hosted execution workers. Future external agent-member runtimes remain outside this deployment boundary.

```text
                             GitHub
                     repositories, PRs, checks
                              ▲
                              │ webhooks and API
                              │
Browser or PWA ───────► Project Host
HTTPS + WebSocket          │
                           ├── Rust application
                           ├── PostgreSQL
                           ├── MCP endpoint
                           ├── Git and worktree manager
                           ├── GitHub integration
                           └── Codex provider bridge
                                  │
                                  │ JSONL over stdio
                                  ▼
                           Codex App Server

Local Codex desktop, CLI, or IDE
          │
          ├── Streamable HTTP MCP ─────► Project Host
          └── Local Git ───────────────► GitHub

Future externally operated agent runtime
          │
          └── authenticated integration API or MCP ─────► Project Host
```

The future external-agent arrow is an architectural compatibility boundary, not a version-one component.

## 8.2 Conceptual control plane and executor separation

The version-one binary is a modular monolith, but internal boundaries distinguish:

### Core control plane

Owns:

- identities
- actors
- Organizations
- Projects
- memberships
- Channels
- Threads
- Messages
- Work Items
- notifications
- read state
- durable event history
- permissions
- approvals and audit attribution

### Executor

Owns:

- provider processes
- Codex authentication profiles
- repositories
- worktrees
- Git commands
- provider sessions
- tool events
- approvals
- run recovery

These components run together initially. The boundary allows a remote executor later without forcing distributed infrastructure now.

External agent runtimes are neither part of the control-plane process nor the executor. They are external clients that authenticate as Actors and call authorized domain operations. A future connection may use MCP, HTTP APIs, OAuth, scoped tokens, event delivery, or polling; no protocol choice is required for version one.

## 8.3 Why a modular monolith

A modular monolith provides:

- one deployable application
- one database transaction boundary
- direct typed function calls
- low idle resource use
- simple local development
- easier crash recovery
- fewer network failure modes

The modules must have clear ownership, but they do not need separate services.

## 8.4 Host filesystem layout

Production-style layout:

```text
/var/lib/agent-workspace/
├── repositories/
│   └── <repository-id>/
├── worktrees/
│   └── <repository-id>/<thread-or-run-id>/
├── attachments/
├── logs/
├── codex/
│   └── profiles/<execution-profile-id>/
└── temporary/
```

Rules:

- run as a dedicated non-root service user
- permissions default to owner-only for Codex credentials and secrets
- never permit arbitrary user-provided filesystem paths
- every repository path is registered and approved by the application
- worktree paths are generated by the application

## 8.5 Native host process first

Initial production deployment should use:

- the Rust binary managed by systemd or an equivalent service manager
- PostgreSQL
- Caddy or another reverse proxy for TLS
- Codex installed on the host

Containers may be added later, but forcing Codex authentication, local sandboxing, Git credentials, and worktree paths through containers before the core works adds avoidable complexity.

Development may use Docker only for PostgreSQL.

---

# 9. Technology stack

## 9.1 Backend: Rust

Rust is the selected backend language.

This is not merely an aesthetic choice. The backend is a long-running process supervisor and collaboration server that must handle:

- WebSockets
- JSON-RPC
- child processes
- streaming output
- Git subprocesses
- durable jobs
- authorization
- provider protocol boundaries
- filesystem safety
- concurrent agent runs

Rust is a reasonable fit for low idle overhead, strong domain types, process control, and security-sensitive boundaries.

### Rust guardrails

Rust must not become an excuse to over-engineer.

1. Use stable Rust and pin the toolchain in `rust-toolchain.toml`.
2. Add `#![forbid(unsafe_code)]` in application crates.
3. Start with one server crate and modules. Split crates only when boundaries are proven.
4. Do not build a custom actor framework.
5. Do not create a generic effect system.
6. Do not make every function generic.
7. Use direct SQL and typed queries rather than building an internal ORM.
8. Prefer boring structs and enums.
9. Use `thiserror` for typed module errors and `anyhow` only at process boundaries.
10. Avoid `unwrap` in production paths except for startup invariants that cannot recover.
11. Keep provider-specific JSON at the provider boundary.
12. Require formatting, clippy, tests, and dependency checks in CI.

### Backend libraries

Recommended baseline:

- `tokio` for async runtime and process management
- `axum` for HTTP and WebSocket endpoints
- `tower` and `tower-http` for middleware
- `serde` and `serde_json` for protocol serialization
- `sqlx` for PostgreSQL and migrations
- `tracing` and `tracing-subscriber` for structured logs
- `uuid` with time-ordered IDs generated in the application
- `time` for timestamps
- `thiserror` for typed errors
- `reqwest` for GitHub and external HTTP
- `argon2` for local password hashing if local password auth is implemented

Do not select additional libraries until a concrete feature requires them.

## 9.2 Frontend: React and TypeScript

Recommended baseline:

- React
- TypeScript
- Vite
- TanStack Query for server state
- TanStack Router or a similarly typed client router
- a very small UI-state store such as Zustand only for ephemeral interface state
- Radix primitives or equivalent accessible headless components
- CSS variables and a project-owned design system
- `dnd-kit` for Kanban drag and drop when the board is implemented
- a virtualized list library for long Channels and Threads

Do not put fetched server state into a giant global client store.

## 9.3 Database: PostgreSQL

PostgreSQL stores:

- identities and memberships
- Projects
- Conversations
- Messages
- Work Items
- read states
- Agent Runs
- normalized events
- jobs
- notifications
- GitHub metadata
- audit records

Use PostgreSQL full-text search initially. Do not add Elasticsearch.

## 9.4 Realtime and API

- HTTP JSON API for durable reads, writes, and actions
- WebSocket for realtime notifications and streaming
- MCP Streamable HTTP endpoint for local Codex
- JSONL over stdio for Codex App Server
- GitHub webhooks for repository events

## 9.5 Background work

Use a PostgreSQL-backed job table and worker loop.

Job claiming should use row locking such as `FOR UPDATE SKIP LOCKED`.

Do not introduce Redis only to hold a queue.

## 9.6 Object storage

Version one:

- local filesystem behind a storage interface

Later:

- S3-compatible storage

Large command logs, attachments, and generated artifacts should not be forced into ordinary Message rows.

---

# 10. Repository structure

Initial monorepo structure:

```text
agent-workspace/
├── PROJECT_SOURCE_OF_TRUTH.md
├── README.md
├── rust-toolchain.toml
├── Cargo.toml
├── package.json
├── pnpm-workspace.yaml
├── .editorconfig
├── .gitignore
├── .env.example
├── docker-compose.dev.yml
├── docs/
│   └── compatibility/
│       ├── codex-app-server.md
│       ├── mcp.md
│       ├── git-worktrees.md
│       └── github-app.md
├── apps/
│   ├── server/
│   │   ├── Cargo.toml
│   │   ├── migrations/
│   │   └── src/
│   │       ├── main.rs
│   │       ├── config.rs
│   │       ├── error.rs
│   │       ├── auth/
│   │       ├── organizations/
│   │       ├── projects/
│   │       ├── conversations/
│   │       ├── work_items/
│   │       ├── actors/
│   │       ├── execution_workers/
│   │       ├── providers/
│   │       │   └── codex_app_server/
│   │       ├── repositories/
│   │       ├── github/
│   │       ├── mcp/
│   │       ├── jobs/
│   │       ├── realtime/
│   │       ├── notifications/
│   │       ├── artifacts/
│   │       └── audit/
│   └── web/
│       ├── package.json
│       ├── vite.config.ts
│       └── src/
│           ├── app/
│           ├── routes/
│           ├── features/
│           ├── components/
│           ├── design-system/
│           ├── api/
│           └── realtime/
├── schemas/
│   └── codex-app-server/
├── tools/
│   ├── codex-bridge-spike/
│   └── mcp-spike/
└── .github/
    └── workflows/
```

Start with one Rust server crate. The `tools` directory may contain temporary technical spikes. Do not create ten internal crates before module boundaries become painful.

---

# 11. Backend module boundaries

## 11.1 Authentication and sessions

Responsibilities:

- Actor identities
- users
- secure login
- invite acceptance
- sessions
- logout and revocation
- optional identity linking later

Identity rules:

- each human user has one Actor used for project attribution and authorization
- a future external agent connection authenticates as its own non-human Actor
- execution workers and system actions also have attributable Actor identities
- credentials, browser sessions, and integration tokens never substitute for Actor IDs in domain records
- no request may choose an arbitrary `actor_id`; authentication resolves the Actor server-side

Recommended application session model:

- opaque session ID in an HttpOnly, Secure, SameSite cookie
- server-side session record containing only a hash of the presented token
- rotation after authentication and privilege changes
- absolute and inactivity expiration
- CSRF protection for state-changing browser requests
- no access tokens in browser local storage

For an early private build, one bootstrap admin account is acceptable.

Bootstrap rules:

- create the first account through a host-local CLI command, not an indefinitely exposed web setup route
- permit bootstrap only while no user exists
- read the initial password from an interactive TTY or another non-logging secret input
- store only an Argon2 password hash
- create the personal Organization, ownership membership, and first session-capable user in one transaction
- create the human Actor and its ownership membership in the same transaction
- ship no default username, password, or bootstrap token

Before team use, implement invite-based account creation and resolve O-004.

## 11.2 Organizations

Responsibilities:

- personal Organization creation
- Organization membership
- ownership
- eventual shared settings

The solo UI hides the Organization layer until needed.

## 11.3 Projects

Responsibilities:

- Project creation and archive
- Project key such as `TRI`
- Project instructions
- member roles
- navigation state
- repositories
- default Work Item workflow
- default Execution Worker

Every project-scoped database query must include `project_id` and pass authorization.

Project keys:

- are unique case-insensitively inside an Organization
- match `[A-Z][A-Z0-9]{1,9}`
- become immutable after the first Work Item key is issued

Project display names remain editable.

## 11.4 Conversations

Responsibilities:

- Channels
- Threads
- Messages
- mentions
- attachments
- reactions later
- read state
- Thread sidebar queries

Recommended model:

```text
conversations
├── id
├── project_id
├── kind: channel | thread
├── parent_channel_id: nullable
├── origin_message_id: nullable
├── repository_id: nullable
├── name
├── next_entry_sequence
├── archived_at
└── timestamps
```

Invariant:

- Channels have no parent.
- Threads have one parent Channel.
- A Thread cannot be the parent of another Thread.
- Channels have no repository binding.
- A Thread may bind to one repository; the binding becomes immutable after its first Agent Run starts.

Enforce the invariant in application code and with a database trigger or equivalent integrity check.

### Ordered conversation timeline

Messages and significant Agent Run events must share one durable ordering model so the client can paginate and render a Thread without merging unrelated timestamp queries.

Use a lightweight timeline projection:

```text
conversation_entries
├── id
├── conversation_id
├── sequence
├── kind: message | worker_event | artifact | system
├── message_id: nullable
├── agent_run_event_id: nullable
├── artifact_id: nullable
└── created_at
```

Rules:

- `sequence` is monotonically increasing and unique inside a Conversation
- allocate the next sequence transactionally from the parent Conversation
- a durable Actor-authored or consolidated Worker Message receives one entry
- only significant provider events rendered in the timeline receive entries
- streamed token deltas never receive individual timeline entries
- per-Actor read state stores the highest seen Conversation sequence
- completed-and-unseen state compares the completion entry sequence with the viewing Actor's last seen sequence
- timestamps remain metadata and are not the primary ordering key

## 11.5 Message content

Do not store mentions only as formatted text.

The Message `version` identifies the content-schema version, not an editable revision number. Sent Messages are immutable in version one so the application transcript cannot silently diverge from content already delivered to a provider session. A correction is a new Message.

Store versioned rich content:

```json
{
  "version": 1,
  "nodes": [
    {"type": "text", "text": "Please work on "},
    {"type": "mention", "actor_id": "..."},
    {"type": "text", "text": " for "},
    {"type": "work_item_ref", "work_item_id": "..."}
  ]
}
```

Also store a derived plain-text representation for search, notifications, and exports.

Every Message stores `author_actor_id`. Authentication supplies the author; clients cannot claim another Actor. Optional audit metadata may record the originating credential or client connection without exposing it in ordinary conversation UI.

## 11.6 Work Items

Responsibilities:

- keys
- types
- titles and descriptions
- statuses
- priorities
- assignment to an authorized human or external-agent Actor
- acceptance criteria
- comments and activity
- relations
- linked Conversations
- linked pull requests and commits

Work Item status definitions belong to the Project so workflows can be customized later.

Store `assignee_actor_id`, nullable. If a Project later requires a human sponsor, reviewer, or approver for agent-assigned work, model that as a separate responsibility or policy rather than misrepresenting who is assigned.

Each status has a stable ID, display name, order, and semantic category. Business logic and automation use the status ID or category, never the editable display name.

Initial semantic categories:

```text
backlog
unstarted
started
review
verification
completed
canceled
```

Default statuses:

```text
Backlog
Ready
In Progress
Review
Verification
Done
Canceled
```

## 11.7 Actors, Project memberships, and Execution Workers

Responsibilities:

- durable attribution identity
- human, external-agent, worker, integration, and system Actor kinds
- project-local display profile, handle, and avatar
- Project membership and role
- Execution Worker configuration and provider binding

Suggested core fields:

```text
actors
├── id
├── kind: human | external_agent | execution_worker | integration | system
├── status
└── timestamps

project_memberships
├── id
├── project_id
├── actor_id
├── role
├── display_name
├── handle
├── avatar_ref
├── enabled
└── timestamps
```

The version-one UI creates human memberships and one default Execution Worker. The schema accepts the `external_agent` Actor kind, but no external-agent credential or runtime connection ships until the later integration milestone.

Identity invariants:

- one human User references exactly one human Actor
- one Project membership exists at most once for a given `(project_id, actor_id)`
- active handles are unique case-insensitively inside a Project
- an Execution Worker's Actor has the reserved Worker role in the same Project
- an Execution Worker record, its Actor membership, and its Provider Connection agree on Project ownership
- disabling an Actor or membership prevents new attributable actions while preserving history
- historical Messages, Agent Runs, approvals, and audit events retain their Actor references after disablement

Suggested fields:

```text
execution_workers
├── id
├── project_id
├── actor_id
├── provider_connection_id
├── default_execution_profile_policy
├── enabled
└── timestamps
```

## 11.8 Worker provider connections

A Worker Provider Connection describes how an Execution Worker can execute.

Version one provider:

```text
kind = codex_app_server
host = local_project_host
```

The provider boundary should expose capabilities rather than pretending all providers are identical.

Example capabilities:

```text
supports_streaming
supports_commands
supports_file_changes
supports_approvals
supports_questions
supports_resume
supports_interrupt
supports_pull_request_creation
supports_remote_execution
```

This capability interface is only for bounded execution providers. It is not the future external-agent connection interface.

## 11.9 Execution profiles

Responsibilities:

- provider authentication
- isolated Codex home directory
- provider configuration
- ownership
- availability
- rate-limit metadata

Each Codex Execution Profile receives a separate `CODEX_HOME` directory.

Version-one solo behavior:

- one personal Execution Profile
- device-code or browser login on the Project Host
- one supervised App Server process

Team-safe behavior later:

- no sharing a person's ChatGPT credential file among teammates
- personal profiles remain personal
- a project service profile uses an appropriate project-owned API key or organization-managed access mechanism
- a Thread records the Execution Profile that owns its provider session

## 11.10 Agent Runs and provider sessions

Separate the application Thread from the provider session.

```text
Application Thread
├── Actor-authored and Worker Messages
├── linked Work Items
├── linked artifacts
└── Agent Runs
    └── Provider Session reference
```

Suggested objects:

```text
agent_runs
provider_sessions
agent_run_events
agent_approvals
```

An Agent Run records:

- Execution Worker
- application Thread
- requesting Actor
- Execution Profile
- Provider Session, nullable until start or resume succeeds
- selected repository
- repository workspace, nullable until the run starts
- state and timestamps

The requesting Actor, Execution Worker, Execution Profile, and selected repository are fixed when the run is created. The repository workspace binding becomes immutable once the run enters `starting`. A retry creates a new Agent Run even when it safely reuses the Provider Session or worktree.

The requester is stored as `requesting_actor_id`, never only as a user ID. The run also preserves a snapshot of the authorization decision and delegated execution scope needed for audit and recovery. A worker cannot enlarge that scope merely because its provider can technically perform a broader action.

Technical-alpha scheduling:

- default to one active provider turn per Execution Profile
- queue additional Agent Runs durably in creation order
- apply a configurable global host concurrency ceiling
- never rely on App Server process concurrency as the product scheduler

The per-profile limit may increase after contract tests demonstrate safe concurrent turns, correct notification routing, and acceptable account-rate behavior.

A single application Thread can have several historical runs. Version one permits one active run at a time.

## 11.11 Git and worktree manager

Responsibilities:

- repository registration
- clone or fetch
- base branch resolution
- branch naming
- worktree creation
- worktree ownership
- status inspection
- commit and push
- cleanup and repair

Use the Git command-line executable through direct argument arrays. Do not concatenate untrusted strings into shell commands.

Recommended branch names:

```text
agent/TRI-142-repository-selector-ab12cd
agent/thread-ab12cd-auth-investigation
```

Worktree behavior:

- substantial hosted work receives a dedicated branch worktree
- if O-002 later permits repository-aware inline reads, that contract may use an ephemeral detached read-only worktree
- failed dirty worktrees are preserved for inspection
- merged or abandoned clean worktrees are eligible for cleanup
- submodule repositories require a documented fallback because worktree support is less complete

## 11.12 GitHub integration

Use a GitHub App rather than long-lived personal access tokens for the real team product.

Responsibilities:

- installation onboarding
- selected repository access
- webhook signature verification
- installation token creation
- pull request metadata
- review state
- check status
- merge state
- branch and commit linkage

Webhook deliveries must be idempotent. Store the GitHub delivery ID with a unique constraint.

## 11.13 MCP server

The MCP endpoint allows local Codex clients to interact with Agent Workspace.

Initial development authentication:

- scoped bearer token

Team-ready authentication:

- OAuth
- per-Actor and per-client scopes
- revocation

Initial read tools:

```text
project_get
project_instructions_get
work_item_get
work_item_search
work_item_context_get
repository_get
pull_request_get
```

Initial write tools:

```text
work_item_set_status
work_item_post_update
work_item_update
work_item_create_subtask
work_item_link_pull_request
work_item_mark_blocked
work_item_mark_ready_for_review
```

MCP tools must be domain operations, not arbitrary database mutation.

Read tools prefer immutable IDs or canonical Agent Workspace URLs. A human-readable Work Item key is accepted only with enough authenticated Organization or Project context to resolve it unambiguously.

Non-commutative write tools require an expected Work Item version or equivalent concurrency token to avoid overwriting a teammate's newer edit.

Every MCP write is audited as:

```text
actor: authenticated Actor
channel: MCP
client: Codex
```

Version-one MCP setup authenticates local Codex on behalf of a human Actor. A later external agent connection may reuse MCP domain tools or use an equivalent HTTP integration surface, but it must resolve to its own Actor rather than impersonating its owner.

## 11.14 Realtime module

Responsibilities:

- authenticated WebSocket
- project-scoped subscriptions
- message delivery
- Work Item updates
- Thread state updates
- agent stream events
- notification badges
- reconnect and resynchronization

The WebSocket is not the source of truth. If an event is missed, the client refetches durable state.

Browser WebSocket rules:

- authenticate the upgrade with the server-side browser session
- validate the `Origin` header against the configured application origins
- authorize every Project subscription
- carry realtime delivery and ephemeral stream control only
- perform durable product mutations through authenticated HTTP action endpoints
- close or rescope subscriptions when membership or session authorization changes

## 11.15 Jobs and transactional outbox

Use two persistent mechanisms:

### Jobs

For work that must execute or retry:

- start provider session
- fetch repository
- create worktree
- process webhook
- clean worktree
- refresh GitHub data

Jobs use at-least-once execution. A job record carries:

- versioned kind and payload
- state
- attempt count and maximum attempts
- `available_at`
- lease owner and `lease_expires_at`
- optional idempotency or deduplication key
- last error summary

Workers claim ready jobs transactionally with `FOR UPDATE SKIP LOCKED`, commit the lease, and perform the external work outside the claim transaction. Handlers must tolerate retry after an uncertain outcome. Exhausted jobs remain durably failed and visible to operators; they are not silently discarded.

### Transactional outbox

When a database transaction changes durable product state, it also writes an outbox event in the same transaction. A worker publishes that event to the realtime delivery layer and records `published_at`.

This prevents "database changed but WebSocket notification vanished" failures.

`published_at` means that the server processed the event for broadcast. It does not claim that every browser received or acknowledged it.

Version one does not require per-client event replay. On reconnect, the client refetches durable navigation state and the currently open resource. Durable notifications and per-Actor read state preserve attention even when a realtime event is missed.

Outbox publication is at least once. Clients tolerate duplicate realtime events by `event_id`, and handlers refetch or apply version-aware updates rather than assuming every notification is unique or ordered across reconnects.

## 11.16 Notifications

Notifications are durable and addressed to an Actor or Project role. Version-one delivery and unread UI are implemented for human users. Future external agent connections decide through their own integration contract whether to poll, subscribe to, or ignore addressed events.

Initial notification types:

- mention
- Worker blocked for Actor
- Worker completed unseen
- Worker failed
- review requested
- pull request changed

Email and push notifications are later features. In-app notifications come first.

## 11.17 Audit

Audit records capture important security and collaboration events:

- login and logout
- member invitation and role change
- Actor credential creation, rotation, revocation, and connection disablement
- repository connection
- Agent Run request and execution start
- approval decision
- MCP write
- Work Item status change
- GitHub link change
- provider profile authentication change

Audit logs are not a replacement for ordinary activity feeds.

Each record identifies the initiating Actor, authenticated credential or session class, Project, action, target resource, outcome, request or correlation ID, and timestamp. Sensitive credential material is never recorded.

## 11.18 Artifacts

Artifacts provide durable links from project work to outputs without copying the source system's complete data model.

Responsibilities:

- artifact kind and lifecycle metadata
- links to Agent Runs, Conversations, and Work Items
- external identifiers and URLs
- local generated-file metadata behind the storage interface
- deduplication of repeated provider and GitHub observations

Pull requests, commits, checks, and deployments retain their specialized records. An Artifact may reference those records so ordinary project views can render a common linked-output model.

---

# 12. Core data model

The initial schema should include these primary tables or equivalent structures.

## Identity and tenancy

```text
actors
users
user_password_credentials
organizations
organization_memberships
sessions
invitations
```

`users` represent human login accounts and reference a human Actor. Domain authorship and authorization reference `actors.id`, not `users.id`.

## Projects and members

```text
projects
project_memberships
project_navigation_states
project_instructions
project_counters
```

Project memberships reference Actors. Version one creates memberships for human Actors and the default Execution Worker. The Actor kind and membership shape reserve compatibility for later external agent members.

## Conversations

```text
conversations
conversation_entries
messages
message_mentions
conversation_read_states
attachments
```

## Work

```text
work_item_types
work_item_statuses
work_items
work_item_relations
work_item_conversation_links
work_item_repository_links
work_item_comments
work_item_activity
artifacts
artifact_conversation_links
artifact_work_item_links
```

## Execution workers

```text
execution_workers
provider_connections
execution_profiles
provider_sessions
agent_runs
agent_run_events
agent_approvals
agent_run_acknowledgements
```

Future external-agent integration adds credential and connection records only when its protocol is designed. Do not add a placeholder secret-bearing `external_agent_connections` table in version one merely to make the architecture look generic.

## Git and GitHub

```text
repositories
repository_workspaces
git_worktrees
pull_requests
commits
check_runs
webhook_deliveries
```

## Infrastructure

```text
jobs
outbox_events
notifications
audit_events
```

## Actor foreign-key rules

Use Actor references wherever the domain means “who,” including:

```text
project_memberships.actor_id
messages.author_actor_id
conversation_read_states.actor_id
work_items.assignee_actor_id
work_item_comments.author_actor_id
work_item_activity.actor_id
agent_runs.requesting_actor_id
agent_approvals.requesting_actor_id
agent_approvals.responded_by_actor_id
agent_run_acknowledgements.actor_id
notifications.recipient_actor_id
audit_events.actor_id
```

Use `user_id` only when the record specifically describes a human account, browser session, human-only preference, or authentication ceremony. Do not use a polymorphic `user_or_agent_id` pair.

## ID strategy

Use application-generated UUIDv7 values for internal IDs.

Use readable project-scoped keys for Work Items:

```text
TRI-142
ASTRA-37
```

Generate the numeric portion transactionally with a per-Project counter.

The stored Work Item ID is canonical. The readable key is a stable lookup and display value; references in Messages store the Work Item ID rather than reparsing the rendered key.

Use a database sequence or another transactionally ordered cursor for outbox publication. Do not use timestamp ordering as a delivery cursor.

## Versioning and optimistic concurrency

Mutable records such as Work Items should have a version integer.

Non-commutative updates from the web client and MCP require the expected version. A conflicting update returns the current version and a conflict response rather than silently overwriting newer work.

Append-only actions such as posting a new Message use an idempotency key instead of a record version.

## Archive rather than delete

Use `archived_at` for Projects, Channels, Threads, and Work Items where history matters.

Hard deletion is reserved for security-sensitive account removal, retention policy, or administrator cleanup.

Archive guards:

- a Project, Thread, or repository cannot be archived or disabled while it owns a non-terminal Agent Run
- archiving a Channel hides it from ordinary navigation but preserves its child Threads and history
- archived Conversations reject new Messages and Agent Runs
- archived Work Items retain links, activity, and artifacts
- restore clears `archived_at` only after authorization and invariant checks

---

# 13. State machines

## 13.1 Agent Run state

Internal lifecycle:

| State | Allowed next states |
|---|---|
| `created` | `queued`, `starting`, `failed`, `canceled` |
| `queued` | `starting`, `failed`, `canceled` |
| `starting` | `working`, `blocked`, `recovering`, `failed`, `canceled`, `interrupted` |
| `working` | `blocked`, `recovering`, `completed`, `failed`, `canceled`, `interrupted` |
| `blocked` | `working`, `recovering`, `failed`, `canceled`, `interrupted` |
| `recovering` | `working`, `blocked`, `failed`, `canceled`, `interrupted` |
| `completed` | none |
| `failed` | none |
| `canceled` | none |
| `interrupted` | none |

Terminal states:

```text
completed
failed
canceled
interrupted
```

Non-terminal states:

```text
created
queued
starting
working
blocked
recovering
```

Rules:

- a terminal Agent Run never transitions back to a non-terminal state
- retry creates a new Agent Run and may reuse the same Provider Session and worktree when safe
- interruption is terminal even when the user immediately retries
- at most one non-terminal Agent Run exists per Thread
- every transition uses one centralized domain function and writes a durable activity or run event in the same transaction

Queueing is shown with the ordinary active treatment and explicit `Queued` text. It does not receive a separate permanent sidebar color.

## 13.2 Thread attention state

Thread attention is derived from the newest Agent Run plus per-Actor read and acknowledgement state. Version-one navigation renders it for human Actors.

Priority for the current user:

1. blocked for this user
2. active: created, queued, starting, working, or recovering
3. newest run failed and is unacknowledged
4. completed and unseen
5. inactive and seen

The interface maps those to orange, blue, red, white, and gray.

Canceled and interrupted runs use the inactive treatment after the user has seen them. They retain explicit text and icons in the Thread history. A user-initiated cancellation does not create a failure notification.

An older failed run may retain a durable notification until acknowledged, but it does not override the color of a newer active or completed run.

## 13.3 Work Item state

Default transition model:

| Status | Allowed next statuses |
|---|---|
| Backlog | Ready, Canceled |
| Ready | Backlog, In Progress, Canceled |
| In Progress | Ready, Review, Canceled |
| Review | In Progress, Verification, Done, Canceled |
| Verification | In Progress, Done, Canceled |
| Done | In Progress |
| Canceled | Backlog |

`Canceled` can be reached from any non-closed state. Reopening Done or Canceled is an explicit user action and creates a visible activity entry.

Worker completion does not automatically mean Work Item completion.

Recommended automations:

- work begins: optionally move to In Progress
- pull request opens: optionally move to Review
- pull request merges: move a linked code Work Item to Verification by default

These automations must be configurable, may be disabled per Project, and must create visible activity entries. They never reopen Done or Canceled work and never force an invalid custom-workflow transition. A human or explicit Project rule moves Verification to Done.

## 13.4 Provider session state

```text
new
active
idle
archived
unavailable
```

Provider session state is not directly rendered as Thread color.

---

# 14. Codex App Server integration

## 14.1 Why App Server

The product needs a rich Codex client integration with:

- authentication
- durable provider Threads
- streamed events
- commands
- file changes
- approvals
- questions
- resume and interrupt

Use Codex App Server rather than treating Codex as a single prompt-response API.

## 14.2 Transport

Use the default stdio transport:

```text
Rust supervisor
   ├── stdin: JSONL requests
   ├── stdout: JSONL responses and notifications
   └── stderr: provider diagnostics
```

Do not expose App Server directly to the public internet.

## 14.3 Process model

Version one:

- one supervised App Server process per active Execution Profile
- restart with bounded exponential backoff
- health and authentication state visible in settings
- request IDs mapped to Rust oneshot channels
- notifications routed by provider Thread ID
- spawn with an explicit environment allowlist rather than inheriting the complete Rust server environment
- provide only the profile-specific `CODEX_HOME` and required non-secret runtime variables
- place the process in a supervisable process group or platform-equivalent job boundary
- on shutdown, attempt bounded graceful termination and then stop the full child process tree
- bound individual protocol-line and stderr-buffer sizes so malformed provider output cannot exhaust server memory

## 14.4 Authentication setup

The Project Host setup screen must support:

- browser ChatGPT login
- device-code ChatGPT login for headless hosts
- API key login when selected
- logout
- account status
- rate-limit status

Each Execution Profile has its own `CODEX_HOME`.

Credential files are treated as secrets and never sent to the web browser or stored in PostgreSQL.

## 14.5 Protocol versioning

Pin a tested Codex version for each release of Agent Workspace.

During CI or release preparation:

```text
codex app-server generate-json-schema --out ./schemas/codex-app-server/<version>
```

Check the generated schema into the repository.

Maintain a small provider adapter around the exact methods the product uses. Do not scatter raw JSON-RPC method names throughout the codebase.

Stable APIs are used by default. Experimental App Server features require a deliberate feature flag and compatibility test.

Before changing the pinned Codex version, contract tests must create a Provider Session with the previous supported version and attempt to read or resume it with the candidate version. If cross-version resume is not safe, the release notes and UI must preserve the old transcript and offer an explicit new Provider Session rather than silently corrupting or discarding history.

## 14.6 Application Thread mapping

Initial mapping:

- one hosted Execution Worker application Thread maps to one durable Codex provider Thread
- multiple application messages become multiple Codex turns
- multiple historical Agent Runs may refer to the same provider Thread
- provider Thread ID is stored in `provider_sessions`

## 14.7 Event normalization

Normalize provider events into a small common event model:

```text
agent_message_delta
agent_message_completed
command_started
command_output
command_completed
file_change
approval_requested
approval_resolved
question_requested
question_resolved
test_result
artifact_created
run_completed
run_failed
```

Retain raw provider payloads only when needed for debugging, behind a bounded retention policy and diagnostic authorization. Apply size limits and known-secret redaction before persistence. Raw payloads never become ordinary user-visible Messages automatically.

Do not store every streamed token as a separate PostgreSQL row. Stream deltas live, then persist the consolidated message.

Treat command output as potentially sensitive and hostile display content. Apply size bounds, strip unsafe terminal control sequences, escape it in the browser, and redact known application-owned secrets before durable persistence. The product cannot guarantee detection of arbitrary repository secrets, so diagnostic access and retention remain bounded.

Provider notification handling is idempotent. Persist a provider event identity or adapter-generated deterministic key where the protocol permits it, and make consolidated Message completion safe to retry after reconnect or process restart.

## 14.8 Approvals and blockers

A provider approval or question creates an `agent_approval` record.

It includes:

- requesting Agent Run
- provider request ID
- type
- summary
- structured options
- eligible Actor, Project role, or policy group
- whether separation of duty or a human decision is required
- status
- response
- timestamps

The related Thread becomes orange for viewing human Actors who can answer and have not been superseded by another answer. Future external-agent delivery follows its connection contract; the durable approval record and eligibility policy are transport-independent.

Approval response rules:

- recheck the responder's current authorization at response time
- accept the first valid response with an atomic pending-to-resolved transition
- return the durable resolved state for an idempotent repeat by the same action
- reject a conflicting later response without forwarding it to the provider
- reconcile provider-side resolution notifications even when they arrive before the browser response completes

## 14.9 Failure and recovery

On App Server crash:

1. mark active runs as recovering
2. restart the provider process
3. attempt to resume the stored provider Thread
4. inspect the worktree and Git state
5. either continue or mark the run interrupted with a clear recovery action

Never delete a dirty worktree automatically after a provider crash.

---

# 15. Git and worktree execution model

## 15.1 Repository registration

A Project links one or more GitHub repositories.

Store:

- GitHub repository ID
- owner and name
- clone URL
- default branch
- GitHub App installation ID
- local mirror path
- enabled state

### Repository selection for Agent Runs

A code-changing hosted Agent Run owns exactly one repository workspace in version one.

Selection is deterministic:

1. use the existing Thread repository binding
2. otherwise use the repository explicitly selected by the user for the invocation
3. otherwise use the linked Work Item's primary repository
4. otherwise use the Project repository when exactly one enabled repository exists
5. otherwise require the user to choose before the Agent Run is created

Do not ask the model to guess a repository.

A Work Item may link several repositories but has at most one primary repository. A Thread linked to Work Items with different primary repositories has no implicit repository choice.

The first code-changing Agent Run binds its application Thread to the selected repository in the same transaction that creates the run. This keeps the durable Codex Provider Session, worktree history, and application conversation in one repository context.

If an invocation explicitly selects a repository that conflicts with an existing Thread binding, reject it with a `Start a new Thread` action.

Cross-repository work uses separate application Threads, Agent Runs, and worktrees in version one. The Work Item may link those Threads and resulting artifacts together.

## 15.2 Repository storage

Recommended host layout:

```text
repositories/<repo-id>/mirror.git
worktrees/<repo-id>/<thread-or-run-id>/
```

Use one shared repository object database and linked worktrees where practical.

## 15.3 Worktree creation

Keep logical and physical workspace records separate:

- `repository_workspaces` owns the repository, application Thread, branch name, base commit, and lifecycle across retries
- `git_worktrees` records the generated physical path, creation and inspection state, and the owning logical workspace

Version one permits at most one live physical Git worktree per logical repository workspace.

For a Work Item:

```text
base branch: repository default or selected branch
branch: agent/<work-item-key>-<slug>-<short-thread-id>
worktree: generated host path
```

For an unlinked Thread:

```text
branch: agent/thread-<short-id>-<slug>
```

Rules:

- the database stores the exact base commit used to create the workspace
- branch uniqueness comes from a stable opaque ID suffix, never from a title slug alone
- one logical repository workspace owns one branch inside a repository
- a retry in the same Thread may reuse its branch and worktree only after Git state inspection
- a new concurrent Thread or a run after a merged branch receives a new unique branch

## 15.4 Concurrency

Use a short repository-scoped lock around operations that mutate shared Git metadata:

- fetch
- branch creation
- worktree add/remove
- prune and repair

Do not hold that lock during the full Codex execution.

## 15.5 Cleanup policy

A worktree can be automatically cleaned only when:

- no active Agent Run owns it
- it is clean
- the associated pull request is merged, closed, or explicitly abandoned
- the retention window has elapsed

Dirty or failed worktrees require explicit cleanup or a safe preservation policy.

## 15.6 Security

- run Git with argument arrays, not shell interpolation
- validate branch and path components
- treat repository hooks and configuration as potentially executable
- run the Project Host only for trusted repositories and trusted collaborators in version one
- use sandbox and approval policies for provider commands
- treat a worktree as concurrency and change isolation, not as a security boundary
- start every hosted Codex turn with its generated worktree as `cwd`
- restrict provider writable roots to the owned worktree and explicitly approved temporary paths
- do not expose other Projects' repositories, worktrees, attachments, or Execution Profile directories as writable roots
- execute authenticated push and pull-request operations in the backend integration layer; do not place GitHub installation tokens in the provider environment
- never pass browser session secrets, database credentials, GitHub App private keys, or MCP bearer tokens to Codex subprocesses
- run credentialed Git commands with a sanitized environment and known Git executable
- disable repository hooks for backend-owned credentialed operations
- ignore repository-controlled credential helpers and unsafe global Git configuration during credentialed operations
- verify that the authenticated remote uses the expected GitHub HTTPS host and registered repository identity
- provide the short-lived installation token through an ephemeral credential callback or askpass mechanism, never in a remote URL, command-line argument, durable Git config, or log
- remove any temporary credential helper material immediately after the Git process exits

The version-one trusted-host model still means the dedicated service account can operate the application and provider files it owns. Separate `CODEX_HOME` directories prevent accidental credential reuse; they are not equivalent to separate operating-system users or hostile-tenant isolation.

---

# 16. GitHub integration model

## 16.1 GitHub App permissions

Request the narrowest practical permissions.

The initial expected repository permissions are:

- Metadata: read
- Contents: read and write for clone, fetch, branch push, and commit lookup
- Pull requests: read and write
- Checks: read
- Actions: read only if the checks UI cannot be implemented from Checks data alone

Subscribe only to the webhook events listed below. Exact permissions and events must be confirmed during the GitHub delivery spike and documented in the checked-in compatibility note.

## 16.2 Installation tokens

The backend generates short-lived GitHub App installation tokens when needed.

Do not store installation tokens as durable secrets.

Installation tokens are scoped to the registered installation and repository set. The backend injects them only into the credentialed Git or GitHub API child operation that needs them and never into an Agent Run process.

## 16.3 Webhooks

Initial events:

- installation and repository access changes
- push
- pull request opened, updated, closed, merged
- review requested and submitted
- check suite and check run changes

Webhook processing:

1. verify signature
2. record delivery ID idempotently
3. enqueue processing job
4. update durable state
5. write outbox events
6. notify clients

## 16.4 Pull request linking

Link pull requests through one or more signals:

- explicit API or MCP call
- Agent Run artifact creation
- Work Item key in branch name
- Work Item key in pull request body

Explicit links win over heuristic links.

## 16.5 Review behavior

The app displays:

- pull request title and state
- checks
- review status
- merge status
- important activity

Detailed code review remains on GitHub or in a later dedicated view.

---

# 17. Local Codex and MCP integration

## 17.1 Open in Codex

The Work Item action constructs a Codex deep link using:

- `originUrl` for Git remote matching
- `prompt` for initial composer text

Conceptual prompt:

```text
Use the Agent Workspace MCP server.
Load work item TRI-142 using its canonical Agent Workspace URL or immutable ID, then load the related project instructions.
Work in this local repository.
Set the item to In Progress when meaningful work begins.
Post only useful progress updates.
Link the pull request and mark the item ready for review when complete.
```

The prompt is prefilled, not automatically sent.

Do not rely on a human-readable Work Item key being globally unique across every Organization the user can access.

## 17.2 No automatic Thread

The local workflow updates the Work Item directly through MCP. It does not require or create an application Thread.

A user may separately create or link a Thread later if discussion is useful.

## 17.3 MCP setup

Local Codex clients connect to a Streamable HTTP MCP URL exposed by the Project Host.

Development setup may use a bearer token stored in an environment variable.

Team-ready setup should use OAuth with user-specific scopes.

## 17.4 MCP instructions

The MCP initialization instructions should concisely explain:

- how to identify Projects and Work Items
- when to update status
- when not to create noise
- concurrency behavior
- how to link pull requests
- permission limits

The first portion must contain the most important rules because models may use it while deciding whether to call a tool.

## 17.5 Tool design

Read tools should be marked read-only. Write tools should carry accurate destructive and idempotency metadata where supported.

Avoid one giant `update_work_item` tool for every operation. Use specific tools for common transitions and a constrained general update tool for fields that genuinely need it.

## 17.6 Conflict handling

If a local Codex session tries to update an outdated Work Item version:

- reject with conflict
- return the current version and changed fields
- let Codex ask the user or retry with an informed update

Do not silently overwrite a teammate's work.

---

# 18. Frontend architecture

## 18.1 Routing

Conceptual routes:

```text
/home
/projects/:projectId
/projects/:projectId/channels/:channelId
/projects/:projectId/threads/:threadId
/projects/:projectId/work
/projects/:projectId/work/:workItemId
/projects/:projectId/repositories
/projects/:projectId/members
/projects/:projectId/settings
/settings
```

## 18.2 State ownership

Use TanStack Query for data fetched from the server.

Use local component state or a small ephemeral store for:

- panel widths
- open drawers
- composer state
- local filters before persistence
- command palette state

Do not duplicate server entities in multiple frontend stores.

## 18.3 Realtime behavior

The WebSocket sends typed events containing:

```json
{
  "event_id": "...",
  "event_cursor": 1842,
  "version": 1,
  "project_id": "...",
  "type": "thread.agent_run.updated",
  "occurred_at": "...",
  "payload": {}
}
```

On reconnect:

- reconnect with backoff
- refetch active project navigation and currently open resource
- refetch durable notifications and attention counts
- do not depend on replaying missed WebSocket events for correctness

`event_cursor` is useful for diagnostics and a possible later replay protocol. Version one clients do not treat it as a guarantee that every intermediate event remains replayable.

## 18.4 Message rendering

Messages render from the versioned content AST.

Human and future external-agent Messages use the same ordinary Message model and chronological timeline. The author profile may identify the Actor kind without making external-agent conversation look like provider telemetry. Disabled or disconnected Actors retain their historical names and attribution.

Execution Worker provider events render as cards or expandable rows:

- command
- file change
- approval
- question
- test result
- pull request

Large output is collapsed and lazy-loaded.

## 18.5 Performance rules

- virtualize long Channels and Threads
- paginate historical Conversation entries
- paginate Work Items
- lazy-load diff and code viewers
- coalesce token deltas before React renders
- do not subscribe to full event streams for inactive Projects
- cache Project navigation metadata
- use optimistic updates for ordinary messages and simple Work Item changes
- keep the initial route bundle small

## 18.6 Responsive behavior

Desktop is the primary experience.

On narrow screens:

- project rail and sidebar become drawers
- main workspace remains primary
- Context Drawer becomes a sheet
- heavy diff views may open in a dedicated screen

The web app should remain usable from a phone for messaging, blockers, Work Item updates, and review status.

## 18.7 PWA

Add installability and web push only after the core web experience is stable.

Do not let service-worker complexity block the initial product.

---

# 19. API design

## 19.1 General rules

- base path `/api/v1`
- JSON requests and responses
- typed action endpoints for meaningful transitions
- cursor pagination
- structured error envelope
- idempotency keys for retried agent and integration actions
- optimistic concurrency on mutable collaborative records

## 19.2 Example endpoints

```text
POST   /api/v1/projects
GET    /api/v1/projects/:id
GET    /api/v1/projects/:id/navigation

POST   /api/v1/projects/:id/channels
POST   /api/v1/channels/:id/messages
POST   /api/v1/channels/:id/threads
GET    /api/v1/conversations/:id/timeline

POST   /api/v1/projects/:id/work-items
PATCH  /api/v1/work-items/:id
POST   /api/v1/work-items/:id/transition
POST   /api/v1/work-items/:id/links/threads

POST   /api/v1/threads/:id/agent-runs
POST   /api/v1/agent-runs/:id/interrupt
POST   /api/v1/agent-approvals/:id/respond

POST   /api/v1/integrations/github/webhook
POST   /api/v1/mcp
```

## 19.3 Authorization

Do not spread string role comparisons throughout handlers.

Use centralized action checks:

```text
project.view
project.manage
member.invite
channel.create
message.send
work_item.edit
worker.invoke
approval.respond
repository.manage
execution_profile.manage
```

Handlers ask whether the authenticated Actor may perform an action on a resource. The policy input may include the requester's Project role, resource relationship, delegated run scope, approval addressee, and separation-of-duty requirement. It must not branch on “human versus agent” when the policy itself does not require that distinction.

Every state-changing integration request requires an idempotency key. Authentication resolves the Actor server-side; an `actor_id` in a client payload is never trusted as authorship.

## 19.4 Future external-agent integration surface

The later Hermes, OpenClaw, and external-agent milestone should expose the same application command layer used by the web backend rather than a privileged parallel API.

Candidate transports include MCP and ordinary HTTP APIs. The milestone must decide authentication, credential registration, event delivery or polling, presence semantics, cursor behavior, and connection diagnostics before publishing a stable contract.

Regardless of transport:

- the connection authenticates as exactly one external-agent Actor
- Project access comes from that Actor's memberships and policies
- reads are Project-scoped
- writes are typed domain actions, not arbitrary database mutation
- Messages are authored by the authenticated Actor
- retries are idempotent
- revocation takes effect without restarting the external runtime
- an authorized Actor may invoke an Execution Worker through the ordinary Agent Run action

No external-agent endpoint or credential flow is part of version one.

---

# 20. Team model

## 20.1 Roles

Initial Project roles:

```text
Owner
Admin
Member
Viewer
Worker
```

Roles map to actions through centralized policy. `Worker` is a reserved role for an Execution Worker and is always intersected with the immutable scope delegated to its active Agent Run. Human and external-agent members may receive the ordinary Owner, Admin, Member, or Viewer roles.

Roles are assigned to Actor memberships. Human and future external-agent Actors use the same role-to-action engine. Authentication method and Actor kind do not silently grant additional Project authority.

Baseline version-one policy:

| Action family | Owner | Admin | Member | Viewer | Worker |
|---|---:|---:|---:|---:|---:|
| View Project content | Yes | Yes | Yes | Yes | Delegated scope |
| Create Channels, Threads, Messages, and Work Items | Yes | Yes | Yes | No | Bound-Thread Messages only |
| Edit ordinary Project work | Yes | Yes | Yes | No | Delegated operations only |
| Invoke an Execution Worker | Yes | Yes | Yes | No | No |
| Answer an approval addressed to the Actor or their role | Yes | Yes | Yes | No | No by default |
| Invite or remove non-owner members | Yes | Yes | No | No | No |
| Manage repositories and Execution Worker bindings | Yes | Yes | No | No | No |
| Manage shared Execution Profiles and Project security settings | Yes | Yes | No | No | No |
| Transfer ownership | Yes | No | No | No | No |
| Remove the final owner | No | No | No | No | No |

An Actor must satisfy the role action, resource-specific condition, and any delegated capability scope. For example, a Member cannot answer an approval addressed only to another Actor, and an Execution Worker cannot use its provider process to escape the Project, repository, Thread, or operation scope of its Agent Run.

## 20.2 Invitations

Initial team onboarding:

- Owner or Admin creates a one-time invitation
- invitation may be copied as a link before email delivery exists
- invitee creates or links an account
- invitation expires and is single-use
- invitation tokens are high entropy and only a hash is stored
- acceptance validates and consumes the invitation in one transaction

## 20.3 Per-Actor state

The underlying ownership keys for the following must be Actor-compatible from the start:

- read state
- unseen Thread completion
- failed-run acknowledgement
- Project navigation state
- notifications
- drafts where stored

Version one creates and renders this state for human Actors. A future external agent connection may maintain a conversation cursor or notification subscription through its own protocol without changing message authorship or the authorization model.

## 20.4 Execution Workers and team credentials

This is a deliberate design boundary.

An Execution Worker is shared as a visible project resource and attributable Actor, but a Provider Session belongs to one Execution Profile.

Version-one solo mode uses the owner's personal profile.

Before true team-hosted agent execution ships, choose one safe policy:

1. personal invoker profile, with clear ownership and continuation limits
2. project service profile using project-owned API credentials
3. organization-managed Codex access mechanism

Do not share a personal ChatGPT credential file among team members.

## 20.5 External agent members are external clients

A future Hermes, OpenClaw, or similar connection is registered by an authorized human and authenticates as its own Actor. Removing its Project membership or disabling its connection immediately prevents new reads, writes, subscriptions, and Worker invocations.

Agent Workspace does not:

- run or supervise the external agent model
- own its memory or prompt stack
- create its schedules or automations
- impose application-level reasoning-loop or session-duration controls
- accept responsibility for its provider billing or internal runtime security

Agent Workspace does enforce the same Project authorization, approval, validation, rate-limit, immutable-message, audit, and resource-isolation rules it enforces for any other client.

## 20.6 Trusted-team boundary

The first team release assumes trusted collaborators with authorization to work on the linked repositories.

Supporting hostile multi-tenant users who can submit arbitrary code to a shared execution host is a separate security project.

---

# 21. Security model

## 21.1 Primary risk

The Project Host runs agent-generated commands against source repositories. This is the defining security risk of the system.

Repository and conversation content may influence agent behavior. Trusting a repository does not make every string inside it safe. Sandboxing, scoped writable roots, approval policy, secret separation, and backend-owned GitHub credentials remain required.

## 21.2 Mandatory controls

- dedicated non-root service user
- registered repository allowlist
- generated worktree paths
- provider sandboxing
- approval policy for elevated commands and network access
- centralized authorization
- per-user sessions
- server-resolved Actor identity for every authenticated request
- scoped, revocable credentials for non-browser clients
- CSRF protection
- secure cookies
- restrictive Content Security Policy and safe response headers
- rate limits on authentication and write endpoints
- webhook signature verification
- short-lived GitHub installation tokens
- encrypted or OS-protected secret storage where practical
- audit logging
- no shell interpolation
- strict path canonicalization
- bounded log and message sizes
- attachment content-type and size validation
- idempotency enforcement for retried integration writes
- immutable authorship and requester attribution

## 21.3 Secrets

Secrets include:

- Codex auth cache
- API keys
- GitHub App private key
- OAuth client secrets
- session secrets
- MCP tokens
- future external-agent connection credentials

Rules:

- never return secrets through normal APIs
- never write secrets to application logs
- never commit them
- store file-based secrets with owner-only permissions
- support revocation and rotation

## 21.4 Repository trust

Git configuration and hooks may execute code. Version one should only connect repositories trusted by the host owner.

## 21.5 Actor and Worker approvals

Approval policy applies to actions regardless of whether the requesting Actor is human, external agent, integration, or execution worker. Approval requests name an eligible Actor, Project role, or policy group rather than assuming that the requester is a user.

Approval UI must clearly distinguish:

- ordinary shell execution
- destructive commands
- network access
- filesystem scope expansion
- Git push or PR creation where relevant

The backend records the requesting Actor, action and resource scope, eligible approvers, deciding Actor, decision, and timestamp.

Where a policy requires separation of duty, the requesting Actor cannot approve its own action. A future external agent member may answer an approval only when it is an eligible Actor and the policy does not require a human or a distinct approver.

## 21.6 Messages and connected Actor security

Messages from all Actors are untrusted collaborative content, not application commands. Mentions, Work Item references, and other structured nodes are validated against the authenticated Actor's Project access.

Rules:

- the server derives `author_actor_id` from the authenticated session, credential, delegated run, or system job
- clients cannot submit another Actor as the author
- the versioned Message AST uses an allowlist and does not accept arbitrary HTML
- rendering escapes text, permits only approved URL schemes, and opens external links with safe isolation attributes
- sent Messages are immutable in version one; corrections are new attributed Messages
- state-changing requests use idempotency keys so client retries do not duplicate Messages, Work Items, or Worker runs
- payload, attachment, and rendered-output limits apply to every client
- attachments are served with validated media types, `nosniff`, and safe disposition behavior
- provider output and external-agent content never bypass authorization merely because they came from a trusted connection
- Message content alone never grants permission or expands an Agent Run's repository, filesystem, network, credential, or approval scope
- only authorized, explicit invocation behavior selects which Conversation content enters a Worker Provider request
- removing membership, disabling an Actor, or revoking a credential closes or reauthorizes active subscriptions
- raw credentials, provider payloads, hidden prompts, and secrets never appear in ordinary Message records

Future external-agent credentials must be high entropy, stored only as hashes where bearer-token verification permits, scoped to their Actor and allowed Organizations or Projects, rotatable, revocable, and auditable. The registration flow must require an authorized human; an external runtime cannot self-enroll into a Project or self-assert another Actor identity.

The exact connection protocol, event-subscription model, and authentication ceremony are deferred to the external agent integration milestone and require a focused threat model before implementation.

## 21.7 Public exposure

Expose only the Rust web application through TLS.

Do not publicly expose:

- PostgreSQL
- Codex App Server
- raw Git service ports
- local worktree paths
- internal health endpoints without access controls where sensitive

---

# 22. Performance and resource budgets

These are design targets, not promises.

## Core server

- idle application RSS target below 150 MB, excluding PostgreSQL and Codex
- no periodic full-project scans
- one WebSocket per active browser session
- bounded in-memory event buffers
- paginated database access

## Frontend

- initial authenticated shell target below 500 KB compressed, excluding lazy code and diff viewers
- no rendering every token delta individually
- long lists virtualized
- inactive Project streams unsubscribed

## Database

- indexes begin from actual access patterns
- project scope included in indexes
- full-text search in PostgreSQL
- no N+1 Message or Work Item loading

## Provider processes

- one App Server process per active Execution Profile, not per Message
- Agent Runs and repository builds are expected to dominate resource use
- concurrency limits configurable per host and profile

## Disk

- shared Git objects through worktrees
- cleanup policy for merged and abandoned worktrees
- compressed retention for large logs
- attachment quotas later

---

# 23. Observability and recovery

## 23.1 Structured logging

Every log entry should carry relevant identifiers:

- request ID
- user ID where safe
- project ID
- conversation ID
- Agent Run ID
- provider session ID
- job ID
- GitHub delivery ID

## 23.2 Health

Expose separate checks for:

- process alive
- database reachable
- migrations current
- job worker alive
- Codex profile availability
- GitHub integration status

Do not make temporary Codex unavailability mark the whole collaboration app unhealthy.

## 23.3 Metrics later

Initial structured logs are enough. Add Prometheus-style metrics after the core is functioning.

Useful future metrics:

- active WebSockets
- job queue depth
- Agent Run durations
- provider restart count
- database query latency
- GitHub webhook failures
- worktree disk usage

## 23.4 Recovery jobs

On startup:

- release stale job locks
- inspect non-terminal Agent Runs
- inspect registered worktrees
- reconcile provider sessions
- retry unpublished outbox events
- verify pending GitHub webhook jobs

## 23.5 Backup and restore contract

The supported backup set includes:

- a consistent PostgreSQL dump
- attachments and locally stored generated artifacts
- application configuration needed to reconstruct registered Projects and integrations
- dirty or unpushed worktrees that contain unique human or Worker work, plus the Git object data required to restore them

Repository mirrors and clean merged worktrees are rebuildable caches and may be excluded.

Codex authentication caches, session secrets, GitHub App private keys, and other credentials are excluded from ordinary backups by default. If an operator backs them up, the archive must use a separately documented encrypted-secret procedure. A normal restore may require reauthentication and secret re-entry.

The private-beta gate requires a restore drill onto a clean test location and verification that the application can reconcile restored database records, files, repositories, and preserved dirty worktrees.

---

# 24. Testing strategy

## 24.1 Rust unit tests

Test:

- authorization policies
- Actor-kind-independent authorization and resource-specific approval rules
- immutable Actor attribution for Messages, Agent Runs, and audit events
- exhaustive Agent Run state transitions and terminal-state invariants
- Thread attention derivation
- Conversation sequence allocation and per-Actor read cursors
- Work Item key generation
- branch name sanitation
- provider event normalization
- webhook idempotency
- MCP concurrency conflicts

## 24.2 Database integration tests

Run against real PostgreSQL.

Test:

- migrations
- transaction behavior
- project isolation
- job claiming
- outbox atomicity
- outbox publication recovery without claiming per-client delivery
- unique webhook delivery handling
- per-Actor read state
- Actor-backed membership and read-state integrity
- Message authorship spoofing and cross-Project mention rejection

## 24.3 Codex provider contract tests

Maintain recorded protocol fixtures from the pinned Codex version.

Test:

- initialize handshake
- login state
- thread start and resume
- turn streaming
- approvals
- interruption
- completion
- malformed line handling
- process restart

## 24.4 Git integration tests

Use temporary local repositories.

Test:

- mirror clone
- branch creation
- worktree add and remove
- dirty worktree preservation
- parallel worktrees
- repair and prune
- unsafe branch input rejection

## 24.5 MCP tests

Test with an MCP client against the real HTTP endpoint:

- authentication
- scope enforcement
- read tools
- write tools
- optimistic conflict
- audit entry
- idempotent retry
- rejection of client-supplied authorship

## 24.6 Frontend tests

- component tests for state displays
- accessibility tests
- Thread sidebar visibility rules
- Work Item editor conflicts
- mention composer behavior
- Message AST escaping, unsafe URL rejection, and hostile provider-output rendering

## 24.7 End-to-end tests

Use Playwright for the critical loop:

```text
create human Actor and Project
create Channel
create Work Item
start Execution Worker Thread
receive streamed events
answer blocker
complete run
restart and reconcile provider session and worktree
link pull request
review unseen state
invite second user
verify per-Actor read state
```

The future external-agent milestone adds a separate end-to-end contract suite. Version one tests schema compatibility and authorization with a non-human Actor fixture but does not connect a real Hermes or OpenClaw runtime.

---

# 25. Development workflow and quality gates

## Required checks

Rust:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
```

Frontend:

```text
pnpm lint
pnpm typecheck
pnpm test
pnpm build
```

Integration:

```text
migration test
PostgreSQL integration tests
end-to-end smoke test
```

## Code conventions

- UTC timestamps at boundaries
- domain IDs use newtypes in Rust
- API DTOs remain separate from database rows
- transactions are explicit around multi-object changes
- provider-specific payloads stay in provider modules
- no business logic in HTTP handlers
- no direct database calls from WebSocket handlers
- all project-scoped actions pass authorization

---

# 26. Build phases

## Phase 0: repository and risky technical spikes

Goal: prove the difficult external boundaries before building a polished shell.

### Spike A: Codex bridge

Prove in a small Rust CLI:

- spawn `codex app-server`
- send `initialize` with Agent Workspace client metadata, then `initialized`
- generate or consume the schema
- inspect account and rate-limit state
- complete browser login
- complete device-code login where the account or workspace enables it
- document the headless fallback when device-code login is unavailable
- start a provider Thread in a temporary Git repository
- start a turn
- stream an agent message
- receive a command or file event
- handle one approval or question
- interrupt or complete
- resume the provider Thread

Use stable methods by default. Any experimental method or field must be named in the compatibility note with its feature flag and fallback behavior.

Exit criterion: the bridge works reliably from Rust and has a written compatibility note containing the tested Codex version, method list, authentication results, known failure modes, and protocol fixtures captured for contract tests.

### Spike B: MCP

Prove:

- Streamable HTTP MCP endpoint in Rust
- bearer authentication
- one read tool for a fake Work Item
- one write tool with version conflict
- local Codex connects and calls both

Exit criterion: local Codex can read and update a fake Work Item without any application Thread.

### Spike C: Git worktrees

Prove:

- repository mirror or canonical clone
- two parallel linked worktrees
- branch creation
- dirty failure preservation
- cleanup

Exit criterion: two isolated tasks can operate without disturbing one another.

### Spike D: GitHub delivery

Prove against a disposable private test repository:

- install a development GitHub App on only the selected repository
- generate a short-lived installation token
- clone or fetch using installation credentials
- push a namespaced test branch
- open a pull request
- receive and verify a signed pull request webhook
- read checks or actions state with the narrowest working permission set
- confirm that the installation token is never persisted or exposed to Codex

The webhook test may use a temporary operator-approved tunnel or another bounded development delivery path. It does not require a permanent public deployment of Agent Workspace.

Exit criterion: the exact GitHub App permissions, webhook subscriptions, Git authentication flow, and token-handling boundary are captured in a compatibility note.

Delete or close disposable test artifacts after the spike when doing so is safe and intentional.

## Phase 0.5: end-to-end walking skeleton

Goal: prove that the isolated technical spikes compose into the real product loop before building broad application surfaces.

Build the smallest development-only vertical slice that can:

- seed one development actor, Organization, Project, Channel, and Thread
- register one trusted disposable repository
- create a managed worktree
- start Codex App Server with an isolated Execution Profile
- start one Agent Run with the worktree as `cwd`
- stream a consolidated agent Message and at least one significant provider event
- surface and answer one approval or question
- complete or interrupt the run
- restart the Rust process and reconcile the durable run, provider session, and worktree records

No polished navigation, Work Item system, team support, or GitHub UI is required.

Use production-shaped tables and module boundaries for the minimum required fields. Do not create a parallel throwaway schema that Phase 1 must replace. Authentication may use a development-only fixed actor until the host-local bootstrap command is implemented.

Exit criterion: one browser-driven Thread run completes end to end without manually editing the database or moving files between spike programs.

## Phase 1: application foundation

Build:

- Rust server shell
- React shell
- PostgreSQL migrations
- bootstrap account
- Actor identity and Actor-backed Project membership
- Organization and Project
- project rail
- project sidebar
- basic routing
- structured logging
- CI

## Phase 2: Conversations

Build:

- Channels
- versioned Message content
- sending and pagination
- WebSocket and outbox
- Threads
- read state
- Channel sidebar Thread visibility
- Project navigation restoration
- Actor-authored Messages and Actor-based structured mentions

## Phase 3: hosted Execution Worker

Build:

- Execution Worker entity, Actor identity, and provisional `@agent` handle
- Execution Profile setup
- Codex process supervisor
- browser and device-code login UI
- trusted repository registration
- production repository mirror and managed worktree slice
- branch creation and Agent Run workspace selection
- provider Thread mapping
- Agent Runs
- event streaming
- approvals and blockers
- blue, orange, white, gray, and red states
- crash recovery baseline

Phase 3 supports Thread-based invocation only. Inline Channel execution remains gated by O-002.

## Phase 4: Work Items

Build:

- Work Item types and statuses
- list view
- detail view
- Actor-compatible assignment
- acceptance criteria
- comments and activity
- links between Work Items and Threads
- primary and related repository links
- structured Work Item references
- basic Kanban view

## Phase 5: GitHub delivery and review

Build:

- GitHub App
- webhook ingestion
- installation-token Git authentication
- branch push
- pull request linking
- checks and reviews
- Work Item automation from pull request state
- cleanup policy

## Phase 6: local Codex

Build:

- production MCP endpoint
- scoped auth
- read and write tools
- optimistic concurrency
- Open in Codex deep link
- local Codex setup instructions
- audit trail

## Phase 7: teams

Build:

- invitations
- roles and permissions
- member management
- per-user notifications and read state validation
- team-safe Execution Profile policy
- a complete two-user workflow

## Phase 8: hardening and private beta

Build:

- backup and restore
- migration safety
- rate limiting
- secret rotation guidance
- worktree repair tools
- retention policies
- accessibility pass
- performance profiling
- deployment documentation

## Phase 9: external agent members after version one

This phase is deliberately outside the version-one definition of done.

Before implementation, complete a focused product and security design for:

- externally operated agent registration by an authorized human
- credential issuance, storage, rotation, and revocation
- exact MCP, HTTP, OAuth, token, polling, subscription, or event-delivery contract
- Project membership and role assignment
- Actor-authored Messages, mentions, Work Item assignment, and activity
- approval request routing and separation-of-duty policies
- Execution Worker invocation by an authorized external Actor
- idempotent retries, cursor semantics, connection health, and audit history
- removal and incident-response behavior

Build the first real connector against one chosen runtime, such as Hermes or OpenClaw, only after that contract is proven. Do not build multiple adapters or a universal agent framework in the first integration milestone.

---

# 27. Initial backlog

The IDs below are planning IDs for the repository backlog. They may be imported into the product later.

Milestones M0 and M1 are complete. Milestone M2 is authorized by its detailed
criteria below. Before beginning any later milestone, replace its title-only
backlog entries with scoped acceptance criteria, dependencies, expected tests,
and explicit exclusions. This elaboration happens one milestone ahead, not for
the whole roadmap at once.

## Milestone M0: Bootstrap and spikes

### M0-001 Create repository foundation

- use the existing Git repository or initialize it when absent
- add this source-of-truth document
- add short `README.md` and repository-scoped `AGENTS.md`
- add Rust workspace
- add React TypeScript app
- add PostgreSQL development compose file
- add editor and formatting configuration

### M0-002 Add CI baseline

- Rust format, clippy, test
- frontend lint, typecheck, build
- dependency audit
- PostgreSQL service for integration tests

### M0-003 Add configuration and tracing

- environment loader
- typed config validation
- JSON structured logs
- request IDs
- health endpoint

### M0-004 Codex App Server schema capture

- pin tested Codex version
- generate JSON schema
- commit schema
- document used methods

### M0-005 Build Codex bridge spike

Acceptance criteria are defined in Phase 0 Spike A.

### M0-006 Build MCP spike

Acceptance criteria are defined in Phase 0 Spike B.

### M0-007 Build Git worktree spike

Acceptance criteria are defined in Phase 0 Spike C.

### M0-008 Build GitHub delivery spike

Acceptance criteria are defined in Phase 0 Spike D.

### M0-009 Build end-to-end walking skeleton

Acceptance criteria are defined in Phase 0.5.

### M0 exit criteria

**Status:** Satisfied on July 31, 2026.

- all four compatibility notes are checked in
- the walking skeleton completes one real Thread run in an isolated worktree
- restart reconciliation succeeds without manual database or filesystem repair
- spike shortcuts are either removed or clearly confined to development-only code
- every invalidated assumption in this document is corrected with a Decision Log entry

The compatibility records under `docs/compatibility` and their redacted
fixtures retain the exact M0 evidence. The accepted browser run used the real
hosted Codex path on Red Hat, and the restart exercise resumed the stored
provider Thread, classified the interrupted provider turn, and preserved the
worktree without manual repair.

## Milestone M1: Identity and Projects

### M1-001 Complete identity and Project schema

Complete the production identity and Project schema with Actors, users,
password credentials, Organizations, Actor-backed memberships, Projects,
counters, sessions, Project instructions, per-Actor navigation state, and
security audit events. Extend the walking-skeleton migration history; never
replace or rewrite M0 migrations.

**Acceptance criteria**

- append migration `0004_m1_identity_projects.sql` without changing existing
  M0 IDs or relationships
- add `users`, `user_password_credentials`, `organization_memberships`,
  `sessions`, `project_counters`, `project_instructions`,
  `project_navigation_states`, and `audit_events`
- expand Project membership roles to Owner, Admin, Member, Viewer, and Worker
- enforce human-Actor ownership for users and role-compatible Worker
  memberships in PostgreSQL
- store only Argon2id password PHC strings and SHA-256 session/CSRF token
  digests; plaintext credentials never enter rows, logs, URLs, or command
  arguments
- preserve case-insensitive Organization-scoped Project-key uniqueness across
  archived Projects and enforce `[A-Z][A-Z0-9]{1,9}`
- backfill Project counters, instructions, and provable Organization ownership
  for the M0 data set without inventing a user, password, or session

**Dependencies:** accepted M0 migrations, Actor records, Project ownership, and
the M0 PostgreSQL integration harness.

**Expected tests:** real-PostgreSQL M0-to-M1 upgrade, foreign-key and role
invariants, case-insensitive uniqueness, credential digest storage, session
expiry/revocation, navigation cross-Project rejection, and concurrent Project
counter allocation.

**Exclusions:** invitation tokens, password reset, passkeys, OIDC, external
agent credentials, Work Item tables, and speculative team-authentication
schema.

### M1-002 Bootstrap administrator

Create safe first-user setup and the private-build browser session boundary.

**Acceptance criteria**

- add a host-local `bootstrap-admin` command that succeeds only while `users`
  is empty, holds an advisory bootstrap lock, and creates or explicitly adopts
  one human Actor, User, personal Organization, and Organization Owner
  membership transactionally
- read the password from a non-logging interactive terminal, validate a bounded
  length, hash it with Argon2id in a blocking worker, and zeroize the plaintext
  buffer after use
- support an upgraded M0 host only through an unambiguous active human owner or
  an explicit non-secret Actor/Organization selection; never rewrite authorship
- implement login, logout, and session-read APIs with opaque rotating cookies,
  idle and absolute expiry, generic credential failures, exact Origin checks,
  and CSRF protection for unsafe cookie-authenticated requests
- permit non-Secure development cookies only when development mode and a
  loopback listener are both proven

**Dependencies:** M1-001 identity, credential, session, and Organization
membership schema.

**Expected tests:** single-winner concurrent bootstrap, rollback atomicity, M0
Actor adoption, Argon2 verification and dummy unknown-user verification,
cookie attributes, session rotation/expiry/revocation, CSRF mismatch, exact
Origin rejection, logout idempotency, and secret redaction.

**Exclusions:** a browser setup route, default credentials, password recovery
or change UI, email verification, invitations, passkeys, OIDC, and the final
M7 authentication choice.

### M1-003 Project creation and archive

Implement Project list, create, detail, switch, and archive domain commands and
HTTP APIs.

**Acceptance criteria**

- an Organization Owner or Admin can create a Project with a normalized key
  and bounded display name
- one transaction creates the Project, creator Owner membership, counter,
  instructions, initial navigation state, and redacted audit event
- all list/detail queries join through the authenticated Actor's enabled
  membership and include `project_id` in resource lookups
- a non-member receives the same not-found response as a missing Project;
  members without a management privilege receive forbidden
- archive locks and versions the Project, records the responsible Actor,
  rejects any Project with a non-terminal Agent Run, preserves history, and is
  idempotent for an already archived Project
- archived Projects disappear from ordinary rail/search results and reject new
  mutations while remaining explicitly readable to authorized members

**Dependencies:** M1-001 schema, M1-002 authenticated Actor extraction, and
M1-004 policy decisions.

**Expected tests:** atomic creation, key/name validation, duplicate-key
conflict, Owner/Admin/Member/Viewer behavior, cross-Project non-disclosure,
active-run archive conflict, archive attribution/versioning, idempotent retry,
and preservation of M0 Project history.

**Exclusions:** hard deletion, restore UI, Organization administration,
default Channels, Work Item workflow, repositories, Execution Workers, and
bulk Project operations.

### M1-004 Project membership policy layer

Use Actor IDs in memberships and centralized typed action-on-resource checks.
Version one exercises human Actors; include non-human authorization fixtures
without creating an external-agent connection.

**Acceptance criteria**

- define one pure policy layer for Organization Project creation, Project view
  and management, navigation read/write, Worker invocation, run interruption,
  and approval response
- authority derives from active Actor status, enabled membership, role, and any
  immutable delegated Worker scope; Actor kind alone grants no authority
- Owner/Admin may manage, Member may perform ordinary collaboration and Worker
  invocation, Viewer is read-only, and Worker is limited to exact delegated
  scope
- retrofit M0 timeline, Agent Run, interrupt, approval, and WebSocket paths to
  use the authenticated Actor and centralized policy
- authenticate WebSocket upgrades, validate the exact Origin, and filter
  events by current Project access so one Project cannot observe another

**Dependencies:** M1-001 roles and sessions plus the existing M0 request and
realtime boundaries.

**Expected tests:** exhaustive role/action matrix, disabled Actor/membership,
same-role human and external-agent fixtures, Worker scope, human-required and
separation-of-duty approval rules, cross-Project HTTP access, WebSocket
handshake/filtering, and session revocation.

**Exclusions:** invitation/member-management UI, external-agent registration
or credentials, a system/integration superuser bypass, and final team
Execution Profile policy.

### M1-005 Project rail and switcher

Replace the static M0 Project rail with authenticated server data and the
minimum Project creation/archive navigation.

**Acceptance criteria**

- add sign-in, Home, Project creation, Project overview, Project settings, and
  existing Thread routes behind a session boundary
- render Home, pinned Projects, five recent unpinned Projects, the active
  Project fallback, attention counts supplied by the server, bounded search,
  and Project creation access
- use links for navigation, buttons for actions, server-returned capabilities
  for action visibility, and no client-submitted Actor identity
- remove archived Projects from ordinary navigation and route successful
  archive back to Home without silently choosing another Project
- preserve the restrained M0 visual system, move global navigation outside the
  main landmark, add a skip link, and replace breakpoint hiding with
  keyboard-accessible rail/sidebar drawers

**Dependencies:** M1-002 session APIs, M1-003 Project APIs, and M1-004
capabilities.

**Expected tests:** authenticated/signed-out/expired session states, login and
logout, rail grouping/search/attention labels, create validation and
idempotency, archive confirmation/conflict, keyboard and focus behavior,
phone-sized drawers, hostile text escaping, and accessibility smoke tests.

**Exclusions:** final branding, polished global navigation, derived global
attention, team member surfaces, Channels beyond the preserved M0 route, Work
Items, and repository/GitHub UI.

### M1-006 Remember per-user Project route

Persist and restore each Actor's last successfully opened location independently
for every Project.

**Acceptance criteria**

- navigation state is keyed by `(project_id, actor_id)`, versioned, and updated
  only after the referenced same-Project resource loads successfully
- clients send a structured location kind and canonical resource ID, never an
  arbitrary authoritative URL or Actor ID
- switching Project A to Project B and back restores each Project's own last
  location across reload and relogin
- stale, archived, unauthorized, or cross-Project locations fall back to the
  Project overview without exposing the inaccessible resource
- pin/recent changes and navigation writes use optimistic versions and
  serialize retries so an older response cannot overwrite a newer location

**Dependencies:** M1-001 navigation schema, M1-003 scoped Project/resource
queries, and M1-005 routes.

**Expected tests:** A-to-B-to-A restoration, reload/relogin persistence,
same-Project composite foreign keys, stale-resource fallback, cross-Project
rejection, optimistic conflict, pin ordering, and no browser-storage
dependency.

**Exclusions:** Conversation read cursors, Work view/filter/drawer state not
yet backed by M2/M4 resources, cross-device live synchronization, and
client-authored redirects.

### M1 exit criteria

**Status:** Satisfied on July 31, 2026.

- host-local first-admin bootstrap succeeds exactly once without credential
  disclosure
- the administrator can sign in and out, create two Projects, archive an
  eligible Project, and see only authorized active Projects in the rail
- switching between the two Projects restores independent remembered routes
  after reload and relogin
- existing M0 timeline and hosted-run behavior works through real session
  authentication and Project policy
- PostgreSQL migration/upgrade, session/CSRF, cross-Project, non-human policy,
  realtime isolation, frontend accessibility, and production-build tests pass
- the M2 Conversations backlog is elaborated before M2 implementation begins

The acceptance record at `docs/acceptance/m1.md` retains the exact accepted
commit, Red Hat release, automated checks, and redacted browser/runtime
evidence. The accepted browser flow proved first-user bootstrap, private
sessions, Project lifecycle and navigation restoration, and the existing M0
hosted-run path through centralized M1 Project policy.

## Milestone M2: Conversations

### M2-001 Conversation schema and invariants

Extend the M0 conversation projection into the production-shaped Channel and
Thread foundation without replacing its durable IDs or timeline history.

**Acceptance criteria**

- append a new migration after M1; preserve and evolve the M0
  `conversations`, `messages`, and `conversation_entries` tables rather than
  introducing parallel replacements
- add the missing origin-Message relationship, archive attribution/versioning,
  and same-Project composite constraints needed by Channel and Thread commands
- enforce in PostgreSQL and application code that a Channel has no parent or
  repository, a Thread has exactly one same-Project Channel parent, and a
  Thread can never parent another Thread
- preserve transactional, monotonically increasing, unique
  `conversation_entries.sequence` values inside each Conversation; timestamps
  remain metadata, not ordering keys
- prevent Channel or Thread archive while it owns a non-terminal Agent Run,
  preserve child history, and never cascade-delete Messages or entries
- make a Thread's repository binding immutable after its first Agent Run while
  leaving repository selection and workspace creation owned by M3

**Dependencies:** accepted M1 Actor/session/Project policy, the existing M0
Conversation and Agent Run rows, and application-generated UUIDv7 IDs.

**Expected tests:** M0-to-M2 migration, same-Project parent enforcement, nested
Thread rejection, Channel repository rejection, concurrent sequence
allocation, archive guards, repository-binding immutability after a run, and
historical M0 timeline preservation.

**Exclusions:** attachments, reactions, Work Item links, repository
registration UI, provider-session changes, and destructive Conversation
deletion.

### M2-002 Versioned Message AST and Actor authorship

Implement immutable version-one Message content with server-resolved Actor
authorship.

**Acceptance criteria**

- define a bounded, explicitly tagged version-one AST supporting text and
  Actor-mention nodes; reject unknown node types, malformed UUIDs, invalid
  shapes, unsafe URLs, excessive depth, and over-limit text before persistence
- derive bounded plain text on the server from the validated AST for search,
  notification, and export use; never trust a client-supplied plain-text
  rendering
- resolve `author_actor_id` exclusively from the authenticated session and
  reject any client-supplied authorship field
- validate every mentioned Actor against an enabled membership in the
  Message's Project at send time while retaining historical display
  attribution if that Actor is disabled later
- treat a sent Message as immutable in M2; a correction is another Message,
  and `content_version` describes the AST schema rather than an edit revision

**Dependencies:** M2-001 Conversation constraints, M1 authenticated Actor and
Project membership policy, and the existing Actor-backed M0 Messages.

**Expected tests:** AST round trip and plain-text derivation, hostile and
oversized nodes, authorship spoof rejection, human/Worker mention parity,
disabled or cross-Project mention rejection, HTML escaping, and preservation
of existing consolidated Worker Messages.

**Exclusions:** Message editing/deletion, attachments, reactions, Markdown or
HTML as authoritative storage, Work Item reference nodes before M4, and
plain-text `@handle` scanning as identity.

### M2-003 Message API and pagination

Add authorized Channel and Thread Message commands plus one durable timeline
pagination contract.

**Acceptance criteria**

- implement `POST /api/v1/channels/:id/messages` and the equivalent
  Thread-targeted send command through one Conversation domain operation
- require a bounded idempotency key for Message append; the same Actor,
  Conversation, and key returns the original durable result, while a
  mismatched payload conflicts
- insert the Message, allocate its Conversation sequence, create exactly one
  `conversation_entries` row, and enqueue its outbox event in one transaction
- extend `GET /api/v1/conversations/:id/timeline` with an opaque keyset cursor,
  bounded page size, stable ascending render order, and a cursor for loading
  older entries without timestamp merging
- authorize every send and timeline query through exact Project membership;
  a missing or unauthorized Conversation has the same not-found response
- return AST content, derived display data, Actor attribution, and significant
  Worker-event cards through typed DTOs without exposing raw provider payloads

**Dependencies:** M2-001 ordered entries, M2-002 Message validation, M1 policy
and sessions, and M2-005 outbox insertion.

**Expected tests:** concurrent sends with gap-free unique sequences,
idempotent retry and payload mismatch, forward and backward page boundaries,
empty/final pages, cross-Project non-disclosure, inactive membership, bounded
payloads, and stable mixed Message/Worker-event ordering.

**Exclusions:** full-text search, Message edits, file upload, reactions,
per-client event replay, and one database row per streamed token.

### M2-004 WebSocket connection

Evolve the authenticated M1 realtime boundary into the typed M2 Project event
connection while keeping HTTP and PostgreSQL authoritative.

**Acceptance criteria**

- keep one authenticated `/api/v1/realtime` WebSocket per browser session,
  validate the exact allowed Origin during upgrade, and never accept tokens or
  Actor IDs through the URL
- emit bounded version-one envelopes containing `event_id`, monotonic
  diagnostic `event_cursor`, `project_id`, type, occurrence time, and a typed
  payload
- authorize Project scope at connection/use time and again before delivery;
  close or rescope promptly when the session, Actor, or membership is revoked
- permit realtime delivery and ephemeral stream control only; all durable
  Message, read-state, and Conversation mutations remain authenticated HTTP
  actions
- on reconnect, have the client refetch Project navigation, the open
  Conversation page, read state, and attention data with bounded exponential
  backoff; correctness never depends on replaying missed events
- tolerate duplicate event IDs and unknown future event types without
  duplicating durable UI state or crashing the connection

**Dependencies:** M1 authenticated/revocable WebSocket, M2-005 outbox
publication, and M2-003 durable refetch endpoints.

**Expected tests:** missing/expired/revoked session, exact-Origin rejection,
cross-Project suppression, membership removal during a connection, duplicate
event tolerance, bounded frame handling, reconnect/refetch, and no
WebSocket-only durable mutation.

**Exclusions:** per-client acknowledgement, durable replay logs, public
subscriptions, browser bearer tokens, cross-Project firehoses, and presence or
typing indicators.

### M2-005 Transactional outbox

Make durable Conversation changes observable without coupling database commits
to an in-memory WebSocket send.

**Acceptance criteria**

- add `outbox_events` with an application UUIDv7 event ID, database-ordered
  publication cursor, schema version, Project/resource scope, bounded typed
  payload, attempt metadata, and nullable `published_at`
- write the outbox row in the same transaction as every M2 Message,
  Conversation, and read-relevant state change
- claim unpublished rows in cursor order with a short transactional lease or
  `FOR UPDATE SKIP LOCKED`, publish only after the originating transaction
  commits, and record completion without claiming browser receipt
- run publication at least once, recover unpublished rows after restart, and
  make duplicate publication harmless through stable event IDs and
  version-aware/refetching clients
- bound retries and payload sizes; exhausted or malformed events remain
  inspectable in redacted operator diagnostics rather than silently
  disappearing or blocking later events forever
- keep database access in the outbox worker/domain layer rather than the
  WebSocket handler

**Dependencies:** PostgreSQL, the M2-003 transactional command layer, the
M2-004 realtime broadcaster, and the existing structured logging boundary.

**Expected tests:** transaction rollback emits nothing, commit produces one
event, competing-worker single claim, crash before and after publish,
unpublished restart recovery, duplicate client receipt, poison-event
isolation, ordered cursor behavior, and secret/payload redaction.

**Exclusions:** Kafka or another broker, per-browser delivery guarantees,
unbounded replay, exactly-once network delivery, and the broader durable job
queue required by later integrations.

### M2-006 Channel UI

Add the first durable Conversation workspace inside an authenticated Project.

**Acceptance criteria**

- render active Channels in the Project sidebar and a routed Channel workspace
  with title, mixed durable timeline, load-older control, and Message composer
- render from the validated AST with Actor identity and accessible structured
  mentions; escape hostile text and never inject persisted HTML
- send optimistically with a stable idempotency key, reconcile the canonical
  sequence from the server, and expose retryable versus terminal failures
  without duplicating a Message
- consume realtime invalidations, coalesce refetches, and recover the open
  Channel from HTTP after disconnect, reload, or relogin
- preserve the M1 responsive rail/sidebar drawers, skip-link/main-landmark
  structure, keyboard operation, focus handling, and readable pending/error
  states
- retain independent per-Project remembered routes and fall back to Project
  overview when a Channel is archived or no longer authorized

**Dependencies:** M1 shell/navigation, M2-003 timeline and send APIs, M2-004
realtime connection, and M2-010 mention composer.

**Expected tests:** empty and long Channels, historical pagination, optimistic
send/retry, duplicate realtime event, disconnect/refetch, hostile AST output,
keyboard-only composer and drawers, screen-reader labels/status, narrow
viewport behavior, and Project switch restoration.

**Exclusions:** final visual design, attachments, reactions, edit/delete,
typing presence, rich link previews, offline sending, and inline hosted-worker
execution.

### M2-007 Thread creation and linking

Create focused child Conversations from a Channel or an originating Message.

**Acceptance criteria**

- implement an idempotent `POST /api/v1/channels/:id/threads` command with a
  bounded title and optional same-Channel origin Message
- create one same-Project Thread, preserve `parent_channel_id` and
  `origin_message_id`, initialize its sequence/read projections, and enqueue
  the Conversation event in one transaction
- reject nested Threads, cross-Project origins, origins from another Channel,
  archived parents, and unauthorized callers without leaking resource
  existence
- provide Channel actions to start a Thread and open existing linked Threads;
  direct Thread routes render the same paginated timeline and composer
- archive a Thread only through a versioned action with the active-run guard,
  preserve its history, and return the parent Channel as the safe navigation
  fallback

**Dependencies:** M2-001 invariants, M2-003 Message/timeline commands, M2-005
outbox, and M1 Project policy/navigation.

**Expected tests:** atomic creation, idempotent retry, nested/cross-Project
rejection, origin ownership, archived-parent rejection, active-run archive
guard, direct route/reload, parent fallback, and concurrent duplicate
requests.

**Exclusions:** Work Item links, repository selection, provider-session
creation, Agent Run start behavior beyond preserving the M0 route, automatic
AI-generated titles, and Thread children.

### M2-008 Per-user read state

Persist the highest Conversation sequence seen independently for every Actor.

**Acceptance criteria**

- add `conversation_read_states` keyed by exact
  `(project_id, conversation_id, actor_id)` with a highest-seen sequence,
  optimistic version, and timestamps
- require matching enabled Project membership and same-Project Conversation
  foreign keys; clients never submit another Actor's identity
- implement an idempotent mark-seen action that advances with an atomic
  maximum and can never move a cursor backward or beyond the current durable
  Conversation sequence
- mark seen only after the client has successfully rendered the corresponding
  durable entry; loading a sidebar or receiving a WebSocket event alone does
  not consume attention
- derive unread/unseen state from durable sequences independently for each
  human or non-human Actor fixture and preserve it across reload and relogin

**Dependencies:** M2-001 sequence model, M1 Actor authorization, M2-003
timeline responses, and M2-004 refetch behavior.

**Expected tests:** first read, monotonic concurrent updates, stale idempotent
retry, future-sequence rejection, cross-Project and disabled membership,
per-Actor isolation, reload/relogin persistence, and completed-after-cursor
derivation.

**Exclusions:** read receipts shown to other members, typing presence, device
specific cursors, notification-delivery acknowledgement, and failed-run
acknowledgement semantics owned by M3.

### M2-009 Thread sidebar surfacing rules

Implement the compact, deterministic Thread projection defined in section 6.5.

**Acceptance criteria**

- beneath each Channel, always surface Threads whose newest Agent Run is
  active, blocked for the current Actor, failed and unacknowledged, or
  completed after that Actor's read cursor
- for the selected Channel, also include its five most recently active
  Threads, deduplicate overlaps, and sort with a stable server-defined order
- remove seen inactive Threads from the compact list when the user leaves the
  parent Channel while retaining them behind `Show more`, `View all Threads`,
  and an explicit archived filter
- derive blue/orange/white/gray/red presentation from durable run/read state;
  every color has equivalent text and icon meaning and is never stored as
  business state
- calculate the projection in bounded Project-scoped queries and return no
  Thread the Actor cannot currently view
- update through outbox invalidations and refetch so reconnect, duplicate
  delivery, or missed events converge on the durable projection

**Dependencies:** M2-007 Threads, M2-008 read state, existing M0 Agent Run
states, M2-004 realtime, and M1 Project sidebar.

**Expected tests:** each attention reason, addressed blocked Actor, selected
Channel recent-five boundary, overlap deduplication, seen inactive removal,
stable ordering ties, archived filtering, cross-Project isolation,
color-independent labels, and reconnect convergence.

**Exclusions:** global Home attention aggregation, notification inbox, custom
sorting, arbitrary user pinning of Threads, inline Channel runs, and the full
M3 acknowledgement/blocker UI.

### M2-010 Mention autocomplete for human and worker Actors

Provide a keyboard-accessible structured Actor picker without making handles an
authorization or invocation protocol.

**Acceptance criteria**

- expose a bounded Project-scoped Actor search endpoint returning enabled human
  and Execution Worker memberships the sender may mention, with immutable
  Actor ID, display name, handle, kind, and safe avatar reference
- search normalized display names and handles with deterministic ordering and
  pagination; exclude disabled, archived, and cross-Project memberships
- insert a semantic mention node into composer state and submit its Actor ID
  in the Message AST while rendering the current Project handle
- support keyboard open, search, selection, dismissal, focus restoration, and
  screen-reader announcement without trapping focus or relying on color
- revalidate the referenced Actor and membership on send; handle races by
  preserving the draft and showing an actionable validation error
- treat an `@agent` mention as ordinary structured conversation data in M2;
  it must not silently start an Agent Run or scan text for magic strings

**Dependencies:** M1 Actor-backed Project memberships and policy, M2-002 AST,
M2-003 send API, and M2-006 composer.

**Expected tests:** human/Worker results, duplicate handles in different
Projects, disabled and cross-Project exclusion, bounded search/pagination,
keyboard and screen-reader interaction, hostile display names, send-time
membership race, AST round trip, and literal `@agent` text remaining text.

**Exclusions:** Work Item autocomplete, external-agent connection setup,
presence, directory-wide search, fuzzy ranking infrastructure, automatic
Worker invocation, and O-002 inline execution.

### M2 exit criteria

- an authenticated member can create a Channel, exchange immutable
  Actor-attributed Messages, create and revisit a linked Thread, and paginate a
  mixed durable timeline without sequence gaps or duplicates
- two Actors retain independent read cursors and attention states across
  reload, relogin, missed realtime delivery, and reconnect
- realtime events originate from the transactional outbox, survive server
  restart, remain Project-scoped, and converge through HTTP refetch
- the Project sidebar implements the exact compact Thread surfacing rules with
  accessible non-color status and archived history still reachable
- structured human and Worker mentions survive storage/rendering while
  spoofed authorship, cross-Project references, and plain-text magic invocation
  are rejected
- PostgreSQL, HTTP, WebSocket, frontend accessibility, hostile-content,
  concurrency, and browser-driven two-Actor acceptance tests pass
- the M3 Execution Worker backlog is elaborated before M3 implementation begins

## Milestone M3: Execution Worker and hosted Codex

M3 is authorized by the accepted M2 evidence in `docs/acceptance/m2.md` against
one exact green Red Hat release. It productionizes the hosted-execution slice
proven in M0; it must evolve those tables, routes, fixtures, and compatibility
contracts rather than create a second execution path. M3 remains Thread-only.
O-002 continues to prohibit inline Channel execution.

### M3-001 Execution Worker entity and Actor identity

Create the default Execution Worker with provisional `@agent` handle.

**Acceptance criteria**

- evolve the existing `actors`, `project_memberships`, and
  `execution_workers` rows so every Worker has one `execution_worker` Actor,
  one enabled same-Project Worker membership, and one same-Project Provider
  Connection
- add an idempotent Project command that creates or returns the default Worker
  with display name `Agent` and handle `agent`, creating or adopting the one
  same-Project local Codex Provider Connection in the same transaction; do not
  create a duplicate when the seed or a retry already established either row
- enforce same-Project ownership and the reserved Worker membership role in
  PostgreSQL as well as the domain command; active handles remain
  case-insensitively unique inside a Project
- expose bounded Project-scoped Worker identity and capability data to the
  Thread composer and settings UI without returning provider credentials or
  provider-private configuration
- resolve the Worker and requesting Actor server-side when an Agent Run is
  created; the client may select an authorized Worker ID but cannot submit
  authorship, requester identity, or delegated authority
- disabling the Worker, its Actor, or its membership prevents new runs and
  mentions while retaining historical Messages, runs, approvals, and audit
  attribution

**Dependencies:** accepted M2 Actor mentions and centralized policy, and the
existing M0 `execution_workers` and `provider_connections` projections.

**Expected tests:** idempotent default creation, existing-seed adoption,
same-Project foreign keys, duplicate-handle rejection, disabled Worker and
membership behavior, spoofed requester/Worker rejection, human/Worker mention
parity, and retained historical attribution.

**Exclusions:** external agent members, Worker marketplaces, arbitrary custom
Worker providers, multiple simultaneous Workers per invocation, autonomous
Worker scheduling, and final product naming.

### M3-002 Execution Profile schema and isolated CODEX_HOME

Turn the M0 profile fixture into a durable, owner-attributed execution
configuration without storing provider credentials in PostgreSQL.

**Acceptance criteria**

- append a migration after M2 that evolves `provider_connections` and
  `execution_profiles` with optimistic versioning, enabled/availability state,
  owning Actor, safe account-status metadata, concurrency limit, last health
  time, and bounded failure category
- keep `kind = codex_app_server` and `host = local_project_host` for M3; record
  the exact provider capability set and pinned binary version on the
  connection
- generate each profile's absolute `CODEX_HOME` beneath the configured data
  root, reject caller-provided filesystem paths, create it with service-account
  ownership and owner-only permissions, and prevent path escape or symlink
  substitution
- store provider credential files only inside that profile directory; database
  rows, browser DTOs, logs, audit metadata, and diagnostics contain status and
  redacted categories only
- select the solo profile deterministically and reject an unavailable,
  disabled, foreign-Project, or wrong-owner profile before queue admission
- keep the technical-alpha concurrency limit at one active provider turn per
  profile and make the global host limit a bounded typed configuration value

**Dependencies:** M3-001 Worker ownership, the accepted M0 App Server
compatibility record, configured runtime/data roots, and host service-account
permissions.

**Expected tests:** migration of the M0 fixture, idempotent profile creation,
same-Project and owner constraints, generated-path containment, symlink/path
escape rejection, permission checks, status-only serialization, secret
redaction, disabled/unavailable selection, and concurrency-bound validation.

**Exclusions:** shared personal credentials, O-003 team profile policy, public
multi-tenant credential isolation, remote provider hosts, billing, and provider
version upgrades.

### M3-003 Codex process supervisor

Replace per-request process ownership with one bounded supervised App Server
process per active Execution Profile.

**Acceptance criteria**

- start the pinned Codex binary by verified absolute path, clear the inherited
  environment, and pass only the profile `CODEX_HOME`, required safe runtime
  variables, and the worktree `cwd` on each Thread/turn request
- perform `initialize` with `clientInfo.name = "agent_workspace"`, then send
  `initialized`, before accepting profile work; keep raw method names and
  provider JSON inside the adapter module
- correlate JSONL responses to bounded pending requests and route
  notifications by provider Thread/turn identity so events cannot cross Agent
  Runs or Projects
- enforce protocol-line, pending-request, stderr-buffer, request-timeout, event,
  and shutdown limits; malformed or oversized provider output fails the
  affected process without exhausting server resources
- supervise the process group with bounded exponential restart backoff,
  graceful shutdown followed by full process-tree termination, and a circuit
  state visible through redacted profile health
- admit no more than one active turn per profile in M3; process concurrency is
  never used as a substitute for the durable scheduler

**Dependencies:** M3-002 isolated profile, committed Codex `0.146.0` schemas
and compatibility fixtures, the existing bridge implementation, and the
server's supervised-task shutdown boundary.

**Expected tests:** handshake ordering, request correlation, interleaved
Thread routing, malformed/oversized lines, bounded stderr, request timeout,
child-tree shutdown, backoff/circuit behavior, environment allowlist, profile
isolation, cancellation, and deterministic fake-App-Server fixtures with no
live credentials in CI.

**Exclusions:** exposing App Server over the network, unpinned provider
upgrades, multiple active turns per profile, generic all-provider transport,
and raw provider JSON outside the adapter.

### M3-004 Codex account setup UI

Provide an authenticated settings flow for preparing and inspecting one
isolated Execution Profile.

**Acceptance criteria**

- add authorized status, browser-login start, device-code-login start/poll,
  API-key-login when explicitly selected, and logout commands beneath the
  Execution Profile resource; every mutation requires session, exact Origin,
  CSRF, current Project policy, and a bounded idempotency key where retryable
- return browser callback URLs, device verification URLs/codes, ceremony
  expiry, account summary, authentication state, and rate-limit summary only;
  never return cached credentials, API-key contents, cookies, or profile files
- bind every ceremony to the initiating session, Actor, and Execution Profile,
  permit only one active ceremony per profile, expire it durably or in bounded
  server memory, and make cancel/restart behavior explicit
- support the SSH-forwarded callback path used by the private Red Hat host and
  the headless device-code path; keep any public callback/tunnel activation an
  explicit operator checkpoint
- refresh account and rate-limit state through the pinned `account/read` and
  `account/rateLimits/read` adapter operations and present unavailable/expired
  status without leaking provider error bodies
- make logout stop new admission, terminate the profile supervisor safely,
  invoke the provider logout operation, and preserve application Threads,
  Agent Runs, and transcripts

**Dependencies:** M3-002 profile ownership, M3-003 supervisor/account adapter,
M1 private sessions, and the M0 browser/device authentication evidence.

**Expected tests:** authorization and CSRF, ceremony ownership/expiry,
concurrent ceremony rejection, bounded device polling, callback state
validation, status/rate-limit redaction, logout with an active or idle process,
profile isolation, and manual browser plus device-code acceptance on Red Hat.

**Exclusions:** final team authentication, credential sharing, automatic login,
API-key purchase or billing changes, OAuth for MCP, and public callback
exposure without an operator checkpoint.

### M3-005 Trusted repository registration

Add the Project-owned repository record and UI required before hosted code work
can leave the development seed.

**Acceptance criteria**

- implement authorized Project repository list, register, and archive commands
  with optimistic versioning and bounded idempotency; a new registration is
  `pending` until M3-006 verifies its mirror, and an archived repository remains
  linked to history but cannot be selected for new runs
- register immutable GitHub repository identity, normalized owner/name,
  expected HTTPS clone URL, default branch, trust acknowledgement, enabled
  state, and server-generated mirror location; clients never submit a host
  filesystem destination
- validate supported GitHub HTTPS identity, safe owner/name/ref syntax, and
  uniqueness inside the Project before any clone or fetch; reject embedded
  credentials, local/file URLs, SSH command options, and ambiguous remotes
- make trust explicit in the UI and state that M3 runs repository code under
  the dedicated service account; only authorized Owner/Admin Actors can change
  trust or repository registration
- define the internal activation result consumed by M3-006 so only a verified
  mirror with the expected `origin` and resolved default branch can change a
  registration from `pending` to `ready`
- keep GitHub App installation credentials and pull-request delivery out of the
  provider environment; M3 may use the already approved private repository
  access path, while full GitHub delivery remains M5

**Dependencies:** M1 Project settings/policy, the M0 Git and GitHub spike
results, the existing repository projection, and configured repository roots.

**Expected tests:** valid registration and idempotent replay, duplicate and
cross-Project rejection, role enforcement, unsafe URL/ref/path input,
credential-bearing URL rejection, pending-to-ready activation authorization,
remote-identity mismatch result, archive with historical runs, archived
selection denial, and UI trust disclosure.

**Exclusions:** arbitrary Git hosts, repository creation, GitHub App setup UI,
pull requests/checks/reviews, submodule guarantees beyond a documented
fallback, and untrusted public repositories.

### M3-006 Repository mirror and managed worktree lifecycle

Promote the M0 Git spike into the service-owned repository workspace manager.

**Acceptance criteria**

- create or repair one canonical bare mirror beneath
  `repositories/<repository-id>/mirror.git`, using a known Git executable,
  direct argument arrays, sanitized environment, disabled hooks, and validated
  registered remotes
- activate a pending repository only after the mirror's `origin` matches the
  immutable registered identity and the configured default branch resolves;
  persist a bounded failed state for inspection when preparation fails
- serialize fetch, branch creation, worktree add/remove, prune, and repair with
  a short repository-scoped metadata lock; never hold that lock while Codex is
  executing
- keep `repository_workspaces` as the logical branch/base-commit owner and
  `git_worktrees` as the generated physical instance; allow at most one live
  physical worktree per logical workspace
- generate all physical paths beneath
  `worktrees/<repository-id>/<workspace-id>`, reject path escape/symlink
  substitution, and verify the resulting worktree belongs to the expected
  mirror and branch
- inspect and persist ready, dirty, missing, failed, retained, and cleanup
  state with bounded operator-safe diagnostics; never delete or reset a dirty
  or failed worktree automatically
- permit safe removal only when no non-terminal run owns the workspace, the
  tree is clean, the branch is eligible, and the retention policy is met;
  prune/repair must preserve recoverable state

**Dependencies:** M3-005 trusted repository identity, the M0 worktree spike
and compatibility note, the dedicated service account, and generated runtime
roots.

**Expected tests:** canonical mirror creation, concurrent repository locking,
two repositories in parallel, one-live-worktree uniqueness, branch/path
sanitation, hook/config isolation, dirty preservation, safe clean removal,
missing worktree inspection, prune/repair, crash during creation, and no lock
held during an execution fixture.

**Exclusions:** hostile-code sandboxing, automatic dirty cleanup, repository
garbage collection beyond measured need, cross-repository worktrees, and
authenticated push owned by M5.

### M3-007 Branch creation and Agent Run workspace selection

Choose and bind one repository, base commit, branch, logical workspace, and
physical worktree deterministically for each admitted code-changing Thread.

**Acceptance criteria**

- apply the repository precedence from section 15.1: existing Thread binding,
  explicit authorized selection, future primary Work Item repository, the sole
  enabled Project repository, otherwise a typed choice-required response; the
  model never chooses
- reject an explicit repository that conflicts with a Thread's existing
  binding and return a `Start a new Thread` action without mutating either
  Conversation
- when the scheduler admits a run to `starting`, fetch/resolve the registered
  default or selected base ref, record the exact commit, allocate a
  collision-safe branch with a stable opaque suffix, bind the Thread if
  unbound, and create the logical workspace in one coordinated operation
- create no mirror mutation or worktree while a run remains `created` or
  `queued`; compensate a failed physical add by retaining inspectable failed
  database state rather than silently deleting ownership rows
- make the run's repository immutable at creation and its workspace immutable
  once `starting`; the Thread repository is immutable after its first run as
  already enforced by M2
- reuse a Thread workspace only after a locked Git inspection proves the
  branch/worktree is the owned, unmerged, safe continuation; otherwise require
  a new Thread or allocate a new eligible workspace under the documented rule

**Dependencies:** M3-006 workspace manager, the existing M0 Agent Run and
transition foundation, the M2 one-active-run constraint, and later M4 primary
repository as an optional precedence input only when present.

**Expected tests:** every selection-precedence branch, zero/one/multiple
repository behavior, conflicting Thread binding, exact base commit under a
concurrent upstream change, branch collision resistance, queued run creating
no worktree, immutable run/workspace bindings, failed add preservation, and
safe versus unsafe retry reuse.

**Exclusions:** model-selected repositories, one run spanning repositories,
automatic rebases, branch merging, pull-request creation, and Work Item schema
before M4.

### M3-008 Provider session start and resume

Map one hosted application Thread and Execution Profile to one durable Codex
provider Thread without conflating either identity.

**Acceptance criteria**

- after the workspace is ready, start or resume through the profile supervisor
  with the generated worktree as `cwd`, the stored provider capability policy,
  and the bounded approval/sandbox configuration
- create `provider_sessions` only from a successful `thread/start` response;
  on later runs, call `thread/resume` for the exact stored provider Thread and
  bind the Agent Run only after resume succeeds
- enforce one Provider Session per `(application Thread, Execution Profile)`;
  preserve multiple historical Agent Runs against that session and prevent a
  session from changing application Thread, Project, profile, or repository
- start one provider turn for the admitted Agent Run, persist its provider turn
  ID before treating the run as `working`, and route only matching
  notifications and requests to that run
- start every turn with the same verified owned worktree; do not pass database,
  GitHub, browser-session, webhook, MCP, or other profile credentials in the
  turn environment
- when start/resume is rejected, preserve the workspace and return a bounded
  actionable application failure; never invent a replacement provider Thread
  silently for an existing transcript

**Dependencies:** M3-003 supervisor, M3-007 ready workspace, the existing M0
Agent Run transition foundation, and the pinned App Server adapter/schema.

**Expected tests:** first `thread/start`, subsequent `thread/resume`, bind only
after success, turn-ID persistence, duplicate start idempotency, Thread/profile
uniqueness, wrong-worktree and cross-Project rejection, notification routing,
environment isolation, provider refusal/timeout, and previous-version resume
compatibility fixture.

**Exclusions:** Provider Sessions for Channels, silent transcript replacement,
cross-profile resume, remote Codex Cloud, and flattening provider-specific
capabilities into generic chat.

### M3-009 Agent Run lifecycle

Make Agent Run creation, durable scheduling, state transitions, and audit
ownership the single production entry to hosted execution.

**Acceptance criteria**

- evolve `POST /api/v1/threads/:id/agent-runs` into an idempotent authenticated
  command that records server-resolved `requesting_actor_id`, Execution Worker,
  delegated-scope snapshot, Execution Profile, immutable repository selection,
  bounded prompt, and nullable Provider Session/workspace until their start
  operations succeed
- enforce transactionally at most one non-terminal run per Thread, one active
  provider turn per Execution Profile, and the configured global host ceiling;
  additional eligible runs remain durably `queued` in creation order
- use the exhaustive state graph in section 13.1 through one centralized domain
  transition function; every transition validates timestamps/reason shape and
  writes a significant run event, Conversation entry when user-visible, and
  outbox invalidation in the same transaction
- let the scheduler claim queued runs with short database leases and
  `FOR UPDATE SKIP LOCKED`, recover expired claims, and move a run to `starting`
  before M3-007 creates its workspace
- keep terminal runs immutable; a retry always creates a new Agent Run even
  when a safe Provider Session/workspace is reused
- return typed run state, capabilities, queue position where meaningful,
  repository/base/branch ownership, current blocker, and bounded failure reason
  without raw provider payloads or server filesystem paths

**Dependencies:** M3-001 through M3-008, M2 transactional outbox and ordered
Conversation entries, and the existing M0 transition tests.

**Expected tests:** exhaustive allowed/forbidden transitions, terminal
immutability, concurrent one-active-per-Thread creation, idempotent start and
payload mismatch, profile/global scheduler fairness, lease expiry/recovery,
queued runs creating no workspace, Actor/scope attribution, disabled Actor or
Worker, transition/outbox atomicity, and safe DTO redaction.

**Exclusions:** run priorities, scheduled/autonomous runs, multiple active runs
in one Thread, unbounded queues, external-agent credentials, and inline Channel
execution.

### M3-010 Event normalization and ordered timeline persistence

Convert App Server notifications into bounded application events while
preserving one coherent Conversation timeline.

**Acceptance criteria**

- normalize the event set from section 14.7 inside the Codex adapter and keep
  raw method names/payload shapes out of Conversation, API, and frontend
  modules
- assign each significant event a provider identity or deterministic adapter
  key, make persistence idempotent, allocate exactly one ordered
  `conversation_entries.sequence`, and enqueue its realtime invalidation in the
  same transaction
- stream `agent_message_delta` ephemerally with bounded coalescing, then persist
  exactly one consolidated immutable Worker Message and one timeline entry on
  completion; never store one row per token
- persist command, file-change, approval/question, test, artifact, completion,
  and failure summaries as typed significant cards with bounded safe payloads;
  large output is referenced/lazy rather than embedded without limit
- treat command and repository output as hostile: strip terminal control
  sequences, escape in the browser, enforce byte/line limits, and redact known
  application-owned secrets before persistence or logging
- retain unknown/malformed provider events only as redacted bounded diagnostics;
  they do not become ordinary Messages or block later valid events forever

**Dependencies:** M3-003 notification routing, M3-008 turn identity, M3-009
transitions, M2 Message/outbox/timeline contracts, and the Codex fixtures.

**Expected tests:** every normalized event shape, duplicate and out-of-order
notification handling, delta coalescing, consolidated Message retry after
restart, sequence ordering with concurrent human Messages, output/control
character bounds, secret redaction, unknown event isolation, raw payload
non-disclosure, and malformed fixture recovery.

**Exclusions:** arbitrary raw-provider event UI, permanent token-delta storage,
unbounded command logs, complete terminal emulation, semantic interpretation
of repository output, and GitHub artifact synchronization before M5.

### M3-011 Streamed Thread UI

Complete the browser Thread loop for starting and observing a real hosted run.

**Acceptance criteria**

- add a Thread-only `Work with Agent` action and bounded prompt composer that
  presents deterministic repository choice when required, keeps the prompt on
  retryable failure, and starts through the M3-009 idempotent command
- render created/queued/starting/working/recovering state, queue text,
  repository/branch context, coalesced Worker text, and significant provider
  cards in the existing ordered timeline without replacing ordinary Messages
- coalesce stream deltas per animation frame or bounded interval, discard
  duplicates by run/event identity, and reconcile the final durable Worker
  Message through HTTP without visual duplication
- after reload, relogin, missed events, or WebSocket reconnect, reconstruct the
  run and timeline solely from durable HTTP state; ephemeral deltas may be lost
  without losing correctness
- collapse large command/file output, use accessible status/live-region rules,
  preserve focus and keyboard operation, and render hostile output as inert
  text with non-color state labels
- keep the Channel composer non-executing; a semantic `@agent` mention may
  offer `Run in a Thread` but never starts work until the user confirms a Thread
  and submits the run action

**Dependencies:** M2 Channel/Thread UI and one-session realtime client,
M3-005 repository choices, M3-009 run API, and M3-010 normalized events.

**Expected tests:** repository-choice prompt, idempotent start retry, each run
state, delta coalescing/final reconciliation, duplicate/missed event,
reload/relogin recovery, long hostile output, keyboard and screen-reader
behavior, narrow viewport, and literal or semantic Channel `@agent` not
auto-running.

**Exclusions:** final visual design, inline Channel execution, diff review,
pull-request UI, attachments, offline run creation, and multiple simultaneous
run panels in one Thread.

### M3-012 Approval and blocker flow

Make provider approvals and questions durable, Actor-authorized blockers.

**Acceptance criteria**

- create one `agent_approvals` row for each provider approval or question with
  Agent Run, provider request ID, kind, bounded summary/options, eligible Actor
  or Project role, human-required/separation-of-duty flags, status, and
  timestamps; duplicate provider requests return the same row
- transition the run to `blocked`, add one ordered significant event, and emit
  an outbox invalidation in the same transaction; the eligible viewing Actor
  receives response-required attention without exposing hidden provider data
- implement `POST /api/v1/agent-approvals/:id/respond` with session/Origin/CSRF,
  bounded idempotency, response-time authorization, typed approval decision or
  question answer, and server-resolved responder Actor
- accept the first valid pending-to-resolved response atomically, return the
  durable result for the same idempotent retry, reject a conflicting later
  response, and forward to the exact waiting provider request at most once
- reconcile provider-side resolution arriving before or after the browser
  response, transition back to `working` only when the turn remains active,
  and record the answering Actor and normalized resolution event
- on interruption, denial, revocation, timeout, or recovery failure, resolve or
  cancel pending approvals explicitly so no stale orange state survives

**Dependencies:** M3-008 request routing, M3-009 state machine, M3-010 ordered
events, and M2 policy/outbox. M3-013 consumes the durable eligibility and
resolution state established here.

**Expected tests:** approval and free-text/choice question, eligible and
ineligible Actors, authorization revoked while pending, concurrent first
response wins, idempotent same response, conflicting later response, provider
resolution race, denial/interruption/timeout cleanup, response redaction, and
two-Actor orange-state isolation.

**Exclusions:** approval delegation outside the Project, public approval links,
provider policy editing, autonomous approval by the Worker, notifications
outside the app, and O-003 team service-profile policy.

### M3-013 Thread attention colors and per-user unseen logic

Replace the M2 interim run-state projection with the complete durable
per-Actor attention semantics from section 13.2.

**Acceptance criteria**

- derive precedence as response required for this Actor, active, newest failed
  and unacknowledged, completed after this Actor's read cursor, then inactive
  and seen; store causes and cursors, never color names
- add the minimum per-Actor Agent Run acknowledgement record and authorized
  idempotent acknowledgement command needed for failed-run attention; clients
  cannot acknowledge for another Actor or before rendering the durable event
- use approval eligibility for orange response-required state, Agent Run state
  for blue active state, acknowledgement for red failure state, and
  `conversation_read_states.highest_seen_sequence` for white completion state
- treat queued as active with explicit `Queued` text; canceled and interrupted
  become inactive after their durable entries are seen and retain explicit
  labels/icons in history
- preserve the compact selected-Channel recent-five union, deduplication,
  stable server ordering, Show more/View all, archived filter, and Project
  scoping established by M2
- invalidate through the outbox and converge through HTTP refetch so two Actors
  can hold different attention for the same Thread across reload and reconnect

**Dependencies:** M2 read state/sidebar projection, M3-009 durable run events,
M3-012 approval eligibility, and one-session realtime invalidation.

**Expected tests:** every precedence state, queued label, addressed versus
unaddressed blocker, two-Actor failure acknowledgement, completion before/after
read cursor, canceled/interrupted seen behavior, older failed run versus newer
active/completed run, compact-list boundaries, duplicate/missed events,
cross-Project non-disclosure, and non-color accessibility.

**Exclusions:** global Home attention inbox, email/web-push notification,
presence, read receipts shown to peers, custom Thread pinning, and arbitrary
attention ranking.

### M3-014 Interrupt and retry

Provide terminal interruption and explicit new-run retry without destructive
Git recovery.

**Acceptance criteria**

- keep `POST /api/v1/agent-runs/:id/interrupt` idempotent and Actor-authorized;
  only a non-terminal visible run can be interrupted, and terminal state never
  reopens
- persist `interrupted`, a bounded reason, canceled pending approvals, one
  significant timeline entry, and outbox invalidation transactionally before
  or while issuing the bounded provider `turn/interrupt`
- treat provider interrupt success, timeout, already-terminal response, and
  process loss as reconciliation inputs; none may reset or delete the owned
  worktree
- add an explicit retry command that creates a new Agent Run with a new ID and
  authorization snapshot while linking its predecessor for history
- before reusing a Provider Session, branch, logical workspace, or worktree,
  inspect provider and Git ownership/state; reuse only when the original Thread
  and repository still match and the state is provably safe
- preserve dirty/failed worktrees and return an actionable choice when reuse is
  unsafe; a merged, closed, missing, foreign, or ambiguous workspace is never
  silently repaired by reset

**Dependencies:** M3-006 inspection, M3-008 provider interrupt/resume, M3-009
terminal lifecycle, and M3-012 pending-approval cleanup.

**Expected tests:** interrupt from every non-terminal state, terminal repeat,
unauthorized/cross-Project access, provider success/timeout/crash race,
transactional approval cleanup, retry creates a distinct run, safe session and
workspace reuse, dirty/missing/merged rejection, and preserved branch/base
commit history.

**Exclusions:** pause/resume as a non-terminal user feature, force-killing the
whole server, automatic reset/rebase, retrying in another repository inside the
same Thread, and destructive cleanup.

### M3-015 Provider crash and restart recovery

Generalize the accepted M0 conservative recovery proof across supervised
profiles, queued runs, durable events, approvals, and managed worktrees.

**Acceptance criteria**

- on application startup, claim every non-terminal run idempotently and move it
  through `recovering` with a durable event; separately release expired
  scheduler/outbox leases without duplicating domain work
- restart the exact Execution Profile supervisor, resume and read the stored
  provider Thread/turn when available, and inspect the registered mirror,
  branch, logical workspace, physical worktree, and dirty state
- complete a run only when provider history proves its turn completed and the
  consolidated Message/events can be idempotently reconciled; reattach to a
  still-running turn only when the pinned adapter proves notification routing
  and turn identity are safe
- otherwise mark the old run terminal `interrupted` with an actionable bounded
  reason, cancel pending approvals, return the Provider Session to a consistent
  state, and preserve the branch/worktree exactly as found
- make reconciliation re-entrant across a second crash at every checkpoint;
  duplicate provider events, outbox publication, and Conversation entries must
  converge through stable identities and unique constraints
- expose recovery result and safe next action in the Thread UI and redacted
  operator logs; require no manual database edits or filesystem moves

**Dependencies:** M3-003 supervisor, M3-006 inspection, M3-008 Provider
Session identity, M3-009 lifecycle/scheduler, M3-010 idempotent events,
M3-012 approval cleanup, and accepted D-032 behavior.

**Expected tests:** crash before/after workspace add, provider Thread bind, turn
start, delta, consolidated Message, approval request/response, completion, and
outbox publish; clean/dirty/missing worktree; provider completed/running/
interrupted/missing/ambiguous history; second-crash reentrancy; profile session
state repair; and systemd restart acceptance on Red Hat.

**Exclusions:** inventing provider continuation, deleting or resetting dirty
state, cross-version resume without compatibility proof, operator database
repair as an acceptance step, and remote executor failover.

### M3 exit criteria

- an authorized Actor can prepare an isolated Codex Execution Profile, register
  a trusted repository, open a Thread, select the deterministic repository, and
  start a real hosted run through the authenticated browser
- queued admission creates no worktree before `starting`; the admitted run
  records exact repository, base commit, unique branch, logical workspace,
  physical worktree, requesting Actor, Worker, delegated scope, profile,
  Provider Session, and provider turn identity
- the pinned App Server streams one coalesced Worker Message plus significant
  command/file/test events into the ordered durable timeline without raw
  provider payloads, unbounded output, or one row per token
- two Actors observe correct independent blue, orange, red, white, and gray
  attention; an eligible Actor can answer one approval/question atomically and
  an ineligible Actor cannot
- interruption is terminal, retry creates a new Agent Run, and safe reuse versus
  dirty/ambiguous preservation is deterministic and requires no destructive
  cleanup
- a systemd restart during a non-terminal real run produces either a proven
  resume/completion or an actionable terminal interruption while preserving
  Provider Session, branch, base commit, and worktree consistency
- provider, Git, PostgreSQL, HTTP, WebSocket, scheduler, concurrency,
  hostile-output, secret-isolation, frontend accessibility, and browser-driven
  Red Hat acceptance tests pass against one exact green commit
- `docs/compatibility/codex-app-server.md`, Git worktree compatibility evidence,
  and the Decision Log record every changed M0 assumption
- the M4 Work Items backlog is elaborated before M4 implementation begins

## Milestone M4: Work Items

### M4-001 Work Item schema and readable keys

Add the canonical Project-scoped Work Item and immutable readable identity.

**Acceptance criteria**

- add `work_item_types`, `work_item_statuses`, `work_items`, and the minimum
  same-Project composite keys required by later M4 links; every internal ID is
  application-generated UUIDv7
- allocate the numeric portion under a transaction lock on the existing
  `project_counters` row, store it separately from the Project key, and expose
  the stable display key `<PROJECT-KEY>-<number>` without treating it as the
  canonical foreign key
- make Project key changes fail once the first number has been allocated;
  concurrent creates receive distinct gap-free numbers for committed rows and
  an idempotent replay returns the originally created Work Item
- store bounded title, bounded plain-text description, type, status, optimistic
  version, creating Actor, archive attribution, and timestamps; browser input
  cannot supply Project, creator, key number, version, or audit identity
- require enabled same-Project membership for reads and authorized create
  policy for writes; missing, cross-Project, and unauthorized resources use the
  existing non-disclosure boundary
- archive rather than delete and retain all keys, links, activity, comments,
  runs, and future artifacts

**Dependencies:** accepted M1 Actor/Project policy, `project_counters`, UUIDv7
and idempotency infrastructure, and the source-of-truth identity rules.

**Expected tests:** M0-to-M4 migration, same-Project foreign keys, concurrent
key allocation, transaction rollback without a consumed committed number,
idempotent replay/payload mismatch, title/description/control bounds, Project
key immutability, archive retention, Actor spoof rejection, and cross-Project
non-disclosure.

**Exclusions:** globally unique readable keys, deletion/retention UI, rich-text
descriptions, subtask hierarchy, estimates, dates, labels, custom fields, and
Work Item import.

### M4-002 Default Project workflow

Create stable Project-owned type and status definitions for existing and new
Projects.

**Acceptance criteria**

- seed Feature, Bug, Task, Request, Research, and Improvement types with stable
  opaque IDs, bounded names, deterministic order, and one default type
- seed Backlog, Ready, In Progress, Review, Verification, Done, and Canceled
  statuses with stable opaque IDs, deterministic order, semantic categories,
  and one default status
- store allowed directed status transitions explicitly and seed the transition
  graph from section 13.3; domain logic uses status IDs/categories rather than
  editable display names
- create the full default workflow in the same transaction as every new
  Project and add it idempotently for retained Projects during migration/seed
- require exactly one active default type and status per Project, reject
  duplicate active names/order values, and prevent disabling a definition that
  is referenced by active Work Items
- keep the definitions read-only in the M4 browser; future customization can
  evolve these rows without rewriting existing Work Items

**Dependencies:** M4-001 Project-scoped schema, accepted Project creation and
seed commands, and the default transition model in section 13.3.

**Expected tests:** existing/new Project provisioning, repeated seed, exact
defaults/order/categories/transitions, same-Project constraints, duplicate
rejection, referenced-definition protection, invalid transition rejection,
and display-name-independent behavior.

**Exclusions:** workflow editor, custom type/status creation UI, conditional
transitions, automation rules, per-type workflows, and GitHub-driven status
changes.

### M4-003 Work Item list

Provide the first durable Work surface as a paginated list projection.

**Acceptance criteria**

- implement Project-scoped list and create endpoints with opaque bounded
  cursors, deterministic ordering, stable filter shapes, and a maximum page
  size
- filter by bounded search, type, status, semantic category, priority,
  assignee, unassigned, and archived state; every supplied ID must belong to
  the authorized Project
- return only bounded card fields, display key, version, type/status semantic
  metadata, assignee projection, and capabilities; never return internal audit
  payloads or other Projects' filter matches
- render a keyboard-reachable list with loading, empty, filtered-empty, error,
  and pagination states inside the existing four-region shell
- preserve the selected Work view and filters through the existing per-Actor
  Project navigation state without making the browser cache authoritative
- invalidate/refetch through the transactional outbox after create or mutation;
  duplicate realtime invalidations must not duplicate cards

**Dependencies:** M4-001/002 records, M2 outbox/realtime convergence, M1
navigation state, and existing Project shell/query conventions.

**Expected tests:** authorization/non-disclosure, each filter and combination,
cursor bounds/order, create/replay/conflict, hostile text escaping, duplicate
invalidation, reload restoration, keyboard/accessibility smoke, and narrow
viewport behavior.

**Exclusions:** saved/shared views, bulk editing, full-text search service,
roadmap, grouping, sorting customization, CSV export, and offline creation.

### M4-004 Work Item detail

Implement the canonical detail route and constrained optimistic edits.

**Acceptance criteria**

- implement lookup by immutable UUID and authorized Project-qualified readable
  key; an unqualified key never resolves across all Organizations
- render title, description, type, status, priority, assignee, acceptance
  criteria, links, comments, activity, version, archive state, and capabilities
  from one authorized detail response
- add constrained update and archive/restore commands requiring exact Origin,
  CSRF, expected version, centralized policy, and bounded idempotency where a
  network retry could duplicate an append-only effect
- apply every non-commutative field change with one compare-and-swap update;
  return the authoritative current resource/version on conflict without
  overwriting either edit
- write a typed bounded activity row and outbox invalidation in the same
  transaction as every successful mutation; no-op replay creates neither a
  version bump nor duplicate activity
- preserve an unsaved browser draft on conflict or retryable failure and offer
  explicit reload/reapply behavior

**Dependencies:** M4-001/002, centralized Work Item policy from M4-003,
existing error/idempotency/outbox envelopes, and M4-006 through M4-009 detail
projections as they land.

**Expected tests:** UUID/key lookup, non-disclosure, field bounds, exact-version
success/conflict, concurrent editors, no-op/replay behavior, archive/restore,
activity/outbox atomicity, hostile rendering, draft preservation, and
accessibility smoke.

**Exclusions:** collaborative live editing, revision restore, rich-text editor,
attachments, time tracking, custom fields, and destructive delete.

### M4-005 Actor-compatible assignment and priority

Add explicit planning ownership without conflating users and Actors.

**Acceptance criteria**

- store nullable `assignee_actor_id` and a stable bounded priority value of
  `no_priority`, `low`, `medium`, `high`, or `urgent`
- permit assignment only to an active human or external-agent Actor with an
  enabled membership in the same Project; Execution Workers, integrations,
  systems, disabled Actors, and foreign members are rejected
- resolve the mutating Actor from authentication and record assignee changes as
  activity by that Actor; the client cannot submit authorship
- expose a bounded assignable-Actor search using the same safe identity
  projection/limits as mention search, with no email or credential metadata
- require expected Work Item version for assignment or priority changes and
  converge optimistic UI from the authoritative conflict response
- retain historical assignee identity after membership disablement while
  preventing a new assignment to it

**Dependencies:** M4-004 mutation boundary, accepted Actor/membership schema,
and centralized policy.

**Expected tests:** human/external-agent parity, Worker/integration/system
rejection, disabled/foreign Actor rejection, unassignment, every priority,
concurrent conflict, spoofed author rejection, historical display, and search
redaction/bounds.

**Exclusions:** multiple assignees, teams as assignees, workload balancing,
watchers, sponsors/reviewers, due dates, and notification delivery.

### M4-006 Acceptance criteria

Model an ordered checklist as part of the Work Item rather than embedding an
unvalidated document blob.

**Acceptance criteria**

- add UUIDv7 criterion rows with Work Item/Project identity, bounded plain
  text, deterministic integer position, completion state, completing Actor,
  and completion time
- create, edit, reorder, complete, reopen, and remove criteria only through the
  parent Work Item expected-version boundary; each successful command advances
  the parent version once
- serialize concurrent reorder/edit operations on the Work Item and preserve a
  gap-free unique order after commit
- validate completing Actor from the authenticated command and keep completion
  attribution when rendered; reopening clears completion attribution through
  an explicit activity
- write one bounded activity item and one outbox invalidation per command,
  while idempotent replay creates no duplicate criterion or activity
- render the checklist with native controls, visible counts, keyboard support,
  and inert hostile text

**Dependencies:** M4-004 detail/version boundary, M4-007 activity, and Actor
attribution.

**Expected tests:** create/edit/reorder/remove, concurrent reorder, exact
version conflict, completion/reopen attribution, row/project integrity,
idempotent replay, text/control/quantity bounds, ordering after deletion, and
accessible browser interaction.

**Exclusions:** nested criteria, rich text, per-criterion discussions,
criterion assignment, templates, automated completion, and AI-generated
criteria.

### M4-007 Work Item comments and activity

Provide append-only collaboration and a trustworthy mutation history.

**Acceptance criteria**

- store immutable Actor-authored comments with the existing version-one
  structured content shape, derived plain text, bounded mentions/references,
  and application-generated UUIDv7 IDs
- post comments through an idempotent command; authentication supplies the
  author and a client cannot edit, delete, backdate, or impersonate a comment
- store typed activity separately from comments with Actor, kind, bounded safe
  summary/payload, optional source idempotency identity, and timestamp
- create activity in the same transaction as every Work Item mutation and
  expose comments/activity as one deterministic paginated detail timeline
- validate structured Actor mentions and Work Item references inside the same
  Project and project them through dedicated relation rows
- publish bounded invalidations through the existing outbox; realtime is never
  the source of comment/activity truth

**Dependencies:** M2 structured content, M4-004 detail, M4-010 Work Item
references, Actor policy, idempotency, and outbox infrastructure.

**Expected tests:** immutable comment/replay, author spoof rejection, rich
content/control bounds, mention/reference scope, activity atomicity and type
allowlist, deterministic pagination, concurrent comment/mutation order,
reconnect/refetch, and hostile rendering.

**Exclusions:** comment editing/deletion, reactions, attachments, email
notifications, arbitrary audit JSON, and merging comments into Conversations.

### M4-008 Thread and Work Item many-to-many links

Connect planning and discussion without turning either object into the other.

**Acceptance criteria**

- add a same-Project unique link table between Work Items and thread-kind
  Conversations; Channels and cross-Project Conversations are rejected in the
  database and domain command
- implement authorized idempotent link/unlink commands from both detail
  contexts with Actor attribution, activity, and outbox invalidation
- return bounded linked-Thread cards with parent Channel, archive state, newest
  run state, and href; return bounded linked-Work-Item cards in Thread context
- preserve links when either side is archived and hide archived targets from
  ordinary pickers unless explicitly requested
- make unlink remove only the relation, never either object, Provider Session,
  Agent Run, Message, or timeline history
- keep one Thread linkable to several Work Items and one Work Item linkable to
  several Threads without selecting a primary Conversation

**Dependencies:** accepted M2 Conversations, M3 Thread/run projections,
M4-004 detail/activity, and centralized Project policy.

**Expected tests:** many-to-many cardinality, duplicate/replay, Channel and
cross-Project rejection, unauthorized non-disclosure, archive preservation,
unlink safety, activity/outbox atomicity, picker bounds, and Thread/detail UI.

**Exclusions:** linking Channels, automatic link inference from plain text,
Thread ownership by a Work Item, moving Messages, and inline Channel execution.

### M4-009 Work Item primary and related repository links

Attach deterministic code context without weakening M3 repository trust.

**Acceptance criteria**

- add a same-Project Work Item/Repository link with role `primary` or
  `related`, uniqueness per pair, and a partial uniqueness constraint allowing
  at most one primary repository per Work Item
- implement authorized optimistic link, unlink, and set-primary commands;
  archived, disabled, untrusted, pending, failed, or foreign repositories are
  not eligible for a new primary link
- retain historical links when a repository or Work Item is archived and show
  its unavailable state without making it selectable for new execution
- update repository selection so an unbound Thread linked to exactly one Work
  Item may use that Work Item's ready primary repository at the already defined
  precedence position; ambiguity still requires an explicit choice
- never change an existing Thread repository binding and never allow a Work
  Item link command to move, reset, or delete a workspace
- record Actor-attributed activity and outbox invalidation transactionally

**Dependencies:** M3 trusted Repository and deterministic selection contracts,
M4-004 detail, M4-008 Thread links, and optimistic policy boundary.

**Expected tests:** one-primary constraint, related-to-primary swap, duplicate
and concurrent commands, archived/unready/foreign rejection, historical
display, every M3 precedence branch including ambiguous linked Work Items,
immutable Thread binding, and no Git side effects.

**Exclusions:** repository creation, GitHub App credentials, branch selection,
cross-repository Agent Runs, submodule semantics, and push/pull-request work.

### M4-010 Work Item reference autocomplete

Extend the version-one structured-content model with canonical Work Item
references.

**Acceptance criteria**

- implement a bounded authorized Project autocomplete endpoint accepting safe
  title/key search and returning immutable ID, display key, title, status
  category, archive state, and href only
- accept the already specified `work_item_ref` content node in new Messages and
  Work Item comments, resolving every ID against the current authorized Project
- persist a distinct Message/comment-to-Work-Item projection for query and
  notification use while retaining every occurrence/order in the immutable AST
- render references as safe links with a readable label resolved from durable
  state; stale or archived references remain readable and cannot inject HTML
- reject caller-supplied display labels as authority, cross-Project IDs,
  unknown fields, excessive nodes/results, and unqualified global key lookup
- keep plain-text strings such as `TRI-142` ordinary text; they never create a
  structured link or automation implicitly

**Dependencies:** accepted M2 content AST and composer, M4-001 identity,
M4-007 comments, and centralized Project read policy.

**Expected tests:** autocomplete search/bounds/non-disclosure, Message and
comment references, duplicate AST occurrences with one projection, archived
and stale rendering, hostile title escaping, unknown node/field rejection,
plain-text non-conversion, and keyboard picker behavior.

**Exclusions:** global cross-Project search, automatic text parsing, rich link
previews, backlink notifications, slash commands, and external issue keys.

### M4-011 Kanban projection

Render the same Work Items by status without creating a second planning model.

**Acceptance criteria**

- implement a paginated board projection grouped by active Project status ID
  and ordered by a bounded per-status rank plus immutable tie-breaker
- use the same filters, card DTO, policy, archive rules, and query invalidation
  as the list; list and board must converge on the same Work Item version
- move a card only through an expected-version status/rank command that verifies
  an allowed workflow transition and returns the authoritative conflict on a
  race
- provide pointer drag-and-drop plus equivalent keyboard move controls and
  status menus; status, position, and feedback are never color-only
- use measured bounded rank allocation/rebalancing inside one status and avoid
  rewriting unrelated columns or every Project row during an ordinary move
- roll back the optimistic card on rejection and refetch after duplicate or
  missed realtime invalidation

**Dependencies:** M4-002 workflow transitions, M4-003 list filters/cards,
M4-004 versioned mutation, and the existing realtime client.

**Expected tests:** list/board parity, allowed/forbidden transitions, rank
ordering/rebalance, concurrent move conflict, duplicate/missed invalidation,
filter parity, archived status behavior, pointer and keyboard operation,
screen-reader labels, and narrow viewport smoke.

**Exclusions:** swimlanes, WIP limits, custom board views, multi-select/bulk
drag, roadmap, timeline/Gantt, automation, and final visual polish.

### M4-012 Work with Worker action

Connect one Work Item to the accepted M3 hosted Thread loop through an explicit
human action.

**Acceptance criteria**

- add a Work Item detail action that requires an authorized Execution Worker,
  a selected active parent Channel, a bounded prompt, and deterministic ready
  repository context before invocation
- create a new application Thread, link it to the Work Item, and record the
  Actor-attributed activity idempotently before calling the existing M3 Agent
  Run command; a replay returns the same Thread/run identities
- default repository selection to the Work Item's ready primary repository,
  then the existing sole-ready-Project rule; multiple/ambiguous repositories
  require the human to choose before any run or worktree is created
- derive a bounded Thread name and provider prompt containing the immutable Work
  Item ID/key plus user instructions; never place arbitrary comment/activity
  history, credentials, or browser state into the provider prompt implicitly
- if run creation fails after the Thread/link transaction, preserve the linked
  empty Thread with an actionable retry; never invent a hidden Thread or delete
  discussion history as compensation
- navigate to the created Thread and rely entirely on the M3 streamed timeline,
  approvals, interrupt, retry, attention, and recovery behavior; Worker
  completion never changes Work Item status automatically

**Dependencies:** M4-008/009 links, M3 authenticated run path and repository
selection, active Channels, default Worker/profile, and idempotency.

**Expected tests:** authorized invocation, Channel/Worker/repository choice,
idempotent replay, primary/sole/ambiguous repository cases, no worktree before
admission, bounded prompt/name, partial failure with retryable linked Thread,
browser navigation, full real hosted run, and no automatic status transition.

**Exclusions:** inline Channel runs, autonomous/scheduled work, multi-Worker or
multi-repository runs, GitHub push/PR creation, local Codex deep links, Work
Item automation, and model-selected repositories.

### M4 exit criteria

- an authorized Actor can create, find, filter, open, edit, archive, and restore
  a stable readable Work Item through the browser with optimistic conflict
  handling and complete Actor attribution
- retained and newly created Projects have the exact default types, statuses,
  semantic categories, and transition graph; list and Kanban are consistent
  projections of the same versioned rows
- assignment, priority, ordered acceptance criteria, immutable comments,
  activity, Thread links, repository links, and structured references enforce
  same-Project identity in PostgreSQL and the application policy layer
- two authenticated Actors can demonstrate a real edit conflict without lost
  updates, forged authorship, cross-Project disclosure, or duplicate activity
- Work with Worker creates one linked Thread and one M3 run, uses deterministic
  repository context, completes one real browser-driven hosted flow, and never
  changes Work Item status merely because the Worker finished
- migrations, PostgreSQL concurrency/integrity, HTTP, outbox/realtime,
  structured-content, hostile-output, frontend accessibility, Red Hat, and
  browser-driven tests pass against one exact green commit
- the M5 GitHub delivery backlog is elaborated before M5 implementation begins

## Milestone M5: GitHub delivery and review

### M5-001 GitHub App setup

Promote the retained development GitHub App proof into a host-owned,
Project-scoped integration boundary without exposing credential material to the
browser, provider, repository, or database.

**Acceptance criteria**

- load the GitHub App ID, private-key file path, webhook-secret file path, API
  base URL, and pinned REST version from typed server configuration; secret
  files must be absolute, regular, owner-restricted, and outside repository and
  release roots
- persist installation identity, GitHub account identity, selected repository
  access, enabled/revoked state, bounded last-success/failure category, and
  optimistic version; never persist the App private key, webhook secret,
  installation token, JWT, or arbitrary GitHub response bodies
- create one active Project-scoped Integration Actor for GitHub attribution and
  reuse it idempotently; the Actor grants no browser login and no Worker or
  Project-management authority
- expose an owner/admin status-only API and settings projection showing App
  identity, installation health, selected repositories, permissions, and last
  synchronization without returning host paths or credential fragments
- fail closed when the installation does not include the exact registered
  GitHub repository ID, omits the required metadata/contents/pull-request/check
  set, or grants undocumented elevated access beyond the optional Actions-read
  fallback
- retain GitHub repository/App creation, installation, permission changes, and
  browser authorization as explicit operator actions outside ordinary product
  APIs

**Dependencies:** accepted M0 GitHub compatibility note and development App,
M1 Actor/Project policy, M3 Repository identity, typed secret configuration,
and the host-local deployment boundary.

**Expected tests:** missing/unsafe secret files, configuration redaction,
installation/repository scope, permission projection, revoke/restore state,
Integration Actor idempotency, owner/admin/viewer policy, cross-Project
non-disclosure, and retained Red Hat credential-file permissions.

**Exclusions:** GitHub App creation UI, personal access tokens, user OAuth,
GitHub Enterprise Server, arbitrary Git hosts, organization-wide installation
management, and storing encrypted App keys in PostgreSQL.

### M5-002 Webhook verification and idempotency

Receive the minimal subscribed GitHub events on the isolated webhook listener
and convert verified deliveries into durable retryable work.

**Acceptance criteria**

- serve only `POST /api/v1/integrations/github/webhook` on the port-3101
  listener; ordinary browser/session routes remain on loopback port 3100 and
  the webhook listener never accepts browser cookies as authentication
- read a bounded raw request body, verify `X-Hub-Signature-256` in constant
  time before JSON decoding, require bounded `X-GitHub-Delivery` and the exact
  allowlisted event/action, and reject missing or ambiguous headers
- validate installation, repository numeric ID, owner/name, and supported event
  shape against durable registration before acknowledging the delivery
- insert the delivery ID under a unique constraint with received event/action,
  repository and installation IDs, payload checksum, processing state, and
  timestamps; do not retain arbitrary raw payload or headers after extracting
  the bounded allowlisted fields required by the job
- enqueue one versioned processing job in the same transaction; an exact
  duplicate returns success without a second job, state transition, activity,
  notification, or outbox event, while a reused delivery ID with a different
  checksum is rejected and audited
- acknowledge verified durable receipt promptly, process outside the request,
  and retain exhausted failures with a redacted category and operator retry
  path

**Dependencies:** M5-001 installation registration, existing request limits,
jobs/outbox design, port-3101 systemd/runtime boundary, and M0 signature and
duplicate-delivery fixtures.

**Expected tests:** valid/invalid signatures, header/body/control bounds,
unknown event/action, wrong installation/repository, exact duplicate,
same-delivery checksum mismatch, concurrent duplicate receipt, transaction
rollback, stale job lease recovery, poison payload isolation, and no secret or
raw payload leakage in responses/logs/database.

**Exclusions:** permanent public ingress, unauthenticated event replay,
repository dispatch, workflow-job log ingestion, issue webhooks, and treating
webhook order or completeness as authoritative without API reconciliation.

### M5-003 Installation-token Git authentication

Create a reusable backend client that mints one narrowly scoped installation
token for one child operation and then forgets it.

**Acceptance criteria**

- generate an RS256 App JWT with a bounded backdated issue time, expiration no
  longer than ten minutes, exact App issuer, and clock-skew validation; parse
  the configured PEM without logging errors that include key material
- request an installation access token for the exact repository ID and only
  the permissions required by the child API or Git operation, using REST API
  version `2026-03-10` and GitHub's JSON media type
- treat token bytes and expiry as opaque secret values, never assume a format
  or fixed length, keep them out of Debug/Display/serialization, and zeroize or
  drop the value immediately after the child operation
- apply bounded connect/request/response limits, verify HTTPS GitHub hosts, map
  provider errors into stable redacted categories, and never retry a mutation
  blindly after an uncertain response
- provide Git credentials through an owner-only ephemeral `GIT_ASKPASS`
  callback and child-only sanitized environment; disable prompts, hooks,
  inherited credential helpers, and unsafe repository/global configuration
- ensure tokens, JWTs, private-key paths, browser/database/MCP secrets, and
  unrelated environment keys are absent from Agent Run and provider children

**Dependencies:** M5-001 configuration and installation state, M0 token flow
fixtures, existing secret wrapper/process limits, and trusted Repository IDs.

**Expected tests:** JWT claims, clock bounds, malformed key, opaque long token,
repository/permission request scope, HTTP timeout/oversize/redaction, askpass
mode and deletion, exact child environment, disabled helper/hook configuration,
wrong remote host/repository, and child failure cleanup.

**Exclusions:** durable token cache, PAT fallback, token delivery to Codex,
SSH deploy keys, user impersonation, refresh tokens, and credentials embedded
in clone URLs, arguments, Git config, logs, or database rows.

### M5-004 Push branch with installation credentials

Publish an accepted managed workspace branch through an explicit authorized
action without hiding local Git state or inventing a commit.

**Acceptance criteria**

- expose an idempotent publish command for an authorized terminal Agent Run or
  its repository workspace; lock and inspect the exact database-owned mirror,
  branch, worktree, recorded base, repository identity, and current commit
- require no active run, a clean worktree, a valid owned branch, ancestry from
  the recorded base, and at least one local commit ahead of base; if dirty or
  commitless, preserve everything and return an actionable instruction to use
  the Thread/Worker to commit intended changes
- enqueue the external push as a durable job, release every database and
  repository metadata lock before network I/O, and serialize only short shared
  Git metadata mutations rather than unrelated repository execution
- mint a child-only token through M5-003, push the exact local commit to the
  exact expected remote branch with argument-array Git, and never use force,
  force-with-lease, wildcard refspecs, or caller-provided remote URLs
- resolve retries and uncertain outcomes by comparing the exact GitHub remote
  ref commit with the intended local commit: equal is success, absent may retry,
  and divergent is a visible conflict that is never overwritten
- persist published commit/time and Actor-attributed activity/audit only after
  the remote identity is proven; emit outbox invalidation and keep the local
  workspace intact on every failure

**Dependencies:** accepted M3 repository workspace/worktree ownership, M5-003
credential client, durable jobs, terminal Agent Runs, and short repository
metadata locks.

**Expected tests:** authorization, active/dirty/clean/commitless/divergent Git
states, branch/remote sanitation, no-force command arrays, no lock during child
execution, successful push, exact replay, concurrent publish, uncertain success
reconciliation, token/askpass cleanup, outbox/audit attribution, and live push
to the retained private spike repository.

**Exclusions:** backend staging or committing arbitrary dirty files, force
push, branch deletion, tags/releases, arbitrary refspecs/remotes, pushing from
an unmanaged worktree, and granting network credentials to the provider.

### M5-005 Pull request model and linking

Create and project the minimal durable GitHub pull request artifact, with
explicit links taking precedence over bounded heuristics.

**Acceptance criteria**

- add Project-scoped pull-request records keyed by GitHub repository and pull
  request numeric IDs, with immutable node/URL/head/base identity, bounded
  title, author login, draft/state/merge fields, synchronization timestamps,
  and optimistic version
- add same-Project many-to-many links to Work Items, application Threads,
  repository workspaces, Agent Runs, and commits where their identities are
  known; clients never supply Project or repository ownership columns
- expose an idempotent create/link action after M5-004 proves the branch is
  published; create through the installation API with bounded title/body/base,
  exact head branch, and no hidden fork or maintainer-modification assumptions
- resolve an uncertain create by querying the exact repository/head/base tuple
  before retrying; one workspace branch cannot silently create duplicate open
  pull requests
- link in precedence order: explicit authenticated action, recorded Agent Run
  artifact, exact Work Item key in branch, then exact key marker in body;
  heuristic matches never cross a Project/repository and remain visible and
  removable
- render linked pull-request summary and GitHub link on Work Item and Thread
  views; detailed diff, comments, review submission, merge, and close remain on
  GitHub

**Dependencies:** M4 Work Item/Thread/Repository links, M5-004 published branch,
GitHub API client, artifacts model, and Project-scoped policy/outbox.

**Expected tests:** schema foreign keys/uniqueness, explicit and heuristic link
precedence, false-positive/cross-Project rejection, create replay and uncertain
outcome, duplicate-open-PR prevention, hostile title/login escaping, archived
links, outbox/realtime refetch, and real private-spike PR creation.

**Exclusions:** in-app code diff or review submission, merge/close controls,
fork pull requests, stacked PRs, issue linkage, release notes, and copying full
GitHub bodies/comments into Agent Workspace.

### M5-006 Checks and review state

Maintain a bounded GitHub-owned status projection sufficient to answer whether
linked code is ready for human review or verification.

**Acceptance criteria**

- process pull-request, review, push, check-suite, and check-run deliveries into
  versioned synchronization jobs, then fetch canonical current state when an
  event is partial, duplicated, out of order, or ambiguous
- store bounded check-run identity/name/status/conclusion/details URL and latest
  timestamps plus summarized review decision/requested-reviewer state; do not
  ingest logs, annotations, diffs, review bodies, email addresses, or arbitrary
  webhook JSON
- compute one deterministic pull-request projection for draft/open/closed/
  merged state, checks pending/success/failure, review pending/approved/changes
  requested, and mergeability unknown/blocked/ready without claiming stronger
  certainty than GitHub supplies
- make stale/unknown state explicit, record last successful sync and a redacted
  failure category, and provide authorized manual refresh through an
  idempotent rate-limited job command
- render the summary, significant changes, and direct GitHub link on linked
  Work Items and Threads with accessible text/icons; realtime is invalidation
  and every reconnect or missed delivery converges through HTTP refetch
- attribute GitHub-originated activity to the Integration Actor and preserve
  immutable external event time separately from local receipt time

**Dependencies:** M5-002 jobs/webhooks, M5-005 pull-request identity/links,
Checks read permission, optional Actions read only if documented as necessary,
and existing realtime/outbox convergence.

**Expected tests:** event-order permutations, duplicate/missed webhooks, API
reconciliation, check/review state reduction, stale/unknown projection,
hostile external strings, payload/row bounds, refresh authorization/rate limit,
realtime reconnect/refetch, and live check-state read on the spike PR.

**Exclusions:** workflow logs, annotation browsing, required-check policy
editor, review comments, requested-review notifications beyond the initial
summary, merge queue, deployment status, and GitHub Actions control.

### M5-007 Work Item automation from pull request state

Apply only explicit Project-owned workflow rules and make every resulting
transition visible and reversible by ordinary authorized work.

**Acceptance criteria**

- store optimistic Project GitHub automation settings with pull-request-open
  transition disabled by default and linked-pull-request-merge transition to
  semantic `verification` enabled by default; owner/admin can disable either
  rule without changing workflow definitions
- evaluate automation only after canonical M5-006 state is committed and only
  for explicitly linked code Work Items; heuristic-only links require human
  confirmation before status automation
- move an eligible open item to the configured semantic category only when one
  unambiguous enabled status and an allowed transition exist; otherwise leave
  state unchanged with a bounded visible skip reason
- never reopen semantic `completed` or `canceled` items, never mark work Done,
  never infer completion from an Agent Run, and never bypass optimistic locking
  or custom Project workflow transitions
- make the transition idempotent per pull-request state/version and record
  Integration Actor activity containing previous/new status and the linked PR,
  plus audit and outbox entries in the same transaction
- concurrent human edits win or produce ordinary version-aware reevaluation;
  automation never overwrites a status based on stale state

**Dependencies:** M4 workflow/activity/versioning, M5-001 Integration Actor,
M5-005 explicit links, and canonical M5-006 pull-request state.

**Expected tests:** default/disabled settings, open/merge rules, heuristic-link
hold, invalid/ambiguous transition skip, Done/Canceled preservation, duplicate
and out-of-order delivery, concurrent human edit, Actor attribution, outbox,
and browser-visible live merge-to-Verification evidence.

**Exclusions:** automatic Done, reopening, model-decided status, arbitrary rule
builder, label/assignee automation, GitHub-side mutation, scheduled automation,
and Worker-completion automation.

### M5-008 Safe cleanup and repair

Reconcile GitHub, database, Git, and filesystem state without deleting unique
work or hiding integration failure.

**Acceptance criteria**

- add startup/periodic reconciliation for stale jobs, installation access,
  registered repositories, published refs, open pull requests, checks/reviews,
  and worktree/branch ownership; every external check is bounded and retryable
- expose owner/admin integration diagnostics and retry actions using stable
  failure categories, last attempt/success times, and exact affected resource
  IDs without raw provider responses, secrets, host paths, or repository data
- mark revoked installations and removed repository access unavailable, stop
  minting tokens immediately, and preserve all local mirrors, branches,
  worktrees, runs, links, and activity for recovery
- allow automatic worktree cleanup only after no active run owns it, Git status
  is clean, its pull request is merged/closed or the workspace is explicitly
  abandoned, and the configured retention window elapsed
- re-inspect immediately before removal, hold only the short repository lock,
  remove through the owned Git worktree command, prune/repair metadata, and
  record success; dirty, missing, foreign, divergent, or uncertain state is
  preserved for explicit operator repair
- branch deletion, repository/App deletion, and destructive dirty-worktree
  cleanup always remain separately authorized actions and are not M5 exit
  automation

**Dependencies:** accepted M3 recovery/Git ownership, M5 durable jobs and state,
M5-004/005 published branch and PR lifecycle, and configured retention policy.

**Expected tests:** stale leases, revoked access, missing remote ref, divergent
branch, merged/closed/open PR, active run, clean/dirty/missing/foreign worktree,
retention boundary, concurrent re-dirty race, idempotent prune/repair, audit and
operator retry, restart convergence, and retained dirty-tree census on Red Hat.

**Exclusions:** deleting the retained GitHub App or spike repository, automatic
dirty cleanup/reset, force push, remote branch deletion, repository garbage
collection that can discard unique objects, permanent public tunnel, and full
disaster-recovery/backup policy (M8).

### M5 exit criteria

- the private development GitHub App is configured through owner-restricted
  host files, installed only on the retained spike repository, and exposes no
  secret material through logs, PostgreSQL, browser DTOs, or provider children
- a real browser action publishes one clean managed branch with a short-lived
  installation token, creates and explicitly links one pull request, and
  renders canonical PR/check/review state on its Thread and Work Item
- a signed webhook delivered through the bounded temporary tunnel is verified,
  deduplicated, processed through a durable job, and converges after a duplicate
  and one intentionally missed/out-of-order refresh case
- merging the explicitly linked PR moves an eligible Work Item to Verification
  exactly once with Integration Actor activity; disabled, closed, ambiguous,
  and invalid-transition cases remain unchanged and visible
- controlled restart recovers stale webhook/GitHub jobs and reconciles remote
  state without manual database edits, duplicate PRs/activity, leaked tokens,
  force push, lost commits, or changed dirty worktrees
- the tunnel is stopped immediately after delivery proof; the disposable PR is
  closed or merged and its test branch is deleted only through an explicit
  accepted cleanup step, while the private spike repository and App remain
- PostgreSQL, job/idempotency, Git, HTTP, webhook, provider-environment,
  frontend accessibility, Red Hat, and browser tests pass against one exact
  green commit, and the M6 backlog is elaborated before M6 begins


## Milestone M6: Local Codex and MCP

### M6-001 MCP Streamable HTTP endpoint

### M6-002 Scoped bearer-token development auth

### M6-003 MCP read tools

### M6-004 MCP write tools

### M6-005 Optimistic conflict handling

### M6-006 MCP audit records

### M6-007 Open in Codex deep link

### M6-008 Local setup documentation

### M6-009 OAuth design and implementation

## Milestone M7: Teams

### M7-001 Invitation links

### M7-002 Member management

### M7-003 Centralized role-to-action policy

### M7-004 Viewer restrictions

### M7-005 Per-user Thread attention validation

### M7-006 Team Execution Profile policy

### M7-007 Two-user end-to-end test

## Milestone M8: Hardening

### M8-001 Backup and restore

### M8-002 Rate limits and abuse controls

### M8-003 Secret rotation

### M8-004 Retention and disk quotas

### M8-005 Accessibility review

### M8-006 Performance budgets and profiling

### M8-007 Deployment guide

### M8-008 Private beta checklist

## Milestone M9: External agent members

**Status:** Deferred until after version one. Not authorized by the current implementation approval.

### M9-001 External agent connection threat model and protocol decision

Resolve O-007 before implementation.

### M9-002 External-agent credential registration, rotation, and revocation

### M9-003 External Agent Member onboarding and Project membership

### M9-004 Actor-authored Messages, mentions, and activity through the integration surface

### M9-005 Work Item reads, writes, and assignment

### M9-006 Approval request routing and decision policy

### M9-007 Execution Worker invocation by an external Actor

### M9-008 Event delivery or polling, cursors, idempotency, and connection diagnostics

### M9-009 One-runtime end-to-end connector

Choose one runtime, such as Hermes or OpenClaw. Do not generalize across several runtimes until one real connector proves the contract.

### M9 exit criteria

- an authorized human can connect, permission, rotate, disable, and remove one external agent member
- the external agent appears and communicates as its own Actor
- its Project reads and writes pass the same authorization engine as human actions
- it can create or update a Work Item and invoke an Execution Worker when permitted
- approvals, revocation, retries, messages, and audit attribution pass the focused security test suite
- Agent Workspace does not host or assume control of the external runtime's memory, schedules, model, or autonomy loop

---

# 28. Exact first implementation sequence

Do these in order.

## Step 1: Create the repository

Use the directory containing this document as the repository root.

If `.git` does not exist, initialize it. If it already exists, do not create a nested `agent-workspace` directory.

The canonical document path is:

```text
PROJECT_SOURCE_OF_TRUTH.md
```

Create:

- a short `README.md` that links to this document
- an `AGENTS.md` containing repository commands, verification expectations, architectural-change rules, and the instruction to consult this source of truth

Do not duplicate the entire product plan into `AGENTS.md`.

## Step 2: Pin tools

Create:

```text
rust-toolchain.toml
```

Pin the current stable Rust toolchain at repository initialization.

Use Node active LTS and pin it with the team's chosen version manager. Use `pnpm` for the frontend workspace.

## Step 3: Bootstrap the Rust server

```bash
mkdir -p apps
cargo new apps/server --bin
```

Create root `Cargo.toml` workspace configuration.

Add only the baseline dependencies needed for:

- configuration
- Axum health endpoint
- Tokio
- tracing
- SQLx

## Step 4: Bootstrap the frontend

```bash
pnpm create vite apps/web --template react-ts
```

Add the root pnpm workspace configuration.

Do not build the final visual design yet. Build the four-region application shell and route structure.

## Step 5: Add PostgreSQL development environment

Create `docker-compose.dev.yml` with PostgreSQL only.

Add `.env.example` containing non-secret configuration names.

Create the first SQLx migration.

## Step 6: Add CI before features

CI must run formatting, linting, tests, and builds for both Rust and TypeScript.

## Step 7: Complete the four technical spikes

Do not spend weeks polishing Channels before App Server, MCP, worktrees, and GitHub delivery are proven.

Complete:

1. Codex bridge spike
2. MCP spike
3. Git worktree spike
4. GitHub delivery spike

Record findings and update this document where assumptions were wrong.

## Step 8: Build the walking skeleton

Compose the spike results into one deliberately plain end-to-end Thread run.

Do not advance because four isolated demos worked independently. Advance when the application owns the durable records and can reconcile them after restart.

## Step 9: Build the application foundation

Then build Actor, human User, Organization, Actor-backed Project membership, project rail, Channel, Actor-authored Message, and Thread foundations.

## Step 10: Integrate hosted `@agent`

Implement `@agent` as the default Execution Worker and complete the Thread loop, including a registered repository and managed worktree, before the full Work Item system.

## Step 11: Add Work Items and GitHub

Once Worker execution works, connect it to structured work and pull requests.

---

# 29. Definition of done for technical alpha

The technical alpha is complete when one user can:

1. sign in
2. create two Projects
3. switch between them without losing navigation state
4. create Channels
5. create and use Threads
6. create a Work Item
7. link a Work Item to a Thread
8. configure the default `@agent` Execution Worker
9. sign the Project Host into Codex
10. start hosted Execution Worker work in a Thread
11. see streamed Worker messages and significant tool events
12. answer a blocker or approval
13. see correct blue, orange, white, gray, and red states
14. run the work in an isolated Git worktree
15. push a branch and link a pull request
16. open the same Work Item in local Codex
17. let local Codex read and update it through MCP without creating a Thread
18. see GitHub checks and merge state reflected in the Work Item

---

# 30. Definition of done for version one

Version one additionally requires:

1. invite at least one teammate
2. enforce Project roles
3. maintain Actor-backed read and unseen state for human users
4. prevent cross-Project data access
5. support safe team Execution Profile policy
6. audit Worker approvals, requester attribution, and MCP writes
7. recover from process restart without losing durable project state
8. back up and restore the database and application data
9. document self-hosted installation
10. pass accessibility and performance gates

Version one does not require a Hermes, OpenClaw, or other external-agent connection. It does require the Actor-backed identity, authorship, assignment, authorization, approval, and Agent Run requester fields that keep Milestone M9 additive.

---

# 31. Decision log

## D-001: Rust backend

**Status:** Accepted

**Reason:** Strong fit for a long-running, low-overhead, process-supervising, security-sensitive server. Motivation matters, and Rust is acceptable if constrained by the guardrails in this document.

## D-002: React and TypeScript frontend

**Status:** Accepted  
**Reason:** Mature ecosystem for complex collaborative UI, virtualization, drag and drop, editors, and realtime state.

## D-003: Modular monolith

**Status:** Accepted  
**Reason:** Preserves clear boundaries without paying the operational and cognitive cost of microservices.

## D-004: Single Project Host for version one

**Status:** Accepted  
**Reason:** Simple always-on shared system with predictable hosted Codex execution.

## D-005: Default Execution Worker handle is `@agent`

**Status:** Accepted as provisional product language  
**Reason:** The user-facing Worker identity should not be inseparable from its provider. The handle may change during naming and UX work.

## D-006: Codex App Server is the version-one hosted provider

**Status:** Accepted  
**Reason:** Required rich-client capabilities are available through App Server.

## D-007: Local Codex does not create an app Thread

**Status:** Accepted  
**Reason:** Local work is an independent workflow that reads and updates Work Items through MCP.

## D-008: Work Item is canonical, Kanban is a view

**Status:** Accepted  
**Reason:** One flexible planning object can support list, board, review, and attention views without duplicating workflow state.

## D-009: GitHub remains code and review source of truth

**Status:** Accepted  
**Reason:** Reimplementing Git hosting and full pull-request review would dilute the coordination product and create conflicting artifact state.

## D-010: Organization exists in the schema but is hidden for solo use

**Status:** Accepted  
**Reason:** The ownership boundary is cheap to establish before data exists and expensive to retrofit, while the solo interface does not need to expose it.

## D-011: No generic agent framework in version one

**Status:** Accepted  
**Reason:** A narrow Codex Worker Provider adapter can preserve provider strengths and prove the product without speculative abstraction. External agent members use a separate client-integration boundary rather than the Worker Provider interface.

## D-012: Codex cloud provider is deferred

**Status:** Accepted  
**Reason:** Preserve the provider boundary, but do not implement against an incomplete integration contract.

## D-013: No visible queued color

**Status:** Accepted  
**Reason:** Queueing is an active substate that deserves explicit text, not a durable navigation meaning or another attention color.

## D-014: One active Agent Run per Thread initially

**Status:** Accepted  
**Reason:** One active run keeps provider-session ownership, approvals, recovery, and derived Thread attention deterministic.

## D-015: Message references are structured

**Status:** Accepted  
**Reason:** Stable mentions and Work Item references must not depend on parsing display text.

## D-016: Pull request merge moves linked code work to Verification

**Status:** Accepted  
**Reason:** A merged implementation is not automatically verified product work. The automation is visible and configurable, and a human or explicit Project rule closes the final step to Done.

## D-017: Start with a minimal structured composer

**Status:** Accepted  
**Reason:** Version one begins with plain text plus structured mention and Work Item reference tokens. Do not adopt a general rich-text editor framework until editing requirements exceed this model.

## D-018: Conversation entries provide durable timeline ordering

**Status:** Accepted  
**Reason:** Messages and significant provider events need one pagination and read-state cursor. Timestamps alone are not a sufficient ordering contract.

## D-019: Technical alpha hosted execution is Thread-based

**Status:** Accepted  
**Reason:** Threads provide clear ownership for provider sessions, worktrees, approvals, recovery, and attention. Inline Channel execution remains a gated version-one extension.

## D-020: Managed worktrees ship with hosted execution

**Status:** Accepted  
**Reason:** Worktree creation is part of the hosted Agent Run contract, not a later GitHub presentation feature.

## D-021: First-user bootstrap is host-local

**Status:** Accepted  
**Reason:** A host-local, users-table-empty bootstrap command avoids default credentials and an exposed setup route while keeping private development simple.

## D-022: Sent Messages are immutable in version one

**Status:** Accepted  
**Reason:** Editing content already delivered to a provider would make the application transcript disagree with provider history. Follow-up corrections keep both histories truthful.

## D-023: One Actor model owns attribution and authorization

**Status:** Accepted  
**Reason:** Human users, external agent members, execution workers, integrations, and system actions need one durable identity shape so Messages, assignments, Worker requests, approvals, permissions, and audit history do not require a future migration from user-only fields.

## D-024: External agent members and Execution Workers are separate roles

**Status:** Accepted  
**Reason:** Hermes, OpenClaw, and similar systems behave as externally operated project teammates, while Codex App Server performs bounded execution. Forcing both through one provider interface would conflate collaboration identity with execution mechanics.

## D-025: External agent runtimes remain externally owned

**Status:** Accepted  
**Reason:** Agent Workspace controls connected Actor access to its own Projects and resources. It does not own the external agent's model, memory, prompts, schedules, automations, budgets, loops, or session duration.

## D-026: External agent integration is deferred until after version one

**Status:** Accepted  
**Reason:** Actor-compatible schema and policy seams are inexpensive and necessary now, while a safe credential, event, presence, and runtime-specific integration contract requires a later focused milestone.

## D-027: The combined collaboration and work-management product is the core

**Status:** Accepted  
**Reason:** Channels, Messages, Threads, Work Items, list and Kanban views, Execution Workers, and GitHub artifacts form one coordination loop. The product should feel Slack-like in collaboration without being reduced to a chat client or split into unrelated mini-applications.

## D-028: Commercial options remain open without entering version-one scope

**Status:** Accepted  
**Reason:** The application domain, data, UX, and Worker Provider boundaries should remain commercially usable and replaceable. Billing, public SaaS isolation, and a final OpenAI authentication and cost model are deferred and must not block the private self-hosted build.

## D-029: M0 uses verified published toolchain pins

**Status:** Accepted

**Reason:** The planned Rust `1.98.0` artifact did not exist for the target architectures when M0 began. M0 pins Rust `1.97.1`, the current published stable release verified from the official channel manifest. Future pin changes require the same artifact check before CI or host installation.

## D-030: PostgreSQL 18 mounts the cluster parent directory

**Status:** Accepted

**Reason:** The pinned PostgreSQL 18 container image stores versioned database
clusters below `/var/lib/postgresql`. Mounting a named volume at the older
`/var/lib/postgresql/data` path is rejected by the image and prevents startup.
Docker Compose and rootless Podman Compose therefore mount the isolated
Agent Workspace volume at `/var/lib/postgresql`.

## D-031: Codex 0.146.0 negotiates MCP protocol 2025-06-18

**Status:** Accepted

**Reason:** The isolated local Codex `0.146.0` profile negotiated MCP protocol
`2025-06-18` over the SSH-forwarded Streamable HTTP endpoint and called both
the read and optimistic-write tools. The exact compatibility result is pinned
for M0; a Codex or SDK upgrade must repeat negotiation and fixture validation.

## D-032: Startup recovery completes only when provider history proves completion

**Status:** Accepted

**Reason:** On restart, the host restarts the profile App Server, resumes the
stored provider Thread, reads its latest turn, and inspects the physical
worktree. Recovery must match that history to the exact durable
`provider_turn_id`; a missing application turn binding or a different latest
provider turn is ambiguous and can never inherit an older completion. A
completed provider turn may be reconciled as completed. An
interrupted, non-terminal, missing, or ambiguous turn becomes an actionable
application `interrupted` result with its worktree preserved. This proved the
required M0 recovery boundary without inventing continuation or resetting
dirty state. Recovery is re-entrant for a run already marked `recovering`, and
an `active` provider session returns to `idle` once no linked Agent Run remains
non-terminal, so a second service interruption cannot strand the Thread.

## D-033: Private-build authentication uses host-bootstrapped local sessions through M6

**Status:** Accepted as an interim private-build boundary

**Reason:** M1 needs real human identity and server-resolved Actor sessions
before Projects, Conversations, and execution can be authorized correctly.
Host-local bootstrap plus Argon2id password verification and opaque,
server-side cookie sessions provide that boundary without exposing a setup
route or choosing the final team authentication product. Passkeys, OIDC, and
the final combination remain O-004 and must be resolved before M7.

## D-034: M2 fixes a small bounded version-one Message AST

**Status:** Accepted

**Reason:** M2 Messages need semantic Actor mentions without adopting a general
rich-text system. Version one therefore accepts only an explicitly tagged
`version = 1` object containing one to 128 flat `text` or `mention` nodes, at
most 32 mention occurrences, and at most 20,000 Unicode scalar values in both
the submitted text and the server-derived plain-text rendering. Mention IDs
must be non-nil UUIDs and resolve to enabled active same-Project memberships;
duplicate occurrences remain in the immutable AST but project to one distinct
Message/Actor relationship. Text permits line feed, carriage return, and tab
but rejects other control characters. Unknown fields, unknown node types,
nested shapes, URL nodes, HTML authority, invalid handles, and empty derived
content are rejected. This bound keeps storage, rendering, notification, and
provider inputs deterministic while leaving future AST versions additive.

## D-035: Conversation existence is hidden with a common not-found response

**Status:** Accepted

**Reason:** A caller who cannot currently view a Channel or Thread must not be
able to distinguish it from a missing Conversation. Conversation-specific
timeline, Message, Thread, archive, read-state, and navigation operations
therefore return the same `404 Not Found` resource response for missing,
cross-Project, disabled-membership, and otherwise unauthorized Conversation
targets. Authentication, Origin, CSRF, malformed input, and authorized role
failures keep their own responses. This Conversation non-disclosure rule
supersedes the older M1 test expectation that every policy denial surface as a
generic `403`.

## D-036: Realtime correctness is at-least-once invalidation plus HTTP refetch

**Status:** Accepted

**Reason:** A durable M2 mutation and its typed outbox event commit in one
PostgreSQL transaction. A supervised worker claims events by database cursor
with a short lease, publishes them at least once under a stable UUIDv7 event
ID, and records `published_at` after handing the event to the in-process
Project-scoped broadcaster. Publication does not claim that any browser
received the event. The single authenticated session WebSocket is an
invalidation and ephemeral-stream channel: clients bound duplicate IDs,
tolerate future event types, coalesce refetches, and recover navigation,
timeline, read, and attention state through authorized HTTP after reconnect.
No per-client replay or exactly-once network-delivery promise is implied.

## D-037: Applied migrations are immutable; M2 corrections remain additive

**Status:** Accepted

**Reason:** The retained Red Hat validation database records the exact SHA-384
checksums of every applied migration. `0005_m2_conversations.sql` remains
byte-for-byte immutable. Its initial `message_mentions` shape included a
`node_index`; the same unreleased M2 migration set already projected repeated
AST occurrences to one distinct Actor row. The immutable additive
`0006_m2_message_mentions_projection.sql` drops the occurrence key and
establishes `(message_id, actor_id)` as the projection key while retaining
same-Project/AST validation and mutation rejection. The Message AST remains
the authoritative occurrence and ordering record.

`0007_m2_archived_parent_navigation.sql` makes a Thread below an archived
Channel invalid as a remembered active route while preserving explicit
history access. `0008_m2_conversation_lock_boundaries.sql` replaces the parent
and navigation validation functions additively so `FOR SHARE` serializes their
active-state decisions with concurrent archive updates. Versions `0005`
through `0008` are now applied to the retained validation database and are all
immutable. Any later correction must use a new migration; application code,
tests, and deployment tooling must never repair a checksum by rewriting an
applied file or editing `_sqlx_migrations`.

## D-038: M3 execution admission is fail-closed and personal-profile owned

**Status:** Accepted

**Reason:** An adopted provider or repository row is not proof that its current
host state is executable. M3 therefore records adopted Execution Profiles as
`account_state = unknown` and adopted Repositories as
`preparation_state = pending`. A non-terminal Agent Run may enter admission,
`starting`, or `working` only when its Provider Connection, Worker Actor and
membership, assigned personal Execution Profile and owner, and trusted prepared
Repository are all active and same-Project. The requesting Actor must own the
personal profile used by the M3 run. Project-service and team-sharing policy
remains intentionally open under O-003 for M7.

The retained validation database applied `0009_m3_execution_foundation.sql`
before its final cross-review, so that migration remains byte-for-byte
immutable. `0010_m3_execution_admission_hardening.sql` additively enforces
profile authentication, repository preparation, Worker/Profile identity,
runtime-path uniqueness, archive-attribution immutability, and deferred
Repository/run consistency. The application revalidates the same admission
facts before selection and when a queued run becomes executable.

Each profile directory is created beneath the configured data root through
descriptor-relative no-follow operations with owner-only permissions. The
pre-`0009` startup preflight may canonicalize only the two relative paths
written by the known M0 development seed and may conservatively disable a
Worker whose dependencies are unavailable. Unknown legacy paths or ambiguous
ownership fail closed for explicit operator repair; startup never guesses,
moves credentials, or rewrites an applied migration.

## D-039: Retained private mirrors activate through verified offline import

**Status:** Accepted

**Reason:** M3 owns repository isolation and worktree execution but does not
yet own GitHub App credential minting, which remains M5. The retained M0 spike
repository is private, and the validation host intentionally stores no GitHub
credential. Startup therefore copies its already verified legacy mirror into
the canonical UUID-scoped location and records the immutable legacy path.
Only that recorded imported-mirror case may activate without a network fetch:
the preparation command must find the canonical mirror, verify its exact
credential-free origin, and resolve the configured default branch. Missing,
mismatched, or invalid imported state fails closed and is never cloned or
repaired by guessing. Newly registered repositories continue to clone or
perform a pruned remote update, while short-lived authenticated fetch and push
remain M5 responsibilities. No GitHub credential enters the M3 service,
provider environment, repository configuration, database, or logs.

## D-040: M3 accepts live command blockers without requiring experimental questions

**Status:** Accepted

**Reason:** The pinned App Server schema includes the experimental
`item/tool/requestUserInput` request and the application retains a bounded,
typed, durable question path for compatibility. During exact-release M3
acceptance, however, the hosted model reported that `request_user_input` was
unavailable in its current execution mode and emitted an ordinary Message
instead of that provider request. Live command approval requests did emit and
completed the full durable browser blocker flow. M3 therefore requires one
real supported provider blocker and deterministic coverage of both approval
and question adapter shapes; it does not claim that every hosted model or
execution mode can elect to emit the experimental question request. No raw
experimental provider shape enters the domain or browser contract.

## D-041: M4 uses one versioned Work Item with normalized criteria and append-only collaboration

**Status:** Accepted

**Reason:** The Work Item is the canonical mutable planning record and owns one
optimistic version across its non-commutative field and checklist commands.
M4 descriptions remain bounded plain text. Acceptance criteria are ordered
rows with explicit completion attribution rather than an opaque JSON document.
Comments are immutable Actor-authored records that reuse the version-one
structured content contract, while typed activity is a separate append-only
mutation projection. This keeps concurrency, authorship, references, and
auditing explicit without adopting a general rich-text editor or revision
system.

## D-042: Project workflow definitions are durable; list and Kanban are projections

**Status:** Accepted

**Reason:** Work Item types, statuses, semantic status categories, allowed
transitions, and order belong to the Project and use stable IDs. Domain logic
never depends on editable display names. A Work Item stores its current status
and bounded rank; list and Kanban query the same versioned record. Moving a
Kanban card is therefore an ordinary optimistic status/rank command, not a
mutation of a separate board model. M4 ships the default workflow read-only so
future customization can be additive and measured.

---

# 32. Open decisions

These are intentionally unresolved and should be answered through prototypes or focused planning.

## O-001: Final product name

Do not let naming block the technical spikes. Keep the working title internal.

**Decision required before:** public-facing beta naming and branding.

## O-002: Inline Channel invocation contract and composer control

The technical alpha uses Thread-based execution. Prototype and specify inline run ownership, provider-session mapping, concurrency, repository context, approvals, recovery, and `Continue in Thread` behavior before enabling Reply here.

**Decision required before:** any inline Channel Agent Run implementation. This does not block the M3 Thread loop.

## O-003: Team Execution Profile policy

Choose between invoker-owned and project service-owned execution before team-hosted agent work ships.

**Decision required before:** M7 entry.

## O-004: Final application authentication methods

Private development can use a bootstrap account. Team release needs a final decision among local passwords, passkeys, OIDC, or a combination.

**Decision required before:** M7 entry.

## O-005: Attachment storage limits

Set during hardening.

**Decision required before:** M8 private-beta exit.

## O-006: Public SaaS future

Not part of version one. A hostile multi-tenant execution platform requires a new threat model and remote isolated executors.

**Decision required before:** any public multi-tenant deployment work. It does not block version one.

## O-007: External agent connection protocol

Choose the first supported runtime and specify registration, authentication, credential storage, rotation, revocation, MCP or HTTP operations, event delivery or polling, cursors, presence, rate limits, approval routing, and diagnostics through a focused threat model.

The design must preserve the external ownership boundary and use the ordinary Actor authorization and domain command layers.

**Decision required before:** M9 entry. It does not block version one.

## O-008: Commercial provider and authentication model

Before charging for or broadly distributing a hosted product, choose and validate the supported execution model:

1. each customer connects their own eligible Codex or ChatGPT account
2. each customer supplies their own API credentials
3. Agent Workspace uses project-owned API credentials and incorporates usage into its own pricing
4. another supported Worker Provider supplies execution

Confirm current provider terms, authentication and credential-distribution rules, enterprise or known-client registration requirements where applicable, feature differences by authentication method, unit economics, abuse controls, and legal review. Open-source licensing of the Codex client code does not by itself grant rights to resell or redistribute hosted model access.

**Decision required before:** paid public distribution or billing implementation. It does not block self-hosted version one.

---

# 33. Architecture rules that prevent future destruction

These are the most important long-term safeguards.

1. Application Threads and Provider Sessions remain separate records.
2. External Agent Members, Execution Workers, and Worker Providers remain separate concepts and records.
3. Every attributable action references an Actor; credentials and sessions are not domain identities.
4. Project memberships, Message authorship, Work Item assignment, Agent Run requests, approvals, and audit records are Actor-compatible.
5. Human and non-human Actors pass through the same centralized action-on-resource authorization engine.
6. An external agent runtime remains an external client; Agent Workspace never assumes ownership of its memory, schedules, model, or autonomy loop.
7. Work Items and Conversations remain separate and link many-to-many.
8. Every Project-scoped query is authorized and filtered by Project.
9. Organizations exist before team growth, even if hidden in solo UI.
10. Read and attention state is Actor-compatible from the beginning.
11. Message mentions and references are structured entities.
12. Provider events are normalized through one Worker Provider adapter boundary.
13. Provider raw payloads do not leak throughout the domain model.
14. Background jobs are durable.
15. Realtime delivery uses a transactional outbox.
16. The WebSocket is never the source of truth.
17. GitHub is never reimplemented as an internal fake Git host.
18. Git commands never interpolate user strings into a shell.
19. Secrets never enter ordinary database records or browser APIs.
20. Solo shortcuts must not bypass the permission model.
21. Team and external-agent support must not require changing ownership fields on every core object.
22. Future Worker Providers are added through capabilities, not through pretending every provider behaves like Codex.
23. External agent connections use client-neutral domain operations, never a privileged parallel database API.
24. Rust modules stay boring and explicit.
25. Measured problems, not imagined scale, justify new infrastructure.

---

# 34. Verified external constraints and reference sources

These references should be rechecked when implementing the related integration because external products evolve.

1. OpenAI Codex App Server documentation: rich-client integration, JSON-RPC, stdio transport, authentication, Threads, turns, items, approvals, streaming, and schema generation.  
   https://learn.chatgpt.com/docs/app-server

2. OpenAI Codex authentication documentation: ChatGPT and API-key login, device-code login, `CODEX_HOME`, cached credentials, and credential handling.  
   https://learn.chatgpt.com/docs/auth

3. OpenAI Codex MCP documentation: Streamable HTTP, bearer and OAuth authentication, shared local-client MCP configuration, tool approval behavior, and server instructions.  
   https://learn.chatgpt.com/docs/extend/mcp?surface=cli

4. OpenAI Codex commands documentation: `codex://` deep links, prefilled prompts, absolute paths, and Git remote matching through `originUrl`.  
   https://learn.chatgpt.com/docs/reference/commands

5. OpenAI Workspace Agents trigger documentation: future-provider research only. It is not a version-one integration contract and must be re-evaluated before any implementation.  
   https://learn.chatgpt.com/workspace-agents/trigger-runs

6. Git worktree documentation: linked worktrees, shared repository data, lifecycle, repair, and limitations.  
   https://git-scm.com/docs/git-worktree.html

7. GitHub App documentation: webhook delivery and short-lived installation access tokens.  
   https://docs.github.com/en/rest/apps/apps  
   https://docs.github.com/en/rest/apps/webhooks

8. OpenAI Codex repository and Apache-2.0 license: source-code licensing for Codex and App Server. Service access, authentication, and commercial terms remain separate concerns.  
   https://github.com/openai/codex

9. OpenAI Services Agreement: current service terms must be rechecked before a paid or broadly distributed integration.  
   https://cdn.openai.com/osa/openai-services-agreement.pdf

---

# 35. Final implementation posture

Build this as a serious personal tool with product-quality foundations.

Do not assume it will become a massive company.

Do not sabotage it by assuming it cannot.

The correct target is:

```text
excellent for one technically ambitious person
        +
coherent for a trusted team
        +
architecturally capable of becoming a broader product
```

The intended product should feel immediately understandable as a Slack-like project home:

```text
Projects
├── members
├── Channels, Messages, mentions, and Threads
├── Work Items shown as lists, details, and Kanban boards
├── human and, later, external agent teammates
├── bounded Execution Workers
└── GitHub artifacts and delivery state
```

These are not separate applications placed beside one another. Conversation is where work is discussed, Work Items preserve structure and state, Execution Workers perform bounded implementation, and GitHub preserves code delivery.

The first technical proof is not the finished Kanban board, branding, external-agent integration, or a giant settings system.

The first proof is this loop:

```text
Project Channel
      ↓
Thread with @agent Execution Worker
      ↓
Real Codex App Server execution in an isolated worktree
      ↓
Visible progress and blocker handling
      ↓
GitHub pull request
      ↓
Review and preserved project context
```

Then prove the parallel local loop:

```text
Work Item
      ↓
Open in local Codex
      ↓
Codex reads and updates through MCP
      ↓
GitHub returns the durable engineering result
```

Once both loops work, the project is real.

Then, after version one, prove the external teammate loop:

```text
Hermes or OpenClaw automation wakes outside Agent Workspace
      ↓
External runtime authenticates as its own Project Actor
      ↓
Agent member reads, messages, and manages permitted Work Items
      ↓
Agent member invokes @agent when bounded repository work is needed
      ↓
Permissions, approvals, results, and audit history remain inside Agent Workspace
```

The version-one architecture must make this later loop additive. The version-one implementation must not attempt to build it.
