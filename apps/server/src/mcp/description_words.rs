//! K2.1: what a tool's description may say. Every agent reads these first,
//! so they name no model or harness vendor (principle 6) and state no rule by
//! kind of account ("people only", "agents can't"): what an account may do
//! comes from its roles, and the generated permission text says so.
//!
//! `KNOWN` lists the descriptions that still break the rule today, each with
//! the slice that rewrites it. A new tool or a new sentence that breaks it
//! fails; the list only shrinks.

use std::collections::BTreeMap;

/// Model and harness vendors. Code hosts are named by their own tools
/// (`github_*`, `pull_request_*`) until A1's code-host seam.
const VENDORS: &[&str] = &[
    "codex",
    "claude",
    "openai",
    "anthropic",
    "gemini",
    "chatgpt",
    "gpt-",
    "copilot",
];

/// Rules by kind of account, which K1 replaces with role settings.
const KIND_RULES: &[&str] = &[
    "people only",
    "only people",
    "person only",
    "agents can't",
    "agents cannot",
    "never agents",
    "agents only",
];

/// Today's offenders: tool, the words it still uses, and who removes them.
/// P0.32 rewrites the vendor names (K2.1's soft need); K1's slices replace
/// the kind rules the descriptions report.
const KNOWN: &[(&str, &[&str])] = &[
    ("agent_list", &["codex"]),                   // P0.32
    ("approval_get", &["codex"]),                 // P0.32
    ("approval_list", &["codex", "people only"]), // P0.32, K1.13
    ("approval_respond", &["codex"]),             // P0.32
    ("conversation_continue", &["people only"]),  // K1.13
    ("conversation_update", &["codex"]),          // P0.32
    ("environment_list", &["codex"]),             // P0.32
    ("project_archive", &["codex"]),              // P0.32
    ("project_create", &["people only"]),         // K1.13
    ("repository_list", &["codex"]),              // P0.32
    ("run_get", &["codex"]),                      // P0.32
    ("run_list", &["codex"]),                     // P0.32
    ("run_request", &["codex"]),                  // P0.32
    ("run_start", &["codex"]),                    // P0.32
    ("session_bind", &["codex", "claude"]),       // P0.32
    ("settings_set", &["agents only"]),           // K1.13
    ("task_link", &["codex"]),                    // P0.32
    ("workspace_update", &["agents can't"]),      // K1.13
];

/// The rule's words found in one description, lower-cased.
pub(crate) fn offending_words(description: &str) -> Vec<&'static str> {
    let text = description.to_lowercase();
    VENDORS
        .iter()
        .chain(KIND_RULES)
        .copied()
        .filter(|word| text.contains(word))
        .collect()
}

#[test]
fn descriptions_name_no_vendor_or_kind_rule() {
    let known = KNOWN
        .iter()
        .map(|(tool, words)| (*tool, *words))
        .collect::<BTreeMap<_, _>>();
    let mut failures = Vec::new();
    let mut fixed = Vec::new();
    for tool in super::tool_catalog() {
        let description = tool.description.as_deref().unwrap_or_default();
        let found = offending_words(description);
        let allowed = known.get(tool.name.as_ref()).copied().unwrap_or_default();
        let new = found
            .iter()
            .filter(|word| !allowed.contains(word))
            .collect::<Vec<_>>();
        if !new.is_empty() {
            failures.push(format!("{}: {new:?}", tool.name));
        }
        if allowed.iter().any(|word| !found.contains(word)) {
            fixed.push(tool.name.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "tool descriptions name a vendor or a rule by kind of account (say what the tool does, \
         and let the role's permission decide who may): {failures:?}"
    );
    // Rewritten by another branch (P0.32): fine, take the line out of KNOWN.
    if !fixed.is_empty() {
        eprintln!("description_words: KNOWN can lose {fixed:?}");
    }
}

#[test]
fn the_word_rule_catches_a_vendor_and_a_kind_rule() {
    assert_eq!(offending_words("Start Codex in a thread."), ["codex"]);
    assert_eq!(offending_words("People only: approve it."), ["people only"]);
    assert!(offending_words("Start a run in a thread. Needs runs.launch.").is_empty());
}
