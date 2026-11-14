//! Structure guards (P0.0, P0.26): rules from `AGENTS.md` ("Rules while the
//! foundations are rebuilt") checked over the server's source, with a
//! per-file allowance for today's code in `structure_guards/allowances.json`.
//!
//! A file whose count is above its allowance fails, and so does a rule whose
//! allowances add up to more than its ceiling. A count below its allowance
//! never fails: it prints a hint to lower the number, so two branches that
//! each remove an offender merge in either order. Allowances only go down;
//! when code moves between files, its allowance moves with it.
//!
//! Every guard id here is listed in `docs/plans/master/guards.md`.

use std::collections::BTreeMap;

use crate::test_support::source::{
    code_lines, contains_word, normalise, rust_files, server_src, string_literals, strip_tests,
};

/// One rule: its guard id, the `AGENTS.md` rule it enforces, the most its
/// allowances may add up to, and how to count offences in one file.
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) agents_rule: &'static str,
    pub(crate) ceiling: usize,
    pub(crate) count: fn(&str) -> usize,
}

pub(crate) const RULES: &[Rule] = &[
    Rule {
        id: "raw_role_checks",
        agents_rule: "rule 1, \"Access is decided in one place\"",
        ceiling: RAW_ROLE_CHECKS_CEILING,
        count: raw_role_checks,
    },
    Rule {
        id: "custom_role_allows",
        agents_rule: "rule 1, \"Access is decided in one place\"",
        ceiling: CUSTOM_ROLE_ALLOWS_CEILING,
        count: custom_role_allows,
    },
    Rule {
        id: "workspace_role_enum",
        agents_rule: "rule 1, \"Access is decided in one place\"",
        ceiling: WORKSPACE_ROLE_ENUM_CEILING,
        count: workspace_role_enum,
    },
    Rule {
        id: "account_kind_checks",
        agents_rule: "rule 2, \"No permission depends on the kind of account\"",
        ceiling: ACCOUNT_KIND_CHECKS_CEILING,
        count: account_kind_checks,
    },
    Rule {
        id: "agent_runs_sql",
        agents_rule: "rule 3, \"Runs are read and written through runs.rs\"",
        ceiling: AGENT_RUNS_SQL_CEILING,
        count: agent_runs_sql,
    },
    Rule {
        id: "active_run_states_literal",
        agents_rule: "rule 3, \"Runs are read and written through runs.rs\"",
        ceiling: ACTIVE_RUN_STATES_LITERAL_CEILING,
        count: active_run_states_literal,
    },
    Rule {
        id: "conversation_inserts",
        agents_rule: "rule 4, \"One edge per operation\"",
        ceiling: CONVERSATION_INSERTS_CEILING,
        count: conversation_inserts,
    },
    Rule {
        id: "message_inserts",
        agents_rule: "rule 4, \"One edge per operation\"",
        ceiling: MESSAGE_INSERTS_CEILING,
        count: message_inserts,
    },
    Rule {
        id: "task_core_updates",
        agents_rule: "rule 4, \"One edge per operation\"",
        ceiling: TASK_CORE_UPDATES_CEILING,
        count: task_core_updates,
    },
    Rule {
        id: "direct_tool_calls",
        agents_rule: "rule 4, \"One edge per operation\"",
        ceiling: DIRECT_TOOL_CALLS_CEILING,
        count: direct_tool_calls,
    },
    Rule {
        id: "process_state",
        agents_rule: "rule 5, \"State lives in PostgreSQL, work runs on the job queue\"",
        ceiling: PROCESS_STATE_CEILING,
        count: process_state,
    },
    Rule {
        id: "background_spawns",
        agents_rule: "rule 5, \"State lives in PostgreSQL, work runs on the job queue\"",
        ceiling: BACKGROUND_SPAWNS_CEILING,
        count: background_spawns,
    },
    Rule {
        id: "conversation_listings",
        agents_rule: "rule 7, \"Every conversation read goes through conversation_visible_to\"",
        ceiling: CONVERSATION_LISTINGS_CEILING,
        count: conversation_listings,
    },
    Rule {
        id: "notification_detail_text",
        agents_rule: "rule 10, \"Content stays in its record\"",
        ceiling: NOTIFICATION_DETAIL_TEXT_CEILING,
        count: notification_detail_text,
    },
    Rule {
        id: "vendor_names_core",
        agents_rule: "rule 8, \"Nobody's stack is hard-coded\"",
        ceiling: VENDOR_NAMES_CORE_CEILING,
        count: vendor_names_core,
    },
];

