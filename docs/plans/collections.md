> **Superseded where [`docs/plans/master/`](master/README.md) says otherwise.** This plan explains how the existing pieces were built; the master plan decides what changes.

# Collections

Task AWD-90 (milestone M4 of the AWD-81 plan). Record types a project makes
for itself, such as Playtest feedback, Builds or Testers: each with its own
fields, records and saved views, using the same field types as task fields
(AWD-89). Built in `apps/server/src/collections.rs` and
`apps/server/src/collections/`, with the agent tools in
`apps/server/src/mcp/collection_tools.rs`; schema in migration
`20261111081500_collections.sql`. The `collections` module
(`modules/collections.json`) switches it on and off per workspace and role.

Slice 1 is the server: storage, the web app's routes, the five agent tools
and the server-side query. Slice 2 is the Collections area in the web app
(`apps/web/src/collections/`). Slice 3 embeds a collection's view in a page
as a live block in the page editor (AWD-53).

## What a collection is

- A collection belongs to one project. It has a name (unique among the
  project's active collections, so agents can name it), a description, the
  name of its title column (Name, Version, Tester) and an order. Archiving
  hides it and its records; nothing is deleted. A project keeps at most 50
  active collections, 200 with archived ones.
- Its fields are rows of `field_definitions` with `record_kind = 'record'`
  and the collection's ID, so tasks and collections share one set of field
  types, value checks, filters, sorts and groupings (`crate::fields`): text,
  number, date, single choice, multiple choice, person, web address,
  checkbox, and link (to a task or a page in the same project). A field's
  type never changes; archiving a field hides it and keeps every value.
- A record has a number in its collection (#12), a title and its values,
  keyed by field ID as a task's are. Archiving a record hides it; restoring
  brings it back. A collection holds at most 100,000 records. Every change
  is in `collection_record_history`, with who made it, the app they acted
  through and each value before and after, as it read then.
- A view is a way to look at a collection: a type (`table`, `board`,
  `gallery` or `calendar`; AWD-110), filters, up to three sort keys, a field
  to group by, columns (order, width, hidden) and, for a calendar, a date
  field. A collection keeps at least one view and at most 30. A view
  stores field IDs, so renaming a field never breaks it; a view that names an
  archived field reads as if that part weren't there until the field comes
  back. "me" in a person filter stays "me", and means whoever reads it.

## Boards, galleries and calendars (AWD-110)

The same view settings drive every layout; a view's type doesn't change once
made (add another view instead).

- **Board:** a column for each value of the field the view groups by (a
  single choice's options in order, a person, or a checkbox's checked and
  not), then "No value". A new board written without `groupBy` takes the
  collection's first field that can group, and is refused when there's
  none. Each column reads its own records: the records route's `group`
  parameter keeps only one group's records (a key from `groups`, such as an
  option's ID or `none`), worked out the same way as the group counts, so
  a column pages on its own with a cursor no other column takes. Dragging
  a card to another column, or its menu's Move to (the keyboard's way),
  sets that value; the card shows there at once and saves like any edit.
- **Gallery:** a card for each record, in the view's order, with the title
  and the view's shown fields that have a value. The grouping isn't used.
- **Calendar:** a month (six weeks from a Monday), each record on the day
  its `dateField` holds. A new calendar without one takes the first date
  field. The records route's `dateField`, `from` and `to` (YYYY-MM-DD, `to`
  not included, at most 62 days) narrow the view's records to the weeks on
  screen, on top of its filters. A day's + adds a record on that day;
  dragging a record to another day changes its date. On a phone the month
  is a list of the days that have records. The month is in the address
  (`?month=2026-11`).

Agents write layouts with `collection_write` (`type`, `groupBy`,
`dateField`), and `collection_describe` gives them back by name, so a
definition carries them to another project.

## Who does what

Anyone in the project reads its collections. Two permissions (AWD-74), both
held by owners, admins and members and not by viewers:

- `collections.edit` ("Edit records"): add, change, archive and restore
  records.
- `collections.build` ("Build collections"): create collections, change
  them, and add or change their fields and views.

Every write takes the project row lock first, then the collection's row, and
checks the caller's role again under it, so record numbers, limits and names
are exact. An ID from another project (or workspace) never matches: every
lookup of a collection is by its project, and of a field, view or record by
its collection; values that name a person, task or page are checked to be in
the project.

## The query

