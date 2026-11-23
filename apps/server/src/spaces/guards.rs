//! K4.3: the spaces guard rails (K4 plan, section 6), as unit tests that
//! read the server's source and migrations, like `structure_guards`.
//!
//! - G1: the stored kinds are exactly the ones `SpaceKind` knows; kind
//!   literals in code only go down
//!   (`guards/kind_literals.json`); only the spaces module makes a space.
//! - G2: every SQL string that reads `conversations` or `messages` carries
//!   the visibility rule, or its file and function are in
//!   `conversation_query_allowlist.txt` with the reason it needs none.
//! - G4: tables made after K4.1 are named by `workspace_id` and `space_id`,
//!   never `organization_id` or `project_id` (ruling R1).
//!
//! G6 (the isolation sweep knows `workspace_id` and `space_id`) lives in
//! `tests/integration/support/two_workspaces.rs`.

use std::collections::{BTreeMap, BTreeSet};

use crate::test_support::source::{
    code_lines, contains_word, normalise, rust_files, server_src, string_literals, strip_tests,
};

use super::SpaceKind;

/// Kinds stored in `projects.kind` that aren't spaces, and why. Empty
/// since K4.17 took DMs out of the hidden Messages projects.
const NOT_SPACE_KINDS: [(&str, &str); 0] = [];

/// Files that may insert a space row today, and why. Only ever shrinks:
/// `spaces::create` becomes the one way to make a space.
const SPACE_CREATORS: [(&str, &str); 1] = [(
    "seed.rs",
    "the development seed writes its sample projects directly",
)];

/// The first migration held to G4: K4.1's, which named spaces.
const NAMING_BASELINE: u64 = 20_261_114_190_300;

/// How many hidden-kind literals `source` holds: `kind = 'direct'`,
/// `kind = 'workspace'`, `kind = 'standard'`, `NOT IN ('direct'` and Rust
/// `== "direct"`.
pub(crate) fn kind_literals(source: &str) -> usize {
    code_lines(source)
        .map(|(_, line)| {
            let normal = normalise(line);
            let sql = ["direct", "workspace", "standard"]
                .iter()
                .flat_map(|kind| {
                    ["=", "<>", "!="]
                        .into_iter()
                        .map(move |operator| format!("kind {operator} '{kind}'"))
                })
                .map(|pattern| normal.matches(&pattern).count())
                .sum::<usize>();
            sql + normal.matches("NOT IN ('direct'").count()
                + normal.matches("== \"direct\"").count()
                + normal.matches("!= \"direct\"").count()
        })
        .sum()
}

