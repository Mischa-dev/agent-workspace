# T1: Fields, views and tasks

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's draft, 2026-11-14. Read-only research against `main` at fe95a74, plus the two
pull requests that moved since that checkout (read through the code host's API, not
checked out): **#251 is merged** (2026-11-14 00:51, "tasks as the built-in collection",
step 1 of the owner's six), and **#262 is open** (step 2, "a record under every task, kept
in step by triggers"). The owner's own plan for the move is the Claude Doc "Plan: tasks as
collection records" (linked from AWD-110's last comment); this plan adopts its six steps
and its three promises and builds the rest of T1 around them. Where I cite #251 or #262
code, the path is the file in that pull request.

---

> **Amended by the architecture review (2026-11-14).** R53 (one public task revision, `work_items.version`) amends section 3.1's separate versions. See `LEAD-RULINGS.md`.

## 1. Goal

One way to add typed fields to anything, and one views system (table, board, list,
calendar, timeline, gallery, chart) with saved, personal and shared views, used by tasks,
by every collection's records and by lists of pages. Tasks are the records of each
project's built-in Tasks collection with a task core beside each record (workflow, claims,
criteria, dependencies, keys), so a filter, a view kind or a field type built once works on
tasks and records alike. On top: formula, rollup, two-way relation, file, email, phone,
system and status fields; a triage queue, cycles, initiatives and delegates for tasks;
internal and public forms with conditions; and a first, real Dashboards module.

What a user notices: every collection gets timeline, list and chart views and personal
views; tasks get calendar and gallery views and the collections' field types; formulas and
rollups appear; a "Triage" queue with one-key accept, decline, duplicate and snooze; cycles;
dashboards of charts over any view. What an agent notices: the same `task_*` tools, plus
`fields` by name on them, one `view_query` that reads any view, and no tool that only works
on one side.

---

## 2. Where the code is today

### 2.1 Tasks as records: what is done and what is open