`collections::query` builds one SQL query from a view (or what the caller
says instead): filters, words to search for, sort keys, grouping and
whether archived records count. It reads one page at a time with a keyset
cursor (the sort values of the last row, with a fingerprint of the query, so
a cursor only reads back with the query it came from), counts the total, and
counts each group, without reading every record. Search words must each be
in the title, a text or web address value, a chosen option's name, or be the
record's number. Text, titles and names sort by their first 100 characters,
lowercased (`fields::SORT_PREFIX_CHARACTERS`), so a cursor stays under 8 KB
however long the values are; records alike that far fall through to the next
sort key, and last to their ID.

## Agent tools

Five tools, the same whatever collections a project has:

| Tool | Does |
| --- | --- |
| `collection_list` | A project's collections with their record counts. |
| `collection_describe` | One collection's fields, views and definition: its fields and views by name, in the shape `collection_write` takes, so passing it to `collection_write` in another project makes the same collection there. |
| `collection_write` | Create a collection with its fields and views in one call, or change one. Fields and views are matched by name (or by `field` and `view` to rename), so writing a definition again changes nothing. With `expectedVersion`, a write that would change anything (fields and views included) is a conflict if the collection changed since. Needs `collections.build`. |
| `record_query` | Records found on the server: filters by field name, search words, sort, group counts, a saved view to start from, named records with their history; a page at a time with `nextCursor`. |
| `record_write` | Add or change up to 100 records, all or nothing, by number or ID, values by field name. Needs `collections.edit`. |

## Web routes

Under `/api/v1/projects/{project_id}/collections`: list and create
collections; read and change one (`/{collection_id}`); add and change its
fields (`/fields`, `/fields/{field_id}`) and views (`/views`,
`/views/{view_id}`, which can also be removed); and page through, add, read
and change its records (`/records`, `/records/{record_id}`). The records
route takes a view, or filters, sort and grouping as JSON, a search, and a
cursor. Writes take an `Idempotency-Key`.

## Live updates

Every change sends `collection.updated` with the collection's ID and, for
record changes, up to 100 record IDs (a larger change names none). Open
screens refetch what they show.

## In the web app

A project's Collections area (`/projects/{id}/collections`, the layout part
`area.collections`, one lazy chunk) lists its collections with their record
counts and makes new ones. A collection opens with its views as tabs, and
each view is a table edited in place: a click on a value opens the same
editors tasks use (`fields/FieldEditor.tsx`), a title renames in place, the
last row adds a record, and each column's menu sorts, filters, groups, hides,
edits or archives its field. Search, filters, sort, grouping and columns
change what you see at once; people who build the collection save them into
the view (Save view), and anyone can put the view back (Reset). A record
opens in the side panel (`?record=`) with every field, who added and last
changed it, and its history. On a phone the table becomes cards. Changes
show at once and save one at a time per record, each against the version the
last one left (`useRecordEdits.ts`), and until a change saves it shows on top
of whatever loads meanwhile; `collection.updated` refetches what's open of
the collection, a moment after a burst of changes
(`realtime/liveDocuments.ts`). What changes in a view while it saves stays
on screen, ready to save next.

## In a page

A page can show a collection's view live (slice 3), as an embed block. In a
page's Markdown an embed is a line of its own, at the top level, holding
only a link with the title `embed`:

```markdown
[Playtest feedback · Open](/projects/{projectId}/collections/{collectionId}?view={viewId} "embed")
```

The path must stay inside the app (no scheme, no host, no spaces), and a
backslash goes before each backslash or bracket in the label. Anything that
doesn't know embeds still reads the line as a link. The web app
(`ui/markdown/parse.ts`, `embedOf`) and the server (`page_blocks.rs`,
`embed_of`) read it the same way, and the shared fixtures check it
(`parseParity.json`, `blockParity.json`, `pageDocuments.json`,
`pageEdits.json`). In the page's live document an embed is an `embed`
element with `href` and `label` and no content. The server refuses one
inside another block, one with content, and one whose path leaves the app
(`page_docs/ydoc.rs`).

- **Read**: `memory/embeds/PageEmbed.tsx` draws a collection's path as
  `collections/CollectionEmbed.tsx` (its own chunk). That shows the view's
  first 8 records with the title and up to 4 of the view's columns, the
  count, and a link to the rest; a record opens in the collection. Any
  other path shows as a link.
- **Write**: in the editor an embed is an atom node (`memory/live/schema.ts`)
  that can be selected, moved and deleted. Its view keeps one element per
  embed in `memory/live/embeds.ts`, and the page draws `PageEmbed` into each
  through a portal, so embeds share the app's data and routing. The toolbar's
  Collection view button (shown with the `area.collections` layout part)
  opens `collections/CollectionEmbedPicker.tsx`, which picks a collection
  and a view and adds the embed after the block holding the cursor, or in
  place of an empty paragraph.
