//! A provider that answers from simple rules instead of a model, so tests
//! (and a development server with no key) exercise the whole path. It's
//! only offered outside production.
//!
//! It reads the same input a real model gets and writes a reply that fits
//! the prompt's schema: a task whose text names another task's key is
//! blocked by it, near-identical titles are duplicates, "waiting on" in a
//! message marks a task blocked, and so on. An input with `"fake":
//! "invalid_first"` gets a reply that breaks the schema until the gateway
//! retries; `"invalid_always"` never fits.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{Completion, CompletionRequest, ProviderError, ProviderManifest};
use crate::provider::manifest::ModelEntry;

/// The gateway adds this to the instructions when it retries after a reply
/// that didn't fit the schema.
pub const RETRY_NOTE: &str = "Your previous reply didn't match the required JSON Schema";

pub fn manifest() -> ProviderManifest {
    ProviderManifest {
        id: "fake",
        display_name: "Test provider",
        description: "Answers from simple rules, with no model and no key. For tests and development only.",
        needs_key: false,
        needs_base_url: false,
        default_base_url: None,
        default_quick_model: "rules",
        default_deep_model: "rules",
        models: vec![ModelEntry {
            id: "rules".to_owned(),
            display_name: "Rules".to_owned(),
            is_default: true,
            ..ModelEntry::default()
        }],
        testing_only: true,
    }
}

pub fn complete(request: &CompletionRequest<'_>) -> Result<Completion, ProviderError> {
    let input: Value = serde_json::from_str(request.input)
        .map_err(|_| ProviderError::BadRequest("the input isn't JSON".to_owned()))?;
    let retried = request.system.contains(RETRY_NOTE);
    let reply = match input.get("fake").and_then(Value::as_str) {
        Some("invalid_always") => json!({"unexpected": true}),
        Some("invalid_first") if !retried => json!({"unexpected": true}),
        Some("unavailable") => return Err(ProviderError::Unavailable("HTTP 503".to_owned())),
        _ => answer(request.prompt_id, &input)?,
    };
    let text = reply.to_string();
    Ok(Completion {
        input_tokens: i64::try_from(request.system.len() + request.input.len()).unwrap_or(0) / 4,
        output_tokens: i64::try_from(text.len()).unwrap_or(0) / 4,
        text,
    })
}

fn answer(prompt_id: &str, input: &Value) -> Result<Value, ProviderError> {
    Ok(match prompt_id {
        "ping" => json!({"ok": true}),
        "dependency_judge" => dependency_judge(input),
        "conversation_blockers" => conversation_blockers(input),
        "triage" => triage(input),
        "plan_to_tasks" => plan_to_tasks(input),
        "criteria" => criteria(input),
        "summary" => summary(input),
        _ => return Err(ProviderError::BadRequest("unknown prompt".to_owned())),
    })
}

fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn list(value: &Value, field: &str) -> Vec<Value> {
    value
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "add", "make", "when", "can",
    "are", "our", "its", "not", "all", "new",
];

/// The significant words of a text, lower case, with a plural "s" or "es"
/// taken off.
pub fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.len() >= 3 && !STOP_WORDS.contains(&word.as_str()))
        .map(|word| {
            if word.len() > 5 && word.ends_with("es") {
                word[..word.len() - 2].to_owned()
            } else if word.len() > 4 && word.ends_with('s') && !word.ends_with("ss") {
                word[..word.len() - 1].to_owned()
            } else {
                word
            }
        })
        .collect()
}

/// How alike two titles are: shared words over all words.
pub fn likeness(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    if all == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / all as f64
}

fn mentions(haystack: &str, key: &str) -> bool {
    !key.is_empty() && haystack.to_lowercase().contains(&key.to_lowercase())
}

