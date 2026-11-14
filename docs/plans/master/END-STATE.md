# Agent Workspace: the end state

Draft 4 by the lead, 2026-11-14. Draft 4 says exactly what the privacy promises cover,
makes approval a matter of permission for every kind of account, calls the hosting numbers
targets until they are measured, and points at the build order of 31 large builds (rulings
R44 to R50). Draft 2 was written after reading Agent OS, the harness and
the product references (`R1-agent-os.md`, `R2-harness-and-connecting-agents.md`,
`R3-product-references.md`). Draft 3 adds what the owner changed after reading it (accounts
of every kind are global, DMs belong to the people in them, hosts make agents, chat only is
a first-class shape, files can live elsewhere, the hosted service is built to be cheap) and
what the fourteen workstream plans settled (section 11). The rulings behind each change are
in `LEAD-RULINGS.md`. This is the target the detailed workstream plans build toward. It describes the finished product, not the next step. Where the code already matches,
it says so. Vocabulary is in section 3 and is used the same way everywhere.

## 1. What we are building

**The open workspace where people and agents work as equals.**

One app where a team talks (channels, DMs, groups, threads), plans (pages, whiteboards,
sheets, their own record types) and ships (tasks, runs, code), and where every one of those
can be done by a person or by any agent, from any vendor, running anywhere.

It is the open answer to Claude Tag and Claude's Docs, Sheets, Whiteboards and Data. Those
are closed, paid, and only Claude can use them. This one:

- works with **any agent**: an always-on runtime such as Hermes, an OpenAI-based bot, a
  personal assistant, a coding agent, or one defined in Agent OS;
- is **modular all the way down**, so a team reshapes it instead of forking it;
- is **self-hostable in one command and hosted as a service** from the same code, with a free
  plan that is the whole product;
- is the **frontend for a family**: Agent OS defines and runs agents, a harness is their
  runtime, and Workspace is where they work. Each stands alone. Nothing in one is written
  specially for another.

The feel: Slack, if Slack were a full planning and development environment, with Linear's
speed in tasks and Notion's calm in documents, in one design language.

**It is also allowed to be small.** A group of friends can set one up only to chat, with no
tasks, no projects and no agents in sight, and it should feel like a chat app made for them.
Every other part is something a workspace switches on, never something it has to hide from.
A workspace starts from a **shape** ("Friends", "Team", "Just me and my agents", "Documents",
"Blank") and can change shape later without losing anything.

If you know Discord, the words line up like this: the **server** here is the whole
installation (Discord itself); a **workspace** is one Discord server, a home for one group; a
**space** (a project is the usual kind) is a section inside it, with more than chat in it;
and **DMs** sit outside any workspace, between accounts. An account, whether a person or an
agent, can be in many workspaces and in many spaces in each.

### The test for "done"

A person who has never seen the app can do all of this in one sitting, on a laptop or a phone:

1. Sign up (or run one command to self-host), name a workspace, and invite a teammate.
2. Add an agent three ways: pick one from the directory, paste a link into an agent they
   already run somewhere else, or start one on a host they've linked, where the host's own
   steps appear inside the app. (Describing an agent in a sentence and building it is the
   host's feature, not Workspace's.)
3. Talk with people and agents in a channel. Turn a message into a task, a decision, a page or
   a whiteboard without leaving the conversation.
4. Say "put last week's numbers in the budget sheet" to an agent from their phone and watch
   the sheet change, with the agent's name on the edit and one tap to undo it.
5. Give that agent exactly the access a new hire would get, or exactly the access they have
   themselves, by picking a role.
6. Make a "Playtest feedback" tracker with its own fields, a board and a public form, with no
   code, and have an agent triage what comes in.
7. See what every agent is doing, what it cost, and what it's waiting on, in one place,
   whoever runs it.
8. Switch off everything except chat, or everything except pages, and have the app still feel
   whole.
9. Export the whole workspace and import it on another server.
10. Add their own agent to a second workspace in one step, and DM it once, not once per
    workspace.
11. Set one up with friends only to chat, and never see a task, a project or an agent.

## 2. Principles

These decide arguments. A plan that breaks one needs a written reason.

1. **An agent is an account.** People, agents and connected apps are all accounts. What an
   account may do comes from its roles and nothing else. No code path asks "is this an agent?"
   to decide a permission. The only differences are mechanical: how an account signs in and
   how it's told that something needs it. (Today the code breaks this in places, for example
   an agent's edit to a page it didn't create is forced to be a suggestion. Those become role
   settings.)
2. **One operation, defined once.** Each thing the app can do (send a message, move a task,
   edit a cell) is defined once, with its inputs, its permission and its effect. The web app,
   the agent tools, the public API and automations all call that one definition. Parity
   between people and agents is then true by construction, not by a guard test.
3. **Everything is a thing.** Messages, tasks, pages, sheets, whiteboards, records, files and
   runs share one envelope and one set of services: links, comments, history, search, events,
   permissions, embeds, export. A new kind gets all of them on day one.
4. **Modules build the app.** A module registers what it adds, and the app is assembled from
   those registrations. Core modules (ours), built modules (made of data by people or agents)
   and connected modules (outside apps) all go through the same registration.
5. **Workspace doesn't run agents, and doesn't make them.** Workspace owns the record of
   work: who did what, for which task, what it cost, what it's waiting on. A host does the
   work, and a host is also where an agent is defined, created and changed, through steps the
   host supplies and Workspace only draws. Hosts connect through one public protocol, and the
   built-in host is a separate program that uses it too. An agent nobody "hosts" (a bot
   someone runs) is just as visible, because it reports its own work the same way.
6. **Nothing hard-coded for anyone's stack.** No provider, model, harness or code host by name
   in the core. No machine, path or person. The owner's own setup is one configuration among
   many.
7. **Any number of servers, every row in a workspace, and an idle workspace costs only what
   it stores.** All state lives in PostgreSQL and file storage. Any server process can serve
   any request. Nothing runs because a workspace exists. Hosting adds operations, never
   features. (Accounts are the one thing above workspaces, and DMs live in one reserved scope
   that is stored like a workspace and read only by the people in them.)
8. **Open ways in and out.** Standard protocols before inventions. A full export and import of
   any workspace. Self-hosted and hosted are the same product.
9. **One product on every screen.** One design language. Modules describe what they show and
   the app draws it, so a phone is never an afterthought.
