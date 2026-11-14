//! The typed catalogue of event types (K3 3.3.3). Every event in the log
//! names one of these, never a free string: `events::emit` takes a
//! `&'static EventType`. Each module lists its own; `catalog()` puts them
//! together, and the modules guard checks that every module's manifest
//! (`modules/<id>.json`, `events.emits`) lists exactly its entries.
//!
//! Classes: a *fact* is kept for the workspace's event retention (400 days
//! by default) and feeds audit and activity; a *signal* (a live save, a read
//! mark, a board's scene counter) is kept 7 days and is for live updates and
//! routing only.

/// Whether an event is kept as history or only for a short while.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventClass {
    Fact,
    Signal,
}

impl EventClass {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Signal => "signal",
        }
    }
}

/// One kind of event.
#[derive(Debug)]
pub struct EventType {
    /// Dotted, such as `page.updated`.
    pub name: &'static str,
    /// The module (manifest id) that emits it; its manifest lists it.
    pub module: &'static str,
    /// The kind of thing it is about, when it is about one.
    pub thing_kind: Option<&'static str>,
    pub class: EventClass,
    /// "A page changed": for webhook and rule builder copy.
    pub summary: &'static str,
    /// Shows in the audit trail.
    pub audited: bool,
    /// Can be a rule's trigger (webhooks and automations hear it).
    pub automatable: bool,
    /// Older names for the same event: the outbox's type, which the web's
    /// realtime envelope still carries until W1 moves to catalogue names.
    pub legacy_names: &'static [&'static str],
}

const fn fact(
    name: &'static str,
    module: &'static str,
    thing_kind: Option<&'static str>,
    summary: &'static str,
) -> EventType {
    EventType {
        name,
        module,
        thing_kind,
        class: EventClass::Fact,
        summary,
        audited: true,
        automatable: false,
        legacy_names: &[],
    }
}

const fn signal(
    name: &'static str,
    module: &'static str,
    thing_kind: Option<&'static str>,
    summary: &'static str,
) -> EventType {
    EventType {
        name,
        module,
        thing_kind,
        class: EventClass::Signal,
        summary,
        audited: false,
        automatable: false,
        legacy_names: &[],
    }
}

impl EventType {
    const fn legacy(mut self, names: &'static [&'static str]) -> Self {
        self.legacy_names = names;
        self
    }

    /// Webhooks and rules can hear it.
    const fn automatable(mut self) -> Self {
        self.automatable = true;
        self
    }
}

// --- messages: conversations and their messages ---------------------------
pub static CONVERSATION_CREATED: EventType = fact(
    "conversation.created",
    "messages",
    Some("conversation"),
    "A conversation was started",
)
.legacy(&["conversation.created"]);
pub static CONVERSATION_UPDATED: EventType = fact(
    "conversation.updated",
    "messages",
    Some("conversation"),
    "A conversation's details changed",
)
.legacy(&["conversation.updated"]);
pub static CONVERSATION_ARCHIVED: EventType = fact(
    "conversation.archived",
    "messages",
    Some("conversation"),
    "A conversation was archived",
)
.legacy(&["conversation.archived"]);
pub static CONVERSATION_MEMBER_REMOVED: EventType = fact(
    "conversation.member.removed",
    "messages",
    Some("conversation"),
    "Someone left or was removed from a conversation",
)
.legacy(&["conversation.member.removed"]);
pub static CONVERSATION_REPLY_REQUESTS_CHANGED: EventType = signal(
    "conversation.reply_requests_changed",
    "messages",
    Some("conversation"),
    "Who is answering in a conversation changed",
)
.legacy(&["conversation.reply_requests.updated"]);
pub static CONVERSATION_READ: EventType = signal(
    "conversation.read",
    "messages",
    Some("conversation"),
    "Someone read a conversation",
)
.legacy(&["conversation.read_state.updated"]);
pub static MESSAGE_CREATED: EventType = fact(
    "message.created",
    "messages",
    Some("message"),
    "A message was posted",
)
.legacy(&["conversation.message.created"])
.automatable();
pub static MESSAGE_EDITED: EventType = fact(
    "message.edited",
    "messages",
    Some("message"),
    "A message was edited",
)
.legacy(&["conversation.message.updated"]);
pub static MESSAGE_REMOVED: EventType = fact(
    "message.removed",
    "messages",
    Some("message"),
    "A message was removed",
)
.legacy(&["conversation.message.updated"]);
pub static MESSAGE_REACTIONS_CHANGED: EventType = signal(
    "message.reactions_changed",
    "messages",
    Some("message"),
    "Reactions on a message changed",
)
.legacy(&["conversation.message.reactions.updated"]);

