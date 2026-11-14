> **Superseded where [`docs/plans/master/`](master/README.md) says otherwise.** This plan explains how the existing pieces were built; the master plan decides what changes.

# Modules (AWD-75)

Every feature of Agent Workspace is a module with a manifest. Core modules
(the ones in this repository) fill in the same contract an extension will;
their only privilege is that their code runs in the server's process. A
workspace switches modules on and off, roles narrow that for the people who
hold them, agent limits narrow it for their agents, and layouts decide what
each person sees. Off means hidden, never deleted.

This is the first version. It covers the manifest format, the switches, the
guards, and the shared contract that lets any module work with the others.

## Manifests

One file per core module in `modules/` at the repository root, read by the
server (`apps/server/src/modules/manifest.rs`) and the web app
(`apps/web/src/modules/slots.ts`, which bundles only each manifest's `slots`
and `name`).

| Module | Needs | What it is |
| --- | --- | --- |
| `workspace` | (always on) | Home, search, people, roles, settings, notifications, files |
| `messages` | | DMs and group conversations, replies, scheduling |
| `projects` | | Projects, their members and settings |
| `channels` | projects, messages | A project's channels and threads |
| `tasks` | projects | Tasks, board, timeline, milestones, planning, views, templates |
| `pages` | projects | Pages, decisions, skills, the guide |
| `whiteboards` | projects | Whiteboards |
| `collections` | projects | Record types a project makes: fields, records and views (AWD-90) |
| `code` | projects | Repositories and pull requests |
| `runs` | tasks, channels, code | Runs on harnesses, approvals, usage |
| `agents` | | The agent directory, profiles, @-mentioning agents |
| `inbox` | | The inbox and Later |
| `automations` | projects | Rules and webhooks |
| `suggestions` | | Model suggestions and summaries |

A manifest declares:

- who it is: `id`, `name`, `description`, `version`, `kind` (core, built,
  connected), `switchable`, `needs` and `worksWith`;
- `permissions`: the permission keys (AWD-74) it owns;
- `tools`: its MCP and public API tools, each with what it does (see the
  agent tool pattern below);
- `routes`: its server routes, as the router writes their paths (a trailing
  `*` matches anything after it; the longest match wins);
- `events` it emits and listens to, `notifications` it sends, `search` kinds
  it answers, and the automation `triggers` and `actions` it brings;