// The most each rule's allowances may add up to: today's total, set when
// P0.26 landed. Lower one when its allowances go down; never raise one.
const RAW_ROLE_CHECKS_CEILING: usize = 40;
const CUSTOM_ROLE_ALLOWS_CEILING: usize = 25;
const WORKSPACE_ROLE_ENUM_CEILING: usize = 32;
const ACCOUNT_KIND_CHECKS_CEILING: usize = 264;
const AGENT_RUNS_SQL_CEILING: usize = 178;
const ACTIVE_RUN_STATES_LITERAL_CEILING: usize = 19;
const CONVERSATION_INSERTS_CEILING: usize = 11;
const MESSAGE_INSERTS_CEILING: usize = 4;
const TASK_CORE_UPDATES_CEILING: usize = 10;
const DIRECT_TOOL_CALLS_CEILING: usize = 4;
const PROCESS_STATE_CEILING: usize = 14;
const BACKGROUND_SPAWNS_CEILING: usize = 16;
const CONVERSATION_LISTINGS_CEILING: usize = 77;
// P0.36. The four left are system text, not anyone's words: why a scheduled
// message wasn't sent (scheduled_messages.rs), why an automation was turned
// off and which automation posted (automations.rs), and a usage alert's
// suggested batch (usage/alerts.rs).
const NOTIFICATION_DETAIL_TEXT_CEILING: usize = 4;
const VENDOR_NAMES_CORE_CEILING: usize = 9;

/// The heading in `AGENTS.md` the rules sit under.
const AGENTS_HEADING: &str = "Rules while the foundations are rebuilt";

/// Files the guards never read: the guards themselves and the scanner, whose
/// samples would count against them.
const SKIPPED: &[&str] = &["structure_guards.rs", "test_support/source.rs"];

/// The built-in roles a raw check names.
const BUILT_IN_ROLES: [&str; 5] = ["owner", "admin", "member", "viewer", "worker"];

/// SQL literals, whitespace collapsed.
fn sql(source: &str) -> impl Iterator<Item = String> {
    string_literals(source)
        .into_iter()
        .map(|literal| normalise(&literal.text))
}

