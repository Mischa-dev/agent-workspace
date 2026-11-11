# Collections

Task AWD-90 (milestone M4 of the AWD-81 plan). Record types a project makes
for itself, such as Playtest feedback, Builds or Testers: each with its own
fields, records and saved views, using the same field types as task fields
(AWD-89). Built in `apps/server/src/collections.rs` and
`apps/server/src/collections/`, with the agent tools in
`apps/server/src/mcp/collection_tools.rs`; schema in migration
`20261111081500_collections.sql`. The `collections` module
(`modules/collections.json`) switches it on and off per workspace and role.

Slice 1 (this change) is the server: storage, the web app's routes, the five
agent tools and the server-side query. Slice 2 is the Collections area in
the web app (an editable table, a record panel with history, cards on a
phone, live updates). Slice 3 embeds a collection's view in a page as a live
block in the page editor (AWD-53).

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
- A view is a way to look at a collection: a type (only `table` so far),
  filters, up to three sort keys, a field to group by, and columns (order,
  width, hidden). A collection keeps at least one view and at most 30. A view
  stores field IDs, so renaming a field never breaks it; a view that names an
  archived field reads as if that part weren't there until the field comes
  back. "me" in a person filter stays "me", and means whoever reads it.

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
record's number.

## Agent tools

Five tools, the same whatever collections a project has:

| Tool | Does |
| --- | --- |
| `collection_list` | A project's collections with their record counts. |
| `collection_describe` | One collection's fields, views and definition: its fields and views by name, in the shape `collection_write` takes, so passing it to `collection_write` in another project makes the same collection there. |
| `collection_write` | Create a collection with its fields and views in one call, or change one. Fields and views are matched by name (or by `field` and `view` to rename), so writing a definition again changes nothing. Needs `collections.build`. |
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
