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