/// How many times `word` appears in `text` with no identifier character
/// right after it.
fn count_word(text: &str, word: &str) -> usize {
    text.match_indices(word)
        .filter(|(index, _)| {
            !text[index + word.len()..]
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .count()
}

/// Matches in code lines (comment lines left out).
fn in_code(source: &str, matches: impl Fn(&str) -> usize) -> usize {
    code_lines(source).map(|(_, line)| matches(line)).sum()
}

/// A role checked by name in SQL: `role IN ('owner', …)`, `role = 'admin'`,
/// `role <> 'viewer'`.
pub(crate) fn raw_role_checks(source: &str) -> usize {
    sql(source)
        .map(|sql| {
            let mut found = 0;
            for role in BUILT_IN_ROLES {
                for operator in ["=", "<>", "!="] {
                    found += sql.matches(&format!("role {operator} '{role}'")).count();
                }
            }
            found + sql.matches("role IN ('").count()
        })
        .sum()
}

/// A call of the SQL helper that checks a custom role's permission list.
pub(crate) fn custom_role_allows(source: &str) -> usize {
    in_code(source, |line| line.matches("custom_role_allows(").count())
}

/// The workspace role enum or its manager check, outside `workspace.rs`
/// (counted there too; that file's allowance covers it).
pub(crate) fn workspace_role_enum(source: &str) -> usize {
    in_code(source, |line| {
        line.matches("WorkspaceRole::").count() + line.matches(".manages()").count()
    })
}

/// A decision by account kind, unless the line says it is mechanical.
pub(crate) fn account_kind_checks(source: &str) -> usize {
    in_code(source, |line| {
        if line.contains("// kind-check: mechanical") {
            return 0;
        }
        let normal = normalise(line);
        let mut found = [
            "actor_kind ==",
            "actor_kind !=",
            "== ActorKind::",
            "!= ActorKind::",
        ]
        .iter()
        .map(|pattern| normal.matches(pattern).count())
        .sum::<usize>();
        for kind in ["human", "external_agent", "execution_worker"] {
            for operator in ["=", "<>", "!="] {
                found += normal.matches(&format!("kind {operator} '{kind}'")).count();
            }
        }
        found
    })
}

/// SQL naming the run tables directly.
pub(crate) fn agent_runs_sql(source: &str) -> usize {
    sql(source)
        .map(|sql| {
            let mut found = 0;
            for table in [
                "agent_runs",
                "agent_run_events",
                "agent_run_usage",
                "agent_approvals",
            ] {
                for verb in ["FROM", "JOIN", "INTO", "UPDATE"] {
                    found += count_word(&sql, &format!("{verb} {table}"));
                }
            }
            found
        })
        .sum()
}

/// The active run states written out instead of `runs::ACTIVE_RUN_STATES`.
pub(crate) fn active_run_states_literal(source: &str) -> usize {
    in_code(source, |line| {
        let compact = line.replace(' ', "");
        compact.matches("'created','queued','starting'").count()
            + compact
                .matches("\"created\",\"queued\",\"starting\"")
                .count()
    })
}

/// A conversation inserted outside its one creation path.
pub(crate) fn conversation_inserts(source: &str) -> usize {
    sql(source)
        .map(|sql| count_word(&sql, "INSERT INTO conversations"))
        .sum()
}

/// A message inserted outside the send pipeline.
pub(crate) fn message_inserts(source: &str) -> usize {
    sql(source)
        .map(|sql| count_word(&sql, "INSERT INTO messages"))
        .sum()
}

/// A task's core row changed outside `work_item_commands.rs` (counted there
/// too; that file's allowance covers it).
pub(crate) fn task_core_updates(source: &str) -> usize {
    sql(source)
        .map(|sql| count_word(&sql, "UPDATE work_items"))
        .sum()
}

/// A tool run from inside the server rather than through its operation.
pub(crate) fn direct_tool_calls(source: &str) -> usize {
    in_code(source, |line| {
        line.matches("call_tool_direct(").count() - line.matches("fn call_tool_direct(").count()
    })
}

/// Shared state held in the process.
pub(crate) fn process_state(source: &str) -> usize {
    in_code(source, |line| {
        let normal = normalise(line);
        let is_static = normal.starts_with("static ")
            || normal.starts_with("pub static ")
            || normal.starts_with("pub(crate) static ");
        let static_lock = is_static && (normal.contains("Mutex") || normal.contains("RwLock"));
        usize::from(static_lock || normal.contains("Mutex<HashMap"))
    })
}

/// A task spawned outside `main.rs` (counted there too; its allowance covers
/// it).
pub(crate) fn background_spawns(source: &str) -> usize {
    in_code(source, |line| line.matches("tokio::spawn(").count())
}

/// Provider names in core strings. The adapters themselves (`provider/`,
/// `provider.rs`, `ai/providers/`, `harnesses.rs`) are left out by
/// [`counts`].
pub(crate) fn vendor_names_core(source: &str) -> usize {
    string_literals(source)
        .into_iter()
        .filter(|literal| {
            VENDOR_NAMES
                .iter()
                .any(|name| contains_word(&literal.text, name))
        })
        .count()
}

/// The display names of today's adapters.
const VENDOR_NAMES: [&str; 4] = ["Codex", "Claude", "OpenAI", "Anthropic"];

/// SQL that reads conversations or messages without the visibility rule. A
/// literal that names `conversation_visible_to(` (or the
/// `conversation_readable_by(` wrapper) is filtered; one that carries a
/// `-- visibility: <why>` comment says why it needs no filter (a by-id
/// lookup after an access check, a worker, an insert).
pub(crate) fn conversation_listings(source: &str) -> usize {
    string_literals(source)
        .into_iter()
        .map(|literal| normalise(&literal.text))
        .filter(|sql| {
            [
                "FROM conversations",
                "JOIN conversations",
                "FROM messages",
                "JOIN messages",
            ]
            .iter()
            .any(|word| contains_word(sql, word))
        })
        .filter(|sql| {
            !sql.contains("conversation_visible_to(")
                && !sql.contains("conversation_readable_by(")
                && !sql.contains("-- visibility:")
        })
        .count()
}

/// A notification `detail` (`detail: json!({…})`, `detail = json!({…})`)
/// with a `preview`, `body`, `text` or `quote` key whose value isn't a
/// string literal or an id. A notification row holds ids; the words are read
/// live when listing, so a removal, an edit or an archive reaches the inbox.
pub(crate) fn notification_detail_text(source: &str) -> usize {
    let mut found = 0;
    let mut depth: Option<i32> = None;
    for (_, line) in code_lines(source) {
        if depth.is_none() {
            let Some(at) = line.find("json!(") else {
                continue;
            };
            let before = line[..at]
                .trim_end()
                .trim_end_matches("serde_json::")
                .trim_end();
            if !(before.ends_with("detail =") || before.ends_with("detail:")) {
                continue;
            }
            depth = Some(0);
        }
        if let Some(value) = DETAIL_TEXT_KEYS
            .iter()
            .find_map(|key| line.trim_start().strip_prefix(key))
        {
            let value = value.trim().trim_end_matches(',').trim();
            let constant = value.starts_with('"');
            let id = value
                .trim_end_matches(".to_string()")
                .rsplit(['.', '_'])
                .next()
                .is_some_and(|last| last == "id");
            if !constant && !id {
                found += 1;
            }
        }
        let open = depth.unwrap_or(0) + line.matches(['(', '{']).count() as i32
            - line.matches([')', '}']).count() as i32;
        depth = (open > 0).then_some(open);
    }
    found
}

const DETAIL_TEXT_KEYS: [&str; 4] = ["\"preview\":", "\"body\":", "\"text\":", "\"quote\":"];

/// Files a rule never reads, since they are where the thing belongs.
fn exempt(rule: &str, path: &str) -> bool {
    match rule {
        "vendor_names_core" => {
            path.starts_with("provider/")
                || path == "provider.rs"
                || path.starts_with("ai/providers/")
                || path == "harnesses.rs"
        }
        _ => false,
    }
}

/// Migrations after this one are held to the table rules (`link_tables`,
/// `comment_tables`); the tables made before it are today's.
const TABLE_RULES_BASELINE: u64 = 20261114070000;

/// Which table rule a new table's name breaks, if any: another link store
/// (rule 6) or another comment store.
pub(crate) fn table_rule(table: &str) -> Option<&'static str> {
    let table = table.to_ascii_lowercase();
    if ["_links", "_references", "_relations"]
        .iter()
        .any(|suffix| table.ends_with(suffix))
    {
        return Some("link_tables");
    }
    if ["comment", "annotation", "remark", "note_thread"]
        .iter()
        .any(|word| table.contains(word))
    {
        return Some("comment_tables");
    }
    None
}

/// The tables a migration creates.
pub(crate) fn created_tables(sql: &str) -> Vec<String> {
    let normal = normalise(sql);
    let upper = normal.to_ascii_uppercase();
    upper
        .match_indices("CREATE TABLE ")
        .filter_map(|(index, found)| {
            let rest = normal[index + found.len()..].trim_start();
            let rest = rest
                .strip_prefix("IF NOT EXISTS ")
                .or_else(|| rest.strip_prefix("if not exists "))
                .unwrap_or(rest);
            let name = rest
                .split(|character: char| {
                    !(character.is_ascii_alphanumeric() || character == '_' || character == '.')
                })
                .next()?;
            (!name.is_empty()).then(|| name.rsplit('.').next().unwrap_or(name).to_owned())
        })
        .collect()
}

/// New tables, in migrations after the baseline, that break a table rule.
pub(crate) fn new_table_offences() -> Vec<String> {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut offences = Vec::new();
    for entry in std::fs::read_dir(&folder).expect("migrations folder") {
        let path = entry.expect("migration").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let Some(version) = name
            .split('_')
            .next()
            .and_then(|version| version.parse::<u64>().ok())
        else {
            continue;
        };
        if version <= TABLE_RULES_BASELINE {
            continue;
        }
        let sql = std::fs::read_to_string(&path).expect("read migration");
        for table in created_tables(&sql) {
            if let Some(rule) = table_rule(&table) {
                offences.push(format!(
                    "{rule}: {name} creates {table}. AGENTS.md, \"{AGENTS_HEADING}\", rule 6, \
                     \"Links and events are shared pieces\": add a kind to work_item_links or \
                     thing_references instead"
                ));
            }
        }
    }
    offences
}

/// The latest migration that defines `conversation_readable_by`, and its
/// definition.
pub(crate) fn latest_readable_by() -> Option<(String, String)> {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut found: Option<(u64, String, String)> = None;
    for entry in std::fs::read_dir(&folder).expect("migrations folder") {
        let path = entry.expect("migration").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let Some(version) = name
            .split('_')
            .next()
            .and_then(|version| version.parse::<u64>().ok())
        else {
            continue;
        };
        let sql = std::fs::read_to_string(&path).expect("read migration");
        if normalise(&sql).contains("FUNCTION conversation_readable_by(")
            && found
                .as_ref()
                .is_none_or(|(latest, _, _)| version > *latest)
        {
            found = Some((version, name, sql));
        }
    }
    found.map(|(_, name, sql)| (name, sql))
}

/// Why a definition of `conversation_readable_by` is more than one call to
/// `conversation_visible_to` with its own two arguments, or None when it is
/// exactly that (ruling R32).
pub(crate) fn readable_by_problem(sql: &str) -> Option<String> {
    let normal = normalise(sql);
    let start = normal.find("FUNCTION conversation_readable_by(")?
        + "FUNCTION conversation_readable_by(".len();
    let close = normal[start..].find(')')? + start;
    let arguments = normal[start..close]
        .split(',')
        .filter_map(|argument| argument.split_whitespace().next())
        .collect::<Vec<_>>();
    let [first, second] = arguments.as_slice() else {
        return Some(format!("it takes {} arguments, not two", arguments.len()));
    };
    let body_start = normal[close..].find("$$")? + close + 2;
    let body_end = normal[body_start..].find("$$")? + body_start;
    let body = normal[body_start..body_end]
        .trim()
        .trim_end_matches(';')
        .trim();
    let expected = format!("SELECT conversation_visible_to({first}, {second})");
    (body != expected).then(|| format!("its body is `{body}`, not `{expected}`"))
}

fn allowances() -> BTreeMap<String, BTreeMap<String, usize>> {
    serde_json::from_str(include_str!("structure_guards/allowances.json"))
        .expect("structure_guards/allowances.json is a map of rule to file to count")
}

/// Every rule's count per file over the server's source, test tails cut.
pub(crate) fn counts() -> BTreeMap<&'static str, BTreeMap<String, usize>> {
    let files = rust_files(&server_src());
    RULES
        .iter()
        .map(|rule| {
            let per_file = files
                .iter()
                .filter(|(path, _)| !SKIPPED.contains(&path.as_str()))
                .filter(|(path, _)| !exempt(rule.id, path))
                .filter_map(|(path, source)| {
                    let found = (rule.count)(strip_tests(source));
                    (found > 0).then(|| (path.clone(), found))
                })
                .collect();
            (rule.id, per_file)
        })
        .collect()
}

