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
    contains_word, normalise, rust_files, server_src, string_literals, strip_tests,
};

/// One rule: its guard id, the `AGENTS.md` rule it enforces, the most its
/// allowances may add up to, and how to count offences in one file.
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) agents_rule: &'static str,
    pub(crate) ceiling: usize,
    pub(crate) count: fn(&str) -> usize,
}

pub(crate) const RULES: &[Rule] = &[Rule {
    id: "conversation_listings",
    agents_rule: "rule 7, \"Every conversation read goes through conversation_visible_to\"",
    ceiling: CONVERSATION_LISTINGS_CEILING,
    count: conversation_listings,
}];

const CONVERSATION_LISTINGS_CEILING: usize = 80;

/// Files the guards never read: the guards themselves and the scanner, whose
/// samples would count against them.
const SKIPPED: &[&str] = &["structure_guards.rs", "test_support/source.rs"];

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
                .filter_map(|(path, source)| {
                    let found = (rule.count)(strip_tests(source));
                    (found > 0).then(|| (path.clone(), found))
                })
                .collect();
            (rule.id, per_file)
        })
        .collect()
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
        let mut failures = Vec::new();
        let mut hints = Vec::new();
        for (rule, per_file) in RULES.iter().zip(counts().into_values()) {
            let allowed = allowances.get(rule.id).cloned().unwrap_or_default();
            let total: usize = allowed.values().sum();
            if total > rule.ceiling {
                failures.push(format!(
                    "{}: the allowances add up to {total}, above the ceiling of {}; \
                     allowances only go down ({})",
                    rule.id, rule.ceiling, rule.agents_rule
                ));
            }
            for (file, found) in &per_file {
                let allowance = allowed.get(file).copied().unwrap_or(0);
                if *found > allowance {
                    failures.push(format!(
                        "{}: {file} has {found}, allowed {allowance}. AGENTS.md {}; \
                         don't raise the allowance",
                        rule.id, rule.agents_rule
                    ));
                }
            }
            for (file, allowance) in &allowed {
                let found = per_file.get(file).copied().unwrap_or(0);
                if found < *allowance {
                    hints.push(format!(
                        "{}: you can lower {file} to {found} in allowances.json",
                        rule.id
                    ));
                }
            }
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