10. **Safe because it's visible and reversible.** Every change has an author, a history and,
    where possible, an undo. Limits on what an account may do are roles and policies a
    workspace sets, never assumptions baked in.

## 3. The concepts

The whole product is these few ideas. Today's name is in brackets where it differs.

### Server, workspace, space

- **Server**: one installation. Holds many workspaces, and every account.
- **Workspace** [organization]: a group's home and the unit of tenancy: a company, a team, a
  studio, a friend group, one person and their agents. Every row of content belongs to
  exactly one. It has members, roles, modules, settings, a plan (when hosted) and an address
  of its own in every URL. One workspace holds as many projects as its people want; nobody
  needs a second workspace to have a second project.
- **Space** [project, plus two hidden kinds of project]: a place inside a workspace with its
  own members, roles, modules and things. A **project** is the ordinary kind of space. Other
  kinds are the same concept with a different starting shape: the workspace's own space (what
  everyone shares, and where channels live in a workspace that has no projects), a team's
  space, and each account's **personal space** (private by default, made on first use; where
  a person keeps drafts and an agent keeps its own notes and pages). A thing may have no
  space.
- **DMs and group conversations belong to the people in them**, not to a workspace and not to
  a space. An account can DM any account it shares a workspace with, if its role there allows
  starting DMs and the other account's own setting allows it. There is one DM per pair on the
  server, and a person sees the same DMs whichever workspace they're looking at. Nobody but
  the participants can read one through the app, the API, search, export or the staff
  console: no role, no admin, no owner, no staff. That is a promise about the application.
  It is not encryption: whoever operates the server and can open its database or backups
  can read what is stored, and the hosted service says so in its privacy terms and logs any
  such access. In storage, DMs sit
  in one reserved **direct scope** that looks like a workspace to the database and has no
  members, roles, spaces or modules. A team that needs the company to keep DMs (retention,
  export, ending with membership) switches its workspace to **kept** DMs, and DMs that start
  inside it then belong to it, as in Slack.

### Accounts, roles, access

- **Account** [actor]: anything that can act. Three kinds: **person**, **agent**, **app**
  (plus the system itself). Every account is global to the server and joins workspaces by
  membership, so one sign-in reaches all of a person's workspaces and one agent can work in
  several. An agent or app account has an **owner**, another account ("my agent") or a
  workspace ("our agent"), who is accountable for it and may manage it, with anyone the
  owner names. The chain of owners always ends at a person or a workspace.
- **Membership**: an account in a workspace, and in each space it belongs to, with a role.
  Joining is the same act for every kind of account. Adding an account someone owns needs
  the adder's permission in the workspace and the agreement of someone who manages the
  account, just as a person accepts their own invitation. A workspace says whether accounts
  owned outside it may join: yes, ask an admin, or no. What others can see of an account is
  its profile and its membership here, never where else it belongs.
- **Role**: a named set of permissions, held as data. A role says which modules its holders
  may use and, for each kind of thing, what they may do: see, comment, suggest, edit, manage.
  Workspaces edit roles and make their own. There are starting roles (owner, admin, member,
  viewer, guest) and a setting for which role a newly added agent gets. That default is the
  only place "agent" and "role" meet.
- **Access**: one function answers "may this account do this operation on this thing?" from
  the account's roles, the space's modules and the thing's own visibility. The same rule is
  available as a SQL filter generated from the same list, so lists and searches can't drift
  from single checks.
- **Suggest instead of edit**: a role may grant "suggest" without "edit" on a kind of thing.
  That account's changes then arrive as **proposals** that someone with "edit" accepts. This
  is how "agents only suggest in people's documents" is expressed, for a workspace that wants
  it, and equally how a junior teammate or a guest could be set up.
- **Acting for a person**: a person may connect a tool that acts as them (an editor's
  assistant, a script), optionally narrowed to part of what they may do. That is delegation,
  not an agent account, and both exist.

### Agents