fn dependency_judge(input: &Value) -> Value {
    let tasks = list(input, "tasks")
        .into_iter()
        .map(|task| (text(&task, "key"), task))
        .collect::<HashMap<_, _>>();
    let verdicts = list(input, "pairs")
        .iter()
        .map(|pair| {
            let index = pair.get("pair").and_then(Value::as_i64).unwrap_or(0);
            let (a_key, b_key) = (text(pair, "a"), text(pair, "b"));
            let (Some(a), Some(b)) = (tasks.get(&a_key), tasks.get(&b_key)) else {
                return json!({"pair": index, "kind": "none", "confidence": 0.9, "reason": "Not enough to go on."});
            };
            let a_text = format!("{} {}", text(a, "title"), text(a, "description"));
            let b_text = format!("{} {}", text(b, "title"), text(b, "description"));
            let shared_labels = list(a, "labels")
                .iter()
                .any(|label| list(b, "labels").contains(label));
            let same_milestone = !a["milestone"].is_null() && a["milestone"] == b["milestone"];
            let (kind, confidence, reason) = if mentions(&b_text, &a_key) {
                ("a_blocks_b", 0.92, format!("{b_key} says it needs {a_key} first."))
            } else if mentions(&a_text, &b_key) {
                ("b_blocks_a", 0.92, format!("{a_key} says it needs {b_key} first."))
            } else if likeness(&text(a, "title"), &text(b, "title")) >= 0.6 {
                ("duplicates", 0.8, "Both titles ask for the same work.".to_owned())
            } else if shared_labels && same_milestone {
                ("relates_to", 0.6, "Same milestone and labels.".to_owned())
            } else {
                ("none", 0.9, "No connection in the text.".to_owned())
            };
            json!({"pair": index, "kind": kind, "confidence": confidence, "reason": reason})
        })
        .collect::<Vec<_>>();
    json!({"verdicts": verdicts})
}

const WAITING: &[&str] = &["waiting on", "waiting for", "blocked by", "blocked on"];

fn conversation_blockers(input: &Value) -> Value {
    let keys = list(input, "tasks")
        .iter()
        .map(|task| text(task, "key"))
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    let mut findings = Vec::new();
    for message in list(input, "messages") {
        let body = text(&message, "text");
        let lower = body.to_ascii_lowercase();
        let Some((phrase_at, phrase)) = WAITING
            .iter()
            .filter_map(|phrase| lower.find(phrase).map(|at| (at, *phrase)))
            .min()
        else {
            continue;
        };
        let after = &body[phrase_at + phrase.len()..];
        let before = &body[..phrase_at];
        let blocked = message
            .get("linkedTask")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| keys.iter().find(|key| mentions(before, key)).cloned());
        let Some(blocked) = blocked else {
            continue;
        };
        let blocker = keys
            .iter()
            .find(|key| **key != blocked && mentions(after, key))
            .cloned();
        let wait = after
            .trim()
            .trim_end_matches(['.', '!'])
            .chars()
            .take(150)
            .collect::<String>();
        findings.push(json!({
            "message": message.get("index").and_then(Value::as_i64).unwrap_or(0),
            "task": blocked,
            "kind": if blocker.is_some() { "blocks" } else { "blocked_reason" },
            "blocker": blocker,
            "reason": format!("Waiting on {wait}."),
            "confidence": 0.85,
        }));
    }
    json!({"findings": findings})
}

const BUG_WORDS: &[&str] = &["bug", "crash", "error", "broken", "fail", "failing"];
const URGENT_WORDS: &[&str] = &["crash", "outage", "security", "down", "loss"];