/// A rule's failures (a file above its allowance, allowances above the
/// ceiling) and hints (a file below its allowance) for one scan.
pub(crate) fn judge(
    rule: &Rule,
    per_file: &BTreeMap<String, usize>,
    allowed: &BTreeMap<String, usize>,
) -> (Vec<String>, Vec<String>) {
    let mut failures = Vec::new();
    let mut hints = Vec::new();
    let total: usize = allowed.values().sum();
    if total > rule.ceiling {
        failures.push(format!(
            "{}: the allowances add up to {total}, above the ceiling of {}; allowances only \
             go down (AGENTS.md, \"{AGENTS_HEADING}\", {})",
            rule.id, rule.ceiling, rule.agents_rule
        ));
    }
    for (file, found) in per_file {
        let allowance = allowed.get(file).copied().unwrap_or(0);
        if *found > allowance {
            failures.push(format!(
                "{}: {file} has {found}, allowed {allowance}. AGENTS.md, \"{AGENTS_HEADING}\", \
                 {}; don't raise the allowance",
                rule.id, rule.agents_rule
            ));
        }
    }
    for (file, allowance) in allowed {
        let found = per_file.get(file).copied().unwrap_or(0);
        if found < *allowance {
            hints.push(format!(
                "{}: you can lower {file} to {found} in allowances.json",
                rule.id
            ));
        }
    }
    (failures, hints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_keeps_within_its_allowances() {
        let allowances = allowances();
        for id in allowances.keys() {
            assert!(
                RULES.iter().any(|rule| rule.id == id),
                "allowances.json names {id}, which is no rule"
            );
        }
        let counts = counts();
        let mut failures = Vec::new();
        let mut hints = Vec::new();
        for rule in RULES {
            let per_file = counts.get(rule.id).cloned().unwrap_or_default();
            let allowed = allowances.get(rule.id).cloned().unwrap_or_default();
            let (failed, hinted) = judge(rule, &per_file, &allowed);
            failures.extend(failed);
            hints.extend(hinted);
        }
        for hint in &hints {
            eprintln!("hint: {hint}");
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// Prints today's counts in the allowances format. Run with
    /// `cargo test -p agent-workspace-server print_structure_counts -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints counts for writing allowances"]
    fn print_structure_counts() {
        println!(
            "{}",
            serde_json::to_string_pretty(&counts()).expect("serialise counts")
        );
    }

    #[test]
    fn no_new_link_or_comment_tables() {
        let offences = new_table_offences();
        assert!(offences.is_empty(), "{}", offences.join("\n"));
    }

    /// `conversation_readable_by` adds no rule of its own (ruling R32).
    #[test]
    fn conversation_readable_by_is_one_call() {
        let (name, sql) =
            latest_readable_by().expect("a migration defines conversation_readable_by");
        if let Some(problem) = readable_by_problem(&sql) {
            panic!(
                "conversation_readable_by in {name}: {problem}. It stays one call to \
                 conversation_visible_to; change the rule there (AGENTS.md, \"{AGENTS_HEADING}\", rule 7)"
            );
        }
        let wrapper = "CREATE FUNCTION conversation_readable_by(c UUID, a UUID) RETURNS BOOLEAN \
                       LANGUAGE sql STABLE AS $$ SELECT conversation_visible_to(c, a) $$;";
        assert_eq!(readable_by_problem(wrapper), None);
        let drifted = wrapper.replace(
            "conversation_visible_to(c, a)",
            "conversation_visible_to(c, a) AND actor_is_admin(a)",
        );
        assert!(readable_by_problem(&drifted).is_some());
        let swapped = wrapper.replace(
            "conversation_visible_to(c, a)",
            "conversation_visible_to(a, c)",
        );
        assert!(readable_by_problem(&swapped).is_some());
    }

    #[test]
    fn table_rules_catch_new_stores() {
        let sql = "CREATE TABLE IF NOT EXISTS task_links (id uuid);\n\
                   create table page_annotations (id uuid); CREATE TABLE settings (id uuid);";
        let tables = created_tables(sql);
        assert_eq!(tables, ["task_links", "page_annotations", "settings"]);
        assert_eq!(table_rule("task_links"), Some("link_tables"));
        assert_eq!(table_rule("record_references"), Some("link_tables"));
        assert_eq!(table_rule("page_annotations"), Some("comment_tables"));
        assert_eq!(table_rule("settings"), None);
    }

    #[test]
    fn each_rule_matches_its_pattern_and_not_its_neighbours() {
        let literal = |sql: &str| format!("let q = r#\"{sql}\"#;");
        assert_eq!(
            raw_role_checks(&literal("WHERE m.role IN ('owner', 'admin')")),
            1
        );
        assert_eq!(
            raw_role_checks(&literal("WHERE role = 'viewer' OR role <> 'admin'")),
            2
        );
        assert_eq!(raw_role_checks(&literal("WHERE role_id = $1")), 0);
        assert_eq!(
            custom_role_allows("let x = \"custom_role_allows(m.role_id, 'tasks.view')\";"),
            1
        );
        assert_eq!(custom_role_allows("// custom_role_allows(m.role_id)"), 0);
        assert_eq!(
            workspace_role_enum("if role.manages() { WorkspaceRole::Owner }"),
            2
        );
        assert_eq!(workspace_role_enum("let role = MembershipRole::Owner;"), 0);
        assert_eq!(
            account_kind_checks("if actor.kind == ActorKind::Human {"),
            1
        );
        assert_eq!(account_kind_checks("let q = \"WHERE kind = 'human'\";"), 1);
        assert_eq!(
            account_kind_checks(
                "if actor.kind == ActorKind::Human { // kind-check: mechanical (sign-in)"
            ),
            0
        );
        assert_eq!(account_kind_checks("let kind = ActorKind::Human;"), 0);
        assert_eq!(
            agent_runs_sql(&literal(
                "SELECT 1 FROM agent_runs run JOIN agent_approvals a"
            )),
            2
        );
        assert_eq!(
            agent_runs_sql(&literal("SELECT 1 FROM agent_runs_archive")),
            0
        );
        assert_eq!(
            active_run_states_literal("\"state IN ('created', 'queued', 'starting')\""),
            1
        );
        assert_eq!(active_run_states_literal("runs::ACTIVE_RUN_STATES"), 0);
        assert_eq!(
            conversation_inserts(&literal("INSERT INTO conversations (id) VALUES ($1)")),
            1
        );
        assert_eq!(
            conversation_inserts(&literal("INSERT INTO conversation_members (id)")),
            0
        );
        assert_eq!(
            message_inserts(&literal("INSERT INTO messages (id) VALUES ($1)")),
            1
        );
        assert_eq!(
            message_inserts(&literal("INSERT INTO message_reactions (id)")),
            0
        );
        assert_eq!(
            task_core_updates(&literal("UPDATE work_items SET title = $2")),
            1
        );
        assert_eq!(
            task_core_updates(&literal("UPDATE work_item_links SET kind = $2")),
            0
        );
        assert_eq!(
            direct_tool_calls("let x = call_tool_direct(state, name).await;"),
            1
        );
        assert_eq!(
            direct_tool_calls("pub(crate) async fn call_tool_direct("),
            0
        );
        assert_eq!(
            process_state("static CACHE: LazyLock<Mutex<HashMap<Uuid, u8>>> = x;"),
            1
        );
        assert_eq!(process_state("let guard = state.lock.lock();"), 0);
        assert_eq!(
            background_spawns("tokio::spawn(async move { work().await });"),
            1
        );
        assert_eq!(
            background_spawns("tokio::task::spawn_blocking(move || x);"),
            0
        );
        assert_eq!(vendor_names_core("let label = \"Ask Claude\";"), 1);
        assert_eq!(vendor_names_core("let label = \"Ask the agent\";"), 0);
        assert_eq!(
            notification_detail_text(
                "notification.detail = serde_json::json!({\n    \"title\": \"New comment\",\n    \"preview\": preview,\n    \"href\": href,\n});"
            ),
            1
        );
        assert_eq!(
            notification_detail_text(
                "detail: json!({\n    \"body\": format!(\"{} said\", who),\n    \"quote\": \"fixed\",\n    \"text\": comment.id,\n}),\nlet other = json!({ \"preview\": preview });"
            ),
            1
        );
        assert_eq!(
            notification_detail_text(
                "let detail = json!({\n    \"pageCommentId\": id,\n    \"href\": href,\n});\n\"preview\": text,"
            ),
            0
        );
        assert!(exempt("vendor_names_core", "provider/codex_cloud.rs"));
        assert!(!exempt("vendor_names_core", "mcp.rs"));
    }

    /// A new offender in a file with no allowance fails, naming the rule and
    /// the AGENTS.md heading; a file under its allowance passes.
    #[test]
    fn a_new_offender_fails_and_a_lower_count_passes() {
        let rule = RULES
            .iter()
            .find(|rule| rule.id == "raw_role_checks")
            .expect("rule");
        let scratch =
            "let q = \"SELECT 1 FROM organization_memberships WHERE role IN ('owner','admin')\";";
        let allowed = BTreeMap::new();
        let found = (rule.count)(scratch);
        let failures = judge(
            rule,
            &BTreeMap::from([("scratch.rs".to_owned(), found)]),
            &allowed,
        );
        assert_eq!(failures.0.len(), 1, "{failures:?}");
        assert!(failures.0[0].contains("raw_role_checks"), "{failures:?}");
        assert!(failures.0[0].contains(AGENTS_HEADING), "{failures:?}");
        let allowed = BTreeMap::from([("scratch.rs".to_owned(), 3)]);
        let failures = judge(
            rule,
            &BTreeMap::from([("scratch.rs".to_owned(), found)]),
            &allowed,
        );
        assert!(failures.0.is_empty(), "{failures:?}");
        assert_eq!(failures.1.len(), 1, "a hint to lower it: {failures:?}");
    }

    #[test]
    fn conversation_listings_counts_unfiltered_reads() {
        let unfiltered = "let x = r#\"SELECT name FROM conversations WHERE project_id = $1\"#;";
        let filtered = "let x = r#\"SELECT name FROM conversations c WHERE conversation_visible_to(c.id, $2)\"#;";
        let explained = "let x = \"SELECT 1 FROM messages -- visibility: by id after conversation_access\n WHERE id = $1\";";
        let other_table = "let x = \"SELECT 1 FROM conversation_entries WHERE id = $1\";";
        assert_eq!(conversation_listings(unfiltered), 1);
        assert_eq!(conversation_listings(filtered), 0);
        assert_eq!(conversation_listings(explained), 0);
        assert_eq!(conversation_listings(other_table), 0);
    }
}
