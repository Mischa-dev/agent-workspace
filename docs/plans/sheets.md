# Sheets (AWD-54)

Spreadsheets in a project, computed on the server, that people and agents
edit together. Step 7 of the build-out plan, after collections (AWD-90). The
October 21 write-up on docs, sheets and Google set the shape: sheets are built
like whiteboards, the formula engine is IronCalc (Rust, MIT or Apache-2.0)
behind one trait, edits are per cell, and agents get `sheet_*` tools.

## What a sheet is

- A **sheet** is a workbook: a name, a description, one or more **tabs**, and
  a **revision** that goes up by one with every change.
- A **cell** keeps what was typed (`input`: `12`, `Due`, `TRUE`,
  `=SUM(B2:B9)`, `'00123`) and what it computes to (`kind` and `value`:
  number, text, boolean, error such as `#DIV/0!`, or empty). Only cells that
  hold something are stored.
- Limits: 100 active sheets a project (500 kept), 20 tabs, 10,000 rows, 200
  columns (A to GR), 50,000 filled cells a sheet, 10,000 characters a cell,
  2,000 changes a write.

## How a change works

`crate::sheets::apply` is the one way cells and tabs change. It loads the
sheet's cells, hands them and the requested operations to the engine, and
writes back only what differs: inputs that changed get the new revision and
the writer's name; values that changed because of them are updated in place.

- **Per-cell conflicts.** A cell edit may say the revision it was read at
  (`ifRevision`). It's refused only if that cell, or its tab's rows and
  columns, changed since. Edits to different cells never clash, so a person
  and an agent can fill one sheet at once.
- **Structure follows the spreadsheet.** Inserting or deleting rows and
  columns and renaming a tab go through the engine, so formulas that point at
  them are rewritten the way any spreadsheet does it. A removed tab leaves
  `#REF!` behind.
- **Rows by key.** A row change finds the row whose value under a header
  (row 1) matches, or adds one under the last row; a header that isn't there
  yet joins row 1. This is how agents keep a table up to date without
  counting cells.
- **All or nothing.** Any refused change refuses the write.
- **History.** Every revision is a `sheet_revisions` row: who, through which
  connected app, and what kind of change.

## The engine

`crate::sheets::engine::FormulaEngine` takes a workbook's inputs and a list
of operations and returns every cell's input and value. `IronCalc` is the
implementation, in English on UTC. It runs on a blocking thread. Number
formats from input (`10%`, `$5`) aren't kept yet; values are.

Keeping the engine behind one trait is what leaves the Google link (AWD-55)
possible: a linked sheet can compute in Google instead.

## Who can do what

Everyone in a project reads its sheets. `sheets.edit` (on for owners, admins
and members, off for viewers) creates and changes them. Sheets is a module of
its own that a workspace or role can switch off.

## Slices

1. **Server and tools** (#231, merged). Migration `20261112193000_sheets.sql`,
   the engine, `sheet_list`, `sheet_read` (a range, or rows as objects by
   header with a filter) and `sheet_write` (create, rename, archive, tabs,
   cells, rows).
2. **The web app** (this PR). HTTP routes under
   `/api/v1/projects/{project_id}/sheets`, a Sheets area and a grid that edits
   in place: formulas, copy and paste, resizing, a frozen header, live updates
   from other people and agents (`sheet.updated` events), and cards on a
   phone.
3. **Pages and files** (this PR). A sheet's tab or range
   (`/projects/{p}/sheets/{s}?tab=…&range=A1:D9`) embedded live in a page
   through the embed block; `.xlsx` (every tab, formulas kept) and CSV (one
   tab's values) export at `…/sheets/{s}/export`; import of either makes a
   new sheet at `…/sheets/import` or through `sheet_write`'s `import` (a
   file ID or CSV text). Google Sheets goes through `.xlsx`. Files are up to
   5 MB, and an `.xlsx` that unpacks past 100 MB is refused.
