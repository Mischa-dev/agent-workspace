//! Readers of a task's shared columns move to `task_rows` (T1.4a-c, owner
//! step 3b in `docs/plans/master/plans/T1-fields-views-tasks.md`).
//!
//! A task's title, field values and archive state belong to its record
//! (T1.3); `work_items` keeps a copy only until every reader has moved to
//! the `task_rows` view, and then the copy is dropped (T1.4d). So a query
//! that reads one of those columns from `work_items` (`FROM work_items item
//! … item.title`, or `FROM work_items … archived_at` with no alias) may stay
//! only in the files listed below, at most as many times as today. The list
//! only shrinks, and T1.4d removes it with the columns.

use std::collections::BTreeMap;

use crate::test_support::source::{normalise, rust_files, server_src, string_literals, strip_tests};

/// The columns the record owns, under their old names.
const SHARED: [&str; 4] = ["title", "field_values", "archived_at", "archived_by_actor_id"];

/// Words that can follow `FROM work_items` without being its alias.
const NOT_ALIASES: &[&str] = &[
    "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "FULL", "CROSS", "ON", "SET", "ORDER", "GROUP",
    "LIMIT", "FOR", "UNION", "USING", "RETURNING", "HAVING", "WINDOW", "OFFSET", "EXCEPT",
    "INTERSECT", "AND", "OR",
];

/// Today's readers that haven't moved, file by file. Lower a number when a
/// reader moves; never raise one or add a file.
const ALLOWED: &[(&str, usize)] = &[
    // Everything else (T1.4c).
    ("src/agent_profiles.rs", 1),
    ("src/ai/detect.rs", 3),
    ("src/ai/http.rs", 2),
    ("src/ai/ready.rs", 4),
    ("src/ai/suggestions.rs", 1),
    ("src/ai/summaries.rs", 1),
    ("src/conversations.rs", 3),
    ("src/github.rs", 2),
    ("src/later.rs", 1),
    ("src/launch.rs", 3),
    ("src/modules/things.rs", 3),
    ("src/notifications.rs", 2),
    ("src/onboarding.rs", 1),
    ("src/outbox.rs", 3),
    ("src/run_views.rs", 1),
    ("src/search.rs", 1),
    ("src/usage/ready.rs", 2),
    ("src/webhooks.rs", 1),
    ("src/whiteboards.rs", 1),
];

const HOW: &str = "Read a task's title, field values and archive state from the task_rows view \
     (its record joined to its core), not from work_items, whose copies go in T1.4d \
     (docs/plans/master/plans/T1-fields-views-tasks.md, step 3b).";

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `sql` names `column` on its own (`archived_at`, not
/// `item.archived_at` or `archived_at_x`), or as `alias.column`.
fn names(sql: &str, alias: Option<&str>, column: &str) -> bool {
    let needle = match alias {
        Some(alias) => format!("{alias}.{column}"),
        None => column.to_owned(),
    };
    sql.match_indices(&needle).any(|(index, _)| {
        let before = sql[..index].bytes().next_back();
        let after = sql[index + needle.len()..].bytes().next();
        !before.is_some_and(|byte| is_ident(byte) || byte == b'.')
            && !after.is_some_and(is_ident)
    })
}

/// How many times one SQL text reads a shared column from `work_items`.
fn reads_in(sql: &str) -> usize {
    let mut count = 0;
    for keyword in ["FROM work_items", "JOIN work_items"] {
        for (index, _) in sql.match_indices(keyword) {
            let rest = &sql[index + keyword.len()..];
            if rest.bytes().next().is_some_and(is_ident)
                || (keyword.starts_with("FROM") && sql[..index].trim_end().ends_with("DELETE"))
            {
                continue;
            }
            let mut words = rest.split_whitespace();
            let mut alias = words.next();
            if alias == Some("AS") {
                alias = words.next();
            }
            let alias = alias
                .map(|word| word.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_')))
                .filter(|word| {
                    !word.is_empty()
                        && word.bytes().all(is_ident)
                        && !NOT_ALIASES.contains(&word.to_ascii_uppercase().as_str())
                });
            if SHARED.iter().any(|column| names(sql, alias, column)) {
                count += 1;
            }
        }
    }
    count
}

/// Reads of the cached columns in one file, before its test tail.
pub(crate) fn cached_reads(source: &str) -> usize {
    string_literals(strip_tests(source))
        .iter()
        .map(|literal| reads_in(&normalise(&literal.text)))
        .sum()
}

#[test]
fn readers_of_the_shared_columns_only_move_to_task_rows() {
    let allowed = ALLOWED.iter().copied().collect::<BTreeMap<_, _>>();
    let mut problems = Vec::new();
    for (path, source) in rust_files(&server_src()) {
        if path == "task_rows_guard.rs" {
            continue;
        }
        let path = format!("src/{path}");
        let found = cached_reads(&source);
        let limit = allowed.get(path.as_str()).copied().unwrap_or(0);
        if found > limit {
            problems.push(format!("{path}: {found} (allowed {limit})"));
        } else if found < limit {
            eprintln!("{path}: {found} reads now; lower its allowance from {limit}");
        }
    }
    assert!(problems.is_empty(), "{}\n{HOW}", problems.join("\n"));
}

#[test]
fn a_read_is_found_with_or_without_an_alias() {
    assert_eq!(cached_reads(r#"query("SELECT item.title FROM work_items item")"#), 1);
    assert_eq!(cached_reads(r#"query("SELECT 1 FROM work_items WHERE archived_at IS NULL")"#), 1);
    assert_eq!(cached_reads(r#"query("SELECT version FROM work_items WHERE id = $1")"#), 0);
    assert_eq!(
        cached_reads(r#"query("SELECT page.title FROM work_items item JOIN pages page ON true")"#),
        0
    );
    assert_eq!(cached_reads(r#"query("SELECT item.title FROM task_rows item")"#), 0);
    assert_eq!(
        cached_reads(r#"query("DELETE FROM work_items WHERE archived_at IS NOT NULL")"#),
        0
    );
}