- **Agent account**: a profile (name, picture, what it's for), its owner, and its role in
  each workspace it has joined. It joins workspaces and spaces like anyone else. A personal
  agent follows its person: they add it to each workspace where they may add agents, and it
  keeps one identity, one profile and one DM with them. Managing an agent is checked by one
  rule everywhere (its owner, or the manage-agents permission in the workspace that owns
  it). Nobody can give an agent more than they could give a person.
- **Outside reference**: an optional record on an agent account that says which agent
  somewhere else this is: who issued it (an Agent OS installation, another platform), its id
  there, and its lineage (which id it was moved or re-keyed from). The account's own id is
  never the outside id. When the agent moves host, changes runtime or model, or goes from
  self-hosted to a cloud, the reference is re-attached and the account, its roles, its name
  and its history stay. A copy of an agent is a new account.
- **Connection**: how an agent account reaches us and is reached. It holds credentials and a
  **subscription**. Credentials are short-lived tokens got by an exchange where the agent's
  side can do that, a long-lived secret as the simple option, and narrow tokens for one run
  ("may work on this task for the next hour"). An account can have several connections. A
  connection belongs to the account, of any kind, and reaches every workspace the account
  is in unless it is narrowed to one workspace or to some spaces. A credential that reaches
  several workspaces names the workspace on each call.
- **Delivery**: everything that should reach an agent is an event in one ordered stream with
  one cursor per connection. The subscription says which kinds of event and which scopes
  (everything I can see, these spaces, these conversations). The agent receives the same
  events by whichever transport it can manage: a signed webhook, a stream it holds open, or
  polling. A missed webhook is caught by the next poll from the same cursor. Each event says
  why the agent is being told (a reply is required, a reply is welcome, it was mentioned, it
  was assigned, a request was answered, it's only visible), and access is checked again at
  delivery time.
- **Host**: a system that runs agents. Agent OS is a host. The built-in runner is a host. A
  cloud coding service is a host. A host enrols with a workspace once, says what it can do
  (what it **offers**, which **actions** its agents have, whether it can steer, relay
  approvals, report steps and cost), and then starts, stops and reports on work. The host may
  be reached by push, or it may dial out to Workspace and wait, so a host behind a home
  router works. A host is an app account, so it has an owner too: a person links their own
  host once and can bring its agents into any workspace where they may add agents, and a
  workspace links a host for everyone in it.
- **Run**: the record of one piece of work by an agent. Who, for what (a task, a thread, a
  message, a schedule), on which host, who asked, its state, its activities, what it used and
  cost, what it's waiting on, and what it produced (links to a pull request, a page, a
  sheet). Every agent gets runs, whoever runs it. Workspace starts some runs and **adopts**
  others that a host started by itself (its own schedule or trigger fired). A run can have
  child runs when an agent hands part of the work on.
- **Activity**: one typed entry in a run: a plan, a step, a tool call, a message, a question,
  an error, usage, a result. The run's state follows from its activities instead of being
  set by hand, and "not responding" shows when nothing arrives in time. The names follow the
  AG-UI event vocabulary so an agent that already emits such a stream needs no special code.
  Activities are best effort: a host that can't report every step says where the gap is.
- **Request**: something an account needs from someone: an approval, an answer to a question,
  or acceptance of a proposal. Requests show as cards in the inbox and can be answered from a
  phone. Any account can raise one, on any host. They are the one mechanism behind approvals,
  "ask a person" and suggestions.
- **Status**: every account has one, people and agents alike. A line a person sets by hand
  shows everywhere. A line about work ("Working on AWD-142", "Asleep until 09:00", "Paused"),
  kept current by the agent or its host with its schedule and how much of its budget it has
  used, is stored for the workspace the work is in and shown only there. Pause holds
  deliveries and refuses writes; pause and resume are events delivered to the agent.
- **Requested by**: every action records three things: the account that acted, the app it
  acted through, and the account that asked for it, if any. The audit trail and the run
  record show all three.
- **Budget**: spend and rate limits a workspace sets per workspace, per account, per space or
  per conversation. They work the same way for a person and an agent, and they count what
  Workspace can see: operations, deliveries, reported usage and anything we host.
- **Memory**: what a team knows lives in pages (a space's guide, its decisions, a memory page
  per space or channel that people and agents can read and edit, with every version kept).
  An agent's own memory belongs to the agent and its host. Workspace can show it through the
  host, and an agent can keep notes in its personal space. Memory follows the same visibility
  rule as conversations, so nothing from a private place leaks into a shared one.

### Things and what they share

- **Thing**: anything with an address: a conversation, a message, a task, a page, a sheet, a
  whiteboard, a record, a collection, a file, a run, a decision, a space. Every thing has an
  id, a kind, a title, a status, an owner, a space and a version.
- **Link**: one thing pointing at another, with a kind (part of, blocks, about, made from,
  relates to). Stored in one place, readable both ways. Mentions, link fields, "made from this
  message" and embeds all create links.
- **Event**: a fact that something changed: which thing, what happened, who did it, when.
  One ordered log. Live updates, notifications, webhooks, automations, search, agent wake-ups,
  the audit trail and activity feeds all read it.
- **Discussion**: one message system. A channel, a thread, a DM, the comments on a task, the
  comments on a page or a cell are all conversations. So mentions, reactions, replies, files
  and waking an agent work the same everywhere.
- **Fields and views**: one way to add typed fields to things and one set of views (table,
  board, list, calendar, timeline, gallery) with saved filters. Tasks, the records of any
  collection and lists of pages all use it.
- **Files**: attachable to any thing. Two separate ideas. **Storage** is where the bytes of
  our own uploads live: disk or any object store, behind one seam, chosen by the operator
  (and, hosted, a workspace may bring its own bucket). A **file source** is an outside store
  (a Drive, a Dropbox, a Git repository, a bucket) attached to a space or a workspace, whose
  files show up as things by reference: browse, attach, link, preview, search, open in the
  source. Nothing is copied unless a person asks. A source that accepts writes can be where
  a space's new uploads go. The space's roles decide who sees a source's files here.
- **History**: every thing keeps who changed what and can show or restore earlier versions.
- **Search**: one index over every kind of thing, filtered by access.

### What people work in

Each of these is a module built on the shared pieces above.

| Module | What it is | Built on |
| --- | --- | --- |
| Chat | Channels, threads, DMs, groups, the composer, reactions, scheduling | Discussion |
| Inbox | What needs you, what's for your information, requests, Later | Events, requests |
| Tasks | Tasks, boards, timelines, milestones, claims, workflows | A collection with a task core |
| Pages | Rich documents edited live, with embeds of any thing, decisions, guides, skills | Documents |
| Sheets | Spreadsheets with formulas, edited live | Documents |
| Whiteboards | Canvases edited live | Documents |
| Collections | Record types a team makes: fields, records, views, forms | Fields and views |
| Dashboards | Charts and numbers over collections, sheets and connected data | Fields and views |
| Agents | The directory, profiles, adding and managing agents, hosts | Accounts, runs |
| Runs | What agents are doing, steps, usage and cost, requests | Runs |
| Code | Repositories and pull requests through a code-host seam | Links, runs |
| Automations | When this happens, do that; ask an agent | Events, operations |
| Suggestions | Summaries and drafts from a model the workspace chooses | Operations |

### Modules

- **Core module**: written by us, compiled in. Its only privilege is where its code runs.
- **Built module**: made of data. A set of collections, fields, views, forms, rules, roles and
  pages that a person or an agent assembles. "Playtest feedback" is a built module.
- **Connected module**: an outside app with its own account and approved scopes. It adds
  things, operations, events and panels. Panels are described in blocks that Workspace draws,
  so they work on phones and match the product, or are the app's own page in a frame for
  anything secret. A host's own steps and a host's actions on its agents are panels of this
  kind. A file source is a connected module with a small fixed contract (list, get, content,
  thumbnail, changes, and optionally put).
- **Package**: a file that bundles a setup (modules, collections, roles, layouts, pages,
  agent templates and skills, never accounts) so a workspace or a space starts shaped for a
  purpose. No secrets inside. A workspace's **shape** is a package.

A workspace switches modules on and off, a space may switch one off (not on), and roles
narrow that for their holders. Off means hidden and refused, never deleted. Every module can
be off, Tasks and Agents included, and what is left is whole: with only Chat on, nothing in
the interface or in an email says project, task, agent or run.

## 4. How agents fit in

### Three ways to add an agent

The "Add an agent" flow is the same for every workspace and has no knowledge of any one
vendor. Every path ends the same way: an agent account with a role, a connection, and the
agent holding the standard package for working here.

1. **Pick one**: from the workspace's directory, from agents you already own or manage in
   another workspace (one step: pick it, pick its role here), or from what a linked host
   already has.
2. **Connect one you already run**: Workspace shows a short setup message to paste into the
   agent ("read this link and follow it"), or an installable package (an Agent Plugin: the
   tool server's address plus skills for working here). The agent picks the transport it can
   support. Always-on runtimes get a ready channel adapter, so a message in Workspace reaches
   them the way a message in any chat app would.
3. **Start one on a host**: making an agent is the host's job. Workspace does four things
   and knows nothing else about it:
   - it lists what each linked host **offers**. An offer is a title, a line of description
     and a first step. The host names its offers ("a new agent from a template", "a copy of
     this one", "describe what you want"), and Workspace doesn't know what any of them mean;
   - it draws the host's steps: forms and panels in Workspace's own blocks, or the host's
     own page for anything secret, such as model keys, which never pass through Workspace;
   - it receives the result: the host registers an agent (a profile, an outside reference,
     how to reach it), and an account that may add agents approves it, a person or an
     agent (a workspace can ask for a person's review as a setting);
   - it asks its one question of its own: which role the agent gets here, and in which
     spaces.

   So a builder that turns a sentence into an agent, templates, backends, what the agent may
   touch on its machine and how proactive it is are all features of a host. When Agent OS
   has them, they appear here with no change to Workspace. Another platform's steps appear
   the same way.

If no host is linked, "start one" offers the choices in the owner's sketch: a hosted agent
service, linking a host you run, or instructions for running one.

### What an agent can do

Everything its role allows, through the same operations the web app uses. An agent can also:

- **follow** spaces, conversations and things, and be told when they change, so it can be
  proactive rather than only answer mentions. A workspace can switch this "ambient" behaviour
  on per agent and per conversation;
- **be mentioned by anyone**, including other agents, with loop limits a workspace sets;
- **schedule its own follow-ups** ("look at this again on Tuesday"), kept by Workspace for
  agents that have no clock of their own and shown on the agent's page either way;
- **ask** a person or a role for an answer or an approval and carry on when it comes;
- **show its work**: a typing or working line in a conversation, and a run with activities
  and cost;
- **keep its own pages** in its personal space, and read the guides and skills of the spaces
  it's in.

### Five ways to connect, on one foundation

Each agent uses what it can support. Most use the first two.

| Way | What it's for | Who uses it | Built from |
| --- | --- | --- | --- |
| 1. Act | Do anything a role allows | Every agent, and every app acting as a person | The operation layer, served as MCP tools and as the public API |
| 2. Be told | Learn that something needs it or changed | Every agent that isn't only used interactively | The event stream: signed webhook, held-open stream, or poll, one cursor |
| 3. Show work | Runs, activities, questions, usage, status | Anyone doing more than a one-line reply | Run operations; later, reading an agent's own progress stream directly |
| 4. Live in chat | Workspace as one more chat platform for an always-on runtime | Hermes, OpenClaw and similar gateways | Small plugins on the runtime's side, built from ways 1 and 2. Nothing new on the wire |
| 5. Be started | Workspace launches work on a platform that runs agents | Agent OS, the built-in host, coding clouds, any agent platform | The host protocol: enrol, describe, start, cancel, steer, answer, and way 3 back |

```
   something needs the agent           the agent acts
   (mention, DM, assignment,           (any operation its
    request answered, schedule,         role allows)
    a followed thing changed)
              |                                  ^
              v                                  |
   +-----------------------+          +---------------------+
   | The event stream      |          | Agent tools and API |
   | webhook / stream /    |          | (the one operation  |
   | poll, one cursor      |          |  layer)             |
   +-----------------------+          +---------------------+
              |                                  ^
              v                                  |
   +-----------------------------------------------------------+
   |  The agent, wherever it runs                              |
   |  - by itself (a bot someone runs)                         |
   |  - inside an always-on runtime, through a channel adapter |
   |  - on a host (Agent OS, the built-in host, a cloud)       |
   +-----------------------------------------------------------+
              |                                  ^
              v                                  |
   +-----------------------+          +---------------------+
   | Host protocol         |  ---->   | Run record          |
   | start, cancel, steer, |          | state, activities,  |
   | answer (push or the   |          | cost, requests,     |
   | host dials out)       |          | results             |
   +-----------------------+          +---------------------+
```

- An agent that connects by itself reports its own runs with the same run operations a host
  uses, so its work is as visible as anyone's.
- Launching a task to an agent always creates a run. If a host runs the agent, Workspace asks
  the host to start. If the agent connects by itself, the start arrives as an event.

### Standards, so nothing is invented that already exists

Workspace's own inventions are the meaning of its events and the shape of a run. Everything
on the wire is a standard.

| Need | Standard | Notes |
| --- | --- | --- |
| Acting | MCP, current revision (stateless), with the older revisions served for a deprecation window | Long operations use the MCP Tasks extension. Guides and skills are readable as resources. Stateless suits any number of servers |
| Signing in | OAuth 2.1 for apps acting as a person; client credentials or a token exchange for agent accounts; a plain bearer secret as the simple path | |
| Push | Standard Webhooks (headers, signing, rotation, retries over hours) | Today's own header names stay as aliases for a release |
| Stream and poll | Server-sent events with resume from the cursor; the same address without streaming for poll and long-poll | Not WebSocket: one direction is enough and it scales from the log |
| Progress | AG-UI's event names for run activities | An adapter can later read a real AG-UI stream. The spec is a draft, so pin a version |
| Starting work on someone else's platform | A2A as one binding of the host protocol | Task states map onto run states; Workspace's extras (repository, branch, pull request) go in a small published extension |
| Driving coding agents | ACP, inside the built-in host | One protocol for Claude Code, Codex, Gemini CLI, Hermes and others. Never in the server |
| Packaging | Agent Skills and Agent Plugins | "Connect any agent" is installing one package |

### Hosts

- **Enrolment, not keys.** A workspace links a host with a one-time code that the host's
  administrator approves on their side. Both sides get narrow credentials they can revoke.
  Workspace never holds a host's administrator login or its SSH keys.
- **Push or dial-out.** The host protocol works over a connection the host opens (a long
  poll or a stream) as well as over push, with one connection per host covering all its
  agents. Reporting is also legal both ways: the host reports to Workspace, or a connector
  beside the host reads the host's own log and reports for it.
- **Capabilities.** `describe` says what the host can do. Everything past the minimum is
  optional: creating agents, steering, relaying approvals, reporting steps, reporting cost.
  Workspace shows what a host can't do instead of pretending.
- **Honest states.** A start may be accepted, refused or **uncertain** (Workspace then
  repeats the same request id and never sends a second, different start). Cancel is
  **requested** before it is **cancelled**. Usage is either reported for the run or
  attributed from a time window, and says which. Steps may have gaps.
- **Offers and actions, not a fixed lifecycle.** Beyond offers (above), a host publishes the
  **actions** its agents have: a label, whether it needs a confirmation or a panel, whether
  it's dangerous. Workspace draws them on the agent's page and sends the chosen one back.
  Edit, deploy with a review, copy, move and retire are a host's actions if it has them;
  Workspace has no names for them. The fixed part of the protocol is only what Workspace's
  own records need: enrol, describe, start, cancel, steer, answer, report (runs, activities,
  usage, status), and three notices from the host: an agent's profile changed, its outside
  reference changed (moved or re-keyed, with lineage), it was retired. Workspace never
  stores the host's definition of an agent.
- **Proof that the protocol is enough.** Workspace ships a host client library, the built-in
  host that uses it, a small example host, and a conformance suite any host can run.
- **Who opens the pull request.** The host pushes its work. Workspace records the result and
  can hand the host a short-lived token for one repository when the run starts.
- **The built-in host** is a separate program that drives coding agents on a machine through
  ACP and the existing adapters. It ships beside the server for self-hosters and uses only
  the public host protocol.

### Agent OS, concretely

Agent OS's own rule is that interaction layers use only its public interfaces and none is
privileged. Workspace's matching rule is that hosts use only Workspace's public protocol and
none is privileged. What the research found shapes the link:

- **Agent OS is pull-only today.** It has no way in from the network and never calls out. A
  message to an agent is a "turn" a client submits and then polls. So the link is a
  **connector**: a small program beside an Agent OS installation that dials out to Workspace,
  receives events and host commands, calls Agent OS locally, and reports back. It replaces
  today's owner-specific bridge, lives on Workspace's side of the line, speaks only the
  public host protocol, and names nobody's setup. A hosted Agent OS would speak the host
  protocol natively.
- **Two modes, both needed.** *Bridged*: the connector turns a Workspace event into a turn
  and posts the answer; this works for any Agent OS agent but the agent can only chat.
  *Native*: the agent has Workspace's tool server as one of its own tools, with its Workspace
  credential in an Agent OS credential slot, and does everything a person can. The wake
  still arrives as a turn. Native is the target; bridged is the floor.
- **Identity.** An Agent OS agent's Workspace account carries an outside reference: the Agent
  OS installation and context that owns it, its agent id, and lineage. Today a move in Agent
  OS issues a new id, so Workspace re-attaches by lineage with an administrator's
  confirmation. Workspace never stores the host, machine, runtime or model as part of who the
  agent is, and shows a runtime's name only if the host offers it (Agent OS runs blind trials
  where the runtime must stay hidden).
- **Two permission systems that never mirror each other.** Workspace roles decide what the
  account may do in Workspace. Agent OS authority decides what the process may do on its
  machine. The creation flow shows both side by side.
- **Runs.** An Agent OS task-agent run maps onto a Workspace run, adopted when Agent OS's
  own trigger started it. A turn to a persistent agent is a chat reply unless it does long
  work. Temporary agents (one per task or issue) are runs of one account per template:
  people mention `@fixer`, and each execution is "Fixer, run 1234".
- **Creating an agent** is Agent OS's feature from start to finish. The steps in the owner's
  sketch (where it runs, from scratch or a template or a copy, the backend, what it may
  access, how proactive it is, model keys, and later "describe it in a sentence") are Agent
  OS offers and Agent OS panels. Workspace draws them and adds the role. What is missing is
  on the Agent OS side: a way to be reached remotely and safely, and the panels themselves.
- **Anything Agent OS can do here, another platform can do** by implementing the same
  contracts. Agent OS is the first and richest host, not a special one.

**What we ask of Agent OS** (each is general, none is for Workspace only):

1. Keep an agent's id when it moves, or publish its lineage.
2. An enrolment step and a remote interface for more than one caller, ideally by dialling out.
3. A sender and attachments on a turn, and turns against the deployed agent instead of a
   source revision.
4. One normalised event stream, including "approval requested" and an answer.
5. Credentials and usage per run for temporary agents.
6. A signed webhook as a trigger source.
7. A capabilities list that is current, including the backends installed.

Until these exist the connector covers the gap, and the plan says where.

**What is whose.** Agent OS is far from finished and will change, so Workspace's plan does
not wait on it and does not build its features for it.

| In Workspace's plan | Agent OS's work, done in the later three-project integration |
| --- | --- |
| The host protocol, the host client library, the conformance suite | Implementing the protocol (or keeping the connector beside it) |
| The built-in host and a small example host | Offers, panels and actions for creating and changing agents, including any builder |
| Drawing a host's offers, panels and actions; registering what a host hands over; the role | The seven asks above |
| The generic bridged connector that replaces the owner-specific bridge in this repository | Native mode: an Agent OS agent holding Workspace's tools itself |
| Outside reference with lineage on an account; rebinding with a confirmation | Keeping an id across a move, or publishing lineage |

**What we ask of the harness** (the owner's own runtime, and the best test client for "an
agent Workspace doesn't run"): accept the typed events instead of only the retired wake
format, negotiate the MCP revision instead of pinning one, read the tool list instead of
wrapping tools by hand, and replace "the owner" with "accounts whose role lets them approve
this here". Workspace keeps the old wake format and old MCP revisions served until it has.

### Safety in shared spaces

An agent in a room with several people is the hard case. The rules are mechanisms a
workspace can see and change, not assumptions:

- **Scope is membership and role**, per space and per conversation, shown on one screen per
  agent: where it is, what it may do there, what it's told about.
- **Visibility holds everywhere Workspace decides**: delivery, search, memory pages and
  summaries all use the one access rule, so Workspace never hands an agent a conversation
  it may not see, and removing an agent stops every later read and delivery at once.
- **Workspace cannot make an agent forget.** What an agent was shown, its host may still
  hold. One identity across workspaces is not one memory: whether an agent can use in one
  workspace what it learned in another is a property of its host, which states it
  (shared, per workspace, per space, none). The scope screen shows that statement as the
  host's, and a workspace may refuse outside accounts that don't state a level it accepts.
- **Who asked is always recorded**, and an action taken because of someone's message can be
  limited to what that person could have done themselves, as a role setting.
- **Content is data.** Messages, pages and files reach an agent marked as content from a
  named author, never as instructions from the system.
- **Budgets, loop limits and turn allowances** per agent and per conversation. Stop on one
  reply and pause on the whole agent, both delivered as events.
- **Risky operations can require a request** (an approval) by role, for anyone.
- **Apps that read conversations ask consent** by kind: shared channels by an admin, private
  conversations and DMs by their participants.
- **A team can add its own guard** as a module: a rule or a reviewing agent that sees
  proposed actions first.

## 5. How the app is put together

### The operation layer

An operation has a name, an input shape, the permission it needs, whether it can be proposed,
and a function that does the work in one transaction: check access, change the data, write
history, emit the event. From that one definition come:

- the web route and its typed client for the web app;
- the agent tool, with its description and schema;
- the public API endpoint;
- the automation action;
- the entry in the audit trail.

Conventions are fixed once: names read `<thing>_<verb>`; every write takes a request id so a
retry is safe, and the version it expects; every result has one envelope; every list pages the
same way; errors come from one list. Each operation is one of five kinds (describe, query,
create, change, suggest), which is how agents learn a new module from an old one.

### The kernel and the modules

```
+--------------------------------------------------------------------+
|  Web app          Agent tools (MCP)     Public API     Automations |
+--------------------------------------------------------------------+
|                         The operation layer                        |
+--------------------------------------------------------------------+
|  Modules: chat, inbox, tasks, pages, sheets, whiteboards,          |
|  collections, dashboards, agents, runs, code, automations,         |
|  suggestions, built modules, connected modules                     |
+--------------------------------------------------------------------+
|  Kernel: accounts and access, spaces, things, links, events,       |
|  discussion, fields and views, documents, files, history, search,  |
|  requests, jobs, module registry                                   |
+--------------------------------------------------------------------+
|  PostgreSQL                         File storage (disk or objects) |
+--------------------------------------------------------------------+

   Hosts (outside the server): built-in host, Agent OS, clouds, yours
   Code hosts (behind a seam): GitHub first
```

- Modules talk to each other only through the kernel: links, events and operations. A module
  never reads another module's tables.
- The server stays one program for a self-hoster. Inside, it becomes a kernel library and one
  library per module once modules meet only through the kernel.
- Background work (jobs, deliveries, indexing) runs in the same program with a role switch, so
  a small install runs one process and a big one runs several of each.

### Live updates

Events go into the log in the same transaction as the change. Typing, presence, cursors and
streaming text are never written to it. Every server process hears about new events through
the database (a notification where there is one, a short poll of the log's position where
there isn't) and sends them to the browsers and agents connected to it. Browsers hold a
WebSocket; agents use a stream or poll; both resume from the same cursor. The agent event stream in section 4 is this same log read with a cursor, so a browser, a
webhook and a polling agent can never disagree about what happened. Each connection subscribes to the spaces and conversations it can see, so access is
decided when it subscribes and when access changes, not once per event. The web app updates
exactly the things an event names. A browser hears its current workspace, its DMs, and
unread counts for its other workspaces.

The program has three roles, all in one process for a self-hoster: `api` (stateless), `live`
(holds sockets and streams, nothing else) and `work` (jobs, deliveries, indexing, the
scheduler). The live role works from a small published contract: a signed ticket from `api`
that names what a connection may hear, event envelopes in, pushes out. It needs no database,
so a service that keeps idle sockets open for nothing can stand in for it.

### The web app

- One shell, mounted once, with the workspace in the address (`/w/acme/...`). Account
  settings and public forms sit outside it; a DM opens inside whichever workspace you're in.
- The web app is static files that any server or content network can serve.
- Types and the client for every operation are generated from the server's definitions.
- Navigation, tabs, panels, settings pages and commands come from module registrations.
- Lists, tables, boards, calendars, galleries, detail views and forms are drawn by shared
  pieces for every module, wide and narrow.
- Installable on a phone, with push, and sending that survives a lost connection.

## 6. Running it

### Self-hosted

One command starts the server and its database from a published image. First run sets up the
first workspace in the browser. Sign-in is by password or the operator's own single sign-on.
Backups cover the database and files. Every feature works: there is no second edition.
Agents can run on the operator's own machine through the built-in host, which ships beside
the server as its own program, in folders the operator allows.

### Hosted

The same image, run by us for many workspaces.

| Area | Hosted adds |
| --- | --- |
| Getting in | Open sign-up with a verified email, sign-in with common providers, company single sign-on and directory sync |
| Tenancy | One shared database where every row of content belongs to a workspace, checked by isolation tests and by the database's own row rules as a second net. Accounts sit above workspaces in a short published list of global tables. DMs sit in the direct scope, which row rules open only to their participants. Groups of servers ("cells") later for scale and regions |
| Plans | Plans are data. One metering module counts people, storage and whatever we pay to run, and enforces limits in one place |
| What is never charged | Agents are accounts but never billed seats. Bringing your own agents, models and keys costs nothing. Message and document history is never cut |
| What is metered | Only what the host pays for: model calls we make for a workspace that has no key of its own, the hosted agent service, storage above the plan, and heavy automation. Each has a spend limit a person sets per workspace, per agent and per conversation |
| Billing | Through a merchant of record that handles tax |
| Agents | Never run on our shared machines. People bring a host, use a provider's cloud, or pay for a hosted agent service (hosted Agent OS), which is just another host |
| Operations | Managed mail, object storage, backups, monitoring, a staff console outside the app, abuse controls, a status page |
| Trust | Export any time, delete on request, terms and privacy, a published promise about the free plan and about export |

**Built to be cheap.** A free plan that is the whole product only works if a quiet workspace
costs almost nothing. That doesn't take a rewrite or a second database: the Rust server and
PostgreSQL stay, so self-hosted and hosted remain one codebase. It takes rules the server
keeps from now on:

1. The web app is static files, served from anywhere.
2. File bytes go straight between the browser and object storage with signed links. They
   don't pass through the server, and the store is one with no charge for downloads.
3. The server uses the database in a way a shared connection pooler allows (no state kept on
   a connection, short transactions, leases instead of session locks).
4. `api` processes hold nothing in memory and can stop when nobody is asking; `work` can be
   woken by a timer; `live` is the only part that holds connections, and it sits behind a
   contract so an edge service that sleeps idle sockets can replace it.
5. Nothing runs because a workspace exists: no timer, loop, connection or cache per
   workspace. A test with ten thousand empty workspaces keeps it true.
6. Old log rows and old history move to cheap storage. History is never cut; old history may
   open a little slower.
7. Every cost has a counter per workspace before the service opens. Email and push are the
   outbound costs, so they have digests and plan limits.
8. Agents never run on our machines and model calls are brought by the workspace.

Three reference shapes are written down with what each costs when idle: **one box** (a
self-hoster), **small hosted** (two small machines, managed PostgreSQL, object storage, a
content network), and **edge** (the static app on a content network, files in zero-egress
object storage, the live role on edge objects that sleep, `api` in containers that stop when
idle, PostgreSQL behind a pooler). The edge shape is designed and its contracts are tested
now, and built when the numbers call for it. Rules 3 and 4 are exactly what such a platform
requires, checked against one provider's current documentation. No provider is named in the
code.

The free plan is the whole product for a small team (every module, full history, a people
limit and a storage limit), affordable because model calls and compute are brought by the
people using them. These are targets: the free plan's numbers, history that is never cut
and the idle cost become public promises only once the meters and the idle-cost test have
run against real hosted use and the numbers hold. Paid plans sell operations: more people, more storage, single sign-on,
longer audit retention, support. Hosting is free for open-source, education and non-profit
groups.

### Moving

Any workspace exports to one archive and imports on any server: hosted to self-hosted and
back. Each module says how its things export, so nothing is left behind. Memberships and a
profile for each member account, of any kind, travel with the workspace, and import matches
accounts that already exist on the new server (by outside reference for agents and apps, by
verified email for people). A workspace's export holds only what belongs to it. A DM is in
it only when it is a kept DM; personal DMs, including old ones that still carry a
workspace's id in storage, are exported by the people in them. An operator's full-server
export includes everything. The agent itself moves between hosts by its host's own means, and its outside
reference is re-attached.

## 7. From here to there

The health check found the code closer to this than it looks. The send pipeline, the task
command layer, custom fields, the job queue and the adapters are already the right shape. The
work is to make the good patterns the only patterns, in this order. Each workstream has its
own detailed plan. The plans' slices are built as 31 large builds in six stages
(`BUILD-ORDER.md`): one long session and one pull request per build, a working
demonstration at the end of each stage, and one journey test that proves the foundations
together before the product is built on them.

| Phase | Aim | Workstreams |
| --- | --- | --- |
| 0. Steady | Fix the known bugs, stop the copying | P0 Steady ground |
| 1. One of each | One identity and access model, one operation layer, one thing, link and event model, spaces, any number of servers, one fields and views system | K1 Accounts and access; K2 The operation layer; K3 Things, links and events; K4 Spaces; K5 Any number of servers; T1 Fields, views and tasks |
| 2. Agents anywhere | Any agent, hosted anywhere, is a full citizen | A1 Runs, requests and hosts; A2 Connecting agents |
| 3. Modules for real | The app is assembled from modules; people and outside apps can add them | M1 Modules; C1 Discussion and documents |
| 4. Depth | Each surface is as good as its reference | C2 Chat, inbox and the daily feel; C1 and T1 continued; W1 The web app |
| 5. Open up | Self-host in one command, the hosted service, the public release | H1 Hosting and release |

Phases overlap. Phase 4 work continues throughout, because the product has to keep getting
better to use while its foundations are rebuilt.

## 8. How good each part has to be

The references set the bar. Full notes, with what the app has today, are in the product
research (`R3-product-references.md`). The gaps that matter most:

| Part | The bar | What is strong today | Biggest gaps to close |
| --- | --- | --- | --- |
| Chat | Slack: an unread state you trust, threads, a sidebar you arrange | Threads with a status and an outcome, scheduling, drafts, slash commands | Private channels finished, user groups, status text, guests, a page tab per channel, calls later |
| Inbox | Slack's Activity and Later, Linear's one-key triage | Needs you and FYI, Later, questions from agents, push with actions | Saved inbox views, snooze until something happens, keywords, digests, quiet hours |
| Tasks | Linear: speed, keyboard, cheap views, triage, cycles | Claims and handoff, criteria, dependencies, templates, recurring, import | Cycles, a triage queue with rules, initiatives, one views system shared with collections |
| Pages | Notion's composability, Claude Docs' "ask in a comment" | Live editing, versions, suggestions, embeds of sheets, collections, tasks, boards | Suggest-or-edit by role (today forced on agents), an agent answering in the comment thread that asked, sharing levels, Word and PDF export, events and automations |
| Sheets | The grid of Google Sheets, Claude Sheets' marked agent edits | Formulas, per-cell conflicts, rows by key for agents, history, import and export, live embeds | Number formats, charts, cell comments, validation, find and replace, filters, marking what an agent just changed, search and events |
| Whiteboards | Miro and FigJam, Claude Whiteboards' "ask about this region" | People and agents edit together, agents see a rendered picture, history | Live cursors and follow, comments on elements, stickies into tasks, board into a page, templates, events |
| Collections | Notion databases, Airtable interfaces | Shared field types, table, board, gallery and calendar, public forms, record history | Formula, rollup, file and system fields, timeline and chart views, record templates and comments, richer automation actions |
| Dashboards | Claude Data, Notion charts, Linear dashboards | Nothing yet beyond usage charts | The whole module: charts over tasks, collections and sheets, then read-only outside data with reviewed queries |
| Agents | Claude Tag's teammate shape, Linear's sessions, Mattermost's scoping | Agents as accounts, claims and leases, briefing, typed wake-ups, profiles | Everything in section 4: runs for every agent, events beyond replies, ambient mode, self-scheduled follow-ups, budgets, the scope screen, memory pages |
| Search | One box over everything, filters, answers with sources | Full text over most kinds, one tool for agents | Sheets and boards in the index, filters and saved searches, meaning-based search with a model the workspace chooses |
| Automations | Notion and Slack builders, agents as steps | Triggers on tasks, messages, records, runs, pull requests, schedules | "Ask an agent" as an action, every module's events and operations, incoming webhooks, a clear builder |
| Phone | Slack's mobile app as a full peer | Installable, push with actions, share in, quick add | Every list and document usable narrow, offline reading, answering requests from the lock screen |
| Admin | Roles as data, scopes per agent, audit, export | Roles with module groups, audit, isolation tests, templates | One access path, guests, directory sync, retention rules, spaces |

What the open, any-agent version does that the closed ones can't is the reason for the
product, and each plan should protect it: any agent is a full member under the same roles as
people; agents work with each other in the open; nobody pays us for tokens they bring; work
continues from anywhere through the tools; every surface feeds every other; suggest-or-edit
is a role for anyone; memory is pages you own; agents move between hosts; every protocol is
open; self-hosting has every feature; safety is inspectable; and a team reshapes it without
asking a vendor.

## 9. Words shared with Agent OS and the harness

The three projects use some of the same words for different things. In Workspace's design
and interface:

| Word | Here it means | Elsewhere | Rule |
| --- | --- | --- | --- |
| workspace | The team's home, the unit of tenancy | In Agent OS, a folder an agent may use | Show Agent OS's as "folders" |
| task | A piece of tracked work | In Agent OS, one turn sent to an agent | Call those "turns" |
| run | The team's record of a piece of agent work | In Agent OS, a host's execution record | Our run points at theirs by id |
| host | A system that runs agents | | Not "runner" in new design work |
| backend | The runtime inside a host (Hermes, Claude Code, a harness) | "harness" means different things in each project | Optional, and hidden if the host withholds it |
| connection | An agent account's credentials and subscription | In Agent OS, access to a model provider | Don't use the bare word in shared contracts |
| channel | A conversation | In Agent OS, a chat platform binding | Workspace is reached through the host protocol, not as an Agent OS channel type |
| memory | A team's pages and decisions | The agent's own memory | Both exist, owned separately |

Inside Workspace's own code the older words are renamed over time: organization to
workspace, work item to task, project to space where it means the general concept, actor to
account, memory to pages.

## 10. Calls that are the owner's

None of these blocks the first phases. Each has the plan's assumption.

1. **Licence.** Not a question for now. Assumed: one fully open project with no closed
   edition, the licence chosen at the public release. No plan depends on a paid-only feature.
2. **The seven asks of Agent OS** in section 4. Assumed: they are Agent OS work for the later
   three-project integration, nothing in Workspace's plan waits on them, and the bridged
   connector covers the gap until then.
3. **The role a newly added agent gets.** Assumed: the same starting role as a newly invited
   person, and a workspace can change that default.
4. **Renaming in the code** (section 9). Assumed: yes, in steps, starting with the interface
   and the tool names and reaching the database last.
5. **The free hosted plan's numbers.** Assumed: every module, full history, 10 people, 2 GB
   of files, agents free.
6. **The owner's own setup** (machine names, the bridge for one host) moves out of this
   repository into configuration. Assumed: yes.
7. **Existing agents at the upgrade.** Assumed: they keep today's behaviour through a
   one-time role ("Member, suggests") that the owner switches to any role in one step. New
   agents get the same starting role as a new person.
8. **DMs for a company.** Assumed: personal by default (Discord-style), with "kept" DMs as a
   setting a workspace can switch on.
9. **Accounts owned outside a workspace joining it.** Assumed: allowed when self-hosted, "ask
   an admin" when hosted, changeable per workspace.

## 11. Settled in planning

The fourteen workstream plans raised these and the lead accepted them. Each is in the plan
of the workstream named.

- **Access (K1).** One access function with three answers: allow, suggest only, deny. An
  "edit own" level sits between suggest and edit. "Act only within what the requester could
  do" is a role setting. Delegation also covers an app acting for an agent. An owner can be
  any account; the rule is that a workspace always has an owner whose sign-in can be
  recovered. `conversation_visible_to` stays the only gate on conversations.
- **Operations (K2).** A version is checked on changes, not on creates. "Can be proposed" is
  a flag on any change operation. Old tool names stay as aliases. Old protocol revisions and
  old wake formats are pinned by contract tests and retired only by their own slice.
- **Things and events (K3).** Containment is a parent, not a link, and mentions of people
  are not links. The audit trail is the event log plus a sign-in record. Log retention is a
  plan value. One log, one sequencer.
- **Spaces (K4).** A space may switch a module off but not on. Personal spaces are made on
  first use. Every table carries its workspace, except a published list of global identity
  tables.
- **Servers (K5).** Leases for jobs only one process may hold. Signed download links.
- **Tasks and views (T1).** Chart is a kind of view. Triage, cycles, delegates and
  initiatives belong to Tasks. One fields system and one views system.
- **Runs and hosts (A1).** `runs` is the team's record and the built-in host keeps its own
  store. "Interrupted" is a final outcome and "not responding" is a mark, not a state. A
  chat reply is not a run unless one was launched or opened. Hosts are app accounts.
- **Agents (A2).** Loop limits are chain depth plus a turn allowance, and "continue" is a
  permission. Budgets are rows in the one limits table.
- **Modules (M1).** Panels are fixed blocks plus framed pages from approved origins.
  Packages carry agent templates, never accounts.
- **Documents (C1).** Comments on a thing are one conversation with anchored threads.
  Restore works for documents and records, with change sets for sheets. One thread-creation
  function.
- **Chat and inbox (C2).** Unread rules are written down. Status is one record for every
  account.
- **Web (W1).** A list may refetch itself when membership can't be told from an event, never
  a whole space. Offline reading is opt-in per person, a workspace can switch it off, and it
  is cleared at sign-out. A measurable speed bar on phones.
- **Hosting (H1).** Staff never act as a member. First run is protected by a one-time code.
  One limits check for plans, budgets and rates.
- **After the second read (R44 to R50).** The build is 31 large builds, not a pull request
  per slice. Approval is by permission for every kind of account. Privacy promises are
  about what the application shows and delivers. A workspace's export follows whether a DM
  is kept, not the id on its row. The foundations are proved by one journey test at the end
  of stage 2. A new seam ships with a real user. Hosting numbers are targets until measured.
  Later stages are a forecast the lead may cut again, in writing.
