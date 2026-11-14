//! The freeze on task-side view code (T1.1, guard rail 1 of
//! `docs/plans/master/plans/T1-fields-views-tasks.md`).
//!
//! Tasks and collections grew two list engines side by side: a filter, a
//! sort, a grouping or a board built on one side was then built again on
//! the other. From here on there is one engine, `crate::views` (with the
//! field layer in `crate::fields`), and tasks get what it can do through
//! the Tasks source. So the pieces an engine is made of, a keyset cursor
//! (`fn encode_cursor`, `fn push_after`), group counts (`fn group_counts`),
//! a priority order (`PRIORITY_RANK`) and the sort and grouping enums
//! (`enum Sort`, `enum Grouping`), may be defined only in `src/views/` and
//! `src/fields/`, and in the files listed below, each at most as many times
//! as today. The list only shrinks: when T1.39 deletes the second task
//! engine it is empty.

use std::collections::BTreeMap;

use crate::test_support::source::{rust_files, server_src, strip_tests};

/// What an engine is made of, as its definition is written.
const FROZEN: &[&str] = &[
    "fn push_after",
    "fn encode_cursor",
    "const PRIORITY_RANK",
    "fn group_counts",
    "enum Grouping",
    "enum Sort ",
    "enum Sort{",
];

/// Where the shared engine lives: anything goes there.
const ENGINE: &[&str] = &["src/views/", "src/fields/", "src/fields.rs"];

/// Today's other definitions, file by file. Lower a number when a
/// definition goes; never raise one or add a file.
const ALLOWED: &[(&str, usize)] = &[
    // Cursors over other lists (runs, external events), not views.
    ("src/external_events.rs", 1),
    ("src/run_views.rs", 1),
    // The task list engine and its MCP twin, until T1.9 and T1.39.
    ("src/mcp/tasks.rs", 2),
    ("src/work_items/list.rs", 6),
    ("src/work_items/views.rs", 1),
];

const HOW: &str = "Build view, filter, sort, grouping and board features in the one views \
     engine (apps/server/src/views/, docs/plans/collections.md and T1 in \
     docs/plans/master/plans/T1-fields-views-tasks.md); tasks get them through the Tasks source.";

/// Definitions in the file before its test tail: tests may define what
/// they like.
pub(crate) fn definitions(text: &str) -> usize {
    strip_tests(text)
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("//")
                && FROZEN
                    .iter()
                    .any(|needle| line.contains(needle) || line.ends_with(needle.trim_end()))
        })
        .count()
}

fn counts() -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for (path, text) in rust_files(&server_src()) {
        let relative = format!("src/{path}");
        if relative == "src/views_guard.rs"
            || ENGINE.iter().any(|prefix| relative.starts_with(prefix))
        {
            continue;
        }
        let count = definitions(&text);
        if count > 0 {
            found.insert(relative, count);
        }
    }
    found
}

#[test]
fn view_engine_pieces_are_defined_only_in_the_views_engine() {
    let allowed = ALLOWED.iter().copied().collect::<BTreeMap<_, _>>();
    let found = counts();
    let over = found
        .iter()
        .filter(|(file, count)| **count > allowed.get(file.as_str()).copied().unwrap_or(0))
        .map(|(file, count)| {
            format!(
                "{file}: {count} (allowed {})",
                allowed.get(file.as_str()).copied().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>();
    assert!(over.is_empty(), "{}\n{HOW}", over.join("\n"));
    let under = allowed
        .iter()
        .filter(|(file, count)| found.get(**file).copied().unwrap_or(0) < **count)
        .map(|(file, count)| {
            format!(
                "{file}: {} (allowed {count})",
                found.get(*file).copied().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>();
    assert!(
        under.is_empty(),
        "fewer definitions than allowed: lower the numbers in ALLOWED in src/views_guard.rs\n{}",
        under.join("\n")
    );
}

#[test]
fn a_new_engine_piece_is_caught() {
    assert_eq!(definitions("fn push_after(builder: &mut B) {}"), 1);
    assert_eq!(
        definitions("pub(super) enum Grouping {\n    Status,\n}\n"),
        1
    );
    assert_eq!(
        definitions("pub enum Sort {\n}\nconst PRIORITY_RANK: &str = \"\";"),
        2
    );
    assert_eq!(definitions("enum SortBy {}\n// fn push_after\n"), 0);
    assert_eq!(
        definitions("fn a() {}\n#[cfg(test)]\nmod tests {\n    fn push_after() {}\n}\n"),
        0
    );
}
