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

/// A fact addressed to one account (a wake-up for it, A2): kept, never in
/// the audit trail (the change it is about is), never a rule's trigger.
const fn addressed(
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
pub static CONVERSATION_MEMBER_ADDED: EventType = fact(
    "conversation.member.added",
    "messages",
    Some("conversation"),
    "Someone was added to a conversation",
)
.legacy(&["conversation.member.added"]);
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
pub static MESSAGE_MENTIONED: EventType = addressed(
    "message.mentioned",
    "messages",
    Some("message"),
    "You were mentioned in a message",
);
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
pub static MESSAGE_DRAFT_CHANGED: EventType = signal(
    "message_draft.changed",
    "messages",
    None,
    "Someone's draft was saved or cleared",
);
pub static NOTIFICATION_KEYWORDS_CHANGED: EventType = signal(
    "notification_keywords.changed",
    "messages",
    None,
    "Someone's keyword alerts changed",
);
pub static SIDEBAR_CHANGED: EventType = signal(
    "sidebar.changed",
    "messages",
    None,
    "Someone's sidebar sections changed",
);
pub static SAVED_SEARCHES_CHANGED: EventType = signal(
    "saved_searches.changed",
    "workspace",
    None,
    "Someone's saved searches changed",
);
pub static FOLLOWS_CHANGED: EventType = signal(
    "follows.changed",
    "workspace",
    None,
    "What someone follows changed",
);
pub static INBOX_VIEWS_CHANGED: EventType = signal(
    "inbox_views.changed",
    "inbox",
    None,
    "Someone's saved inbox views changed",
);
pub static DM_RULES_CHANGED: EventType = signal(
    "dm_rules.changed",
    "messages",
    None,
    "Who may message someone changed for them",
);
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
pub static RUN_START_ASKED: EventType = addressed(
    "run.start",
    "runs",
    Some("run"),
    "The agent was asked to start a run it was given",
);
pub static RUN_CANCEL_ASKED: EventType = addressed(
    "run.cancel",
    "runs",
    Some("run"),
    "Someone asked the agent doing a run to stop it",
);
pub static REQUEST_RAISED: EventType = fact(
    "request.raised",
    "runs",
    Some("request"),
    "Someone asked for an answer or an approval",
)
.automatable();
pub static REQUEST_ANSWERED: EventType = fact(
    "request.answered",
    "runs",
    Some("request"),
    "A request was answered",
)
.automatable();
pub static REQUEST_CANCELLED: EventType = fact(
    "request.cancelled",
    "runs",
    Some("request"),
    "A request was withdrawn",
);
pub static REQUEST_EXPIRED: EventType = fact(
    "request.expired",
    "runs",
    Some("request"),
    "A request went unanswered until it expired",
);

// --- tasks -----------------------------------------------------------------
pub static TASK_CREATED: EventType =
    fact("task.created", "tasks", Some("task"), "A task was created").automatable();
pub static TASK_UPDATED: EventType =
    fact("task.updated", "tasks", Some("task"), "A task changed").automatable();
pub static TASK_ASSIGNED: EventType = addressed(
    "task.assigned",
    "tasks",
    Some("task"),
    "A task was assigned to you",
);
pub static COMMENT_MENTIONED: EventType = addressed(
    "comment.mentioned",
    "tasks",
    Some("task"),
    "You were mentioned in a comment",
);
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
    "collections",
    Some("view"),
    "A view of a collection was added, changed or removed",
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
/// A page's content was kept as a new version: quick saves merged into one
/// version are one fact, which rules and webhooks hear (K3.5).
pub static PAGE_UPDATED: EventType = fact(
    "page.updated",
    "pages",
    Some("page"),
    "A page was saved as a new version",
)
.automatable();
pub static PAGE_DISCUSSION_CHANGED: EventType = signal(
    "page.discussion_changed",
    "pages",
    Some("page"),
    "A page's comments or suggestions changed",
)
.legacy(&["page.discussion.updated"]);
// --- comments, proposals and changes on documents (C1.28) ------------------
/// A comment on a task, page, sheet, board or record: about the thing, with
/// the message's id (never its text).
pub static COMMENT_CREATED: EventType = fact(
    "comment.created",
    "messages",
    None,
    "Someone commented on a task, page, sheet, whiteboard or record",
)
.automatable();
/// Someone proposed a change to a page, sheet or board.
pub static PROPOSAL_CREATED: EventType = fact(
    "proposal.created",
    "workspace",
    None,
    "Someone proposed a change to a page, sheet or whiteboard",
)
.automatable();
pub static PROPOSAL_DECIDED: EventType = fact(
    "proposal.decided",
    "workspace",
    None,
    "A proposed change was accepted, rejected or withdrawn",
);
/// A change to a page, sheet or board was undone in a step.
pub static CHANGE_UNDONE: EventType = fact(
    "change.undone",
    "pages",
    None,
    "A change to a page, sheet or whiteboard was undone",
);

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
pub static PAGE_VERSION_RESTORED: EventType = fact(
    "page.version_restored",
    "pages",
    Some("page"),
    "An earlier version of a page was put back",
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
.legacy(&["sheet.updated"])
.automatable();
pub static SHEET_VERSION_RESTORED: EventType = fact(
    "sheet.version_restored",
    "sheets",
    Some("sheet"),
    "An earlier revision of a sheet was put back",
);
pub static SHEET_VIEWS_CHANGED: EventType = fact(
    "sheet.views_changed",
    "sheets",
    Some("sheet"),
    "A sheet's filter views were added, changed or removed",
);
pub static SHEET_RANGES_CHANGED: EventType = fact(
    "sheet.ranges_changed",
    "sheets",
    Some("sheet"),
    "A sheet's named ranges were added, changed or removed",
);
pub static SHEET_CHARTS_CHANGED: EventType = fact(
    "sheet.charts_changed",
    "sheets",
    Some("sheet"),
    "A sheet's charts were added, changed or removed",
);
pub static SHEET_PROPOSALS_CHANGED: EventType = fact(
    "sheet.proposals_changed",
    "sheets",
    Some("sheet"),
    "A change to a sheet was proposed, or a proposal on it decided",
);

// --- dashboards ------------------------------------------------------------
pub static DASHBOARD_UPDATED: EventType = fact(
    "dashboard.updated",
    "dashboards",
    Some("dashboard"),
    "A shared dashboard's name or tiles changed, or it was removed",
)
.legacy(&["dashboard.updated"]);

// --- whiteboards -----------------------------------------------------------
pub static WHITEBOARD_SCENE_CHANGED: EventType = signal(
    "whiteboard.scene_changed",
    "whiteboards",
    Some("whiteboard"),
    "A whiteboard's scene or details changed",
)
.legacy(&["whiteboard.updated"]);

/// A whiteboard's scene was kept as a new version (its revision cadence),
/// which rules and webhooks hear (K3.5).
pub static WHITEBOARD_UPDATED: EventType = fact(
    "whiteboard.updated",
    "whiteboards",
    Some("whiteboard"),
    "A whiteboard was saved as a new version",
)
.automatable();
pub static WHITEBOARD_VERSION_RESTORED: EventType = fact(
    "whiteboard.version_restored",
    "whiteboards",
    Some("whiteboard"),
    "An earlier version of a whiteboard was put back",
);
pub static WHITEBOARD_PROPOSALS_CHANGED: EventType = fact(
    "whiteboard.proposals_changed",
    "whiteboards",
    Some("whiteboard"),
    "A change to a whiteboard was proposed, or a proposal on it decided",
);

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

// --- automations -----------------------------------------------------------
/// An outside service's delivery to an incoming hook checked out (K3.26).
/// The payload holds the delivery's id; the body stays in its record.
pub static INCOMING_HOOK_RECEIVED: EventType = fact(
    "incoming_hook.received",
    "automations",
    Some("incoming_hook"),
    "An incoming hook received a delivery",
)
.automatable();

// --- links (K3.15) ---------------------------------------------------------
pub static LINK_CREATED: EventType = fact(
    "link.created",
    "workspace",
    None,
    "Someone linked two things",
);
pub static LINK_DELETED: EventType = fact(
    "link.deleted",
    "workspace",
    None,
    "Someone took a link away",
);

// --- spaces (today's projects) ---------------------------------------------
pub static SPACE_CREATED: EventType = fact(
    "space.created",
    "workspace",
    Some("space"),
    "A space was made",
);
pub static SPACE_UPDATED: EventType = fact(
    "space.updated",
    "workspace",
    Some("space"),
    "A space's name, description, icon or visibility changed",
);
pub static SPACE_ARCHIVED: EventType = fact(
    "space.archived",
    "workspace",
    Some("space"),
    "A space was archived",
);
pub static SPACE_RESTORED: EventType = fact(
    "space.restored",
    "workspace",
    Some("space"),
    "An archived space was brought back",
);
pub static SPACE_MEMBER_CHANGED: EventType = fact(
    "space.member.changed",
    "workspace",
    Some("space"),
    "Someone joined, left or changed role in a space",
)
.legacy(&["project.membership.updated"]);

pub static SPACE_MODULES_CHANGED: EventType = fact(
    "space.modules_changed",
    "projects",
    Some("space"),
    "A space's own module switches changed",
)
.legacy(&["project.modules.updated"]);

pub static WORKSPACE_LIFECYCLE_CHANGED: EventType = fact(
    "workspace.lifecycle_changed",
    "workspace",
    Some("workspace"),
    "A workspace was suspended, resumed, scheduled for deletion or kept",
)
.legacy(&["workspace.lifecycle.changed"]);

// --- workspace -------------------------------------------------------------
pub static FILE_ATTACHED: EventType = fact(
    "file.attached",
    "workspace",
    Some("file"),
    "A file was attached to a message, a task or a page",
);
pub static FILE_REMOVED: EventType = fact(
    "file.removed",
    "workspace",
    Some("file"),
    "A file was removed",
);
pub static FILE_UPLOAD_STARTED: EventType = signal(
    "file.upload_started",
    "workspace",
    None,
    "Someone started a direct upload to the file store",
);
pub static WORKSPACE_FILE_STORE_CHANGED: EventType = fact(
    "workspace.file_store_changed",
    "workspace",
    None,
    "The workspace's own file bucket was set or stopped",
);
pub static WORKSPACE_TRANSFER_CHANGED: EventType = fact(
    "workspace.transfer_changed",
    "workspace",
    Some("workspace_transfer"),
    "A workspace export or import moved on",
);
pub static ACCOUNT_STATUS_CHANGED: EventType = signal(
    "account.status_changed",
    "workspace",
    None,
    "Someone's status changed",
)
.legacy(&["account.status.changed"]);
pub static ACCOUNT_PROFILE_CHANGED: EventType = signal(
    "account.profile_changed",
    "workspace",
    None,
    "Someone's profile or avatar changed",
);
pub static CUSTOM_EMOJI_CHANGED: EventType = signal(
    "custom_emoji.changed",
    "workspace",
    None,
    "A workspace's custom emoji changed",
);
pub static GUARDS_CHANGED: EventType = signal(
    "guards.changed",
    "agents",
    None,
    "A workspace's guards changed",
);

// --- inbox -----------------------------------------------------------------
pub static NOTIFICATION_CHANGED: EventType = signal(
    "notification.changed",
    "inbox",
    None,
    "Someone's notifications changed",
)
.legacy(&["notifications.updated"]);
pub static REPLY_REQUESTED: EventType = addressed(
    "reply.requested",
    "agents",
    Some("reply_request"),
    "You were asked to reply",
);
pub static REPLY_FOLLOWUP: EventType = addressed(
    "reply.followup",
    "agents",
    Some("reply_request"),
    "Someone followed up where you are working",
);
pub static REPLY_CANCELLED: EventType = addressed(
    "reply.cancelled",
    "agents",
    Some("reply_request"),
    "Someone stopped a reply you were asked for",
);
pub static SCHEDULE_DUE: EventType =
    addressed("schedule.due", "agents", None, "A follow-up you set is due");
pub static AGENT_PAUSED: EventType = addressed("agent.paused", "agents", None, "You were paused");
pub static AGENT_RESUMED: EventType =
    addressed("agent.resumed", "agents", None, "You were resumed");
pub static SCHEDULE_CHANGED: EventType = signal(
    "schedule.changed",
    "agents",
    Some("schedule"),
    "A follow-up of yours was set, changed or cancelled",
);
pub static CONNECTION_PUSH_DISABLED: EventType = addressed(
    "connection.push_disabled",
    "agents",
    None,
    "Pushes to a connection's webhook were turned off after failing",
);
pub static CONNECTION_CHANGED: EventType = signal(
    "connection.changed",
    "agents",
    Some("connection"),
    "A connection of yours was added, changed or removed",
);
pub static ACCOUNT_MANAGERS_CHANGED: EventType = signal(
    "account.managers_changed",
    "agents",
    None,
    "Who manages an agent or app changed",
)
.legacy(&["account.managers.changed"]);
/// Who may read what changed for an account (K5.8): staged by triggers
/// on memberships, whatever wrote them, so live connections work out what
/// they hear again.
pub static ACCOUNT_ACCESS_CHANGED: EventType = signal(
    "account.access_changed",
    "workspace",
    None,
    "What someone may see changed",
);
pub static ACCOUNT_REF_CHANGED: EventType = signal(
    "account.ref_changed",
    "agents",
    None,
    "Which agent elsewhere an agent account is changed, or a rebind waits",
);
pub static GROUP_CHANGED: EventType = fact(
    "group.changed",
    "workspace",
    None,
    "A group was made, changed, archived or brought back, or its members changed",
);
pub static THING_SHARE_CHANGED: EventType = signal(
    "thing.share_changed",
    "pages",
    None,
    "One thing was shared, or a share or link was revoked",
);
/// Every event type, in module order.
pub fn catalog() -> &'static [&'static EventType] {
    static CATALOG: &[&EventType] = &[
        &CONVERSATION_CREATED,
        &CONVERSATION_UPDATED,
        &CONVERSATION_ARCHIVED,
        &CONVERSATION_MEMBER_REMOVED,
        &CONVERSATION_MEMBER_ADDED,
        &CONVERSATION_REPLY_REQUESTS_CHANGED,
        &CONVERSATION_READ,
        &MESSAGE_CREATED,
        &COMMENT_CREATED,
        &PROPOSAL_CREATED,
        &PROPOSAL_DECIDED,
        &CHANGE_UNDONE,
        &MESSAGE_MENTIONED,
        &MESSAGE_EDITED,
        &MESSAGE_REMOVED,
        &MESSAGE_REACTIONS_CHANGED,
        &CONVERSATION_PINS_CHANGED,
        &MESSAGE_DRAFT_CHANGED,
        &NOTIFICATION_KEYWORDS_CHANGED,
        &SIDEBAR_CHANGED,
        &SAVED_SEARCHES_CHANGED,
        &FOLLOWS_CHANGED,
        &INBOX_VIEWS_CHANGED,
        &DM_RULES_CHANGED,
        &THREAD_STATUS_CHANGED,
        &RUN_UPDATED,
        &RUN_FINISHED,
        &RUN_START_ASKED,
        &RUN_CANCEL_ASKED,
        &REQUEST_RAISED,
        &REQUEST_ANSWERED,
        &REQUEST_CANCELLED,
        &REQUEST_EXPIRED,
        &TASK_CREATED,
        &TASK_UPDATED,
        &TASK_ASSIGNED,
        &COMMENT_MENTIONED,
        &TASK_PLACEMENT_CHANGED,
        &TASK_COMMENTED,
        &TASK_COMMENT_CHANGED,
        &MILESTONE_CREATED,
        &MILESTONE_UPDATED,
        &LABEL_CHANGED,
        &VIEW_CHANGED,
        &FIELD_CHANGED,
        &PAGE_CONTENT_SAVED,
        &PAGE_UPDATED,
        &PAGE_DISCUSSION_CHANGED,
        &PAGE_COMMENTED,
        &PAGE_COMMENT_CHANGED,
        &PAGE_VERSION_RESTORED,
        &DECISION_RECORDED,
        &DECISION_UPDATED,
        &SHEET_UPDATED,
        &SHEET_VERSION_RESTORED,
        &SHEET_VIEWS_CHANGED,
        &SHEET_RANGES_CHANGED,
        &SHEET_CHARTS_CHANGED,
        &SHEET_PROPOSALS_CHANGED,
        &DASHBOARD_UPDATED,
        &WHITEBOARD_SCENE_CHANGED,
        &WHITEBOARD_UPDATED,
        &WHITEBOARD_VERSION_RESTORED,
        &WHITEBOARD_PROPOSALS_CHANGED,
        &COLLECTION_UPDATED,
        &RECORD_CREATED,
        &RECORD_UPDATED,
        &PULL_REQUEST_CHECKS_FAILED,
        &PULL_REQUEST_CHECKS_PASSED,
        &PULL_REQUEST_MERGED,
        &PULL_REQUEST_CLOSED,
        &INCOMING_HOOK_RECEIVED,
        &LINK_CREATED,
        &LINK_DELETED,
        &SPACE_CREATED,
        &SPACE_UPDATED,
        &SPACE_ARCHIVED,
        &SPACE_RESTORED,
        &SPACE_MEMBER_CHANGED,
        &SPACE_MODULES_CHANGED,
        &WORKSPACE_LIFECYCLE_CHANGED,
        &FILE_ATTACHED,
        &FILE_REMOVED,
        &FILE_UPLOAD_STARTED,
        &WORKSPACE_FILE_STORE_CHANGED,
        &WORKSPACE_TRANSFER_CHANGED,
        &ACCOUNT_STATUS_CHANGED,
        &ACCOUNT_PROFILE_CHANGED,
        &CUSTOM_EMOJI_CHANGED,
        &GUARDS_CHANGED,
        &NOTIFICATION_CHANGED,
        &REPLY_REQUESTED,
        &REPLY_FOLLOWUP,
        &REPLY_CANCELLED,
        &SCHEDULE_DUE,
        &AGENT_PAUSED,
        &AGENT_RESUMED,
        &SCHEDULE_CHANGED,
        &CONNECTION_PUSH_DISABLED,
        &CONNECTION_CHANGED,
        &ACCOUNT_MANAGERS_CHANGED,
        &ACCOUNT_ACCESS_CHANGED,
        &ACCOUNT_REF_CHANGED,
        &THING_SHARE_CHANGED,
        &GROUP_CHANGED,
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
        let sql = concat!(
            include_str!("../../migrations/20261114194000_events_for_quiet_writes.sql"),
            include_str!("../../migrations/20261117140000_access_changed_signal.sql"),
        );
        let mut found = 0;
        for piece in sql.split('\'') {
            let dotted = piece.split('.').count() >= 2
                && piece
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_');
            // Questions became requests (A1.8), whose events are staged
            // from Rust; their trigger went with person_questions.
            let retired = matches!(piece, "question.asked" | "question.answered");
            if dotted && !retired && !piece.starts_with("aw.") {
                assert!(
                    get(piece).is_some(),
                    "{piece} is staged but not in the catalogue"
                );
                found += 1;
            }
        }
        assert!(found >= 15, "{found}");
    }

    #[test]
    fn every_webhook_type_is_in_the_catalogue_and_automatable() {
        for (name, _) in crate::events::EVENT_TYPES {
            let ty = get(name).unwrap_or_else(|| panic!("{name} is not in the catalogue"));
            assert!(ty.automatable, "{name}");
        }
    }
}