pub static CONVERSATION_PINS_CHANGED: EventType = fact(
    "conversation.pins_changed",
    "messages",
    Some("conversation"),
    "Something was pinned to or unpinned from a conversation",
);

// --- channels ----------------------------------------------------------
pub static THREAD_STATUS_CHANGED: EventType = fact(
    "thread.status_changed",
    "channels",
    Some("thread"),
    "A thread's status or outcome changed",
);

// --- runs ------------------------------------------------------------------
pub static RUN_UPDATED: EventType = fact("run.updated", "runs", Some("run"), "A run changed state")
    .legacy(&["conversation.agent_run.updated"])
    .automatable();
pub static RUN_FINISHED: EventType = fact(
    "run.finished",
    "runs",
    Some("run"),
    "A run finished, failed or was stopped",
)
.automatable();

// --- tasks -----------------------------------------------------------------
pub static TASK_CREATED: EventType =
    fact("task.created", "tasks", Some("task"), "A task was created").automatable();
pub static TASK_UPDATED: EventType =
    fact("task.updated", "tasks", Some("task"), "A task changed").automatable();
pub static TASK_PLACEMENT_CHANGED: EventType = signal(
    "task.placement_changed",
    "tasks",
    Some("task"),
    "A task changed or moved on its board",
)
.legacy(&["work_item.updated"]);
pub static TASK_COMMENTED: EventType = fact(
    "task.commented",
    "tasks",
    Some("task"),
    "Someone commented on a task",
);
pub static TASK_COMMENT_CHANGED: EventType = fact(
    "task.comment_changed",
    "tasks",
    Some("task"),
    "A comment on a task was edited or removed",
);
pub static MILESTONE_CREATED: EventType = fact(
    "milestone.created",
    "tasks",
    Some("milestone"),
    "A milestone was added",
);
pub static MILESTONE_UPDATED: EventType = fact(
    "milestone.updated",
    "tasks",
    Some("milestone"),
    "A milestone changed or was archived",
);
pub static LABEL_CHANGED: EventType = fact(
    "label.changed",
    "tasks",
    Some("label"),
    "A label was added, changed or archived",
);
pub static VIEW_CHANGED: EventType = fact(
    "view.changed",
    "tasks",
    Some("view"),
    "A saved view was added, changed or removed",
);
pub static FIELD_CHANGED: EventType = fact(
    "field.changed",
    "tasks",
    Some("field"),
    "A task field was added, changed or archived",
);

// --- pages -----------------------------------------------------------------
pub static PAGE_CONTENT_SAVED: EventType = signal(
    "page.content_saved",
    "pages",
    Some("page"),
    "A page's content or details were saved",
)
.legacy(&["page.updated"]);
pub static PAGE_DISCUSSION_CHANGED: EventType = signal(
    "page.discussion_changed",
    "pages",
    Some("page"),
    "A page's comments or suggestions changed",
)
.legacy(&["page.discussion.updated"]);
pub static PAGE_COMMENTED: EventType = fact(
    "page.commented",
    "pages",
    Some("page"),
    "Someone commented on a page",
);
pub static PAGE_COMMENT_CHANGED: EventType = fact(
    "page.comment_changed",
    "pages",
    Some("page"),
    "A comment on a page was edited, resolved or deleted",
);
pub static DECISION_RECORDED: EventType = fact(
    "decision.recorded",
    "pages",
    Some("decision"),
    "A decision was recorded",
);
pub static DECISION_UPDATED: EventType = fact(
    "decision.updated",
    "pages",
    Some("decision"),
    "A decision changed or was superseded",
);