/// How many times `source` inserts a space row.
pub(crate) fn space_inserts(source: &str) -> usize {
    string_literals(source)
        .into_iter()
        .map(|literal| normalise(&literal.text))
        .map(|sql| {
            ["INSERT INTO projects", "INSERT INTO spaces"]
                .iter()
                .map(|word| {
                    sql.match_indices(word)
                        .filter(|(index, _)| {
                            !sql[index + word.len()..]
                                .bytes()
                                .next()
                                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        })
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

/// The function each unfiltered conversation or message read sits in:
/// `(function, line)` for every SQL string that reads `conversations` or
/// `messages` without `conversation_visible_to(` (or its
/// `conversation_readable_by(` wrapper) and without a
/// `-- visibility: <why>` comment.
pub(crate) fn unfiltered_conversation_reads(source: &str) -> Vec<(String, usize)> {
    let functions = function_starts(source);
    string_literals(source)
        .into_iter()
        .filter(|literal| {
            let sql = normalise(&literal.text);
            [
                "FROM conversations",
                "JOIN conversations",
                "FROM messages",
                "JOIN messages",
            ]
            .iter()
            .any(|word| contains_word(&sql, word))
                && !sql.contains("conversation_visible_to(")
                && !sql.contains("conversation_readable_by(")
                && !sql.contains("-- visibility:")
        })
        .map(|literal| {
            let function = functions
                .iter()
                .rev()
                .find(|(line, _)| *line <= literal.line)
                .map_or_else(|| "(top level)".to_owned(), |(_, name)| name.clone());
            (function, literal.line)
        })
        .collect()
}

/// Where each `fn` starts: `(line, name)`.
fn function_starts(source: &str) -> Vec<(usize, String)> {
    code_lines(source)
        .filter_map(|(line, text)| {
            let at = text.find("fn ")?;
            let before = &text[..at];
            if !(before.trim().is_empty()
                || before.ends_with(' ')
                    && before.split_whitespace().all(|word| {
                        matches!(
                            word,
                            "pub" | "pub(crate)" | "pub(super)" | "async" | "const" | "unsafe"
                        )
                    }))
            {
                return None;
            }
            let name = text[at + 3..]
                .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .next()?;
            (!name.is_empty()).then(|| (line, name.to_owned()))
        })
        .collect()
}

/// `file::function` → reason, from `conversation_query_allowlist.txt`.
fn conversation_query_allowlist() -> BTreeMap<String, String> {
    include_str!("conversation_query_allowlist.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (site, reason) = line
                .split_once(" | ")
                .unwrap_or_else(|| panic!("`file::function | reason`: {line}"));
            (site.trim().to_owned(), reason.trim().to_owned())
        })
        .collect()
}

fn kind_literal_allowances() -> BTreeMap<String, usize> {
    serde_json::from_str(include_str!("guards/kind_literals.json"))
        .expect("guards/kind_literals.json is a map of file to count")
}

/// The kinds the last `projects_kind_check` in `sql` lets `projects.kind`
/// hold, if it defines one.
fn kinds_in(sql: &str) -> Option<BTreeSet<String>> {
    const OPENING: &str = "CONSTRAINT projects_kind_check CHECK (kind IN (";
    let sql = normalise(sql);
    let at = sql.rfind(OPENING)?;
    let list = &sql[at + OPENING.len()..];
    let list = &list[..list.find(')')?];
    Some(
        list.split(',')
            .map(|kind| kind.trim().trim_matches('\'').to_owned())
            .collect(),
    )
}

/// The kinds the latest migration lets `projects.kind` hold.
fn stored_kinds() -> BTreeSet<String> {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut paths = std::fs::read_dir(&folder)
        .expect("migrations folder")
        .map(|entry| entry.expect("migration").path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| kinds_in(&std::fs::read_to_string(path).expect("read migration")))
        .next_back()
        .unwrap_or_default()
}

/// What's wrong with `stored` against the kinds `SpaceKind` knows (and the
/// listed kinds that aren't spaces), if anything.
fn kind_list_problem(stored: &BTreeSet<String>) -> Option<String> {
    let mut expected = SpaceKind::ALL
        .iter()
        .map(|kind| kind.stored().to_owned())
        .collect::<BTreeSet<_>>();
    expected.extend(NOT_SPACE_KINDS.iter().map(|(kind, _)| (*kind).to_owned()));
    (stored != &expected).then(|| {
        format!(
            "projects.kind holds {stored:?}, SpaceKind knows {expected:?}. A new kind needs \
             SpaceKind, this guard's list and a sentence of rationale (K4 section 6, G1)"
        )
    })
}

/// A new table's columns named `organization_id` or `project_id`, or a
/// missing `workspace_id`, in a migration after the baseline.
pub(crate) fn naming_offences(name: &str, sql: &str) -> Vec<String> {
    let Some(version) = name
        .split('_')
        .next()
        .and_then(|version| version.parse::<u64>().ok())
    else {
        return Vec::new();
    };
    if version < NAMING_BASELINE {
        return Vec::new();
    }
    let normal = normalise(sql);
    let mut offences = Vec::new();
    let mut rest = normal.as_str();
    while let Some(at) = rest.find("CREATE TABLE ") {
        let after = rest[at + "CREATE TABLE ".len()..].trim_start();
        let after = after.strip_prefix("IF NOT EXISTS ").unwrap_or(after);
        let table = after
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .next()
            .unwrap_or_default()
            .to_owned();
        // A partition takes its parent's columns.
        if after[table.len()..]
            .trim_start()
            .starts_with("PARTITION OF ")
        {
            rest = &after[table.len()..];
            continue;
        }
        let body_start = after.find('(').map_or(after.len(), |index| index + 1);
        let mut depth = 1;
        let mut end = body_start;
        for (index, character) in after[body_start..].char_indices() {
            match character {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                end = body_start + index;
                break;
            }
        }
        let body = &after[body_start..end];
        let columns = split_top_level(body)
            .into_iter()
            .filter_map(|column| column.split_whitespace().next().map(str::to_owned))
            .collect::<Vec<_>>();
        for old in ["organization_id", "project_id"] {
            if columns.iter().any(|column| column == old) {
                offences.push(format!(
                    "{name}: {table} declares {old}; new tables say workspace_id and space_id \
                     (ruling R1, AGENTS.md rule 11)"
                ));
            }
        }
        if !columns.iter().any(|column| column == "workspace_id") {
            offences.push(format!(
                "{name}: {table} has no workspace_id; every row belongs to a workspace, or the \
                 table is on tenancy::tables' global list with a reason"
            ));
        }
        rest = &after[end.min(after.len())..];
    }
    offences
}

/// `body` split at commas outside parentheses.
fn split_top_level(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (index, character) in body.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(body[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<(String, String)> {
        rust_files(&server_src())
            .into_iter()
            .filter(|(path, _)| {
                !path.starts_with("spaces/guards")
                    && path != "structure_guards.rs"
                    && !path.starts_with("test_support/")
            })
            .collect()
    }

    /// Tables `tenancy::tables` lists as global need no `workspace_id`.
    fn global_tables() -> BTreeSet<&'static str> {
        crate::tenancy::tables::GLOBAL_TABLES
            .iter()
            .map(|(table, _)| *table)
            .collect()
    }

    #[test]
    fn kind_check_is_the_list() {
        let stored = stored_kinds();
        assert!(
            !stored.is_empty(),
            "no projects_kind_check in the migrations"
        );
        if let Some(problem) = kind_list_problem(&stored) {
            panic!("{problem}");
        }
    }

    #[test]
    fn kind_literals_ratchet() {
        // The whole tree holds to the sum of the allowances, so code that
        // moves between files needs no change here.
        let allowed = kind_literal_allowances();
        let allowed_total: usize = allowed.values().sum();
        let found = sources()
            .into_iter()
            .map(|(path, source)| (path, kind_literals(strip_tests(&source))))
            .filter(|(_, found)| *found > 0)
            .collect::<BTreeMap<_, _>>();
        let found_total: usize = found.values().sum();
        if found_total < allowed_total {
            println!(
                "hint: {found_total} hidden-kind literals, {allowed_total} allowed; \
                 spaces/guards/kind_literals.json can come down"
            );
        }
        assert!(
            found_total <= allowed_total,
            "{found_total} hidden-kind literals, {allowed_total} allowed ({found:?}): read the \
             kind through SpaceKind (spaces/kind.rs)"
        );
    }

    #[test]
    fn one_space_creator() {
        let allowed = SPACE_CREATORS
            .iter()
            .map(|(path, _)| *path)
            .collect::<BTreeSet<_>>();
        let mut failures = Vec::new();
        let mut creators = BTreeSet::new();
        for (path, source) in sources() {
            if space_inserts(strip_tests(&source)) == 0 || path.starts_with("spaces/") {
                continue;
            }
            creators.insert(path.clone());
            if !allowed.contains(path.as_str()) {
                failures.push(format!(
                    "{path} inserts a space row; only the spaces module makes spaces (AGENTS.md \
                     rule 11)"
                ));
            }
        }
        for (path, _) in SPACE_CREATORS {
            assert!(
                creators.contains(path),
                "{path} no longer inserts a space: remove it from SPACE_CREATORS"
            );
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn conversation_queries_check_visibility() {
        let allowlist = conversation_query_allowlist();
        let mut failures = Vec::new();
        let mut seen = BTreeSet::new();
        for (path, source) in sources() {
            for (function, line) in unfiltered_conversation_reads(strip_tests(&source)) {
                let site = format!("{path}::{function}");
                if !allowlist.contains_key(&site) {
                    failures.push(format!(
                        "{path}:{line} ({function}) reads conversations or messages without \
                         conversation_visible_to (AGENTS.md rule 7); add the rule, or a \
                         `-- visibility: <why>` comment for a by-id read after an access check"
                    ));
                }
                seen.insert(site);
            }
        }
        for site in allowlist.keys() {
            if !seen.contains(site) {
                failures.push(format!(
                    "{site} no longer reads unfiltered: remove it from \
                     conversation_query_allowlist.txt (the list only shrinks)"
                ));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The tables `sql` drops (`DROP TABLE [IF EXISTS] name`).
    fn dropped_tables(sql: &str) -> Vec<String> {
        let upper = sql.to_ascii_uppercase();
        upper
            .match_indices("DROP TABLE ")
            .filter_map(|(index, word)| {
                let rest = &sql[index + word.len()..];
                let rest = rest
                    .strip_prefix("IF EXISTS ")
                    .or_else(|| rest.strip_prefix("if exists "))
                    .unwrap_or(rest);
                let table = rest
                    .split(|character: char| {
                        !(character.is_ascii_alphanumeric() || character == '_')
                    })
                    .next()?;
                (!table.is_empty()).then(|| table.to_owned())
            })
            .collect()
    }

    #[test]
    fn new_tables_use_workspace_and_space_ids() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let global = global_tables();
        let mut offences = Vec::new();
        // A table a later migration drops is gone: it holds no rows.
        let mut dropped = Vec::new();
        for entry in std::fs::read_dir(&folder).expect("migrations folder") {
            let path = entry.expect("migration").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            let sql = std::fs::read_to_string(&path).expect("read migration");
            dropped.extend(dropped_tables(&sql));
            offences.extend(naming_offences(&name, &sql).into_iter().filter(|offence| {
                !global
                    .iter()
                    .any(|table| offence.contains(&format!(": {table} has no workspace_id")))
            }));
        }
        offences.retain(|offence| {
            !dropped
                .iter()
                .any(|table| offence.contains(&format!(": {table} has no workspace_id")))
        });
        assert!(offences.is_empty(), "{offences:#?}");
    }

    /// Prints today's kind literal counts and unfiltered conversation reads,
    /// for writing the allowances.
    #[test]
    #[ignore = "prints the sites for writing the allowlists"]
    fn print_spaces_guard_sites() {
        for (path, source) in sources() {
            let found = kind_literals(strip_tests(&source));
            if found > 0 {
                println!("KIND {path} {found}");
            }
            for (function, line) in unfiltered_conversation_reads(strip_tests(&source)) {
                println!("READ {path}::{function} {line}");
            }
        }
    }

    // Each guard fails on a planted violation.

    #[test]
    fn a_planted_kind_literal_is_counted() {
        let source = r#"
            fn f() {
                let sql = "SELECT id FROM projects WHERE kind = 'direct'";
                if kind == "direct" {}
                // kind = 'workspace' in a comment doesn't count
                let other = "WHERE kind NOT IN ('direct', 'workspace')";
            }
        "#;
        assert_eq!(kind_literals(source), 3);
    }

    #[test]
    fn a_planted_space_insert_is_counted() {
        let source = r#"
            fn make() {
                sqlx::query("INSERT INTO projects (id) VALUES ($1)");
                sqlx::query("INSERT INTO project_memberships (id) VALUES ($1)");
                sqlx::query("INSERT INTO spaces (id) VALUES ($1)");
            }
        "#;
        assert_eq!(space_inserts(source), 2);
    }

    #[test]
    fn a_planted_unfiltered_read_names_its_function() {
        let source = r#"
            pub(crate) async fn list_everything(pool: &PgPool) {
                sqlx::query("SELECT id FROM conversations WHERE project_id = $1");
                sqlx::query("SELECT id FROM conversations WHERE conversation_visible_to(id, $1)");
                sqlx::query("SELECT id FROM messages -- visibility: by id after a check");
            }
            fn fine() {
                sqlx::query("SELECT id FROM conversations_archive");
            }
        "#;
        assert_eq!(
            unfiltered_conversation_reads(source),
            vec![("list_everything".to_owned(), 3)]
        );
        assert!(!conversation_query_allowlist().contains_key("planted.rs::list_everything"));
    }

    #[test]
    fn a_planted_old_name_in_a_new_table_is_refused() {
        let sql = "CREATE TABLE gadgets (\n id UUID PRIMARY KEY,\n organization_id UUID NOT NULL REFERENCES organizations(id),\n note TEXT CHECK (char_length(note) < 10)\n);";
        let offences = naming_offences("20261201000000_gadgets.sql", sql);
        assert_eq!(offences.len(), 2, "{offences:?}");
        assert!(offences[0].contains("declares organization_id"));
        assert!(offences[1].contains("no workspace_id"));
        let good = "CREATE TABLE gadgets (id UUID PRIMARY KEY, workspace_id UUID NOT NULL, space_id UUID);";
        assert!(naming_offences("20261201000000_gadgets.sql", good).is_empty());
        assert!(naming_offences("20261101000000_old.sql", sql).is_empty());
    }

    #[test]
    fn a_planted_kind_fails_the_list() {
        let migration = "ALTER TABLE projects DROP CONSTRAINT projects_kind_check,\n    ADD CONSTRAINT projects_kind_check\n        CHECK (kind IN ('standard', 'workspace', 'team', 'personal', 'hidden'));";
        let planted = kinds_in(migration).expect("a definition");
        assert!(planted.contains("hidden"), "{planted:?}");
        assert!(kind_list_problem(&planted).is_some());
        let today = kinds_in(&migration.replace(", 'hidden'", "")).expect("a definition");
        assert_eq!(kind_list_problem(&today), None);
    }
}