fn triage(input: &Value) -> Value {
    let labels = list(input, "labels");
    let types = list(input, "types");
    let similar = list(input, "similar");
    let tasks = list(input, "newTasks")
        .iter()
        .map(|task| {
            let key = text(task, "key");
            let body = format!("{} {}", text(task, "title"), text(task, "description"));
            let found = words(&body);
            let chosen_labels = labels
                .iter()
                .filter_map(Value::as_str)
                .filter(|label| {
                    let label_words = words(label);
                    !label_words.is_empty() && label_words.is_subset(&found)
                })
                .take(3)
                .collect::<Vec<_>>();
            let is_bug = BUG_WORDS.iter().any(|word| found.contains(*word));
            let bug_type = types
                .iter()
                .filter_map(Value::as_str)
                .find(|name| name.eq_ignore_ascii_case("bug"));
            let urgent = URGENT_WORDS.iter().any(|word| found.contains(*word));
            let duplicate = similar
                .iter()
                .filter(|other| text(other, "forTask") == key)
                .find(|other| likeness(&text(task, "title"), &text(other, "title")) >= 0.5)
                .map(|other| text(other, "key"));
            json!({
                "task": key,
                "labels": chosen_labels,
                "priority": if urgent { json!("high") } else { Value::Null },
                "type": if is_bug { json!(bug_type) } else { Value::Null },
                "parent": Value::Null,
                "duplicateOf": duplicate,
                "confidence": 0.8,
                "reason": "Matched words in the title and description.",
            })
        })
        .collect::<Vec<_>>();
    json!({"tasks": tasks})
}

fn plan_to_tasks(input: &Value) -> Value {
    let plan = text(input, "plan");
    let mut milestones: Vec<Value> = Vec::new();
    let mut tasks: Vec<Value> = Vec::new();
    let mut last_top: Option<String> = None;
    let mut previous_at_level: HashMap<usize, String> = HashMap::new();
    for line in plan.lines() {
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            if heading.is_empty() || milestones.len() >= 50 {
                continue;
            }
            let (name, date) = match heading.find("(by ") {
                Some(at) => (
                    heading[..at].trim().to_owned(),
                    heading[at + 4..].trim_end_matches(')').trim().to_owned(),
                ),
                None => (heading.to_owned(), String::new()),
            };
            let valid_date = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_ok();
            milestones.push(json!({
                "ref": format!("m{}", milestones.len() + 1),
                "name": name.chars().take(120).collect::<String>(),
                "description": "",
                "targetDate": if valid_date { json!(date) } else { Value::Null },
            }));
            last_top = None;
            previous_at_level.clear();
            continue;
        }
        let item = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| {
                trimmed
                    .split_once(". ")
                    .filter(|(number, _)| number.chars().all(|c| c.is_ascii_digit()))
                    .map(|(_, rest)| rest)
            });
        let Some(item) = item.map(str::trim).filter(|item| !item.is_empty()) else {
            continue;
        };
        if tasks.len() >= 100 {
            break;
        }
        let level = usize::from(indent >= 2);
        let reference = format!("t{}", tasks.len() + 1);
        let parent = if level == 1 { last_top.clone() } else { None };
        let blocked_by = if item.to_lowercase().contains("after") {
            previous_at_level.get(&level).cloned().into_iter().collect()
        } else {
            Vec::new()
        };
        tasks.push(json!({
            "ref": reference,
            "title": item.chars().take(120).collect::<String>(),
            "description": "",
            "criteria": [],
            "labels": [],
            "priority": "no_priority",
            "milestone": milestones.last().map(|milestone| milestone["ref"].clone()),
            "parent": parent,
            "blockedBy": blocked_by,
        }));
        if level == 0 {
            last_top = Some(reference.clone());
        }
        previous_at_level.insert(level, reference);
    }
    json!({"milestones": milestones, "tasks": tasks})
}

fn criteria(input: &Value) -> Value {
    let task = input.get("task").cloned().unwrap_or(Value::Null);
    let title = text(&task, "title");
    let existing = list(input, "existing")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect::<HashSet<_>>();
    let criteria = [
        format!("{} works as the task describes", title.trim()),
        "The cases the description mentions are handled".to_owned(),
        "A test covers the change".to_owned(),
    ]
    .into_iter()
    .filter(|criterion| !existing.contains(&criterion.to_lowercase()))
    .map(|criterion| criterion.chars().take(200).collect::<String>())
    .collect::<Vec<_>>();
    json!({"criteria": if criteria.is_empty() { vec!["Done as described".to_owned()] } else { criteria }})
}