- **Access**: the page keeps only the path and the label. An embed loads as
  whoever reads the page, so someone who can't see the collection gets "isn't
  here, or you can't see it" and never its records.
- **Live**: `collection.updated` refetches an embed like any open view, so a
  record an agent adds shows without a reload.
- **Agents**: `page_blocks` lists an embed as a block of type `embed`, and
  `page_write` or `page_edit` adds one from the line above.
  `collection_describe` says how.

## Forms (AWD-110)

A form is a public page that adds a record to one collection. It grew from
bug report links: a form is a `bug_report_links` row with `collection_id` and
`form` set (migration `20261113063000_collection_forms.sql`), so it shares
the secret token (only its SHA-256 is stored), the rate limit, pausing,
replacing, and turning itself off when its maker loses the right to add
records.

- `form` holds `{intro, fields: [{field, required}], thanks}`. A form can ask
  any active field except people and links, which point inside the
  workspace; the title is always asked. `collection_forms::check` validates
  it on every write.
- `GET /api/v1/forms/{token}` (public, rate limited, no-store) returns only
  the collection's name, the title's name, and the asked fields with their
  options. A task link's token is a 404 there.
- Answers go to `POST /api/v1/bug-reports/{token}` like any report, as JSON
  or a plain HTML form post. Each one calls `record_write` as the link's
  maker, so the record's history says who shared the form. A resend of the
  same answers on the same day comes back as a duplicate, not a second
  record.
- Making or editing a form needs the right to add records in that
  collection. In the app it's "Forms…" in the collection's menu; agents use
  `bug_report_link_write` with `collection` and `form`. Project settings ›
  Bug reports lists only the task links.
- The page is `/forms/{token}` in the web app, lazy loaded and outside the
  signed-in shell.

## Records everywhere (AWD-110)

- **Search.** `collection_records.search_vector` is generated from the title
  (weight A) and every string value (weight B), with a GIN index (migration
  `20261113082000_collection_record_search.sql`). `search` has a `records`
  kind, shown where Collections is on, never from an archived record or
  collection. A hit's path opens the record beside its collection.
- **Things.** `record` is a thing kind (`modules/things.rs`), declared by the
  collections module with the href
  `/projects/{projectId}/collections/{parentId}?record={id}`; `{parentId}` is
  the record's collection. `thing_open` takes a record's ID or its link.
  A record links out to the tasks and pages its link fields name, and those
  tasks and pages list the record back (read apart from their stored links).
  "Referenced in" from text that links a record is AWD-58's to add: its
  `thing_references` table would need `record` as a target kind and the
  collection link (`?record=`) in `thing_reference_targets`.
- **Events.** A record write's `collection.updated` outbox event (when it
  names its records) routes as `record.created` or `record.updated`, one per
  history entry written with it, keyed by that entry. Webhooks send the
  record with values by field name and, for an update, `changedFields`.
- **Automations.** `record_created` and `record_changed` triggers, for one
  collection or any (migration `20261113081000_record_automation_triggers.sql`).
  They take no task conditions and only notify (the maker or the project's
  admins) or post a message; text can use `{{record.title}}`,
  `{{record.url}}` and `{{collection.name}}`.
- **Links between collections.** A link field's target can be `record`, with
  the collection it holds in `FieldConfig.collection` (written
  `record:<collection ID>`; agents and definitions may name the collection,
  resolved by `collections::link_target`). A value is one record of that
  collection, found by ID, `#number` or title, and sorts by its title. Both
  records list the link as things. A definition exports the target as
  `record:<collection name>`.

## Tasks as the built-in collection (AWD-110)

- Each project has one collection with `builtin = 'tasks'` (migration
  `20261113094000_builtin_tasks_collection.sql`), made the first time
  someone in the project lists its collections, with an "All tasks" table.
- Its records are the project's tasks, read through `collections::query`
  like any records: `collections::tasks::source()` is a subquery over
  `work_items` with a record's columns, the built-in fields (Status,
  Assignee, Priority, Start date, Due date, Labels, under fixed IDs) folded
  into the field values next to the task fields. Grouping by Status or
  Priority reads the statuses and the fixed priorities
  (`tasks::group_sql`), since their options aren't in `field_definitions`.
