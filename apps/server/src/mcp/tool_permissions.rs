//! AWD-74: the permission each tool needs.
//!
//! An agent's tool list shows only the tools its permissions allow (a
//! person's permissions cut down by their agent limit, or an agent's own
//! role), and a call to any other tool is refused before it runs. The tools
//! still check access themselves on every call; this is the first check, not
//! the only one.
//!
//! `None` is for tools anyone who can open a project may use: reading, and
//! the caller's own things (notifications, saved items, read state). A tool
//! that does several things names the permission its main use needs; the
//! others are checked inside it.
//!
//! Every tool on the server is listed once (test-enforced), next to related
//! ones, so a new tool must say what it needs.

use crate::policy::Permission::{
    self, ChannelsCreate, MembersView, RunsLaunch, TasksCreate, TasksEdit, TasksView,
    WhiteboardsEdit,
};

pub(super) const TOOL_PERMISSIONS: &[(&str, Option<Permission>)] = &[
    ("project_members", Some(MembersView)),
    ("conversation_list", None),
    ("conversation_open", None),
    ("conversation_read", None),
    ("conversation_start", Some(ChannelsCreate)),
    ("file_read", None),
    ("agent_list", None),
    ("inbox_list", None),
    ("session_bind", None),
    // Custom fields (AWD-89): reading them is seeing tasks; defining them
    // is the workflow's permission. Values are set through task_update.
    ("thing_open", None),
    ("whiteboard_list", None),
    ("whiteboard_read", None),
    ("whiteboard_render", None),
    ("whiteboard_trash_empty", Some(WhiteboardsEdit)),
    ("task_from_template", Some(TasksCreate)),
    ("project_export", Some(TasksView)),
    ("ai_suggestions_list", Some(TasksView)),
    ("ai_suggestion_decide", Some(TasksEdit)),
    ("ai_analyze", Some(TasksEdit)),
    ("ai_summarize", Some(TasksView)),
    ("run_request", Some(RunsLaunch)),
];

/// Whether a caller holding `reach` may use a tool at all. A tool missing
/// from the table is never reachable.
pub(super) fn reachable(tool: &str, reach: crate::policy::PermissionSet) -> bool {
    match needs(tool) {
        Ok(None) => true,
        Ok(Some(permission)) => reach.contains(permission),
        Err(()) => false,
    }
}

/// What a tool needs, or `None` for one any member may use. A tool missing
/// from the table needs everything: the guard test keeps that from
/// happening.
pub(super) fn needs(tool: &str) -> Result<Option<Permission>, ()> {
    TOOL_PERMISSIONS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, permission)| *permission)
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::TOOL_PERMISSIONS;
    use crate::policy::{MembershipRole, PermissionSet};

    #[test]
    fn every_tool_names_the_permission_it_needs_once() {
        // Operations name their permission themselves (`Operation::PERMISSION`).
        let tools = super::super::tool_catalog()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .filter(|name| !crate::ops::mcp::is_operation(name))
            .collect::<BTreeSet<_>>();
        let listed = TOOL_PERMISSIONS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<Vec<_>>();
        let unique = listed.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), listed.len(), "a tool is listed twice");
        let missing = tools.difference(&unique).collect::<Vec<_>>();
        assert!(missing.is_empty(), "say what these tools need: {missing:?}");
        let gone = unique.difference(&tools).collect::<Vec<_>>();
        assert!(gone.is_empty(), "these tools no longer exist: {gone:?}");
    }

    #[test]
    fn archiving_alone_opens_the_task_update_tools() {
        let archive_only = PermissionSet::from_keys(&["tasks.view", "tasks.delete"]);
        assert!(!super::reachable("task_comment", archive_only));
    }

    #[test]
    fn viewers_keep_the_read_tools_they_had() {
        let viewer = PermissionSet::built_in(MembershipRole::Viewer);
        let need = super::needs("project_export").expect("listed");
        assert!(
            need.is_none_or(|need| viewer.contains(need)),
            "project_export"
        );
    }
}
