//! A1.2: guard rails for the run service (A1 section 6).
//!
//! - G1: SQL that reads or writes the run tables lives in `src/runs/`.
//!   `LEGACY_RUN_SQL` names each file not moved yet and the slice that moves
//!   it. The test fails on a new file, and on a listed file that no longer
//!   needs its entry, so the list only shrinks.
//! - G4: lists of active run states are written once, in `runs/state.rs`
//!   (`RunState::is_active`); `LEGACY_STATE_LISTS` names the files that still
//!   spell the built-in host's states out, the same way.
//! - G5: no account-kind checks in the run service: a person running a
//!   script reports exactly as an agent does.

use std::{collections::BTreeMap, path::Path};

/// Files outside `src/runs/` with SQL on the run tables, and the slice
/// that moves each into the run service.
const LEGACY_RUN_SQL: &[(&str, &str)] = &[
    ("account.rs", "A1.6"),
    ("agent_profiles.rs", "A1.6"),
    ("api.rs", "A1.14"),
    ("automations.rs", "A1.6"),
    ("conversations.rs", "A1.6"),
    ("domain.rs", "A1.14"),
    ("events.rs", "A1.6"),
    ("external_events.rs", "A1.6"),
    ("external_execution.rs", "A1.10"),
    ("github.rs", "A1.19"),
    ("home.rs", "A1.6"),
    ("launch.rs", "A1.13"),
    ("mcp.rs", "A1.6"),
    ("mcp/approvals.rs", "A1.8"),
    ("mcp/code.rs", "A1.19"),
    ("mcp/runs.rs", "A1.6"),
    ("mcp/talk.rs", "A1.6"),
    ("metrics.rs", "A1.6"),
    ("notifications.rs", "A1.6"),
    ("onboarding.rs", "A1.6"),
    ("outbox.rs", "A1.6"),
    ("provider.rs", "A1.17"),
    ("recovery.rs", "A1.17"),
    ("retention.rs", "A1.6"),
    ("run_log.rs", "A1.14"),
    ("run_views.rs", "A1.6"),
    ("scheduler.rs", "A1.17"),
    ("usage/history.rs", "A1.15"),
    ("webhooks.rs", "A1.6"),
    ("work_items.rs", "A1.6"),
];

/// Files that still spell out the built-in host's active states, and the
/// slice that replaces each list with `RunState`.
const LEGACY_STATE_LISTS: &[(&str, &str)] = &[
    ("account.rs", "A1.6"),
    ("agent_profiles.rs", "A1.6"),
    ("api.rs", "A1.14"),
    ("conversations.rs", "A1.6"),
    ("domain.rs", "A1.14"),
    ("external_execution.rs", "A1.10"),
    ("github.rs", "A1.19"),
    ("home.rs", "A1.6"),
    ("launch.rs", "A1.13"),
    ("metrics.rs", "A1.6"),
    ("provider.rs", "A1.17"),
    ("recovery.rs", "A1.17"),
    ("retention.rs", "A1.6"),
    ("scheduler.rs", "A1.17"),
];

const RUN_TABLES: &[&str] = &[
    "runs",
    "run_activities",
    "run_usage",
    "usage_reports",
    "agent_runs",
    "agent_run_events",
    "agent_run_usage",
];
const SQL_VERBS: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE"];
const STATES: &[&str] = &[
    "created",
    "queued",
    "starting",
    "working",
    "blocked",
    "recovering",
];

/// Every `.rs` file under `src/`, by its path from there.
fn sources() -> BTreeMap<String, String> {
    fn walk(root: &Path, folder: &Path, found: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(folder).expect("read source folder") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                walk(root, &path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let name = path
                    .strip_prefix(root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.insert(
                    name,
                    std::fs::read_to_string(&path).expect("read source file"),
                );
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::new();
    walk(&root, &root, &mut found);
    assert!(
        found.contains_key("runs/mod.rs") && found.len() > 100,
        "the walk must find the server's sources"
    );
    found
}

fn words(source: &str) -> Vec<&str> {
    source
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .collect()
}

fn touches_run_tables(source: &str) -> bool {
    words(source)
        .windows(2)
        .any(|pair| SQL_VERBS.contains(&pair[0]) && RUN_TABLES.contains(&pair[1]))
}

/// A list of two or more built-in states: `'queued', 'starting'` or
/// `'created','queued'`.
fn lists_states(source: &str) -> bool {
    let quoted = |text: &str| {
        STATES
            .iter()
            .find(|state| text.starts_with(&format!("'{state}'")))
            .map(|state| state.len() + 2)
    };
    source.char_indices().any(|(index, _)| {
        let rest = &source[index..];
        quoted(rest).is_some_and(|length| {
            let after = rest[length..].trim_start();
            after
                .strip_prefix(',')
                .is_some_and(|after| quoted(after.trim_start()).is_some())
        })
    })
}

fn check(
    guard: &str,
    allowed: &[(&str, &str)],
    exempt: impl Fn(&str) -> bool,
    offends: impl Fn(&str) -> bool,
) {
    let sources = sources();
    let mut failures = Vec::new();
    for (name, source) in &sources {
        if exempt(name) || !offends(source) {
            continue;
        }
        if !allowed.iter().any(|(file, _)| file == name) {
            failures.push(format!(
                "{name}: move this into src/runs/ (the run service) instead"
            ));
        }
    }
    for (file, slice) in allowed {
        match sources.get(*file) {
            Some(source) if offends(source) => {}
            _ => failures.push(format!(
                "{file} no longer needs its {guard} entry (was for {slice}): remove it"
            )),
        }
    }
    assert!(failures.is_empty(), "{guard}:\n{}", failures.join("\n"));
}

#[test]
fn g1_run_sql_lives_in_the_run_service() {
    check(
        "G1",
        LEGACY_RUN_SQL,
        |name| name.starts_with("runs/"),
        touches_run_tables,
    );
}

#[test]
fn g4_active_states_are_listed_once() {
    check(
        "G4",
        LEGACY_STATE_LISTS,
        |name| name == "runs/state.rs" || name == "runs/guards.rs",
        lists_states,
    );
}

#[test]
fn g5_the_run_service_never_asks_what_kind_of_account_reports() {
    for (name, source) in sources() {
        if !(name.starts_with("runs/") || name.starts_with("requests/"))
            || name == "runs/guards.rs"
            || name == "runs/mod.rs"
        {
            continue;
        }
        for forbidden in [
            "ActorKind::",
            "actor.kind =",
            "kind = 'human'",          // kind-check: mechanical (G5's own pattern)
            "kind = 'external_agent'", // kind-check: mechanical (G5's own pattern)
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} checks an account's kind ({forbidden}); the run service treats every account alike"
            );
        }
    }
}

#[test]
fn the_guards_see_what_they_look_for() {
    assert!(touches_run_tables("SELECT 1 FROM agent_runs run"));
    assert!(touches_run_tables("UPDATE runs SET"));
    assert!(!touches_run_tables("FROM runs_view"));
    assert!(!touches_run_tables("from runs"));
    assert!(lists_states("state IN ('created', 'queued')"));
    assert!(lists_states("('starting','working')"));
    assert!(!lists_states("state = 'queued'"));
}