fn summary(input: &Value) -> Value {
    let entries = list(input, "entries");
    let lines = entries
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(|entry| {
            let line = format!("{}: {}", text(entry, "author"), text(entry, "text"));
            line.chars().take(200).collect::<String>()
        })
        .collect::<Vec<_>>();
    let thread = text(input, "kind") == "thread";
    let outcome = entries
        .last()
        .filter(|_| thread)
        .map(|entry| text(entry, "text").chars().take(200).collect::<String>());
    json!({
        "lines": if lines.is_empty() { vec!["Nothing has happened yet.".to_owned()] } else { lines },
        "outcome": outcome,
        "status": if thread { json!("Open") } else { Value::Null },
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ai::{prompts, schema};

    fn run(prompt: &prompts::Prompt, input: &Value, system: &str) -> Result<Value, ProviderError> {
        let input = input.to_string();
        let completion = complete(&CompletionRequest {
            model: "rules",
            system,
            input: &input,
            prompt_id: prompt.id,
            schema: prompt.schema(),
            max_output_tokens: prompt.max_output_tokens,
            api_key: None,
            base_url: None,
            timeout: Duration::from_secs(1),
        })?;
        Ok(serde_json::from_str(&completion.text).expect("JSON"))
    }

    #[test]
    fn every_fixture_input_gets_a_reply_that_fits_its_schema() {
        for prompt in prompts::all() {
            let reply = run(prompt, &prompt.fixture()["input"], prompt.system).expect("reply");
            assert_eq!(
                schema::validate(prompt.schema(), &reply),
                None,
                "{}: {reply}",
                prompt.id
            );
        }
    }

    #[test]
    fn the_rules_find_what_the_fixtures_show() {
        let judged = run(
            &prompts::DEPENDENCY_JUDGE,
            &prompts::DEPENDENCY_JUDGE.fixture()["input"],
            "",
        )
        .expect("judge");
        assert_eq!(judged["verdicts"][0]["kind"], "a_blocks_b");
        assert_eq!(judged["verdicts"][1]["kind"], "duplicates");

        let found = run(
            &prompts::CONVERSATION_BLOCKERS,
            &prompts::CONVERSATION_BLOCKERS.fixture()["input"],
            "",
        )
        .expect("blockers");
        assert_eq!(found["findings"][0]["blocker"], "ACME-1");
        assert_eq!(found["findings"][1]["kind"], "blocked_reason");
        assert_eq!(found["findings"][1]["task"], "ACME-4");

        let triaged =
            run(&prompts::TRIAGE, &prompts::TRIAGE.fixture()["input"], "").expect("triage");
        assert_eq!(triaged["tasks"][0]["type"], "Bug");
        assert_eq!(triaged["tasks"][0]["duplicateOf"], "ACME-5");

        let plan = run(
            &prompts::PLAN_TO_TASKS,
            &prompts::PLAN_TO_TASKS.fixture()["input"],
            "",
        )
        .expect("plan");
        assert_eq!(plan["milestones"][0]["targetDate"], "2026-12-01");
        assert_eq!(plan["tasks"][1]["blockedBy"][0], "t1");
    }

    #[test]
    fn invalid_first_fits_only_after_the_retry_note() {
        let input = json!({"fake": "invalid_first"});
        let first = run(&prompts::PING, &input, "check").expect("first");
        assert!(schema::validate(prompts::PING.schema(), &first).is_some());
        let second = run(&prompts::PING, &input, &format!("check\n{RETRY_NOTE}")).expect("second");
        assert_eq!(schema::validate(prompts::PING.schema(), &second), None);
    }
}