- `things`, `links` and `embeds` (the shared contract, below);
- `slots`: where it shows up in the app: `pages` (web routes), `parts`
  (layout parts: sidebar rows, tabs, task fields, composer extras),
  `sidebar` (rail and sidebar rows that aren't parts), `home` sections,
  `settings` pages, `palette` entries, `shortcuts`, `slash` commands and
  `setup` steps;
- `settings` keys and `nouns` it owns.

### Adding something

A new route, tool, layout part, setting, setup step, event, notification
kind, search kind, automation trigger or action, web route, palette entry,
shortcut, slash command, settings page or Home section goes in the manifest
of the module it belongs to. The guards fail until it does:

- server: `cargo test -p agent-workspace-server --lib -- modules::`
  (`apps/server/src/modules/mod.rs`), which reads the routes from the source
  as `policy_inventory` does, and the tool list from the MCP router;
- web: `apps/web/src/modules/slots.test.ts`, which reads `App.tsx`'s routes,
  the command palette's entries, the shortcut sheet, the slash commands, the
  settings registry and the Home sections.

Each value must be claimed by exactly one module, and every claim must match
something that exists.

## Switching modules

- A module's switch is the `modules.<id>` setting at workspace scope, on
  unless set to `false`. The settings routes refuse these keys; they change
  through `PUT /api/v1/organizations/{id}/modules` (owners and admins,
  audited with what was off before and after), which takes `switches` and an
  optional `preset`. `GET` on the same path lists every module and why one is
  off.
- A module is on when its switch is and everything it `needs` is on.
- Presets (`modules/presets.json`): Everything, Chat with my agents
  (projects off, so channels, tasks, pages, whiteboards, code, runs and
  automations go too), Pages only (the workspace's own pages, with Projects off), DMs only (messages).
- Roles narrow modules (AWD-74's roles): a role's `modules_off` turns
  modules off for its holders and their agents, on top of the workspace's
  switches. In a project the project role counts, elsewhere the workspace
  role. A role only narrows; the owner role never does. Owners and admins
  set it in the role editor, or apps acting for them with `role_write`
  (`modulesOff`); it's audited with the role.
- Every permission belongs to exactly one module (test-enforced), matching
  the Roles page's groups; members and settings are the workspace's.

### What follows the switches

- **Layouts.** The layout response (`/projects/{id}/layout`,
  `/organizations/{id}/layout`) carries `modulesOff` and adds every off
  module's parts to `hidden`, which "Show hidden parts" doesn't undo. So
  every `shows(part)` in the web app follows modules with no further code.
  `experience.moduleOn(id)` asks about a module directly.
- **Pages.** A link into an off module opens a "turned off" page that says
  nothing was deleted, not a 404 (`ModuleGate` in
  `AuthenticatedRouteBoundary`).
- **Navigation.** Rail tiles, the Home sidebar, Home sections, the command
  palette, the shortcut sheet and keys, slash commands and settings pages
  leave out what belongs to off modules.
- **Search** searches each kind only where its module is on; DMs need
  Messages, a project's channels and threads need Channels.
- **Notifications** of an off module aren't delivered (no push or email),
  and every notification read leaves them out (one SQL condition generated
  from the manifests).
- **Briefing and `project_open`** leave out sections of off modules;
  `project_open` lists `modulesOff`.
- **Automations and webhooks** don't follow a workspace's events while
  Automations is off; a rule whose trigger or actions belong to an off module
  doesn't fire, and a queued run of one is recorded as skipped with the
  reason.
- **Setup steps** of off modules don't show.
- **Agents' tools** follow modules, roles and agent limits together:
  `tools/list` leaves out tools of modules that are off for the caller
  everywhere they belong, and a call to one where it acts (the project it
  names) is refused with `permission_denied`, `retryCategory: "module"`,
  saying the module is turned off. The same check runs for the public API
  and automations, which call tools the same way. `module_list` says
  what's on, what each module does and why one is off.

## The shared contract (principle 1)

Defined in `apps/server/src/modules/contract.rs`, mirrored in
`apps/web/src/modules/contract.ts`.

- **Things.** Everything a module makes is a `Thing`: `id`, `kind`,
  `module`, `title`, `status`, `owner`, `projectId` and `href`. A manifest
  declares its kinds (`things`) with the fields each carries beyond the
  shared ones. Today tasks, pages, decisions, whiteboards, threads, channels,
  conversations and projects are things (`apps/server/src/modules/things.rs`).
- **Links.** `part_of`, `blocks`, `about`, `made_from`, `relates_to` and
  `duplicates`, each read both ways ("Blocks" and "Blocked by", "About" and
  "Referenced in"). The links stored today (subtasks, dependencies,
  relations, a task's pages, decisions and threads, a whiteboard's links)
  read through `GET /api/v1/things/{kind}/{id}` and the `thing_open` tool, as
  far as the reader can see, from modules that are on. A link to a task,
  page, whiteboard, thread, channel or conversation written in a message, a
  page, a task's description or a task comment is kept in
  `thing_references` by triggers on those tables (every write path), and
  reads as "Referenced in" on the thing (and "About" from the page or task
  that wrote it, comments included); task chips in messages and comments
  read the same way. A DM thread's link (`/messages/{parent}?thread={id}`)
  counts as the thread (AWD-58).
- **Embeds.** A module offers live blocks for pages by declaring them
  (`embeds`: an id, a label, the kind of thing, and the piece that draws it),
  never by changing the page editor. Docs (AWD-53) reads these.
- **Fields and views.** One set of field types, named as task fields
  (AWD-89) store them: `text`, `number`, `date`, `single_choice`,
  `multiple_choice`, `person`, `url`, `checkbox` and `link` (a link to
  another thing, such as a task or a page), plus `long_text`, `status`,
  `people` and `labels`. There is no translation layer between the two;
  "thing" is the contract's word for what links point at, never a field
  type. And view types and view types (`list`, `table`, `board`,
  `calendar`, `timeline`, `gallery`). Task fields (AWD-89) and collections
  (AWD-90) fill these in; a thing's own field values travel in `fields`.
- **Events.** Named `<thing>.<what happened>` (`task.created`,
  `run.finished`); each is emitted by exactly one module, and manifests say
  which they follow.
- **The agent tool pattern.** Each tool does one of five things:
  `describe` (open one thing or the module), `query` (find or list),
  `create`, `change`, or `suggest` (propose a change for a person to accept).
  Every core tool is labelled in its manifest, and `module_list` groups a
  module's tools that way.

## Pieces the app draws (principle 5)

A module never draws its own layout. It names a piece of one kind of thing
with some fields, and the kit draws it for every width
(`apps/web/src/modules/pieces/`):

| Piece | Wide | Narrow (under 600 px of its own width) |
| --- | --- | --- |
| list | rows, grouped by a field | the same, wrapping |
| table | columns | a card per row, each value labelled |
| board | a column per value | scrolls sideways inside itself |
| detail | title, status, fields, actions | the same, stacked |
| fields | labelled values | the same |
| form | fields to fill in, a button | the same |
| card | one thing, small | the same |
| actions | buttons | one menu |

Each piece is its own CSS container, so it fits wherever it sits: a page, a
Home card, a side panel or an embed. `/design-system/pieces` draws every
piece a manifest declares, plus each kind of piece once, with made-up things
(long titles and values on purpose). Two guards:

- `apps/web/src/modules/pieces/Pieces.test.tsx` draws each declared piece and
  runs axe;
- `apps/web/e2e/pieces.spec.ts` opens the gallery at 1440 and 390 px in light
  and dark, and fails on content cut off sideways, the page scrolling
  sideways, or an axe error. A new module's pieces are checked the day it's
  added.

Values read the same in every piece: labels are chips, in the tone a field's
`tones` gives each value (muted otherwise); a moment reads "5 min ago" and a
date names itself ("Edited 5 min ago"), with the person after it reading "by
Ana"; long text sits under the title as a summary, two lines at most, unless
the field is `markdown`, which reads in full. A link field's value is
`{ label, href }`: it names itself ("In #design") and opens where it points, or
is just its name when the reader can't open it. In a row or a card, a field
with nothing to show leaves nothing behind (a table cell still says "None").
A thing may carry a `key` (a task's AWD-7), drawn before its title. Each list row is `#<kind>-<id>`, so a link
can jump to it.

Core pages move onto the pieces over time. The first is a project's Pages
list (AWD-98), drawn as a list of `page` things, then its Decisions list
(AWD-116), a list of `decision` things, then Home's Working now and Your
tasks cards (AWD-132), lists of `run` and `task` things.

## Next

- **Built and connected modules** (AWD-90, M5) fill in the same manifest from
  data; packages (AWD-76) set modules, roles and layouts at once.