// --- sheets ----------------------------------------------------------------
pub static SHEET_UPDATED: EventType = fact(
    "sheet.updated",
    "sheets",
    Some("sheet"),
    "A sheet's cells, tabs or name changed",
)
.legacy(&["sheet.updated"]);

// --- whiteboards -----------------------------------------------------------
pub static WHITEBOARD_SCENE_CHANGED: EventType = signal(
    "whiteboard.scene_changed",
    "whiteboards",
    Some("whiteboard"),
    "A whiteboard's scene or details changed",
)
.legacy(&["whiteboard.updated"]);

// --- collections -----------------------------------------------------------
pub static COLLECTION_UPDATED: EventType = fact(
    "collection.updated",
    "collections",
    Some("collection"),
    "A collection, its fields, views or records changed",
)
.legacy(&["collection.updated"]);
pub static RECORD_CREATED: EventType = fact(
    "record.created",
    "collections",
    Some("record"),
    "A record was added to a collection",
)
.automatable();
pub static RECORD_UPDATED: EventType = fact(
    "record.updated",
    "collections",
    Some("record"),
    "A record in a collection changed",
)
.automatable();

// --- code ------------------------------------------------------------------
pub static PULL_REQUEST_CHECKS_FAILED: EventType = fact(
    "pull_request.checks_failed",
    "code",
    Some("pull_request"),
    "Checks failed on a linked pull request",
)
.automatable();
pub static PULL_REQUEST_CHECKS_PASSED: EventType = fact(
    "pull_request.checks_passed",
    "code",
    Some("pull_request"),
    "Checks passed on a linked pull request",
)
.automatable();
pub static PULL_REQUEST_MERGED: EventType = fact(
    "pull_request.merged",
    "code",
    Some("pull_request"),
    "A linked pull request was merged",
)
.automatable();
pub static PULL_REQUEST_CLOSED: EventType = fact(
    "pull_request.closed",
    "code",
    Some("pull_request"),
    "A linked pull request was closed",
)
.automatable();

// --- spaces (today's projects) ---------------------------------------------
pub static SPACE_MEMBER_CHANGED: EventType = fact(
    "space.member.changed",
    "projects",
    Some("space"),
    "Someone joined, left or changed role in a space",
)
.legacy(&["project.membership.updated"]);

// --- workspace -------------------------------------------------------------
pub static FILE_ATTACHED: EventType = fact(
    "file.attached",
    "workspace",
    Some("file"),
    "A file was attached to a message or task",
);
pub static FILE_REMOVED: EventType = fact(
    "file.removed",
    "workspace",
    Some("file"),
    "A file was removed",
);

// --- inbox -----------------------------------------------------------------
pub static NOTIFICATION_CHANGED: EventType = signal(
    "notification.changed",
    "inbox",
    None,
    "Someone's notifications changed",
)
.legacy(&["notifications.updated"]);
pub static QUESTION_ASKED: EventType = fact(
    "question.asked",
    "inbox",
    Some("question"),
    "Someone was asked a question",
);
pub static QUESTION_ANSWERED: EventType = fact(
    "question.answered",
    "inbox",
    Some("question"),
    "A question was answered",
);

