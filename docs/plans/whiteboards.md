> **Superseded where [`docs/plans/master/`](master/README.md) says otherwise.** This plan explains how the existing pieces were built; the master plan decides what changes.

# Whiteboards

Workstream W16, task AWD-15. Shared Excalidraw boards that people draw on in
the browser and agents read, draw on and look at through MCP, with no browser
open. Built in `apps/server/src/whiteboards/`, `apps/server/src/mcp/whiteboard_tools.rs`
and `apps/web/src/whiteboards/`; schema in migration `20261017070000_whiteboards.sql`.

## What a board is

- A board belongs to a workspace (organization). With no project it is for
  everyone in that workspace; in a project it is for that project's members.
  `whiteboard_visible_to(board, actor)` decides who sees it, and every read
  goes through it. Agents are also checked with `actor_can_access_project` on
  the board's project, every call, and connections confined to one project
  (older credentials) see only that project's boards. Editing a project board
  needs the same permission as editing its tasks.
- Workspace boards are visible to every enabled member of the workspace,
  agents included, whatever projects an agent's connection is limited to by
  project rules. Put a board in a project to keep it to that project.
- Links put one board on any number of tasks, projects and pages
  (`whiteboard_links`), with no copy. Unlinking never touches the drawing.
  The linked task, project or page shows its boards, and the board shows
  where it is linked.
- Rename, duplicate (scene and images, not links), move between a project and
  the workspace, trash with Undo, restore from trash. Trashed boards drop out
  of lists and links but keep everything.
- Search matches titles and the words on a board (text elements and labels).

## The scene

The server owns the scene. Each element is a row in `whiteboard_elements`
holding the JSON Excalidraw writes, so a browser gets back exactly what it
sent and new Excalidraw fields need no server change. The server checks only
what it relies on: an id, a known type, `version` and `versionNonce`, finite
geometry, sizes (256 KiB an element, 20,000 a board, 2,000 a write), and it
refuses `javascript:` and other script links.

Images live in `whiteboard_files` (4 MiB each, 64 MiB a board, PNG, JPEG,
GIF, WebP or SVG checked by their bytes), uploaded before the element that
shows them. SVG images are served with a sandboxing policy.

## Sync

Excalidraw's own rule, no CRDT:

1. A browser sends only the elements whose `version`/`versionNonce` changed
   since it last heard from the server.
2. For each element the server keeps the copy with the higher `version`; on a
   tie, the lower `versionNonce`. That is the rule Excalidraw's
   `reconcileElements` uses, so every client and the server agree.
3. Copies that lose come back in the reply as `rejected`, and the browser
   merges the winner in. Nothing anyone changed is overwritten silently: two
   people editing different elements both keep their edits; two people
   editing the same element converge on one of them, deterministically.
4. Each accepted write bumps the board's `scene_seq` once and goes out
   through the outbox as `whiteboard.updated` on the existing realtime
   channel. Open boards fetch `scene?since=<seq>` and reconcile. After a
   reconnect, or every 5 s while the live connection is down, they pull the
   same way. The board shows Saved, Saving, Unsaved, Offline or Retrying,
   and warns before a tab with unsaved changes closes.

Deletions are elements with `isDeleted`, so they sync like any other change
and can be undone.

## Agents

MCP tools: `whiteboard_list`, `whiteboard_read`, `whiteboard_render`,
`whiteboard_create`, `whiteboard_edit`, `whiteboard_update`,
`whiteboard_duplicate`, `whiteboard_link`; `task_link` also takes a
`whiteboardId`.

- `whiteboard_read` gives a summary per element (type, position, size,
  label, bindings, frame, group) rather than raw JSON (`raw: true` for that),
  plus links, recent editors, saved versions and `sceneSeq`.
- `whiteboard_edit` takes up to 500 operations applied in one transaction:
  add, update, delete, connect (an arrow bound at both ends), move (arrows
  follow), align, distribute, layout (down, right or grid), group, ungroup and
  order. Agents name new elements with readable ids so later operations can
  refer to them. Labels become bound text sized to fit. Every touched element
  gets a new version and nonce, so agents' edits merge with people's by the
  same rule. `expectedSceneSeq` refuses the edit if anyone changed the board
  since the agent read it.
- `whiteboard_render` returns a PNG (as an image the model can see) or SVG,
  drawn on the server with resvg and the bundled Inter font. It is a clean
  rendition of the shapes, labels and arrows, not Excalidraw's hand-drawn
  look; enough to check layout. The boards list uses the same SVG as its
  previews.
- Agents' edits reach open boards live, and the board shows "<agent> is
  drawing" while an agent's edits are recent.

Safeguards: deleting more than 25 elements in one edit needs
`confirmDelete: true`; edits and restores are written to the audit log; the
board saves a version at most every 10 minutes while it changes (100 kept),
always before a write that deletes many elements, and before a restore.
Anyone who can edit can restore a version from History; restoring is itself
undoable from History.

## Web

Excalidraw 0.18.1 (MIT) loads only on a board page, in its own lazy chunks
under `assets/whiteboard/`, which the web budget holds to their own per-chunk
limit (see `scripts/web-budget-lib.mjs`). Its fonts are copied into the build
at `/excalidraw-assets/fonts`, and the build points Excalidraw's CDN fallback
at that same copy, so the app's `font-src 'self'` holds and opening a board
contacts no third party. The page's `img-src` allows `blob:` because
Excalidraw shrinks dropped photos through one.

## The AgentOS boundary

AgentOS (or any other harness) is a client like any agent: it reaches boards
only through these MCP tools or the REST routes, as an agent account or a
person's connected app, under the same permission checks. Nothing in the
whiteboard store knows about AgentOS, and no harness reads or writes the
tables directly. A harness that wants to draw sends operations; one that
wants to see sends `whiteboard_render`; one that wants to react to changes
uses the realtime `whiteboard.updated` event or polls `scene?since=`.

## Not built yet

- Live cursors and selections of other people. The board shows recent
  editors and agents drawing, not where they point.
- Swappable storage for images: they are in PostgreSQL, capped per board.
  Moving them to object storage only changes `store.rs`.
- Comments pinned to elements.
- Hand-drawn style and Excalidraw's fonts in server renders.
