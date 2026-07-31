# Agent Workspace
## Project Source of Truth and Implementation Plan

**Working title:** Agent Workspace  
**Status:** M1 active v0.7

**Owner:** Mischa Nelson  
**Date:** July 30, 2026  
**Document role:** This file is the canonical product and engineering plan until a later version explicitly replaces it.  
**Implementation authorization:** Milestone M0 is complete. Milestone M1 is authorized by the task-level criteria below; every later milestone remains subject to the entry checks defined in this document.

> The product name, visual identity, and public branding are intentionally unresolved. "Agent Workspace" is a neutral working title, not a proposed final name.

## Revision history

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

Milestone M0 is authorized by this document. Before beginning any later milestone, replace its title-only backlog entries with scoped acceptance criteria, dependencies, expected tests, and explicit exclusions. This elaboration happens one milestone ahead, not for the whole roadmap at once.

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

## Milestone M2: Conversations

### M2-001 Conversation schema and invariants

### M2-002 Versioned Message AST and Actor authorship

### M2-003 Message API and pagination

### M2-004 WebSocket connection

### M2-005 Transactional outbox

### M2-006 Channel UI

### M2-007 Thread creation and linking

### M2-008 Per-user read state

### M2-009 Thread sidebar surfacing rules

### M2-010 Mention autocomplete for human and worker Actors

## Milestone M3: Execution Worker and hosted Codex

### M3-001 Execution Worker entity and Actor identity

Create the default Execution Worker with provisional `@agent` handle.

### M3-002 Execution Profile schema and isolated CODEX_HOME

### M3-003 Codex process supervisor

### M3-004 Codex account setup UI

### M3-005 Trusted repository registration

### M3-006 Repository mirror and managed worktree lifecycle

### M3-007 Branch creation and Agent Run workspace selection

### M3-008 Provider session start and resume

### M3-009 Agent Run lifecycle

Record `requesting_actor_id`, Execution Worker, delegated scope, Execution Profile, Provider Session, and immutable repository selection.

### M3-010 Event normalization and ordered timeline persistence

### M3-011 Streamed Thread UI

### M3-012 Approval and blocker flow

### M3-013 Thread attention colors and per-user unseen logic

### M3-014 Interrupt and retry

### M3-015 Provider crash and restart recovery

## Milestone M4: Work Items

### M4-001 Work Item schema and readable keys

### M4-002 Default Project workflow

### M4-003 Work Item list

### M4-004 Work Item detail

### M4-005 Actor-compatible assignment and priority

### M4-006 Acceptance criteria

### M4-007 Work Item comments and activity

### M4-008 Thread and Work Item many-to-many links

### M4-009 Work Item primary and related repository links

### M4-010 Work Item reference autocomplete

### M4-011 Kanban projection

### M4-012 Work with Worker action

## Milestone M5: GitHub delivery and review

### M5-001 GitHub App setup

### M5-002 Webhook verification and idempotency

### M5-003 Installation-token Git authentication

### M5-004 Push branch with installation credentials

### M5-005 Pull request model and linking

### M5-006 Checks and review state

### M5-007 Work Item automation from pull request state

### M5-008 Safe cleanup and repair


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
worktree. A completed provider turn may be reconciled as completed. An
interrupted, non-terminal, missing, or ambiguous turn becomes an actionable
application `interrupted` result with its worktree preserved. This proved the
required M0 recovery boundary without inventing continuation or resetting
dirty state.

## D-033: Private-build authentication uses host-bootstrapped local sessions through M6

**Status:** Accepted as an interim private-build boundary

**Reason:** M1 needs real human identity and server-resolved Actor sessions
before Projects, Conversations, and execution can be authorized correctly.
Host-local bootstrap plus Argon2id password verification and opaque,
server-side cookie sessions provide that boundary without exposing a setup
route or choosing the final team authentication product. Passkeys, OIDC, and
the final combination remain O-004 and must be resolved before M7.

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