- Only its views are stored and change here. Writes to its records, fields,
  name or archive state are refused (409 `builtin_collection`, or a
  conflict from the tools) with where to change tasks instead. It isn't
  copied into workspace templates, can't be a record link's target, and
  has no record events.
- In the app it's listed first; a record opens its task's page, and the
  board, gallery and calendar views are the collection ones.
- Tasks still live in `work_items`, with their own screens and tools.

### Records under tasks (step 2 of moving tasks into records)

Mischa chose to move tasks into collection records for real, in six steps:
the Tasks collection (above), records under tasks, one owner for the shared
columns, editing Tasks like any collection, `fields` and `collection` on the
task tools, and an "acts like a task" switch on any collection. Step 2
(migration `20261114010000_task_records.sql`):

- Every task has a record in its project's Tasks collection with the same
  ID, number, title, custom field values (keyed by the task field IDs the
  collection lists), creator, archive state and times. A project's first
  task makes the Tasks collection (`ensure_tasks_collection`, which
  `tasks::ensure` calls too), taking the first free name of "Tasks",
  "Tasks (built in)", "Tasks (built in 2)" and so on.
- The task is still the one that changes. Triggers on `work_items` copy
  each insert, change and delete onto the record in the same transaction;
  a title is cleaned for the record (trimmed, no control characters).
  `collection_records_task_guard` refuses any other write to a Tasks
  record, so the copy can't drift.
- Reads don't change yet: the collection still reads `tasks::source()`.
  Search and the links resolver skip Tasks records, so a task is found
  once, as the task.

### Editing Tasks like any collection (step 3)

The first plan had the record take over the shared columns, with task code
reading through one view. That meant rewriting about 70 server files and
losing the partial and search indexes the board and search rely on, so the
two copies stay instead, kept in step in the same transaction:

- A write to a Tasks record (`collections::task_writes`) is a task change,
  made by the task commands (`update_locked_work_item`, `set_archived`,
  `create_work_item`), so it gets the task checks, permissions
  (`WorkItemEdit`, `WorkItemArchive`, `WorkItemCreate`), activity, audit
  and events. Title is the task title; Status, Assignee, Priority, Start
  date, Due date and Labels are the task's columns; other values are task
  field values. The triggers copy the result onto the record.
- A record added without a title is an "Untitled task".
- Fields added to Tasks (the fields route or `collection_write`) are task
  fields (`record_kind = 'task'`), changed by those with the workflow
  permission. Built-in fields and their names can't be changed or reused.
- Agents still change Tasks records with the task tools; `record_write`
  on Tasks is refused with where to go.
- In the app, Tasks cells, board moves and calendar moves are editable;
  a record still opens its task. Forms don't target Tasks (a plain bug
  report link already makes tasks).

### Collections that act like tasks (steps 5 and 6)

Any collection can switch on "acts like a task" (`collections.acts_like_task`,
migration `20261114070000_task_like_collections.sql`; always on for the
built-in Tasks collection). Its records are then tasks, the way Tasks'
records are:

- Each record has a task with the same ID; `work_items.collection_id` names
  the collection (NULL for Tasks) and never changes. Switching on
  (`collection_make_task_like`) makes those tasks with the project's default
  type and status and keys from the project's task numbers, which the
  records take as their numbers too. The step 2 triggers then keep each
  record in step with its task, and `task_record_guard` refuses other writes.
- So the records show on the board and in My work, and can be claimed,
  run, commented on and linked, like any task. Search finds them once, as
  tasks.
- Their fields are the collection's own, after the built-in task fields
  (`tasks::fields` with the collection's kind). Task code that reads or
  writes one task's values asks `work_item_fields::definitions_for_task`
  (or `kind_of`); the task page shows the collection's fields.
- Writes from the collection's views go through `task_writes` as Tasks'
  do. `record_write` and forms refuse them, pointing at the task tools.
- Switching off (`collections::task_like`) is only allowed while the
  collection has no records, so a task never loses its record; a collection
  whose records are tasks is archived once they are. A collection with its
  own field named like a built-in one (Status, Priority, …) renames it
  first.
- Agents: `collection_write` takes `actsLikeTask`; `task_create` and
  `task_list` take `collection` (by name or ID); `fields` on the task tools
  follow the task's collection; `task_open` says which collection a task is
  in; `project_open` lists the task fields and the collections that act like
  tasks. No new tools.
- Later: workflows of their own (they share the project's statuses now),
  turning the switch off with records, forms that make task-like records,
  and carrying the switch in workspace templates.
