//! H5: what a step did, as the payload a run stores. Harness adapters
//! report steps (commands, file changes, other tools) and the agent's own
//! notes as neutral events; these turn them into payloads. Every string is
//! redacted first, and no payload passes [`MAX_PAYLOAD_BYTES`]: command
//! output can carry anything a command printed, keys included.

use serde_json::{Map, Value, json};

use crate::harness::adapter::{HarnessItem, ItemDetail, ItemKind, TestCounts};
use agent_workspace_redaction::{
    MAX_COMMAND_CHARS, MAX_FILES_PER_STEP, MAX_NOTE_CHARS, MAX_OUTPUT_CHARS, MAX_PAYLOAD_BYTES,
    cap, clean, redact,
};

// ---------------------------------------------------------------------------
// Tests a command ran

/// Counts tests from the summary line common runners print: cargo, Jest
/// and Vitest, pytest, Go and Mocha. `None` when nothing looks like one.
pub fn detect_tests(output: &str) -> Option<TestCounts> {
    let mut found = None::<TestCounts>;
    for line in output.lines().rev().take(400) {
        let lower = line.trim().to_ascii_lowercase();
        // The first place `word` follows a number ("FAILED. 2 passed; 1
        // failed" names "failed" twice).
        let number_before = |word: &str| -> Option<u32> {
            lower.match_indices(word).find_map(|(at, _)| {
                lower[..at]
                    .trim_end()
                    .rsplit(|c: char| !c.is_ascii_digit())
                    .next()
                    .filter(|digits| !digits.is_empty())?
                    .parse()
                    .ok()
            })
        };
        let looks_like_summary = lower.starts_with("test result:")
            || lower.starts_with("tests:")
            || lower.starts_with("tests ")
            || (lower.starts_with('=') && lower.contains(" in "))
            || lower.contains(" passing");
        if !looks_like_summary {
            continue;
        }
        let passed = number_before(" passed").or_else(|| number_before(" passing"));
        let failed = number_before(" failed").or_else(|| number_before(" failing"));
        let skipped = number_before(" skipped")
            .or_else(|| number_before(" ignored"))
            .or_else(|| number_before(" pending"));
        if passed.is_none() && failed.is_none() {
            continue;
        }
        let counts = found.get_or_insert_with(TestCounts::default);
        // Cargo prints one summary per test binary; add them up.
        counts.passed += passed.unwrap_or(0);
        counts.failed += failed.unwrap_or(0);
        counts.skipped += skipped.unwrap_or(0);
        if !lower.starts_with("test result:") {
            break;
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Payloads

/// The stored payload of one step: redacted, capped, and only the fields
/// the run page shows.
pub fn item_payload(item: &HarnessItem) -> Value {
    detail_payload(item.kind, &item.detail)
}

pub fn detail_payload(kind: ItemKind, detail: &ItemDetail) -> Value {
    let mut payload = Map::new();
    if let Some(tool) = &detail.tool {
        payload.insert("tool".to_owned(), json!(clean(tool, 120)));
    }
    if let Some(command) = &detail.command {
        payload.insert(
            "command".to_owned(),
            json!(clean(command, MAX_COMMAND_CHARS)),
        );
    }
    if let Some(code) = detail.exit_code {
        payload.insert("exitCode".to_owned(), json!(code));
    }
    if let Some(output) = detail.output.as_deref().filter(|output| !output.is_empty()) {
        payload.insert("output".to_owned(), json!(clean(output, MAX_OUTPUT_CHARS)));
    }
    if !detail.files.is_empty() {
        payload.insert(
            "files".to_owned(),
            Value::Array(
                detail
                    .files
                    .iter()
                    .take(MAX_FILES_PER_STEP)
                    .map(|(path, change)| json!({ "path": clean(path, 500), "change": change }))
                    .collect(),
            ),
        );
    }
    let tests = detail.tests.or_else(|| {
        (kind == ItemKind::CommandExecution)
            .then(|| detail.output.as_deref().and_then(detect_tests))
            .flatten()
    });
    if let Some(tests) = tests {
        payload.insert(
            "tests".to_owned(),
            json!({ "passed": tests.passed, "failed": tests.failed, "skipped": tests.skipped }),
        );
    }
    if detail.failed {
        payload.insert("failed".to_owned(), json!(true));
    }
    fit(Value::Object(payload))
}

pub fn note_payload(text: &str) -> Value {
    fit(json!({ "text": clean(text, MAX_NOTE_CHARS) }))
}

/// Drops the output, then everything, rather than store an oversized row.
fn fit(mut payload: Value) -> Value {
    let size = |value: &Value| serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
    if size(&payload) <= MAX_PAYLOAD_BYTES {
        return payload;
    }
    if let Some(object) = payload.as_object_mut() {
        object.remove("output");
        object.insert("truncated".to_owned(), json!(true));
    }
    if size(&payload) <= MAX_PAYLOAD_BYTES {
        return payload;
    }
    json!({ "truncated": true })
}

/// A one-line summary of a step for the thread and the timeline.
pub fn step_summary(kind: &str, detail: &ItemDetail) -> String {
    let line = |text: &str| {
        let first = redact(text.lines().next().unwrap_or_default());
        cap(first.trim(), 160)
    };
    match kind {
        "command_started" => detail.command.as_deref().map_or_else(
            || "The agent started a command in the isolated worktree.".to_owned(),
            |command| format!("Running {}", line(command)),
        ),
        "command_completed" => match (detail.command.as_deref(), detail.exit_code) {
            (Some(command), Some(code)) => format!("Ran {} (exit {code})", line(command)),
            (Some(command), None) => format!("Ran {}", line(command)),
            _ => "The agent completed a command in the isolated worktree.".to_owned(),
        },
        "file_change" => match detail.files.len() {
            0 => "The agent completed a file change in the isolated worktree.".to_owned(),
            1 => format!("Changed {}", line(&detail.files[0].0)),
            count => format!("Changed {count} files"),
        },
        "tool_used" => format!("Used {}", line(detail.tool.as_deref().unwrap_or("a tool"))),
        _ => String::new(),
    }
}
