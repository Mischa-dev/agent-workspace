//! What agents hear (A2 3.4): the log's event types a connection can be
//! told about, by family. Only these are delivered; the rest of the log
//! (read marks, live saves, board signals) is for browsers and consumers.
//!
//! `feed_name` is the name the older agent event feed used for the same
//! happening, which `external_event_poll` still answers with.

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

/// One deliverable event type.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The log's name, such as `reply.requested`.
    pub kind: &'static str,
    /// What it is about: `reply`, `message`, `task`, `request`, `run`,
    /// `page`, `sheet`, `whiteboard`, `record`, `pull_request`, `schedule` or
    /// `connection`.
    pub family: &'static str,
    /// Addressed to one account (heard whatever it subscribes to).
    pub addressed: bool,
    /// What happened, in a few words.
    pub summary: &'static str,
    /// The older feed's name for it (`external_event_poll`).
    #[serde(skip)]
    pub feed_name: &'static str,
}

const fn entry(
    kind: &'static str,
    family: &'static str,
    addressed: bool,
    summary: &'static str,
    feed_name: &'static str,
) -> Entry {
    Entry {
        kind,
        family,
        addressed,
        summary,
        feed_name,
    }
}

/// Every type delivered to agents.
pub const ENTRIES: &[Entry] = &[
    // Addressed: always heard by the account they name.
    entry(
        "reply.requested",
        "reply",
        true,
        "You were asked to reply",
        "reply.requested",
    ),
    entry(
        "reply.followup",
        "reply",
        true,
        "Someone followed up where you are working",
        "reply.followup",
    ),
    entry(
        "reply.cancelled",
        "reply",
        true,
        "Someone stopped a reply you were asked for",
        "reply.cancelled",
    ),
    entry(
        "message.mentioned",
        "message",
        true,
        "You were mentioned in a message",
        "message.mentioned",
    ),
    entry(
        "task.assigned",
        "task",
        true,
        "A task was assigned to you",
        "work_item.assigned",
    ),
    entry(
        "comment.mentioned",
        "task",
        true,
        "You were mentioned in a comment",
        "comment.mentioned",
    ),
    entry(
        "run.start",
        "run",
        true,
        "You were given a run to start",
        "run.start",
    ),
    entry(
        "run.cancel",
        "run",
        true,
        "You were asked to stop a run",
        "run.cancel",
    ),
    entry(
        "schedule.due",
        "schedule",
        true,
        "A follow-up you set is due",
        "reminder.due",
    ),
    entry(
        "connection.push_disabled",
        "connection",
        true,
        "Pushes to this connection's webhook were turned off after failing",
        "connection.push_disabled",
    ),
    // Heard by subscription, or when the account follows the thing.
    entry(
        "message.created",
        "message",
        false,
        "A message was posted",
        "message.created",
    ),
    entry(
        "message.edited",
        "message",
        false,
        "A message was edited",
        "message.edited",
    ),
    entry(
        "message.removed",
        "message",
        false,
        "A message was removed",
        "message.removed",
    ),
    entry(
        "task.created",
        "task",
        false,
        "A task was created",
        "work_item.created",
    ),
    entry(
        "task.updated",
        "task",
        false,
        "A task changed",
        "work_item.changed",
    ),
    entry(
        "task.commented",
        "task",
        false,
        "Someone commented on a task",
        "work_item.changed",
    ),
    entry(
        "request.raised",
        "request",
        false,
        "Someone asked for an answer or an approval",
        "request.raised",
    ),
    entry(
        "request.answered",
        "request",
        false,
        "A request was answered",
        "request.answered",
    ),
    entry(
        "request.cancelled",
        "request",
        false,
        "A request was withdrawn",
        "request.cancelled",
    ),
    entry(
        "request.expired",
        "request",
        false,
        "A request went unanswered until it expired",
        "request.expired",
    ),
    entry(
        "run.updated",
        "run",
        false,
        "A run changed state",
        "run.updated",
    ),
    entry(
        "run.finished",
        "run",
        false,
        "A run finished, failed or was stopped",
        "agent_run.terminal",
    ),
    entry(
        "page.updated",
        "page",
        false,
        "A page was saved as a new version",
        "page.updated",
    ),
    entry(
        "page.commented",
        "page",
        false,
        "Someone commented on a page",
        "page.commented",
    ),
    entry(
        "record.created",
        "record",
        false,
        "A record was added to a collection",
        "record.created",
    ),
    entry(
        "record.updated",
        "record",
        false,
        "A record in a collection changed",
        "record.updated",
    ),
    entry(
        "sheet.updated",
        "sheet",
        false,
        "A sheet's cells, tabs or name changed",
        "sheet.updated",
    ),
    entry(
        "whiteboard.updated",
        "whiteboard",
        false,
        "A whiteboard was saved as a new version",
        "whiteboard.updated",
    ),
    entry(
        "pull_request.checks_failed",
        "pull_request",
        false,
        "Checks failed on a linked pull request",
        "pull_request.checks_failed",
    ),
    entry(
        "pull_request.checks_passed",
        "pull_request",
        false,
        "Checks passed on a linked pull request",
        "pull_request.checks_passed",
    ),
    entry(
        "pull_request.merged",
        "pull_request",
        false,
        "A linked pull request was merged",
        "pull_request.merged",
    ),
    entry(
        "pull_request.closed",
        "pull_request",
        false,
        "A linked pull request was closed",
        "pull_request.closed",
    ),
];

/// The entry for `kind`.
pub fn get(kind: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.kind == kind)
}

/// Every deliverable kind, for the read query's filter.
pub fn kinds() -> Vec<String> {
    ENTRIES.iter().map(|entry| entry.kind.to_owned()).collect()
}

/// Whether a subscription's pattern (`task.assigned`, `task.*`, `*`) or an
/// older name for the same type (`work_item.changed`) matches `kind`.
pub fn matches(pattern: &str, kind: &str) -> bool {
    if pattern == "*" || pattern == kind {
        return true;
    }
    if let Some(family) = pattern.strip_suffix(".*") {
        return kind
            .split_once('.')
            .is_some_and(|(prefix, _)| prefix == family);
    }
    get(kind).is_some_and(|entry| entry.feed_name == pattern)
}

/// Whether `pattern` is a pattern a subscription may hold.
pub fn valid_pattern(pattern: &str) -> bool {
    pattern == "*"
        || ENTRIES
            .iter()
            .any(|entry| matches(pattern, entry.kind) && !pattern.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_a_logged_fact() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in ENTRIES {
            assert!(seen.insert(entry.kind), "{} is listed twice", entry.kind);
            let ty = crate::events::catalog::get(entry.kind)
                .unwrap_or_else(|| panic!("{} is not in the event catalogue", entry.kind));
            assert_eq!(
                ty.class,
                crate::events::EventClass::Fact,
                "{} is a signal",
                entry.kind
            );
        }
    }

    #[test]
    fn patterns_match_kinds_families_and_older_names() {
        assert!(matches("*", "task.assigned"));
        assert!(matches("task.*", "task.assigned"));
        assert!(!matches("task.*", "message.created"));
        assert!(matches("work_item.changed", "task.updated"));
        assert!(matches("reminder.due", "schedule.due"));
        assert!(valid_pattern("reply.*"));
        assert!(valid_pattern("work_item.assigned"));
        assert!(!valid_pattern("nothing.here"));
        assert!(!valid_pattern("conversation.read"));
    }
}
