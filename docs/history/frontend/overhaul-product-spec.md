> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Frontend overhaul product specification

## Product posture

Agent Workspace is a calm, exact operational workspace. It borrows the instant
legibility of Slack and Discord and the task density of Linear while remaining
its own coordination product. Work stays Project-local; global surfaces contain
only the current Actor's cross-Project attention and recent locations.

The interface is dark-first for a technical operator working across mixed-light
desktop environments. Light and system themes are complete alternatives, not
inverted afterthoughts. Decoration never competes with durable state.

## Information architecture

- **Global Home:** recent locations and current-user attention across Projects.
  It does not merge Channel timelines or expose inaccessible Project details.
- **Project:** a durable environment with Overview, Work Items, Channels,
  archived conversations, Settings, repositories, members, integrations, and
  execution context.
- **Primary resource:** Channel, Thread, Work Item collection/detail, settings
  section, or Project overview occupies the main region.
- **Context:** relevant links, repositories, participants, execution state, and
  metadata use a collapsible details region rather than competing with primary
  work.

## Responsive shell

### Desktop

Four structural regions: 64 px global Project rail, 248 px Project sidebar,
flexible primary workspace, and a 288 px optional context region. The primary
resource owns scrolling. Navigation stays still and dense.

### Tablet

The rail and Project sidebar remain visible while context becomes an explicit
right-side sheet. The main region never compresses below a usable reading and
editing width. A visible Details control opens the sheet with modal focus and
Escape/backdrop close.

### Mobile

The main resource is the only persistent region. A compact top bar opens
separate Project, section, and Details drawers. Drawers are not scaled-down
desktop columns: each becomes a single-purpose modal navigation surface with
large targets, inert background, focus restoration, and the current Project or
section in its control label.

## Project switching and navigation

- Pinned Projects precede five recent unpinned Projects; the active Project is
  always reachable. Attention count is textually named.
- Search is a keyboard-first command surface for Project name/key. A future
  command palette may add navigation and safe actions, but no unsupported
  command is shown now.
- Project selection restores the server-owned per-Actor route, filters, and
  context preference when permitted. Viewers navigate without a write.
- Archived and inaccessible remembered locations fall back to a safe Project
  overview without revealing the stale resource.

## Conversations and execution

- Channels show chronological durable messages. Threads are first-class
  one-level conversations beneath a parent Channel.
- The timeline groups ordinary Actor messages, structured system entries,
  significant Worker events, questions, and approvals by meaning rather than
  wrapping every entry in an identical card.
- Streaming deltas are explicitly provisional. Durable refetch replaces them.
- Run state is always icon plus label. Actions appear only from exact
  server-returned capability grants: start, interrupt, retry, answer, approve,
  deny, publish, create/refresh pull request, acknowledge, and refresh.
- Humans, external agent members, and execution workers share an identity
  vocabulary while retaining an explicit kind label. Provider names remain
  diagnostic detail rather than ordinary identity.

## Work Items

- List is the high-density default; board is a projection of the same records.
- URL-backed filters preserve search, type, status, priority, archive, page, and
  view state. Clear filters is always reversible.
- Rows prioritize readable key, title, state, assignee, type, and priority.
- Board columns use semantic status and provide both pointer and keyboard move
  controls. No speculative local synchronization model is introduced.
- Detail separates editable definition, execution entry points, acceptance
  criteria, linked Threads/repositories/pull requests, and activity. Conflicts
  preserve the draft while exposing the authoritative server version.

## Settings hierarchy

Project details and access appear first, followed by collaboration (team and
external agents), execution sponsorship/profile, repositories, GitHub delivery,
and destructive archive actions. Viewer explanations are explicit. Future
global account or Organization settings must use their own route rather than
overloading Project settings.

## State language

- **Loading:** structural skeleton matching the destination.
- **Empty:** explain what belongs here and the permitted next action.
- **Error:** name what is unavailable, preserve known durable state, and offer a
  scoped retry when valid.
- **Degraded/reconnecting:** state that live updates are delayed and durable
  records are being rechecked. Never imply work was lost.
- **Offline:** future explicit offline detection; the current implementation
  reports reconnecting because mutations require the host.
- **Forbidden/not found:** use the established non-disclosing resource language.
- **Archived:** retain history, show read-only state, suppress mutations.

## Keyboard and accessibility contract

- WCAG 2.2 AA target; no conformance claim from automated checks alone.
- Logical DOM and focus order, visible focus, semantic landmarks, labelled
  controls, skip link, no traps, modal focus containment/restoration.
- Color is never the only state signal. Normal text contrast is at least 4.5:1;
  large text and component boundaries meet their applicable thresholds.
- Async success/error changes use bounded live announcements. Hostile output
  remains inert and is never promoted to HTML or an unbounded live region.
- Board, menus, drawers, dialogs, tabs, pickers, approvals, and settings remain
  keyboard operable at 200% zoom and 320 px width.

## Theme, density, and motion

- Theme choices: System, Dark, Light. A validated custom-theme schema may
  override semantic brand/accent roles only; malformed or unsafe values fall
  back without affecting authority or data.
- Density choices: Compact, Comfortable, Spacious. Density adjusts row/control
  rhythm, not readable font size or target minimums on touch layouts.
- Preferences are local presentation-only data. Credentials, tokens, Actor data,
  capabilities, and Project records are never persisted with them.
- State transitions use 120–200 ms exponential ease-out. Reduced motion removes
  nonessential transitions and animation while keeping all content visible.

## Notifications and future command palette

Current attention remains the Project rail, sidebar state, in-page status, and
realtime connection presentation. A future durable notification center and
command palette may unify navigation and current-Actor attention after their
server contracts exist. This overhaul does not fake either capability.
