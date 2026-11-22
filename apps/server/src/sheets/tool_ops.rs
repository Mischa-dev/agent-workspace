//! Sheets as operations (K2.11): listing, reading, making and changing a
//! sheet. Each runs the command its hand-written tool ran (`ops::wrapped`)
//! as the caller the dispatcher checked. `sheet_read` became `sheet_get`
//! and `sheet_write` became `sheet_create` and `sheet_update`; the old names
//! are retired (`ops::retired`).

use crate::{
    mcp::sheet_tools,
    ops::{
        Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{keyed, names_existing, run_tool, tool_op},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<SheetList>()
        .add::<SheetGet>()
        .add::<SheetCreate>()
        .add::<SheetUpdate>();
}

/// A sheet is named by its name or id inside its space, so each of these
/// acts on the space.
fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(uuid::Uuid::nil))
}

macro_rules! in_space {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    space(&self.project_id)
                }
            }
        )*
    };
}

in_space!(
    sheet_tools::SheetListArguments,
    sheet_tools::SheetReadArguments,
    sheet_tools::SheetWriteArguments,
);

tool_op!(SheetList {
    name: "sheet_list",
    module: "sheets",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List sheets",
    description: "List a space's sheets (spreadsheets), last changed first, with their tabs, \
        how many cells they fill and their revision. Read one with sheet_get.",
    emits: [],
    input: sheet_tools::SheetListArguments,
    output: sheet_tools::SheetListValue,
    run: |call, input| { run_tool!(call, sheet_tools::sheet_list, input) }
});

tool_op!(SheetGet {
    name: "sheet_get",
    module: "sheets",
    kind: Describe,
    permission: None,
    idempotency: None,
    title: "Read sheet",
    description: "Read a sheet's tab: its filled cells in a range in A1 form (\"A1:D20\", \
        \"B:D\", \"2:10\"), each with its computed value and formula; or, with asRows, its rows \
        as {header: value} objects using row 1 as the header, filtered with where and paged. \
        Returns the revision to pass back as ifRevision. To show a range live in a page, put \
        `[name](/projects/{projectId}/sheets/{sheetId}?tab={tabId}&range=A1:D9 \"embed\")` on \
        a line of its own.",
    emits: [],
    input: sheet_tools::SheetReadArguments,
    output: sheet_tools::SheetReadValue,
    run: |call, input| { run_tool!(call, sheet_tools::sheet_read, input) }
});

tool_op!(SheetCreate {
    name: "sheet_create",
    module: "sheets",
    kind: Create,
    permission: Some(crate::ops::Perm::SheetsEdit),
    idempotency: Command,
    audited: true,
    guard_kind: "sheet",
    title: "Create sheet",
    description: "Make a sheet: a name, and optionally its tabs, cells (as you'd type them, \
        formulas starting with \"=\"), rows by header, formats, conditional formats and \
        validations; or fill it from an .xlsx or CSV file (import). Formulas are worked out on \
        the server and the computed values come back.",
    emits: ["sheet.updated"],
    input: sheet_tools::SheetWriteArguments,
    output: sheet_tools::SheetWriteValue,
    run: |call, input| {
        names_existing(input.sheet.is_some(), true, "sheet")?;
        keyed!(call, sheet_tools::sheet_write, input)
    }
});

tool_op!(SheetUpdate {
    name: "sheet_update",
    module: "sheets",
    kind: Change,
    // sheets.edit, or sheets.suggest for cells only, which the command
    // keeps as a proposal: it checks which.
    permission: None,
    idempotency: Command,
    audited: true,
    guard_kind: "sheet",
    title: "Update sheet",
    description: "Change a sheet (by name or ID): rename, describe, archive or restore it; add, \
        rename, move or remove tabs, insert or delete rows and columns, freeze rows and set \
        widths (tabs); set cells (cells); add or update rows by a key column (rows); sort \
        blocks (sorts); number formats and styles (formats); rules that colour cells \
        (conditionalFormats); limits on what cells hold (validations). All or nothing; a cell \
        given ifRevision is refused if someone changed it since.",
    emits: ["sheet.updated"],
    input: sheet_tools::SheetWriteArguments,
    output: sheet_tools::SheetWriteValue,
    run: |call, input| {
        names_existing(input.sheet.is_some(), false, "sheet")?;
        keyed!(call, sheet_tools::sheet_write, input)
    }
});