| Step (owner's plan) | State | What it did |
| --- | --- | --- |
| AWD-110 slices 1-3 (#244, #246, #250) | merged | Board, gallery and calendar views of a collection; forms grown from bug report links; records in search, things, events, automations, and links between collections (`docs/plans/collections.md:43-71`, `:195-254`). |
| 1. The Tasks collection (#251) | merged after this checkout | `collections.builtin` (`migrations/20261113094000_builtin_tasks_collection.sql`); `collections/tasks.rs` reads tasks as records through `source()`, a subquery over `work_items` that folds Status, Assignee, Priority, Start date, Due date and Labels into `field_values` under fixed UUIDs (`tasks.rs:33-38`, `:212-231`); `group_sql` for Status and Priority (`:236-291`); `ensure()` makes the collection lazily (`:295-360`). Writes to its records, fields or name are refused with 409 `builtin_collection` (`READ_ONLY_MESSAGE`, `:374`). |
| 2. Records under tasks (#262) | open | Migration `20261114010000_task_records.sql`: backfill a `collection_records` row per task with the same id, number, title, `field_values`, creator, archive state and times (`:82-93`), a count-and-value check that aborts the migration (`:96-114`), AFTER triggers on `work_items` copying insert/update/delete onto the record (`:116-178`), and `collection_records_task_guard` refusing any other write to a Tasks record (`:182-206`). Reads still go through `tasks::source()`. Search and the links resolver skip Tasks records. |
| 3. One owner for shared columns | not started | Owner's wording: "Task code reads through one SQL view and writes title, number, custom fields and archive state on the record only. The copies on `work_items` and the sync triggers are dropped." |
| 4. Edit Tasks like any collection | not started | Fields on Tasks, edit values from table/board/calendar cells, forms make tasks, a Fields section on the task page. |
| 5. MCP | not started | `fields` by name on `task_create`/`task_update`/`task_update_many`, `collection` on create and list, custom fields listed by `task_open` and `project_open`. |
| 6. Acts like a task | not started | A switch on any collection giving its records a task core and project keys. |

The shape is right and must be kept: same ids, so the 36 foreign keys to `work_items`
(health check C5) never move; task keys stay project-scoped; the task core is a one-to-one
extension. Nothing in this plan changes that.

### 2.2 Fields today

- One field layer: `apps/server/src/fields.rs` (2,279 lines). Nine types
  (`FieldType`, `fields.rs:65-76`): text, number, date, single and multiple choice,
  person, url, checkbox, link (to a task, a page, or a record of one collection,
  `LinkTarget`, `:179-207`). `FieldType::parse("formula")` returns `None`
  (`fields.rs:2007`).
- Definitions: `field_definitions` (`migrations/20261105041500_custom_fields.sql:19-50`)
  with `record_kind IN ('task','record')` and `collection_id`
  (`20261111081500_collections.sql:68-94`). The CHECK on `field_type` lists the nine
  types, so every new type needs a migration widening it. `config` JSONB holds options,
  link target and collection (`FieldConfig`, `fields.rs:223-233`), 16 KB cap. Limits: 50
  active fields, 200 rows per record kind (`:37-39`).
- Values: a JSONB object keyed by field id, on `work_items.field_values` (64 KB) and
  `collection_records.field_values` (64 KB). A field with no value has no key. Values that
  name something hold its id (`fields.rs:15-20`).
- Checking and reading: `normalize_value` (`:707`), `display_value` (`:858`).
- Filtering, sorting, grouping as SQL over the JSONB column: `FieldClause` and `FieldOp`
  (`:918-975`), `push_clause` (`:1157-1235`, uses `jsonb_array_elements_text` for "is",
  so no index can help), `sort_keys` (`:1294-1367`), `group_sql` for single choice,
  person and checkbox only (`:1382-1448`). Max 8 clauses (`:55`), flat AND only.
- Two unused or duplicate vocabularies: `modules/contract.rs:125` `FieldType` (13 types,
  used only by the test at `fields.rs:1981-1993`) and `modules/contract.rs:150` `ViewType`
  (six kinds, used nowhere). On the web, `modules/contract.ts:12` and `:28` duplicate them
  beside `fields/fieldModel.ts:13` (health check I3).
- Labels are a second tagging system (`project_labels`, `work_item_labels`, migration
  0044); #251 already exposes them as the Labels multiple-choice field of the Tasks
  collection (`tasks.rs:138-152`, `:168`).
- No validation beyond type checks: no required fields, defaults, unique, or locked
  fields. Forms carry their own `required` flag (`collection_forms.rs:51-56`).

### 2.3 Views today: two engines, three if you count MCP

| Concern | Task side | Collection side |
| --- | --- | --- |
| Stored view | `saved_views` (`migrations/20261017010000_saved_views.sql:9-35`): the list page's URL query as a string map, `layout IN ('list','board')`, `fields TEXT[]`, personal (`owner_actor_id`) or shared, four built-ins (`work_items/views.rs:53-100`), pins (`saved_view_pins`) | `collection_views` (`20261111081500_collections.sql:96-126`): typed `config` `{filters, sort, groupBy, columns, titleWidth, dateField}` by field id (`collections/views.rs:130-150`), `view_type IN ('table','board','list','gallery','calendar')`, shared only, no pins, no owner, max 30 |
| View kinds | list, board; timeline is a separate page (`apps/web/src/views/TimelinePage.tsx`, `work_items/views.rs:1216`) | table, board, gallery, calendar (`collections/views.rs:28-59`) |
| Filters | 25 URL parameters (`work_items/list.rs:57-97`): type, status, category, priority, assignee, labels (all/any/not), milestone, parent, blocked, ready, claimed, due before/after, overdue, include done, plus `fieldFilters` and `groupField` (AWD-113) | `FieldClause` list (`fields.rs:970`), the same clauses tasks use for custom fields |
| Sort | `Sort` enum (`list.rs:282-379`): newest, oldest, updated, priority, due, title, manual (board rank), field | `SortBy` title, number, updated, field, up to 3 keys (`collections/views.rs:79-116`) |
| Group | `Grouping` (`list.rs:387-420`): status, assignee, priority, label, milestone, parent, type, field | one field that `can_group` (`fields.rs:133`) |
| Query | `work_items/list.rs` (2,578 lines): two-step keyset with fingerprint, `BASE_FROM` (`:1351`), `page` (`:1418`), `total` (`:1618`), `group_counts` (`:1642`), board columns (`:2313`), `matching_tasks` for automations (`:1590`) | `collections/query.rs` (803 lines), same design copied (`query.rs:1-9`): `page` (`:511`), `total` (`:590`), `group_counts` (`:618`) |
| MCP | `task_list` has its own engine (`mcp/tasks.rs:600` `TASK_FROM`, `:596` its own `PRIORITY_RANK`, cursor at `:549-590`), borrowing only `ViewFilters` and `push_field` from `list.rs` (`mcp/tasks.rs:43`); `view_list`/`view_write` in `mcp/views_data.rs:80`, `:269` | `record_query` (`mcp.rs:2047`) through `collections::query` |
| Bulk edit | `task_update_many` (`mcp/task_batch.rs`), web `BulkBar.tsx`/`SelectionBar.tsx` | `record_write` up to 100 records (`collections/records.rs:34`) |
| Import/export | `work_items/imports.rs` (CSV, Markdown, GitHub; export JSON `:2206`) | none |
| Templates/recurring | `task_templates`, `recurring_tasks` (`work_items/templates.rs`) | none |
| Automation conditions | a saved view's query or inline params run through `views::matching_tasks` (`automations.rs:1421-1450`, `:1454`) | `record_created`/`record_changed` triggers, no conditions, only notify or post (`docs/plans/collections.md:243-247`) |

The parallel growth is recent and real: board columns by a single-choice field went into
collections (AWD-110), then into tasks (`0b9b38c`, AWD-103, `list.rs` +70), then a
task-only fix (`eec6f0f`, AWD-113, the `group_field` parameter at `list.rs:91-94`).

Web: `work-items/WorkItemList.tsx` (547), `WorkItemBoard.tsx` (785), `FilterBar.tsx`
(648, URL model `filterModel.ts` 281), `ViewsMenu.tsx` (275), `viewDraft.ts` (stores the
URL map, `viewDraft.ts:13-32`), `boardModel.ts` (317) beside `collections/RecordTable.tsx`
(272), `RecordBoard.tsx` (355), `RecordGallery.tsx`, `RecordCalendar.tsx` (253),
`CollectionToolbar.tsx` (218), `RecordPanel.tsx` (297). Shared already:
`fields/FieldEditor.tsx`, `fields/FieldValueView.tsx`, `work-items/FieldFilters.tsx`,
`work-items/useBoardDrag.ts`, `useWindowedRows.ts`, `useCoarsePointer.ts`
(`collections/RecordBoard.tsx:6`, `RecordTable.tsx:7-8`). The "pieces" kit
(`modules/pieces/Pieces.tsx`, 358 lines) draws only the Pages list and embeds (I3).

### 2.4 Performance at 10,000 tasks

- `work_items` indexes: `(project_id, updated_at DESC, id DESC)`,
  `(project_id, status_id, board_rank, id) WHERE archived_at IS NULL`,
  `(project_id, assignee_actor_id, updated_at DESC)` (`0015_m4_work_items_foundation.sql:104-111`),
  title trigram, `(project_id, number DESC) WHERE archived_at IS NULL`, `(project_id, due_date, number)`
  (`20261016100000_tasks_at_scale.sql:11-31`), milestone, parent, search GIN.
- `collection_records` has only `(collection_id, updated_at DESC, id DESC)`
  (`20261111081500_collections.sql:162`), `UNIQUE (collection_id, number)` and the search GIN.
  No title trigram, no index usable by a field filter.
- The scale test `lists_page_every_sort_and_grouping_quickly_at_ten_thousand_tasks`
  (`tests/integration/w3_tasks_http.rs:1139-1260`) seeds 10,000 tasks and checks paging is
  correct for five sort/group combinations; it asserts no time or plan.
- #251's `tasks::source()` is a subquery that builds `field_values` per row with a
  correlated `jsonb_agg` over labels (`tasks.rs:221-223`). Filtering a built-in field there
  reads the folded JSONB, so the `work_items` column indexes can't be used. Fine for the
  read-only Tasks collection today; not the end state.

### 2.5 Writes that touch the columns a record will own

Only four places write `title`, `field_values` or `archived_at` on `work_items`:
`work_item_commands.rs:566` (insert), `:960-963` (update), `:1238` (archive), and
`work_item_move.rs:2600` (archive the original on a move). The five writes that bypass the
command layer (health check C1: board move, mark blocked, ready for review, GitHub,
reorder) touch status and rank only, which stay on the task core. So flipping ownership is
small on the write side; the cost is the 69 server files that read `work_items`.

### 2.6 Account-kind rules found in this area (for K1)

- `work_items/comments.rs:782`: only a person can moderate task comments.
- `work_item_commands.rs:2641`, `:2718`, `work_items.rs:2069`, `:2093`,
  `work_items/list.rs:1706`: watchers and assignee pickers take `human` and
  `external_agent` accounts, not `execution_worker`. This is mechanical today (execution
  workers are per-project machine accounts that A1 retires), but it must not become a rule
  about agents.
- `mcp/writes.rs:2145-2150`: task writes through MCP refuse `execution_worker`.
- `work_items/imports.rs:1527`: an import commits as a person only.

T1 adds no new ones. Delegates (section 3.6) are any account.

---

## 3. The target design

### 3.1 Storage of a task

End state of the owner's steps 3 to 6, with one clarification about numbers.

```
collection_records (the record, every collection)        work_items (the task core, 1:1, same id)
  id, project_id, collection_id                    <---    id (FK to collection_records(project_id, id))
  number            (collection's #n; for Tasks = key)      number  (the project key number, AWD-12; immutable)
  title, field_values, computed_values                      description, type_id, status_id, priority,
  archived_at/by, version, created/updated at/by            assignee_actor_id, delegate_actor_id,
  search_vector                                             parent_work_item_id, milestone_id, cycle_id,
                                                            board_rank, start_date, due_date, estimate,
                                                            blocked_*, snoozed_until, version
```

- **Clarification on `number`.** The owner's plan says the record owns the number. For the
  Tasks collection the two are equal forever (a task never changes number; a move copies,
  `work_item_move.rs:4-12`). For a collection that "acts like a task" (step 6) a record has
  its collection number (`#12`) *and* a project key (`AWD-131`). So the key number stays on
  the task core (`work_items.number`, `UNIQUE (project_id, number)` stays), set once at
  insert; the Tasks record's `number` is set to the same value at insert and never synced
  again. Nothing to keep in step, and every key lookup, alias and reference keeps working.
- **`description` stays on the task core** until C1 gives records a body document. A record
  body is C1's concern; T1 only needs the record panel to show it.
- **`computed_values`** is a new JSONB column on `collection_records` for formula and rollup
  results (section 3.3). Same shape as `field_values`, so filters, sorts and groups reuse
  the same SQL.
- **`version`**: the record's version is the one optimistic-concurrency version for
  record-owned columns; the task core keeps its own `version` for core columns. A task
  write that touches both bumps both in one transaction (the command layer already locks
  the project, then the task, `work_item_commands.rs:363`). `expectedVersion` on the task
  tools keeps meaning the task core's version, so agents see no change.
- **One read shape**: a view `task_rows` (SQL) joining `collection_records` and
  `work_items` on `id`, exposing the columns 69 files read today under their old names
  (`title`, `field_values`, `archived_at`, ...). Readers move to it file by file; the old
  columns on `work_items` are dropped last.

### 3.2 The field system

#### Field sources (the one new abstraction)

Every field the views compiler can name has an *accessor*: where its value comes from in
SQL. This is what lets task core columns, system columns, stored values and computed
values all be "fields" with one filter, sort and group code path.

```rust
// apps/server/src/fields/access.rs
pub enum Accessor {
    /// A stored value: `<alias>.field_values -> '<id>'`.
    Stored,
    /// A computed value (formula, rollup): `<alias>.computed_values -> '<id>'`.
    Computed,
    /// A column or SQL expression of the record or its extension, typed.
    /// Used by system fields (title, number, created/edited by and at, auto id)
    /// and by the task core (status, assignee, delegate, priority, dates, labels,
    /// milestone, cycle, type, parent, estimate, blocked, ready, claimed, triage).
    Column { expr: &'static str, sql_type: &'static str, joins: &'static [&'static str] },
}

pub struct FieldDefinition {      // existing struct (fields.rs:236), two fields added
    pub id: Uuid, pub name: String, pub description: String,
    pub field_type: FieldType, pub config: FieldConfig,
    pub position: i32, pub archived: bool, pub version: i64,
    pub accessor: Accessor,       // new: Stored for every row of field_definitions
    pub system: bool,             // new: true for built-in fields (not in field_definitions)
}
```

`push_clause`, `sort_keys` and `group_sql` (`fields.rs:1157`, `:1294`, `:1382`) take the
`FieldDefinition` (not just column and id) and switch on the accessor. A `Column` accessor
compiles `is` to `expr = ANY($1)` so the existing `work_items` btree indexes apply. #251's
fixed UUIDs (`tasks.rs:33-38`) stay the ids of the Tasks system fields; new system fields
get the next fixed UUIDs in the same namespace, listed in one table in
`fields/system.rs`.

Why accessors and not "fold core columns into JSONB" (what #251 does): folding makes every
filter on status or assignee a JSONB scan; the accessor keeps the task core's indexes and
keeps 10,000-task lists on index paths.

#### Field types: the full list

| Type (stored name) | Value stored | Filter ops | Sort/group | Notes |
| --- | --- | --- | --- | --- |
| text, number, date, single_choice, multiple_choice, person, url, checkbox | as today | as today | as today, plus group by multiple_choice (one row per option, counted once per group) and by date (day, week, month buckets) | |
| `long_text` | string ≤ 20,000 | contains, empty | not groupable | Exists in the contract enum; now real |
| `email` | lowercased string ≤ 320, checked with a small RFC 5321 shape check | as text | as text | |
| `phone` | E.164-ish string ≤ 32 (digits, leading +, spaces kept for display) | as text | as text | No library; normalise digits for `is` |
| `status` | option id | is, is not, empty, plus `category is` | by category order, then option order | Options carry `category` (`triage`, `backlog`, `unstarted`, `started`, `review`, `completed`, `canceled`). The Tasks Status system field becomes this type, options read from `work_item_statuses`. Any collection can have one status field |
| `relation` | array of record ids (≤ 100) | is (any of), is not, empty | by first linked title | Replaces `link` for new fields; `link` stays readable (a one-value relation). Two-way: see below |
| `rollup` | computed | as the result type | as the result type | See formulas |
| `formula` | computed | as the result type | as the result type | See formulas |
| `file` | array of file ids (≤ 20) | empty, not empty | not sortable | Files attach to the record as a thing (C1, K5) |
| `created_time`, `edited_time` | system column | date ops | yes | Accessor columns `record.created_at`, `record.updated_at` |
| `created_by`, `edited_by` | system column | person ops | yes | `record.created_by_actor_id`, `record.updated_by_actor_id` |
| `auto_id` | system column | number ops on the number | yes | Shows `<prefix>-<number>`; prefix in config; for task-like records it is the project key |
| `button` | none | none | none | Config names one operation (K2) and its arguments as a template over the record. Pressing it calls the operation as the presser |

A migration widens the `field_type` CHECK once for all new stored names
(`field_type IN (... 'long_text','email','phone','status','relation','rollup','formula','file','created_time','edited_time','created_by','edited_by','auto_id','button')`).
System types are never rows unless a person adds a visible copy (for example "Created by"
as a column with a custom name): then a row exists with that type and the accessor ignores
`field_values`.

#### Validation, defaults, required, locked

Added to `FieldConfig` (JSONB, no column migration beyond the type CHECK):

```rust
pub struct FieldConfig {
    // existing: options, target, collection
    #[serde(default)] pub required: bool,          // write refused without a value (records made by
                                                   // import or automation get a warning instead)
    #[serde(default)] pub default: Option<Value>,  // applied at create; "me" and "today" allowed
    #[serde(default)] pub unique: bool,            // text, email, number: refused if another active
                                                   // record in the collection has it (checked under
                                                   // the collection lock)
    #[serde(default)] pub min: Option<f64>, #[serde(default)] pub max: Option<f64>,
    #[serde(default)] pub pattern: Option<String>, // text only, ≤ 200 chars, Rust regex, size-limited
    #[serde(default)] pub edit: FieldEdit,         // "anyone" (default) | "builders"
    #[serde(default)] pub inverse: Option<Uuid>,   // relation: the paired field
    #[serde(default)] pub formula: Option<FormulaConfig>,
    #[serde(default)] pub rollup: Option<RollupConfig>,
    #[serde(default)] pub button: Option<ButtonConfig>,
    #[serde(default)] pub prefix: Option<String>,  // auto_id
}
```

**Per-field permissions.** One attribute only: `edit: "builders"` makes the value editable
only by accounts whose role grants `collections.build` on the collection (K1's permission
check, called with the field as context). No per-field read permissions: a field anyone in
the space can read is the rule, matching Notion and Airtable, and per-field read rules would
leak through formulas, rollups, search and exports. If a team needs a private field, it
makes a separate collection with narrower access. This keeps K1's access function free of
field-level row filtering.

#### Formulas and rollups

**Decision: a small expression language of our own, evaluated in Rust, results
materialised in `computed_values`.** Not the sheet engine. Reasons: IronCalc
(`sheets/engine.rs:1-16`) recalculates a whole workbook model and references cells by
A1 address; a record formula references named fields and must run per record, thousands of
times, inside a write transaction. Compiling formulas to SQL was the other option; it was
rejected because a list read would then pay for every formula and rollup on every row, the
cost of a query would depend on what people wrote, and errors could only surface as
database errors. Materialising gives filters, sorts, groups and charts the same index and
SQL paths as stored values. Function names follow spreadsheets so people who know Sheets
know them.

Language (stored normalised, shown with names):

```
expr     := literal | field | call | expr op expr | "-" expr | "(" expr ")"
field    := "{" name "}"            stored as "{#<field uuid>}" so renames never break
literal  := number | "text" | true | false | date("2026-11-14")
op       := + - * / & (text join) = != < <= > >= and or
call     := NAME "(" args ")"
```

Types: number, text, boolean, date, list (of text, numbers, people, records). Functions in
the first slice (about 40): `IF, IFS, SWITCH, AND, OR, NOT, ISBLANK, COALESCE, SUM, MIN, MAX,
AVG, ROUND, FLOOR, CEIL, ABS, MOD, POW, CONCAT, LEFT, RIGHT, LEN, LOWER, UPPER, TRIM,
CONTAINS, REPLACE, TEXT, NUMBER, TODAY, NOW, DATE, DATEADD, DATEDIFF, YEAR, MONTH, DAY,
WEEKDAY, FORMATDATE, COUNT, JOIN, FIRST, LAST, UNIQUE`. `TODAY` and `NOW` mark a formula
*volatile*.

```rust
// apps/server/src/fields/formula/mod.rs
pub struct FormulaConfig { pub source: String, pub result: ResultType, pub volatile: bool }
pub fn parse(source: &str, fields: &[FieldDefinition]) -> Result<Expr, FormulaError>; // names -> ids
pub fn check(expr: &Expr, fields: &[FieldDefinition]) -> Result<ResultType, FormulaError>;
pub fn evaluate(expr: &Expr, record: &RecordValues, budget: &mut Budget) -> Value;    // errors are values
pub fn references(expr: &Expr) -> Vec<Uuid>;
```

Rollup:

```rust
pub struct RollupConfig {
    pub relation: Uuid,              // a relation field on this collection
    pub target: Option<Uuid>,        // a field of the related collection; None counts records
    pub aggregate: Aggregate,        // count, count_values, count_unique, sum, avg, min, max,
                                     // earliest, latest, percent_checked, percent_empty, list
    pub filter: Vec<FieldClause>,    // optional, on the related records (≤ 4 clauses)
}
```

A "lookup" is a rollup with `aggregate: list`.

**Dependencies.** A table records, per field, what it reads:

```sql
CREATE TABLE field_dependencies (
    workspace_id UUID NOT NULL,
    project_id UUID NOT NULL,
    field_id UUID NOT NULL,          -- the formula or rollup
    reads_field_id UUID NOT NULL,    -- a field it reads (on its own or a related collection)
    via_relation_id UUID,            -- NULL: same record; else the relation it crosses
    PRIMARY KEY (field_id, reads_field_id, via_relation_id)
);
CREATE INDEX field_dependencies_reads ON field_dependencies (reads_field_id);
```

Written when a formula or rollup is saved. A cycle (found by a recursive query over this
table under the collection lock) is refused with the path, as task dependencies are
(`work_item_commands.rs:1168`). Chain depth ≤ 5 relations.

**Recalculation.**
1. *Same record, same transaction.* When a record is written, every formula on its
   collection whose references intersect the changed fields is re-evaluated in topological
   order and `computed_values` written with the record. Bounded: ≤ 50 fields, each with an
   evaluation budget.
2. *Across records, in jobs.* When a record changes a field that some rollup reads through a
   relation, the write enqueues `fields.recalculate` (F9 job queue, `jobs.rs:1-22`) with
   `unique("recalc:<collection>:<target record>")` so bursts coalesce. The job locks the
   target records in id order, re-evaluates, writes `computed_values`, and emits one
   `record.updated` event per changed record with `cause: "computed"`.
3. *Whole collection.* Adding or changing a formula or rollup enqueues
   `fields.recompute_field` for that field: 500 records per job run, the last id in the job
   payload, so it resumes after a restart. Until done, the field reads as "calculating" in
   the UI (the definition carries `computed_through` id).
4. *Volatile formulas* recompute nightly per workspace time zone, through the scheduler.

**Limits** (constants in `fields/formula/limits.rs`): source ≤ 2,000 characters, AST depth
≤ 32, ≤ 20 field references, 10,000 evaluation steps per record, text results ≤ 5,000
characters, rollups over ≤ 1,000 related records (beyond that the value is the error
"too many records"), ≤ 10 formula and rollup fields per collection in the first version,
`computed_values` ≤ 64 KB. Errors are values (`{"error": "Divide by zero"}`), shown in the
cell, filterable as empty.

#### Relations both ways

A relation field may have an inverse: creating "Tester" on Builds with "show on Testers"
creates the pair, each with `config.inverse` pointing at the other. Values live on both
records' `field_values` and are written together by `relations::write_pair` in the same
transaction (lock order: project, then both collections by id, then records by id). K3's
link store is the record of the link (`links` row with kind `relates_to` and
`source = field:<id>`), written by the same function once K3 exists; the JSONB arrays are
the fast filter path. A consistency check job compares them nightly and repairs from the
link store. Until K3 lands, the JSONB pair is the source of truth. A relation's target is a
collection in the same space; across spaces of one workspace is allowed when the writer can
see the target (K1 check on write; on read, ids the reader can't see show as "restricted").
The existing `link` type keeps working: reading a `link` value yields a one-item relation.

#### Fields on other kinds of thing (seam, later slice)

`RecordKind` (`fields.rs:154`) generalises to a *field owner*: `(thing_kind, scope_id)`,
where scope is a collection id for records and a space id for pages or runs.
`field_definitions` gains `thing_kind` (default `record`, with `task` kept for the
existing task rows until step 4 moves them onto the Tasks collection) and `scope_id`.
Kinds that don't have their own values column store values in one side table:

```sql
CREATE TABLE thing_field_values (
    workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    thing_kind TEXT NOT NULL, thing_id UUID NOT NULL,
    field_values JSONB NOT NULL DEFAULT '{}', computed_values JSONB NOT NULL DEFAULT '{}',
    version BIGINT NOT NULL DEFAULT 1,
    PRIMARY KEY (thing_kind, thing_id)
);
```

The views compiler's `RecordSource` for pages joins it. First user: page lists (a page's
"Owner", "Status", "Review date" fields), slice T1.36.

### 3.3 The views system

#### The view definition (data)

Kept in `collection_views` (extended in place; renamed to `views` with K's rename slice,
not before). New columns: `owner_actor_id` (NULL = shared), `builtin`, `source_kind`
(`collection` now; `pages`, `my_work` later), and `view_type` widened to
`table, board, list, calendar, timeline, gallery, chart`. Pins and defaults get their own
tables (section 3.3, "Saved, shared, defaults").

```json
{
  "filter": {"and": [
    {"field": "0192f5c0-...-001", "op": "categoryIs", "values": ["started", "review"]},
    {"or": [
      {"field": "0192f5c0-...-002", "op": "is", "values": ["me"]},
      {"field": "0192f5c0-...-00d", "op": "is", "values": ["me"]}
    ]}
  ]},
  "search": "login",
  "sort": [{"field": "0192f5c0-...-003", "direction": "asc"}, {"field": "number", "direction": "desc"}],
  "group": {"field": "0192f5c0-...-001", "hideEmpty": false, "collapsed": ["none"]},
  "subgroup": null,
  "fields": [{"field": "0192f5c0-...-003", "width": 96}, {"field": "c1f...", "hidden": true}],
  "layout": {
    "board":    {"cardFields": ["..."], "cardSize": "small"},
    "calendar": {"dateField": "0192f5c0-...-005"},
    "timeline": {"startField": "0192f5c0-...-004", "endField": "0192f5c0-...-005", "scale": "week", "showDependencies": true},
    "gallery":  {"cover": "files-field-id", "cardFields": ["..."]},
    "list":     {"density": "compact", "showDone": "tucked"},
    "chart":    {"type": "bar", "x": {"field": "...", "bucket": "week"}, "y": {"aggregate": "count"}, "series": {"field": "..."}}
  }
}
```

- `filter` is a tree, depth ≤ 3, ≤ 20 leaf clauses. A flat array (today's `filters`) reads
  as `{"and": [...]}`, so every stored collection view stays valid without a rewrite.
- Leaf clauses are today's `FieldClause` plus two ops: `categoryIs` (status fields) and
  `within` (`{"values": ["last_7_days" | "this_week" | "next_30_days" | "overdue"]}`
  relative dates, resolved at read time).
- `field` names a field id, a system field id, or `title`/`number`/`updated` for back
  compatibility.
- "me" stays "me" in storage and is resolved per reader (`fields.rs:1014`).
- Config stays ≤ 16 KB.

#### The task filters as fields

Every URL filter the task list knows becomes a system field of the Tasks source, so the
saved-view translator is a table, not code:

| URL parameter (`list.rs:57-97`) | Field (fixed id) | Accessor | Clause |
| --- | --- | --- | --- |
| `statusId`, `notStatusId` | Status (`…001`, type `status`) | `core.status_id` | is / isNot |
| `semanticCategory`, `notSemanticCategory`, `includeDone=false` | Status | via `status.semantic_category` join | categoryIs / categoryIsNot |
| `priority`, `notPriority` | Priority (`…003`) | `core.priority` | is / isNot |
| `assigneeActorId`, `notAssigneeActorId`, `unassigned` | Assignee (`…002`) | `core.assignee_actor_id` | is / isNot / empty |
| `labelIds` / `anyLabelIds` / `notLabelIds` | Labels (`…006`) | `EXISTS` over `work_item_active_labels` | hasAll / is / isNot (new op `hasAll` for multiple choice) |
| `typeId`, `notTypeId` | Type (`…007`) | `core.type_id` | is / isNot |
| `milestoneId`, `notMilestoneId` | Milestone (`…008`) | `core.milestone_id` | is / isNot |
| `parentId` | Parent (`…009`, relation) | `core.parent_work_item_id` | is / empty |
| `blocked` | Blocked (`…00a`, checkbox) | `core.blocked_at IS NOT NULL OR has open blocker` (`HAS_OPEN_BLOCKER`) | is / empty |
| `ready` | Ready (`…00b`, checkbox) | the ready expression `list.rs` uses | is |
| `claimed` | Claimed by (`…00c`, person) | live claim join | notEmpty / empty / is |
| `dueBefore`, `dueAfter`, `overdue` | Due date (`…005`) | `core.due_date` | lt / gt / within overdue |
| — | Delegate (`…00d`), Cycle (`…00e`), Estimate (`…00f`), Start date (`…004`), Created by, Created, Edited, Key (auto_id) | core and record columns | |
| `fieldFilters` | the custom field | Stored | as written |
| `groupField` (AWD-113) | the custom field | Stored | an extra `and` leaf |

Sorts: `newest`/`oldest` = number desc/asc, `updated`, `priority`, `due` (due date asc, then
priority, then number), `title`, `manual` (status position, then `board_rank`: a system sort
key `rank` only the Tasks source has), `field:<id>`. Groupings: status, assignee, priority,
label (multiple choice group), milestone, parent, type, field.

#### The one query compiler

```rust
// apps/server/src/views/source.rs
pub trait RecordSource: Send + Sync {
    /// FROM clause with alias `record` (and `core` for task-like sources).
    fn from_sql(&self) -> &str;
    /// Every field the source knows, system fields first.
    async fn fields(&self, conn: &mut PgConnection, include_archived: bool) -> Result<Vec<FieldDefinition>, AppError>;
    /// WHERE fragments that scope the source (collection id, project id), plus M1's
    /// `object_on_sql` (built-module visibility, M1.6) for collections a built module owns.
    fn scope(&self, builder: &mut QueryBuilder<'_, Postgres>);
    /// The reader's access filter, from K1 (`access::sql_filter`), pushed as AND.
    fn access(&self, builder: &mut QueryBuilder<'_, Postgres>, reader: &Reader);
    /// Sort keys only this source has (task `rank`).
    fn extra_sort(&self, key: &str) -> Option<Vec<SortKey>>;
}

pub struct CollectionSource { pub project_id: Uuid, pub collection_id: Uuid }
pub struct TasksSource      { pub project_id: Uuid, pub collection_id: Uuid } // record JOIN core
pub struct PagesSource      { pub project_id: Uuid }                          // later
pub struct MyWorkSource     { pub reader: Uuid, pub projects: Vec<Uuid> }      // across projects, later

// apps/server/src/views/query.rs  (grown from collections/query.rs)
pub struct ViewSpec { pub filter: FilterTree, pub search: Vec<String>, pub sorts: Vec<Sort>,
                      pub group: Option<Group>, pub in_group: Option<String>,
                      pub archived: Archived, pub only: Option<Vec<Uuid>>, pub range: Option<DateRange> }
impl ViewSpec {
    pub fn from_view(view: &ViewDefinition, fields: &[FieldDefinition], reader: Uuid) -> Self;
    pub fn check(&self, fields: &[FieldDefinition]) -> Result<(), ViewProblem>;
}
pub async fn page(src: &dyn RecordSource, spec: &ViewSpec, reader: &Reader, cursor: Option<&str>, limit: i64) -> Result<Result<Page, CursorRefused>, AppError>;
pub async fn total(src: &dyn RecordSource, spec: &ViewSpec, reader: &Reader) -> Result<i64, AppError>;
pub async fn group_counts(src: &dyn RecordSource, spec: &ViewSpec, reader: &Reader) -> Result<Vec<GroupCount>, AppError>;
pub async fn matching(src: &dyn RecordSource, spec: &ViewSpec, reader: &Reader, ids: &[Uuid]) -> Result<Vec<Uuid>, AppError>; // automations
pub async fn aggregate(src: &dyn RecordSource, spec: &ViewSpec, reader: &Reader, measure: &Measure) -> Result<Series, AppError>; // charts
```

- Two-step reads stay (ids in order, then rows by id), with keyset cursors and the
  fingerprint (`collections/query.rs:1-9`). Cursor ≤ 8 KB.
- **Access (K1).** Today every reader of one project's collection is checked once for
  project access before the query (`mcp/collection_tools.rs:85-100`). The compiler calls
  `RecordSource::access`, which for now adds nothing beyond the source's project scope and
  later pushes K1's generated SQL filter for the `record` kind (needed for My work across
  projects, record-level privacy and guests). No view code checks a role itself.
- **Counts.** `total` and `group_counts` are exact up to 50,000 rows; beyond that a
  collection over the cap returns `{"count": 50000, "atLeast": true}` (records cap at
  100,000 per collection, `docs/plans/collections.md:31-32`).
- **Paging per group**, as boards do (`collections.md:52-56`), and per date window for
  calendar and timeline (`query::date_range`, `collections/query.rs:362`, generalised to
  two fields: overlaps `[from, to)` on start..end).

#### Saved, shared, personal, defaults

```sql
ALTER TABLE collection_views
  ADD COLUMN owner_actor_id UUID,                      -- NULL: shared with everyone who can see the collection
  ADD COLUMN builtin TEXT,                             -- 'my_tasks', 'ready_for_agents', 'blocked',
                                                       -- 'recently_changed', 'triage', 'all'
  ADD COLUMN migrated_from_saved_view UUID,            -- the saved_views row it was copied from
  ADD CONSTRAINT collection_views_owner_fkey FOREIGN KEY (project_id, owner_actor_id)
      REFERENCES project_memberships(project_id, actor_id);
-- view_type CHECK widened to add 'timeline', 'chart'.
-- name unique per (collection, owner) instead of per collection.

CREATE TABLE collection_view_pins (
    workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    view_id UUID NOT NULL, actor_id UUID NOT NULL,
    pinned BOOLEAN NOT NULL, position INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (view_id, actor_id),
    FOREIGN KEY (project_id, view_id) REFERENCES collection_views(project_id, id) ON DELETE CASCADE
);

CREATE TABLE collection_view_defaults (
    workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    collection_id UUID NOT NULL,
    role_key TEXT,            -- a role (K1's role id once roles are rows); NULL for everyone
    actor_id UUID,            -- a person's own default; wins over the role's
    view_id UUID NOT NULL,
    UNIQUE NULLS NOT DISTINCT (collection_id, role_key, actor_id)
);
```

- A personal view is visible to its owner only; sharing it (moving `owner_actor_id` to NULL)
  needs `collections.build` (today's rule for shared views) or a new K1 permission
  `views.share` if the owner wants members who can't build to share. Recommended: add
  `views.share`, granted to members by default.
- Limits: 30 shared views per collection (today's `MAX_VIEWS`, `collections/views.rs:22`),
  100 personal views per person per collection (today's task limit, `work_items/views.rs:40`).
- Default view resolution: person's own default, else their role's, else the collection's
  first shared view by position.
- `view_list`, `view_write` (`mcp/views_data.rs:80`, `:269`) keep their names and arguments
  and read and write the new store; their task-shaped output (`query`, `layout`, `fields`)
  is rendered from the definition through the translator in reverse, so agents see no
  change. New optional arguments: `collection`, `definition`.

#### View kinds for every source

| Kind | Needs | Notes |
| --- | --- | --- |
| table | — | Today's collection table; inline edit through `FieldEditor` |
| list | — | Dense task-style rows, keyboard-first, "done tucked" for status fields. Becomes the Tasks default |
| board | a groupable field (status, single choice, person, checkbox, multiple choice; plus cycle, milestone for tasks) | Column paging per group, drag sets the value; manual order within column through `rank` for task-like sources and a new `collection_records.rank` for others |
| calendar | a date field | Today's |
| timeline | a start and an end date field (or one date) | Bars per record, drag to move or resize, grouping as lanes, dependency arrows for task-like sources. Replaces the task Timeline page's data path |
| gallery | — | Cover from a file field |
| chart | a chart spec (section 3.7) | The same renderer as a dashboard tile |

### 3.4 Web pieces: what T1 needs and what it owns

W1 owns the generic, data-agnostic pieces (props in, events out). T1 owns everything that
knows about fields and view definitions, and the two renderers W1 doesn't list.

| Piece | Owner | T1 needs from it |
| --- | --- | --- |
| `Table` with windowing, sticky header, column resize, keyboard cell focus (from kit `Table` + `useWindowedRows`) | W1 | Cell render prop, row selection, a footer slot for "add record" |
| `Board` with drag between and within columns, per-column paging (from `useBoardDrag` + `RecordBoard`) | W1 | Card render prop, `onMove(cardId, toColumn, beforeId)`, column header slot |
| `Calendar` month and week | W1 | Day cell render prop, drag to a day |
| `Gallery` | W1 | Card render prop |
| `ListRows` dense rows with keyboard (`ui/listKeys.ts`) | W1 | Row render prop, selection model, group headers |
| Side panel (detail) | W1 | Header/body slots |
| Form renderer for a list of field inputs | W1 | Field input render prop |
| `fields/*`: `FieldEditor`, `FieldValueView`, field manager, formula editor, rollup editor | **T1** | |
| `views/*`: `ViewHost` (loads definition + data with one `useViewQuery` hook), `ViewTabs`, `FilterEditor` (tree), `SortEditor`, `GroupEditor`, `FieldsMenu`, `ViewSettings`, `ViewDraftBar` (save, save as personal, reset) | **T1** | |
| `views/kinds/*`: table, list, board, calendar, gallery adapters that map a view's records and fields onto W1's pieces; `TimelineView` and `ChartView` renderers | **T1** | |
| Task extras as slots: done tucking, claim and run badge, launch, bulk bar actions | **T1** (task module fills the slots) | |

The views chunk is lazy (initial-load budget is full). The chart renderer is its own chunk
(hand-written SVG over the dataviz palette, no charting library in the first version).

### 3.5 Tasks depth

**Delegate.** Task core column `delegate_actor_id` (any account in the project, no kind
rule) beside `assignee_actor_id`. Assignee is who answers for the task; delegate is who is
doing it now. Launching a run for a task (A1) sets the delegate to the run's agent when the
task has none and the launcher doesn't say otherwise; that is a default, not a permission.
System field "Delegate", filter "delegated to me", event `task.delegated` delivered like
`task.assigned`. `task_update` takes `delegate`.

**Triage.** A new status category `triage` (widen `work_item_statuses.semantic_category`
CHECK, `0015_m4_work_items_foundation.sql:29-34`), and per project a "Triage" status that
projects can enable. Tasks enter it when created through a form, a bug report link, an
import, or by anyone whose triage rule says so; rules decide, never account kind.

```sql
CREATE TABLE triage_rules (
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    position INTEGER NOT NULL, name TEXT NOT NULL,
    filter JSONB NOT NULL,          -- a view filter tree over the incoming task
    action JSONB NOT NULL,          -- {"accept": {"status": id?, "set": {...}}} | {"route": {"project": id}}
                                    -- | {"assign": "<account|round_robin:<group>>"} | {"keep": true}
    enabled BOOLEAN NOT NULL DEFAULT true, version BIGINT NOT NULL DEFAULT 1,
    created_by_actor_id UUID NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
ALTER TABLE work_items ADD COLUMN snoozed_until TIMESTAMPTZ,
                       ADD COLUMN triaged_at TIMESTAMPTZ, ADD COLUMN triaged_by_actor_id UUID;
```

One operation `task_triage { taskIds, action: accept|decline|duplicate|snooze, status?, of?, until?, reason? }`:
accept moves to the project's default unstarted status (or the one given); decline moves to
a canceled status with the reason as a comment; duplicate adds the `duplicates` relation
(`work_item_relations`) and cancels; snooze sets `snoozed_until`, hiding it from the triage
view until then (a scheduled job clears it and notifies). The triage queue is the built-in
view `triage` (status category triage, snoozed hidden) on the Tasks collection, with keys
`1` accept, `2` decline, `3` duplicate, `h` snooze. The inbox (C2) can show the count; AI
suggestions (`ai/` AI4 triage) fill fields on the card.

**Cycles.**

```sql
CREATE TABLE cycle_settings (
    project_id UUID PRIMARY KEY, workspace_id UUID NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT false,
    length_days INTEGER NOT NULL DEFAULT 14 CHECK (length_days BETWEEN 7 AND 56),
    starts_on_weekday SMALLINT NOT NULL DEFAULT 1,
    upcoming INTEGER NOT NULL DEFAULT 2 CHECK (upcoming BETWEEN 1 AND 6),
    roll_over BOOLEAN NOT NULL DEFAULT true, version BIGINT NOT NULL DEFAULT 1
);
CREATE TABLE cycles (
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    number INTEGER NOT NULL, name TEXT NOT NULL DEFAULT '',
    starts_on DATE NOT NULL, ends_on DATE NOT NULL CHECK (ends_on > starts_on),
    completed_at TIMESTAMPTZ, version BIGINT NOT NULL DEFAULT 1,
    UNIQUE (project_id, number), UNIQUE (project_id, id)
);
CREATE TABLE cycle_snapshots (   -- one row per cycle per day, for burn-up charts
    cycle_id UUID NOT NULL, on_date DATE NOT NULL,
    scope INTEGER NOT NULL, started INTEGER NOT NULL, completed INTEGER NOT NULL,
    scope_points DOUBLE PRECISION, completed_points DOUBLE PRECISION,
    PRIMARY KEY (cycle_id, on_date)
);
ALTER TABLE work_items ADD COLUMN cycle_id UUID;  -- FK (project_id, cycle_id)
```

A daily job per project keeps `upcoming` cycles ahead, closes the ended one, rolls
unfinished tasks into the next when `roll_over`, and writes the snapshot. Cycle is a system
field (groupable, boardable). Operations: `cycle_list`, `cycle_write` (settings and names),
and `cycle` on `task_update`.

**Initiatives above projects.** An initiative is not a space (K4): spaces own members and
things; an initiative groups work across spaces. Initiatives are the records of a built-in
collection "Initiatives" in the workspace's own space (the hidden `workspace` project from
`migrations/20261113203000_workspace_space.sql`), so they get fields (owner, status with
categories, target date, health), views, comments and history for free. What they contain
is held by containment, which is a parent and not a link (R11): a milestone thing's
`things.parent_id` is the initiative record, and a space joins an initiative through its
milestones (a space-level "part of initiative" is the space's milestone in it, so no link
kind and no second containment path is needed). Progress is a rollup over the linked milestones' tasks (completed / total), computed
by a job (cross-space rollups are the one case the generic rollup doesn't cover in the first
version). Milestones stay per project (`project_milestones`, `0052:22`) and become a thing
kind (K3) so they can be linked. Visibility: someone sees an initiative's linked spaces and
progress counts only for spaces they can see (K1 filter).

**Keyboard-first.** One shortcut registry for every view (`views/keys.ts`, on W1's
`listKeys`): `j/k` move, `x` select, `shift+x` range, `s` status, `a` assignee, `d`
delegate, `p` priority, `l` labels, `c` create, `e` edit title, `/` filter, `g then t` go to
triage, `cmd+k` palette, `z` undo the last change (through the record's history). Changes
are optimistic with one save queue per record (today's `useRecordEdits.ts` and
`useTaskSave.ts` become one). Budget: a key action paints within one frame from cache; the
first page of a 10,000-task view in under 200 ms server time.

### 3.6 Forms

Today a form is a `bug_report_links` row with `collection_id` and inline `form` JSON
(`20261113063000_collection_forms.sql:7-14`), public only, no person or link questions
(`collection_forms.rs:58-60`), answers posted through the bug report route and written by
`record_write` as the link's maker.

Target:

```sql
CREATE TABLE forms (
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    collection_id UUID NOT NULL, name TEXT NOT NULL,
    audience TEXT NOT NULL CHECK (audience IN ('internal', 'public')),
    definition JSONB NOT NULL,  -- {intro, sections:[{title, questions:[{field, label?, help?,
                                --   required, showIf?: FilterTree over earlier answers}]}], thanks,
                                --   sets: {field: value} applied to every answer, e.g. status=Triage}
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'paused', 'closed')),
    created_by_actor_id UUID NOT NULL, version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
ALTER TABLE bug_report_links ADD COLUMN form_id UUID;  -- the public token of a public form
```

- **Internal forms** open inside the app for anyone who can add records; they may ask
  person, relation and file questions (pickers are access-checked); the submitter is the
  record's author.
- **Public forms** keep the token link and today's rules (no person or relation questions;
  author = the form's maker; a hidden `Submitted by` text field holds the email the person
  typed, if asked).
- **Conditional questions**: `showIf` is a filter tree evaluated on the client for display
  and again on the server; a hidden question's answer is dropped, and `required` applies
  only when shown.
- **What a submission creates**: one record in the collection (a task in Tasks once step 4
  lands, landing in Triage when the project uses it), with `sets` applied; an optional
  confirmation email for internal forms later.
- **Spam limits** (K5 shared limits): per token (today's `check_bug_report`,
  `bug_reports.rs:911`), plus per source address per form (20 per hour), per form per day
  (default 500, a setting), a honeypot field, a signed render token that must be at least 3
  seconds old, a body cap (64 KB, files off by default on public forms and ≤ 3 × 10 MB when
  on, quarantined as "not yet reviewed" until a member opens them), and the existing
  same-day duplicate check. A challenge provider is a seam (`FormChallenge` trait), none
  named. A form that trips the daily cap pauses itself and notifies its maker.

### 3.7 Dashboards (new module)

First version, small and real:

```sql
CREATE TABLE dashboards (
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, project_id UUID NOT NULL, -- the space
    name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
    owner_actor_id UUID,            -- NULL: shared in the space; else personal
    layout JSONB NOT NULL DEFAULT '{"columns": 12}',
    refresh TEXT NOT NULL DEFAULT 'live' CHECK (refresh IN ('live', 'hourly', 'daily', 'manual')),
    archived_at TIMESTAMPTZ, version BIGINT NOT NULL DEFAULT 1,
    created_by_actor_id UUID NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE dashboard_tiles (
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, project_id UUID NOT NULL,
    dashboard_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('chart', 'number', 'view', 'text')),
    title TEXT NOT NULL DEFAULT '',
    source JSONB NOT NULL,  -- {"view": id} | {"collection": id, "filter": {...}} | {"sheet": id, "range": "Tab!A1:D50", "header": true}
    spec JSONB NOT NULL,    -- chart or number spec
    position JSONB NOT NULL, -- {x, y, w, h} on a 12-column grid
    version BIGINT NOT NULL DEFAULT 1
);
```

Chart spec (the same object as a chart view's `layout.chart`):

```json
{"type": "bar" | "line" | "area" | "donut" | "number",
 "x": {"field": "<id>", "bucket": "day|week|month|quarter" },
 "y": {"aggregate": "count" | "sum" | "avg" | "min" | "max", "field": "<number field>"},
 "series": {"field": "<groupable field>"}, "stack": true, "limit": 12,
 "compare": "previous_period"}
```

- `views::aggregate` compiles it to one `GROUP BY` over the view's filter with K1's access
  filter, `statement_timeout` 2 s, ≤ 50 categories × 120 buckets; anything over shows
  "Other". Date buckets use `date_trunc` in the workspace time zone.
- **Access**: a tile always runs as the viewer. Sharing a dashboard shares its layout and
  queries, never the data: someone who can't see a collection sees "You can't see this
  data" on that tile. (Stored results shared beyond the viewer's access come only with
  reviewed outside queries, below.)
- **Refresh**: `live` tiles refetch on `collection.updated`/`record.*` events for their
  source (debounced 2 s); `hourly`/`daily` cache per (tile, viewer) in the browser only.
  No server-side cache in the first version.
- **Second step, sheets as a source**: a tile reads a rectangular range of a sheet with a
  header row; values come from the stored computed cells (C1 provides
  `sheets::read_range(sheet, range) -> Vec<Vec<Value>>`). Aggregation runs in Rust (ranges
  ≤ 10,000 cells).
- **Later step, outside data**: `data_connections` (a read-only connection string kept as an
  encrypted secret, never sent to the browser) and `saved_queries` (SQL text, parameters,
  `status draft|reviewed`, reviewer, reviewed version, result cap 10,000 rows, timeout
  10 s), run in jobs in a read-only transaction, results stored in `query_results` with
  their run time. Only reviewed queries run for viewers, shown with a "reviewed" badge; an
  edit returns the query to draft. Sharing a dashboard over reviewed queries shares the
  stored results (that is the point), so it needs the K1 permission `data.share`.
  Postgres first; other engines behind a `DataEngine` trait.

Operations: `dashboard_list`, `dashboard_get`, `dashboard_write` (with tiles), `tile_query`
(returns the series), later `data_connection_write`, `saved_query_write`,
`saved_query_review`, `saved_query_run`. Events: `dashboard.updated`. Module manifest
`modules/dashboards.json` (M1), layout part `area.dashboards`, lazy page.

### 3.8 Record templates, comments, automations

- **Record templates** (`record_templates`: `workspace_id, project_id, collection_id, name,
  title, values JSONB, body TEXT, extras JSONB, position, version`). `extras` holds task-core
  parts for task-like collections (criteria, subtasks as nested templates, labels). Today's
  `task_templates` become Tasks-collection templates by a copy migration (`extras` from the
  task template's criteria and subtasks); `template_list`/`task_from_template` keep working
  on the copies. Recurring: `recurring_tasks` generalises to `recurring_records` with a
  template id (copy, then the old table becomes read-only).
- **Record comments**: C1 makes comments on any thing a conversation. T1's part is the
  Comments section in the record panel and task-like detail, and `record_open` returning the
  conversation id. No comment storage in T1.
- **Automations on records**: triggers `record_created`, `record_changed`, new
  `record_matches` (enters a view filter, through `views::matching`), and `field_changed`
  with from/to; conditions are a filter tree (stored inline or as a view id), replacing the
  task-only `condition_params` path (`automations.rs:1454`) for Tasks too; actions are any
  K2 operation the rule's owner may run ("set field", "create record", "update records in
  view", "create task", "ask an agent" when A2 provides it), each re-checked as the owner
  when the rule acts (roadmap principle 9).

### 3.9 Operations and events (summary)

Every write is a K2 operation with an agent tool from day one.

| Operation | Kind | Permission (K1 catalogue) | Notes |
| --- | --- | --- | --- |
| `field_list`, `field_write` | describe, change | `collections.build` | exists in part for tasks (`work_item_fields.rs`) and through `collection_write` |
| `view_list`, `view_write`, `view_delete`, `view_pin`, `view_default_set` | describe, change | view own: member; shared: `views.share` | `view_list`/`view_write` keep their task shape |
| `view_query` | query | read the collection | one tool for any view or inline spec; `record_query` and `task_list` stay as shaped aliases |
| `record_write` | change | `collections.edit` (+ `collections.build` for `edit: builders` fields) | exists |
| `task_triage`, `cycle_list`, `cycle_write` | change, query | `tasks.edit`, `tasks.workflow` | |
| `form_list`, `form_write`, `form_submit` | | `collections.edit`; submit: public or member | |
| `dashboard_list`, `dashboard_get`, `dashboard_write`, `tile_query` | | `dashboards.view`, `dashboards.edit` | new module group |
| `record_template_write`, `record_from_template` | | `collections.edit` | |

Events (into K3's log): `record.created|updated|archived|restored` (with `changedFields`,
`cause: "write" | "computed" | "automation"`), `view.created|updated|deleted`,
`field.created|updated`, `form.submitted`, `task.triaged`, `task.delegated`,
`cycle.started|ended`, `dashboard.updated`. Tasks keep emitting `task.*` (health check C9
names the vocabulary problem; K3 owns the fix).

---

## 4. Migration path

The app is live on one server at a commit from 2026-10-31; every migration since is still
to apply in one go. So no migration below assumes #262's triggers or any earlier pending
migration has run against production data in its new shape; each backfill checks its own
preconditions and is idempotent.

1. **Stop the duplicate paths growing (T1.1).** A guard test and an `AGENTS.md` rule (P0
   adds the rule text) before anything else.
2. **Finish owner step 2 (#262, owner's thread)** and land **step 3 in three parts**:
   - *3a, flip ownership (T1.3).* Writes to title, field values and archive state go to
     `collection_records` through one function `task_records::write_shared` called by the
     four writers in section 2.5. Triggers reverse: an AFTER trigger on Tasks records copies
     those columns *back* to `work_items` as a read cache; `#262`'s guard is replaced by a
     guard on `work_items` refusing direct writes of those columns that don't come from the
     trigger. Readers are untouched: the cache keeps all 69 files correct. Backfill: none
     (step 2's already ran, and this migration re-runs step 2's check and repairs from
     `work_items` if any row differs, so it is safe on a database where step 2 ran in the
     same batch).
   - *3b, move readers (T1.4a-c).* A SQL view `task_rows` (record JOIN core, old column
     names). Readers move in three mechanical PRs grouped by area (web routes and list;
     MCP and briefing; everything else). A guard test lists files that still read the cached
     columns from `work_items` and may only shrink.
   - *3c, contract (T1.4d).* When the list is empty and one release has shipped with it
     empty: drop the cache trigger, drop `work_items.title`, `field_values`, `archived_at`,
     `archived_by_actor_id`; `search_vector` and the title trigram index move to records
     (task description stays in the core's own `tsvector`, combined at query time).
3. **One views engine (T1.5-T1.9).** Extract `views` from `collections/{views,query}.rs`
   with `RecordSource`; add the Tasks source with column accessors and indexes; extend
   `collection_views`. Copy every `saved_views` row into a Tasks-collection view
   (`migrated_from_saved_view`), copy pins. Old task list endpoints, `task_list` and
   automations then call the compiler while keeping their wire shapes. `saved_views` stays
   readable (old view ids in URLs and automation conditions resolve through
   `migrated_from_saved_view`) and stops being written; it is dropped in T1.40 after a
   release with no reads (a counter metric logs any).
4. **Owner steps 4 and 5 (T1.11, T1.12)** open Tasks for editing and give the task tools
   `fields`. The task-field definitions (`record_kind = 'task'`) are re-pointed to the Tasks
   collection (`record_kind = 'record'`, `collection_id` set) by a migration that is a pure
   UPDATE on ids, so values keyed by field id need no rewrite.
5. **Web**: the views engine first draws collections (it already has the shape), then the
   Tasks page (list, board, timeline) behind the layout registry, with the old components
   kept until T1.39. Old `/work-items?statusId=…` URLs are translated to a view draft on the
   client by the same table the server uses (a parity fixture JSON shared by both, like
   `page_docs/parity.rs`).
6. **Owner step 6 (T1.25)**: "acts like a task" adds task cores to another collection's
   records; keys come from `project_counters`.
7. **Delete the old paths (T1.39, then T1.40 for the tables)**: `work_items/list.rs` list engine, `mcp/tasks.rs` list
   engine, web `FilterBar`, `WorkItemList`, `WorkItemBoard`, `viewDraft`, `filterModel`,
   `saved_views` writes, `views::matching_tasks`.

---

## 5. Slices

Sizes: S about a day, M a few days, L about a week. "Beside" means it can run in parallel.

### Phase A: stop the copying, finish the storage move

**T1.1 Freeze guard for task-side views (S).** Add `tests/views_guard.rs`-style unit test in
`apps/server/src/views_guard.rs` (and `apps/web/src/views/guard.test.ts`) that scans
sources (pattern from `modules/mod.rs:417-440`): the definitions `fn push_after`,
`fn encode_cursor`, `PRIORITY_RANK`, `fn group_counts`, `enum Grouping`, `enum Sort` may
appear only in an allowlist with today's counts per file (`work_items/list.rs`,
`mcp/tasks.rs`, `collections/query.rs`); the web test allows `FilterBar.tsx`,
`WorkItemBoard.tsx`, `WorkItemList.tsx`, `filterModel.ts`, `boardModel.ts`, `viewDraft.ts`
with their current line counts plus 5 %. Hand P0 the `AGENTS.md` rule (section 6).
Depends on nothing. Acceptance: adding a new `fn push_after` in any other file, or a 6 %
larger `FilterBar.tsx`, fails the test with a message pointing at `docs/plans/collections.md`.

**T1.2 One field and view vocabulary (S).** Make `modules/contract.rs` `FieldType` and
`ViewType` re-exports of `fields::FieldType` and a new `views::ViewKind` (seven kinds), and
the web `modules/contract.ts` types re-exports of `fields/fieldModel.ts` and
`views/kinds.ts`. Replace the test at `fields.rs:1981` with "contract and fields are the
same type". No behaviour change. Beside T1.1. Acceptance: `cargo check`, the contract
tests, web typecheck; a grep test finds one `FieldType` definition per side.

**T1.3 Owner step 3a: the record owns title, values and archive state (M).** Depends on
#262 merged, and on P0.11 and P0.12 (every task move goes through the task command, so the
work_items guard refuses no legitimate writer). Files: new migration `<ts>_task_records_own_shared.sql` (reverse triggers,
work_items guard, repair check), `work_item_commands.rs` (`:566`, `:960`, `:1238`),
`work_item_move.rs:2600`, new `task_records.rs` with `write_shared`. Acceptance: new
`tests/integration/t1_task_record_owner.rs`: a `task_update` title change writes the record
first and the cached `work_items.title` matches; a direct `UPDATE work_items SET title` is
refused; an archive through `task_update` and through a project move both land on the
record; record and task versions bump once each; every existing task test passes unchanged
(the owner's promise 1).

**T1.4a-c Owner step 3b: readers move to `task_rows` (3 × M, parallel with each other).**
(a) `work_items.rs`, `work_items/*.rs`, `work_item_*.rs`; (b) `mcp/*.rs`, `home.rs`,
briefing, My work; (c) the rest (`search.rs`, `events.rs`, `webhooks.rs`, `automations.rs`,
`github.rs`, `things.rs`, AI, exports). Each PR changes `FROM work_items` to
`FROM task_rows` where the query reads a shared column. A unit test lists files that read
`work_items.title|field_values|archived_at` and may only shrink. Depends on T1.3.
Acceptance: the list test's allowlist shrinks to the files of the remaining PRs; all task
tests pass.

**T1.4d Owner step 3c: drop the cache (S).** Depends on T1.4a-c and one release.
Migration drops the reverse trigger and the four columns, moves the trigram and search
pieces. Acceptance: the allowlist is empty and removed; the 10,000-task test passes; an
`EXPLAIN` check (T1.7) still passes.

### Phase B: one views engine

**T1.5 The `views` module and `RecordSource` (M).** Move `collections/views.rs` and
`collections/query.rs` into `apps/server/src/views/` (`definition.rs`, `query.rs`,
`source.rs`), keep re-exports at the old paths, add the filter tree (flat array still read),
`within` and `categoryIs` ops, the `Accessor` on `FieldDefinition`, and `CollectionSource`.
Collections call through it. Beside T1.3. Acceptance: existing `awd90_collections`,
`awd110_collection_views` pass unchanged; new unit tests for tree parsing (depth, 20 leaf
limit, flat back-compat) and for `within` resolving in a fixed time zone.

**T1.6 Personal, shared, pinned and default views (M).** Migration extending
`collection_views` (owner, builtin, `migrated_from_saved_view`, kinds `timeline`, `chart`),
`collection_view_pins`, `collection_view_defaults`; HTTP routes and `view_write`
(`collection` argument) for collections; K1 permission `views.share` (added in K1.9). Depends on
T1.5, K1.9.
Acceptance: integration test: a personal view is invisible to another member over HTTP and
MCP (isolation case), sharing needs `views.share`, defaults resolve person > role >
first shared, pins per person.

**T1.7 The Tasks source with column accessors, and the 10,000-row guard (M).** `TasksSource`
(record JOIN core), every system field in section 3.3's table, `rank` sort, groupings for
milestone/type/parent/label; index migration on `collection_records`:
`(collection_id, number DESC) WHERE archived_at IS NULL`, title trigram, and a
`jsonb_path_ops` GIN on `field_values`; `push_clause` compiles single-value `is` on
choice/person stored fields to `field_values @> $1` so the GIN helps. Replaces #251's folded
`source()` and `group_sql`. Depends on T1.5 (and T1.3 for the join; before T1.3 it joins the
record that #262 keeps in step). Acceptance: new `t1_views_at_scale.rs` seeds 10,000 tasks
(the fixture from `w3_tasks_http.rs:1139`), pages every sort and grouping of section 3.3
through `views::page` without repeats, and runs `EXPLAIN (FORMAT JSON)` on the first-page
query of each, asserting no `Seq Scan` on `work_items` or `collection_records` and total
cost under a pinned ceiling; logs wall time.

**T1.8 Task saved views into the one store (M).** Migration copying every `saved_views` row
(built-ins included) into Tasks-collection views through the translator, and pins; idempotent
by `migrated_from_saved_view`; resumes by skipping copied rows. `mcp/views_data.rs`
`view_list`/`view_write` read and write the new store, rendering the old `query` shape.
Translator in `views/legacy.rs` with a fixture JSON shared with the web
(`apps/web/src/views/legacyParity.json`). Depends on T1.6, T1.7. Acceptance: a test runs
every parameter in `list.rs:57-97` and each built-in through translate and back; an agent's
`view_list` output is byte-identical before and after for the four built-ins.

**T1.9 Tasks lists, `task_list` and automations through the compiler (L, split if needed into
web list/board/groups and MCP+automations).** `work_items/list.rs` handlers parse their URL
query into a `ViewSpec` (via the translator) and call `views::page`, `group_counts`; board
columns use per-group paging; `mcp/tasks.rs` `task_list` does the same and keeps its result
shape and cursor prefix (old cursors are refused cleanly, which the tool already handles);
`automations.rs` conditions call `views::matching` with `TasksSource`. Depends on T1.8.
Acceptance: `w3_tasks_http.rs` and the MCP task list tests pass unchanged; automation tests
pass; a parity test asks the same filters through HTTP list and `task_list` and gets the
same ids.

### Phase C: users see it early

**T1.10 Easy field types (M).** `long_text`, `email`, `phone`, `status` with categories,
system fields `created_time`, `edited_time`, `created_by`, `edited_by`, `auto_id`;
multiple-choice and date-bucket grouping. Migration widens the type CHECK. Web editors and
views in `fields/`. Beside Phase B after T1.5. Acceptance: `awd89_custom_fields` extended:
each type's normalise, filter, sort, group; an agent writes an email by name through
`record_write`; a bad phone is refused with the reason.

**T1.11 Owner step 4: edit Tasks like any collection (M).** Re-point task field definitions
to the Tasks collection (pure UPDATE), lift #251's 409s for fields and values (core fields
still change through task operations: a record write naming Status calls the task command),
forms can target Tasks, a Fields section on the task page (drawn in W1.27c's `ThingDetail`).
Depends on T1.4d, T1.10, P0.11, P0.12.
Acceptance: integration test: add a field on Tasks in the collection screen, set it from a
table cell, see it in `task_open`; a board drag on a Tasks board view changes status through
`update_locked_work_item` and emits `changedFields: ["status","position"]`.

**T1.12 Owner step 5: task tools take `fields` and `collection` (M).** `task_create`,
`task_update`, `task_update_many` take `fields: {name: value}`; `task_open`/`project_open`
list fields; wrong names list the real ones. Depends on T1.11. Acceptance: agent parity
scenario (`awd91_agent_parity.rs`) covers each; schemas don't grow per project.

**T1.13 Web views engine on collections (M).** `apps/web/src/views/`: `ViewHost`,
`useViewQuery`, `ViewTabs`, `FilterEditor` (tree), `SortEditor`, `GroupEditor`,
`FieldsMenu`, personal/shared save, kinds table, board, calendar, gallery adapted onto W1's
pieces (or today's components until W1's land). The adapters turn a view config into the
pieces' generic props (`columns` with T1.13a's renderers and editors, `groups`/`groupOf`,
`dateOf`); `FilterEditor` is the one filter builder (R17; W1 has none), with the URL sync
adapter keeping `?view=` and old query links. Collections switch to it. Depends on T1.6,
W1.17; whichever of this slice and W1.18/W1.19 lands second swaps the collection components
onto the pieces. Acceptance: `collections.spec.ts` e2e passes; a vitest for the filter tree
editor round-trip and URL round trip; registry entries for the new parts.

**T1.13a The field registry (M; moved from W1.16 by R17).** `apps/web/src/fields/registry.ts`
keyed by T1's field types (renderer, editor, filter operators, sort, cell width); manifest
`FieldDecl` becomes a declared subset mapped to `FieldDefinition`; the second `FieldType` enum
in `modules/contract.ts` goes (with T1.2). Editors use W1's kit primitives (W1.17). Depends on
T1.2, W1.17. Acceptance: `fields/registry.test.tsx` renders and edits every field type at both
widths with axe; a field type missing from the registry fails the test (iterates T1's type
list).

**T1.14 Timeline and list kinds for every collection (M).** Server: two-field date range
paging; web `TimelineView` (bars, drag move and resize, lanes by group) and `ListView`
(dense rows, keyboard). Depends on T1.13. Acceptance: a collection with start/end dates
shows a timeline; dragging writes both dates in one record write; e2e on a phone-width
viewport shows the timeline as a list of date rows.

**T1.15 The Tasks page on the views engine: list (M).** The project's Tasks area opens the
Tasks collection's default view in the `list` kind with task slots (done tucking, claim and
run badge, launch, bulk bar, keyboard); the old URL params are read through the shared
translator; layout part stays `area.tasks`. This is also W1.27a and W1.27d (merged here): the
list runs on W1.18's pieces with task slots, and filters on `FilterEditor`. Depends on T1.9,
T1.13, T1.14, W1.18. Acceptance: W1's parity e2e (`e2e/tasks-parity.spec.ts`) passes before
and after; `WorkItemsPage.test.tsx` scenarios ported to the new page pass; old links open the
same filters.

**T1.16 The Tasks page on the views engine: board and timeline (M).** Board with columns by
status, priority, assignee, cycle, milestone, field; within-column order by `rank`;
`views/TimelinePage.tsx` becomes the timeline kind on Tasks with dependency arrows and
milestone markers. This is also W1.27b (merged here), on W1.19's board piece with workflow move
checks and the column composer. Depends on T1.15, W1.19. Acceptance: the parity e2e's board
cases pass; reorder within a column on a phone (roadmap V4's test), drag a milestone's tasks
on the timeline.

### Phase D: deeper fields

**T1.17 Formula language and same-record recalculation (M).** `fields/formula/` (parser,
checker, evaluator, limits), `computed_values` column, formula field type, inline
recalculation in `records::write` and `task_records::write_shared`, formula editor with
field autocomplete and live preview on the first record. Depends on T1.10. Acceptance:
unit tests per function and per limit (depth, steps, length, references); a rename of a
referenced field leaves the formula working; filter and sort by a formula result; an error
value shows and filters as empty.

**T1.18 Relations both ways and rollups, with recalculation jobs (M).** `relation` type with
`inverse`, `relations::write_pair`, `field_dependencies`, cycle refusal, `rollup` type,
`fields.recalculate` and `fields.recompute_field` jobs (resumable), volatile formulas
nightly. Relation values are also written as K3 links through `links::sync_field` in the same
transaction, so backlinks and search see them. Depends on T1.17, K3 links (stopgap: the pair
table alone until K3's link store lands). Acceptance: integration test: a rollup sum on Builds over linked
Testers updates within one job run after a Tester's value changes; a cycle is refused with
its path; a 10,000-record recompute resumes after the job is killed halfway; events carry
`cause: "computed"` and are coalesced to one per record.

**T1.19 Validation, required, defaults, unique and builder-only fields (S).** `FieldConfig`
additions, enforced in `records::write` and the task command; K1 check for `edit: builders`.
Beside T1.17. Acceptance: tests for each rule over HTTP and MCP; import gets warnings not
refusals for `required`.

**T1.20 File field (M).** Values are file ids; files attach to the record as a thing.
Depends on C1/K5's "files on any thing" (`files.rs:277-290` `Destination` gains a thing
target). Acceptance: upload into a cell; a public form with files off refuses a file; an
archived record's files stay readable by members.

**T1.21 Button field (S).** Config names a K2 operation and an argument template; the web
draws a button; pressing calls the operation as the presser. Depends on K2's operation
registry. Acceptance: a "Mark shipped" button sets a status and posts a message; a viewer
without the operation's permission sees it disabled with the reason.

### Phase E: tasks depth

**T1.22 Delegate (S).** Column, system field, `task_update.delegate`, `task.delegated`
event, filter "delegated to me", launch default (A1.13: a launch sets the delegate through the task command in the same transaction). Beside anything after T1.7.
Acceptance: an agent and a person can each be delegate and assignee in any combination; an
isolation case for the new field over HTTP and MCP.

**T1.23 Triage queue (M).** Category `triage`, `triage_rules`, `snoozed_until`,
`task_triage`, built-in Triage view, keys, snooze wake job, forms and bug reports landing in
Triage when enabled. Depends on T1.15 (web), T1.7 (server). Acceptance: a form answer lands
in Triage; rules run in position order and the first match acts; snooze hides and wakes at
the time; one key accepts with no mouse; `task_triage` duplicate adds the relation and
cancels.

**T1.24 Cycles (M).** Tables, daily job (create upcoming, close, roll over, snapshot), Cycle
system field, board by cycle, cycle burn-up chart (uses T1.31's chart). Depends on T1.7.
Acceptance: with a fake clock, a two-week cycle closes and rolls three unfinished tasks
into the next; snapshot counts match.

**T1.25 Owner step 6: acts like a task (L).** `collections.acts_like_task`, task cores for a
collection's records (status from the project workflow, keys from `project_counters`),
turning it off hides but keeps the core and is refused while a claim or run is live; task
tools take `collection`; such records open the task page. Depends on T1.12, T1.16.
Acceptance: a Bugs record gets key `AWD-131`, can be claimed and launched, shows on My work
and the Tasks board; turning the switch off and on loses nothing.

**T1.26 Initiatives (M).** Built-in "Initiatives" collection in the workspace space,
milestones contained by setting their `things.parent_id` to the initiative record (R11; no
`part_of` link), progress job, initiative page with a timeline of its milestones. Depends on
K3 things with `parent_id` (milestone as a thing kind whose parent may be an initiative record, K3 §3.2), K4 spaces (or today's workspace
space), T1.14.
Acceptance: an initiative over two projects shows progress counting only the projects the
viewer can see.

**T1.27 Keyboard-first pass (M).** One shortcut registry across view kinds, one optimistic
save queue (`useRecordEdits` and `useTaskSave` merged), undo of the last change from history,
command palette entries for every view action. Depends on T1.16, T1.41 (undo through K3's
history). Acceptance: roadmap V2's
"triage 20 tasks without the mouse" as an e2e; a vitest for the save queue's ordering.

### Phase F: forms, dashboards, records

**T1.28 Forms as their own definition, internal forms and conditions (M).** `forms` table,
copy of every form from `bug_report_links.form` (idempotent), sections, `showIf`, `sets`,
internal audience with person/relation/file questions. Depends on T1.11 for Tasks targets.
Acceptance: `awd110_collection_forms` passes on the copied forms; a hidden required question
doesn't block; a public form still refuses person questions.

**T1.29 Public form limits (S).** Per-address and per-form-day limits through K5's shared
limiter, honeypot, render token age, body caps, auto-pause and notice. Depends on K5 shared
limits (falls back to the in-process limiter until then). Acceptance: the 21st answer from
one address in an hour is refused; an answer within 3 s is refused; the daily cap pauses the
form and notifies the maker.

**T1.30 Record templates and recurring records (M).** `record_templates`, copy of
`task_templates` into Tasks templates, `recurring_records` generalising `recurring_tasks`;
existing tools keep working. Depends on T1.11. Acceptance: a weekly recurring record in a
non-task collection appears across a restart; `task_from_template` yields the same task as
before.

**T1.31 Aggregates and the chart view kind (M).** `views::aggregate`, chart spec checking,
`ChartView` (bar, line, area, donut, number) as a lazy chunk. Depends on T1.13. Acceptance:
count by status over 10,000 tasks under the plan ceiling; a sum over a formula field; an
over-limit category set folds into "Other".

**T1.32 Dashboards module (M).** Manifest, tables, operations and tools, page with a grid of
tiles (chart, number, view, text), sharing, live refresh; `modules/core/dashboards` with its
web `module.ts`. Depends on T1.31, M1.1 and M1.4 (stopgap: today's manifest list). Acceptance: isolation cases for every route and tool; a viewer
without access to a tile's collection sees the refusal on that tile only; editing a record
refreshes a live tile within 3 s.

**T1.33 Sheets as a dashboard source (S).** Tile source `sheet` with range and header row.
Depends on T1.32, C1.20 (`sheets::read_range`). Acceptance: a chart over a sheet range updates when a
cell changes.

**T1.34 Outside data: connections and saved queries (M).** Secrets-backed read-only
connections, saved queries with parameters, jobs with timeout and row cap, stored results.
Depends on T1.32, K5 (jobs, egress), K1.9 (`data.connect`). Acceptance: a query that writes is
refused by the read-only transaction; a query over the row cap stores the cap and says so.

**T1.35 Reviewed queries and shared results (M).** Review flow (draft, reviewed, edit returns
to draft), badge, sharing a dashboard's stored results under `data.share`. Depends on T1.34.
Acceptance: a viewer of a shared dashboard sees stored results without connection access and
can't run an unreviewed query.

**T1.36 Fields on pages and page lists through views (M).** `thing_field_values`,
`PagesSource`, field definitions scoped to a space for pages, the Pages list as a views-engine
list with fields. Depends on T1.13; coordinates with C1. Acceptance: a "Review date" field on
pages, filtered and sorted in the Pages list; an isolation case.

**T1.37 Record comments slot (S).** Comments section in the record panel and `record_open`'s
conversation id, on C1's `ThingComments` inside W1's discussion panel shell. Depends on C1.3,
W1.21. Acceptance: an agent comments on a record
with an @mention and the person is notified.

**T1.38 Automations on records (M).** `record_matches`, `field_changed`, filter-tree
conditions for records and tasks, K2 operations as actions. Depends on T1.9, K2.
Acceptance: "when a Playtest record's Severity becomes High, create a task" works end to end;
a rule whose owner lost edit rights stops and says why.

### Phase G: delete the old paths

**T1.39 Contract: remove the second task engine (M).** Delete the list engine parts of
`work_items/list.rs` (keep handlers as thin translators), `mcp/tasks.rs` engine
(`TASK_FROM`, its `PRIORITY_RANK`, cursor code), `views::matching_tasks`; stop writing
`saved_views`; web `FilterBar.tsx`, `WorkItemList.tsx`, `WorkItemBoard.tsx`, `boardModel.ts`,
`filterModel.ts`, `viewDraft.ts`; shrink the T1.1 allowlists to empty. Depends on T1.9,
T1.15, T1.16, and one release. Acceptance: W1's parity e2e (`e2e/tasks-parity.spec.ts`) and
the agent parity scenario (`awd91_agent_parity.rs`) pass on the new paths before anything is
deleted; the guards' allowlists are gone; all task tests pass.

**T1.41 Records and tasks can be proposed, with history (M).** `record_update_many` and
`task_update` are marked `PROPOSABLE` (K2), so a caller whose grant is "suggest" (K1's
`access::write_mode` returning `Propose`) gets a C1 proposal instead of a refusal;
`records::write` and the task command take the author and `accepted_by` so an accepted
proposal is written as its author; a batch of cell edits on a record is one change set (C1.9);
K3's `history::History` is implemented for `record` and `task` (list, show, restore of field
values), which T1.27's undo uses. Depends on T1.11, C1.7, C1.9, K3.19. Acceptance: an agent with
the "Member, suggests" role edits a record and the owner accepts it from the request card; the
history shows the author and who accepted; restore of a field value over HTTP and MCP; an
isolation case.

**T1.40 Contract: drop `saved_views` (S).** After a release with zero reads logged. Depends
on T1.39. Acceptance: migration drops the two tables; old view links resolve through
`collection_views.migrated_from_saved_view`.

---

## 6. Guard rails

1. **No view or filter code outside the shared system** (T1.1, made strict by T1.39). Server:
   a unit test scanning `src/**/*.rs` for the definitions listed in T1.1 outside
   `src/views/` and `src/fields/`; web: a vitest scanning `src/**` for files matching
   `*Board.tsx`, `*FilterBar*`, `*filterModel*`, `*viewDraft*`, and for `useBoardDrag(`
   outside `src/views/` and W1's piece folder. Allowlists carry today's files and shrink only.
2. **The rule in `AGENTS.md`** (P0 adds it), proposed text: "Until T1.39 lands, add no
   view, filter, sort, grouping, board or saved-view feature on the task side
   (`work_items/list.rs`, `mcp/tasks.rs` list, `work-items/FilterBar|WorkItemList|WorkItemBoard`).
   Build it in `views/` (server) and `src/views/` (web); tasks get it through the Tasks
   source. A guard test enforces this."
3. **One field type list.** The contract enums are re-exports (T1.2); a test fails if a
   second `enum FieldType` or `type FieldType =` appears.
4. **Translator parity.** `views/legacyParity.json` is read by a Rust test and a vitest, so
   old task URLs and stored views mean the same on both sides.
5. **10,000-row plans.** T1.7's `EXPLAIN` guard runs every system-field sort and grouping and
   fails on a sequential scan or a cost above the pinned ceiling. Any new system field must be
   added to the guard's list (a test checks the list covers `fields::system::ALL`).
6. **Task tests unchanged through steps 3-6.** Each of T1.3, T1.4a-d and T1.11 states in its
   PR that no existing task test was edited except for renamed columns.
7. **Every new route and tool has an isolation case** (`workspace_isolation.rs`,
   `write_isolation.rs`) and an agent tool (the existing `agent_parity` guard).
8. **Formula safety.** Fuzz-style unit tests on the parser (random input never panics, always
   within budget), run in the ordinary test suite with a fixed seed.
9. **Triggers on hot tables (R16).** T1 adds these and removes them as listed; none writes a
   table another plan's trigger also writes:

   | Table | Trigger | Added | Removed |
   | --- | --- | --- | --- |
   | `collection_records` | AFTER trigger copying title, values and archive state of Tasks records to the `work_items` cache | T1.3 | T1.4d |
   | `work_items` | guard refusing direct writes to `title`, `field_values`, `archived_at` (#262's forward sync trigger is replaced by it) | T1.3 | T1.4d (columns dropped) |
   | `collection_records`, `work_items` | none further; recalculation (T1.18) and cycles (T1.24) are jobs, not triggers | | |

   K3's thing sync on these tables (K3's plan) writes `things`, which T1's triggers never write.
10. **Tasks may be off (R22).** Every workspace has the views engine, but not every workspace has
   Tasks: nothing in T1 assumes the Tasks module is on. My work, Triage, Cycles, Initiatives
   and the task slots render only when Tasks is on; field and view screens are reached only
   through modules that have them (Collections, Tasks, Pages, Dashboards), so a chat-only
   workspace ("Friends") shows no fields, views or view words. W1.39's shape walk covers it.

---

## 7. Risks and open questions

| Risk | How we'd notice | Fallback |
| --- | --- | --- |
| Step 3 breaks a reader that the allowlist missed (raw SQL in a string built at runtime) | Task tests, the parity scenario, e2e; the cache keeps old readers right until 3c | 3c waits a release; the cache trigger can be put back by migration |
| Lists get slower once tasks read through record JOIN core | T1.7's EXPLAIN guard and the scale test's logged time | Keep core-column accessors (no JSONB for core fields); add covering indexes; the join is on primary keys |
| GIN on `field_values` bloats writes on large collections | Write latency metrics on `record_write` | Make it per-collection opt-in (partial index by collection) or drop it; filters still work |
| Formula recalculation storms (a popular record many rollups read) | Job queue depth metric, coalescing counters | `unique` keys coalesce; per-workspace job concurrency cap; rollup fan-out cap of 1,000 |
| Two-way relation arrays drift from the link store | Nightly consistency job reports | Repair from K3's link store; before K3 lands, the pair write is one transaction |
| Saved-view translation loses meaning for an odd stored query | T1.8's round-trip test over every parameter; a migration report counting untranslatable rows | Untranslatable rows keep working through `saved_views` until fixed by hand; they are listed in the migration's log |
| Dashboards over outside data leak access | Review of T1.34-35, isolation tests | Ship only the internal-data dashboard (T1.32) until reviewed queries are reviewed |
| Other branches add task view features meanwhile | T1.1 guard fails their CI | The rule in `AGENTS.md` |

Questions for the owner (with my recommendation):

1. **Labels**: keep them as the project's Labels field (one per project, any collection in the
   project may show it), or make them a workspace-wide field? *Recommend project-wide, opt-in
   per collection; workspace-wide later if asked.*
2. **Formula syntax**: spreadsheet-style names with `{Field}` references, as designed?
   *Recommend yes.*
3. **Per-field permissions**: only "builders can edit this field", no hidden fields? *Recommend
   yes; private data goes in a narrower collection.*
4. **Workflows per collection**: step 6 shares the project's workflow (owner's plan says so for
   the first version). Keep that until someone asks. *Recommend yes.*
5. **Outside data in the free hosted plan**: allowed with the user's own database? *Recommend
   yes, with the job and row caps; it costs us little.*
6. **The task key number stays on the task core** (section 3.1) rather than the record owning
   it, so task-like records can have both `#12` and `AWD-131`. *Recommend yes.*

---

## 8. Touch points

**Needs from:**

- **K1**: `access::sql_filter(reader, kind = record)` for the compiler; permissions
  `views.share`, `dashboards.view`, `dashboards.edit`, `data.connect`, `data.share`, and the
  check for builder-only fields; replacements for the account-kind rules in section 2.6
  (comment moderation becomes the `channels.moderate` grant on the thing's comments
  conversation, as C1.4 maps it; watcher and assignee pickers list
  every account kind that can act).
- **K2**: operation definitions and generated tools/routes for every operation in 3.9; the
  operation registry for button fields and automation actions; `<thing>_<verb>` names (kept).
- **K3**: thing kinds `record`, `view`, `dashboard`, `form`, `cycle`, `milestone`,
  `initiative`; `things.parent_id` for containment (initiatives, sub-records; R11); the link
  store for two-way relations through `links::sync_field(tx, from, field_id, kind, targets)`;
  the event log for the events in 3.9 and for live refresh; the `history::History` trait
  (K3.19), which T1.41 implements for records and tasks (T1 needs "undo last change" from it).
- **K4**: space ids (collections, views and dashboards live in a space); the workspace space
  for Initiatives; personal spaces may hold personal dashboards.
- **K5**: shared rate limits for public forms; jobs and leases for recalculation and cycles;
  file storage for file fields and form files.
- **A1**: launching sets the delegate by default; runs show on task-like records.
- **A2**: delivering `task.triaged`, `task.delegated` and record events to agents that follow
  a collection; "ask an agent" as an automation action.
- **M1**: registering the Dashboards module and the views/fields parts (M1.1, M1.4);
  collections as built modules; `object_on_sql` added into `RecordSource::scope` (M1.6).
- **C1**: comments on any thing (`ThingComments`, C1.3), files on any thing,
  `sheets::read_range(sheet, range) -> Vec<Vec<Value>>` (C1.20), the proposals record and
  change sets (C1.7, C1.9) for T1.41, record body documents later.
- **C2**: the triage count and snooze wake-ups in the inbox.
- **W1**: the generic pieces (W1.17 to W1.22) taking `columns`, `groupOf` and `dateOf`
  (W1 3.7); T1's adapters build those from a view config; W1's parity e2e
  (`e2e/tasks-parity.spec.ts`) for T1.15, T1.16 and T1.39.
- **P0**: the `AGENTS.md` rule; the five command-layer bypasses (health check C1) fixed before
  T1.11 so Tasks board drags emit `changedFields`.

**Provides to:**

- **All modules**: one field layer, one views compiler (`RecordSource`), one view store, one
  chart spec.
- **W1**: the view-definition-aware wrappers (adapters), the field registry (T1.13a), the
  filter editor (T1.13) and the timeline and chart renderers (T1.14, T1.31), per R17.
- **H1**: export and import of collections, fields, views, forms and dashboards as data
  (each table here carries `workspace_id`); plan limits hooks for formula and dashboard job
  usage.
- **M1**: built modules are collections plus views, forms, templates and dashboards, all
  data.

---

## 9. Changes to the end state

1. **Views list.** END-STATE section 3, "Fields and views", lists "table, board, list,
   calendar, timeline, gallery". Proposed: "table, board, list, calendar, timeline, gallery
   and chart", since Dashboards (section 3) and the Collections bar (section 8) both need
   charts and they should be the same thing.
2. **Tasks module row** (section 3 table): "Tasks, boards, timelines, milestones, claims,
   workflows" → add "triage, cycles, delegates and initiatives".
3. **Delegate in the vocabulary.** Add under Agents or Things: "**Delegate**: the account
   doing a task now, beside the assignee who answers for it. Any account can be either."
4. **Initiative.** Add: "An **initiative** groups work across spaces. It is a record of the
   workspace's Initiatives collection, not a space."
5. **Task identity** (clarifies the owner's tasks-as-records plan): "A task's key
   (`PROJECT-N`) belongs to its task core; the record carries the collection's own number,
   which for the built-in Tasks collection is the same."

---

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| T1.1 | Freeze guard for task-side views | S | — | T1.2, T1.3 |
| T1.2 | One field and view vocabulary | S | — | T1.1, T1.3 |
| T1.3 | Step 3a: record owns title, values, archive | M | #262 merged, P0.11, P0.12 | T1.1, T1.2, T1.5 |
| T1.4a | Step 3b: task readers move (task routes) | M | T1.3 | T1.4b, T1.4c, Phase B |
| T1.4b | Step 3b: task readers move (MCP, briefing, My work) | M | T1.3 | T1.4a, T1.4c |
| T1.4c | Step 3b: task readers move (the rest) | M | T1.3 | T1.4a, T1.4b |
| T1.4d | Step 3c: drop the cached columns | S | T1.4a-c, one release | Phase C |
| T1.5 | `views` module and `RecordSource` | M | — | T1.3, T1.4a-c |
| T1.6 | Personal, shared, pinned, default views | M | T1.5, K1.9 (`views.share`) | T1.7, T1.10 |
| T1.7 | Tasks source, accessors, indexes, 10k guard | M | T1.5 | T1.6, T1.10 |
| T1.8 | Task saved views into the one store | M | T1.6, T1.7 | T1.10, T1.13 |
| T1.9 | Task lists, `task_list`, automations on the compiler | L | T1.8 | T1.13, T1.14 |
| T1.10 | Easy field types and system fields | M | T1.5 | T1.6-T1.9 |
| T1.11 | Step 4: edit Tasks like any collection | M | T1.4d, T1.10, P0.11, P0.12 | T1.13 |
| T1.12 | Step 5: `fields` and `collection` on task tools | M | T1.11, K2 | T1.14 |
| T1.13 | Web views engine on collections, adapters, `FilterEditor` | M | T1.6, W1.17 (second of it and W1.18/19 swaps) | T1.9, T1.11, W1.18, W1.19 |
| T1.13a | The field registry (from W1.16) | M | T1.2, W1.17 | T1.13 |
| T1.14 | Timeline and list kinds for every collection | M | T1.13 | T1.12 |
| T1.15 | Tasks page on views: list (with W1.27a, W1.27d) | M | T1.9, T1.13, T1.14, W1.18 | T1.17 |
| T1.16 | Tasks page on views: board and timeline (with W1.27b) | M | T1.15, W1.19 | T1.17, T1.18 |
| T1.17 | Formula language, same-record recalculation | M | T1.10 | T1.15, T1.19 |
| T1.18 | Two-way relations, rollups, recalculation jobs | M | T1.17, K3 links (stopgap: pair table only) | T1.16 |
| T1.19 | Validation, required, defaults, builder-only | S | T1.10, K1 | T1.17 |
| T1.20 | File field | M | T1.10, C1/K5 files on things | T1.17 |
| T1.21 | Button field | S | T1.10, K2 registry | T1.17 |
| T1.22 | Delegate | S | T1.7, A1.13 | Phase C |
| T1.23 | Triage queue | M | T1.7, T1.15 | T1.24 |
| T1.24 | Cycles | M | T1.7, T1.31 for burn-up | T1.23 |
| T1.25 | Step 6: acts like a task | L | T1.12, T1.16 | T1.23, T1.24 |
| T1.26 | Initiatives (containment by `parent_id`) | M | T1.14, K3 things `parent_id`, K4 | T1.25 |
| T1.27 | Keyboard-first pass | M | T1.16, T1.41 | T1.23 |
| T1.28 | Forms: definitions, internal, conditions | M | T1.11 | T1.29 |
| T1.29 | Public form limits | S | K5 shared limits | T1.28 |
| T1.30 | Record templates and recurring records | M | T1.11 | T1.28 |
| T1.31 | Aggregates and chart view kind | M | T1.13 | T1.17 |
| T1.32 | Dashboards module | M | T1.31, M1.1, M1.4 (stopgap: manifest list) | T1.33 |
| T1.33 | Sheets as a dashboard source | S | T1.32, C1.20 | T1.34 |
| T1.34 | Outside data: connections, saved queries | M | T1.32, K1.9, K5 | T1.33 |
| T1.35 | Reviewed queries, shared results | M | T1.34 | — |
| T1.36 | Fields on pages; page lists through views | M | T1.13, C1 | T1.31 |
| T1.37 | Record comments slot | S | C1.3, W1.21 | any |
| T1.38 | Automations on records | M | T1.9, K2 | T1.31 |
| T1.39 | Contract: remove the second task engine (deletes W1.27a/b/d's old screens) | M | T1.9, T1.15, T1.16, parity e2e green, one release | — |
| T1.40 | Contract: drop `saved_views` | S | T1.39, one release | — |
| T1.41 | Records and tasks proposable, with K3 history | M | T1.11, C1.7, C1.9, K3.19 | T1.17-T1.21 |

## Review changes

- Initiatives (3.5, T1.26, touch points) contain milestones by `things.parent_id`, not `part_of` links (R11).
- Comment moderation maps to `channels.moderate` on the comments conversation, not a new `comments.moderate` grant (K1's catalogue, C1.4).
- New T1.13a, the field registry, moved from W1.16; T1.13 owns `FilterEditor` and the adapters that build W1's generic `columns`, `groupOf`, `dateOf` (R17). T1.13 depends on W1.17 only; whichever of T1.13 and W1.18/W1.19 lands second does the swap (W1/T1 interleave).
- T1.15 and T1.16 absorb W1.27a, W1.27b and W1.27d (true duplicates) and depend on W1.18 and W1.19; T1.39 deletes the old screens only once W1's parity e2e and the agent parity scenario pass on the new paths (retired path names its proof).
- New T1.41: records and tasks proposable (K2 `PROPOSABLE`, K1 `write_mode`), author and `accepted_by` written, change sets (C1.9), K3 `history::History` for record and task; T1.27's undo depends on it (R14, every write an operation, history on records was homeless).
- `RecordSource::scope` takes M1's `object_on_sql` (M1.6).
- T1.18 writes relation values through K3's `links::sync_field`, with the pair table as stopgap.
- T1.3 and T1.11 depend on P0.11 and P0.12 (the bypasses fixed before the guard turns on); owner steps unchanged: T1.3-T1.4d step 3, T1.11 step 4, T1.12 step 5, T1.25 step 6.
- T1.32 depends on M1.1 and M1.4 with today's manifest list as stopgap; T1.33 on C1.20; T1.37 on C1.3 and W1.21.
- Guard rail 9: R16 trigger table for `collection_records` and `work_items`.
- Guard rail 10: nothing assumes Tasks is on; no fields or views in a chat-only workspace (R22).
- Touch points updated for the above (K3 parent and history, M1, C1, W1).

### Second pass

- `views.share`, `dashboards.*` and `data.*` come from K1.9: T1.6 and T1.34 depend on it (kernel review).
- T1.26 cites K3 §3.2 for a milestone's parent being an initiative record.
- T1.22's "A1 hook" is A1.13 (agents and hosting review).
