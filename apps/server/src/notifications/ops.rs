//! Someone's own notifications and Later list, and finding people, as
//! operations (K2.16). Each runs its hand-written tool's command
//! (`ops::wrapped`) as the caller the dispatcher checked; the reads span
//! every workspace the caller reaches unless a space or workspace is named.
//! Marking notifications and Later items is the caller's own state, so it
//! keeps no audit row, as before.

use uuid::Uuid;

use crate::{
    mcp::{later_tools, notifications, talk},
    ops::{
        Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{run_tool, tool_op},
    },
};

/// The workspace module's.
pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<NotificationList>()
        .add::<NotificationUpdate>()
        .add::<PeopleSearch>();
}

/// The Inbox module's.
pub fn register_later_ops(registry: &mut Registry) {
    registry
        .add::<LaterList>()
        .add::<LaterSave>()
        .add::<LaterUpdate>();
}

/// A space when one is named; otherwise the caller's own.
fn own_or_space(project_id: Option<&str>) -> Target {
    match project_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(project) => Target::Space(parse_id(project).unwrap_or_else(Uuid::nil)),
        None => Target::Account,
    }
}

impl Targeted for notifications::NotificationListArguments {
    fn target(&self) -> Target {
        own_or_space(self.project_id.as_deref())
    }
}

impl Targeted for notifications::NotificationUpdateArguments {
    fn target(&self) -> Target {
        own_or_space(self.project_id.as_deref())
    }
}

impl Targeted for talk::PeopleSearchArguments {
    fn target(&self) -> Target {
        own_or_space(self.project_id.as_deref())
    }
}

macro_rules! own {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    Target::Account
                }
            }
        )*
    };
}

own!(
    later_tools::LaterListArguments,
    later_tools::ReminderSetArguments,
    later_tools::LaterUpdateArguments,
);

tool_op!(NotificationList {
    name: "notification_list",
    module: "workspace",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "List notifications",
    description: "Your notifications, newest first: mentions, tasks assigned to you, \
        approvals waiting on you, your runs, and pull request checks and reviews. Repeats \
        collapse into one with a count. view: inbox (default), unread, snoozed or archived; \
        category: all, mentions, tasks, runs or pull_requests.",
    emits: [],
    input: notifications::NotificationListArguments,
    output: notifications::NotificationListValue,
    run: |call, input| { run_tool!(call, as_tool, notifications::notification_list, input) }
});

tool_op!(NotificationUpdate {
    name: "notification_update",
    module: "workspace",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Update notifications",
    description: "Change your notifications: read, unread, archive, restore, snooze (until \
        snoozeUntil), snooze_until_activity or unsnooze. Pass ids from notification_list, or \
        all: true to mark read or archive everything in the inbox (optionally one projectId).",
    emits: ["notification.changed"],
    input: notifications::NotificationUpdateArguments,
    output: notifications::NotificationUpdateValue,
    run: |call, input| { run_tool!(call, as_tool, notifications::notification_update, input) }
});

tool_op!(PeopleSearch {
    name: "people_search",
    module: "workspace",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "Find people",
    description: "Find people and agents by name or @handle, with the spaces they're in. Use \
        their handle to mention them or start a conversation.",
    emits: [],
    input: talk::PeopleSearchArguments,
    output: talk::PeopleValue,
    run: |call, input| { run_tool!(call, as_tool, talk::people_search, input) }
});

tool_op!(LaterList {
    name: "later_list",
    module: "inbox",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "List Later",
    description: "Your Later list: messages and tasks you saved and reminders you set (view \
        open, the default, soonest reminder first; or done), or view scheduled for your \
        messages waiting to send. A due reminder reaches you as a notification, or an agent as \
        a reminder.due event.",
    emits: [],
    input: later_tools::LaterListArguments,
    output: later_tools::LaterListValue,
    run: |call, input| { run_tool!(call, as_tool, later_tools::later_list, input) }
});

tool_op!(LaterSave {
    name: "later_save",
    module: "inbox",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    home: true,
    audited: true,
    title: "Save for later",
    description: "Save a message (messageId) or task for later, or set yourself a reminder: \
        text and at (RFC 3339, up to a year ahead). Saving the same message or task again \
        updates it.",
    emits: ["later.changed"],
    input: later_tools::ReminderSetArguments,
    output: later_tools::LaterItemValue,
    run: |call, input| { run_tool!(call, as_tool, later_tools::later_save, input) }
});

tool_op!(LaterUpdate {
    name: "later_update",
    module: "inbox",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Update Later item",
    description: "Change an item on your Later list: mark it done (or open again), move its \
        reminder (at, or \"none\"), change its text, or remove it.",
    emits: ["later.changed"],
    input: later_tools::LaterUpdateArguments,
    output: later_tools::LaterUpdateValue,
    run: |call, input| { run_tool!(call, as_tool, later_tools::later_update, input) }
});