/// Every event type, in module order.
pub fn catalog() -> &'static [&'static EventType] {
    static CATALOG: &[&EventType] = &[
        &CONVERSATION_CREATED,
        &CONVERSATION_UPDATED,
        &CONVERSATION_ARCHIVED,
        &CONVERSATION_MEMBER_REMOVED,
        &CONVERSATION_REPLY_REQUESTS_CHANGED,
        &CONVERSATION_READ,
        &MESSAGE_CREATED,
        &MESSAGE_EDITED,
        &MESSAGE_REMOVED,
        &MESSAGE_REACTIONS_CHANGED,
        &CONVERSATION_PINS_CHANGED,
        &THREAD_STATUS_CHANGED,
        &RUN_UPDATED,
        &RUN_FINISHED,
        &TASK_CREATED,
        &TASK_UPDATED,
        &TASK_PLACEMENT_CHANGED,
        &TASK_COMMENTED,
        &TASK_COMMENT_CHANGED,
        &MILESTONE_CREATED,
        &MILESTONE_UPDATED,
        &LABEL_CHANGED,
        &VIEW_CHANGED,
        &FIELD_CHANGED,
        &PAGE_CONTENT_SAVED,
        &PAGE_DISCUSSION_CHANGED,
        &PAGE_COMMENTED,
        &PAGE_COMMENT_CHANGED,
        &DECISION_RECORDED,
        &DECISION_UPDATED,
        &SHEET_UPDATED,
        &WHITEBOARD_SCENE_CHANGED,
        &COLLECTION_UPDATED,
        &RECORD_CREATED,
        &RECORD_UPDATED,
        &PULL_REQUEST_CHECKS_FAILED,
        &PULL_REQUEST_CHECKS_PASSED,
        &PULL_REQUEST_MERGED,
        &PULL_REQUEST_CLOSED,
        &SPACE_MEMBER_CHANGED,
        &FILE_ATTACHED,
        &FILE_REMOVED,
        &NOTIFICATION_CHANGED,
        &QUESTION_ASKED,
        &QUESTION_ANSWERED,
    ];
    CATALOG
}

/// The catalogue entry called `name`.
pub fn get(name: &str) -> Option<&'static EventType> {
    catalog().iter().copied().find(|ty| ty.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_name_is_dotted_unique_and_well_formed() {
        let mut seen = BTreeSet::new();
        for ty in catalog() {
            assert!(seen.insert(ty.name), "{} is listed twice", ty.name);
            let parts = ty.name.split('.').collect::<Vec<_>>();
            assert!(parts.len() >= 2, "{} is not dotted", ty.name);
            for part in parts {
                assert!(
                    part.starts_with(|c: char| c.is_ascii_lowercase())
                        && part
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{} breaks the name rule",
                    ty.name
                );
            }
            assert!(ty.name.len() <= 80);
            assert!(!ty.summary.is_empty(), "{} has no summary", ty.name);
            if let Some(kind) = ty.thing_kind {
                assert!(kind.len() >= 2 && kind.len() <= 40, "{kind}");
            }
            // A signal is never part of the audit trail.
            assert!(
                !(ty.audited && ty.class == EventClass::Signal),
                "{}",
                ty.name
            );
        }
    }

    #[test]
    fn every_legacy_outbox_type_maps_to_the_catalogue() {
        for legacy in crate::outbox::LEGACY_EVENT_TYPES {
            assert!(
                catalog().iter().any(|ty| ty.legacy_names.contains(legacy)),
                "the outbox type {legacy} has no catalogue entry"
            );
        }
    }

    /// The triggers of K3.8 stage events by name in SQL; each is here.
    #[test]
    fn every_type_a_trigger_stages_is_in_the_catalogue() {
        let sql = include_str!("../../migrations/20261114194000_events_for_quiet_writes.sql");
        let mut found = 0;
        for piece in sql.split('\'') {
            let dotted = piece.split('.').count() >= 2
                && piece
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_');
            if dotted && !piece.starts_with("aw.") {
                assert!(
                    get(piece).is_some(),
                    "{piece} is staged but not in the catalogue"
                );
                found += 1;
            }
        }
        assert!(found >= 17, "{found}");
    }

    #[test]
    fn every_webhook_type_is_in_the_catalogue_and_automatable() {
        for (name, _) in crate::events::EVENT_TYPES {
            let ty = get(name).unwrap_or_else(|| panic!("{name} is not in the catalogue"));
            assert!(ty.automatable, "{name}");
        }
    }
}
