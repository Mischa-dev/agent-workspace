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
    self, ChannelsCreate, ChannelsPost, CodeView, MembersManage, MembersView, PagesEdit,
    ProjectsCreate, ProjectsManage, RolesManage, RunsApprove, RunsLaunch, SettingsManage,
    TasksCreate, TasksEdit, TasksView, TasksWorkflow, WhiteboardsEdit,
};

pub(super) const TOOL_PERMISSIONS: &[(&str, Option<Permission>)] = &[
    ("briefing", None),
    ("projects_list", None),
    ("project_open", None),
    ("workflow_get", Some(TasksView)),
    ("settings_get", None),
    ("usage_windows", None),
    ("usage_batch", None),
    ("usage_batch_launch", Some(RunsLaunch)),
    ("project_members", Some(MembersView)),
    ("project_member_set", Some(MembersManage)),
    ("role_list", None),
    ("role_get", None),
    ("my_permissions", None),
    ("role_write", Some(RolesManage)),
    ("role_assign", Some(MembersManage)),
    ("project_create", Some(ProjectsCreate)),
    ("project_update", Some(ProjectsManage)),
    ("project_archive", Some(ProjectsManage)),
    ("project_layout_set", Some(SettingsManage)),
    ("conversation_list", None),
    ("conversation_open", None),
    ("conversation_read", None),
    ("conversation_start", Some(ChannelsCreate)),
    ("message_send", Some(ChannelsPost)),
    ("file_upload", Some(ChannelsPost)),
    ("file_list", None),
    ("file_read", None),
    ("message_edit", Some(ChannelsPost)),
    ("message_delete", None),
    ("message_history", None),
    ("conversation_members", None),
    ("conversation_continue", Some(ChannelsPost)),
    ("mark_read", None),
    ("notification_list", None),
    ("notification_update", None),
    ("later_list", None),
    ("later_save", None),
    ("later_update", None),
    ("message_schedule", Some(ChannelsPost)),
    ("scheduled_message_update", Some(ChannelsPost)),
    ("agent_list", None),
    ("people_search", None),
    ("thread_start", Some(ChannelsPost)),
    ("thread_update", Some(ChannelsPost)),
    ("channel_create", Some(ChannelsCreate)),
    ("conversation_update", Some(ChannelsPost)),
    ("pin_set", Some(ChannelsPost)),
    ("message_react", Some(ChannelsPost)),
    ("inbox_list", None),
    ("reply_claim", Some(ChannelsPost)),
    ("session_bind", None),
    ("reply_send", Some(ChannelsPost)),
    ("reply_progress", Some(ChannelsPost)),
    ("reply_pass", Some(ChannelsPost)),
    ("reply_release", Some(ChannelsPost)),
    ("reply_stop", Some(ChannelsPost)),
    ("task_list", Some(TasksView)),
    ("task_open", Some(TasksView)),
    ("task_create", Some(TasksCreate)),
    ("task_update", Some(TasksEdit)),
    ("task_create_many", Some(TasksCreate)),
    ("task_update_many", Some(TasksEdit)),
    ("task_criteria", Some(TasksEdit)),
    ("task_comment", Some(TasksEdit)),
    ("task_comment_edit", Some(TasksEdit)),
    ("task_comment_delete", Some(TasksEdit)),
    ("task_mark_blocked", Some(TasksEdit)),
    ("task_mark_ready_for_review", Some(TasksEdit)),
    ("task_link_pull_request", Some(TasksEdit)),
    ("task_reorder", Some(TasksEdit)),
    ("task_claim", Some(TasksEdit)),
    ("task_release", Some(TasksEdit)),
    ("task_link", Some(TasksEdit)),
    ("label_list", Some(TasksView)),
    ("label_write", Some(TasksEdit)),
    ("task_move", Some(TasksEdit)),
    ("search", None),
    ("page_list", None),
    ("page_get", None),
    ("skill_list", None),
    ("skill_get", None),
    ("page_write", Some(PagesEdit)),
    ("whiteboard_list", None),
    ("whiteboard_read", None),
    ("whiteboard_render", None),
    ("whiteboard_create", Some(WhiteboardsEdit)),
    ("whiteboard_edit", Some(WhiteboardsEdit)),
    ("whiteboard_update", Some(WhiteboardsEdit)),
    ("whiteboard_duplicate", Some(WhiteboardsEdit)),
    ("whiteboard_link", Some(WhiteboardsEdit)),
    ("decision_record", Some(PagesEdit)),
    ("decision_list", None),
    ("approval_list", None),
    ("task_depend", Some(TasksEdit)),
    ("view_list", Some(TasksView)),
    ("my_work", None),
    ("template_list", None),
    ("task_from_template", Some(TasksCreate)),
    ("task_import", Some(TasksCreate)),
    ("project_export", Some(TasksView)),
    ("webhook_list", None),
    ("webhook_write", None),
    ("webhook_deliveries", None),
    ("automation_list", None),
    ("automation_write", Some(TasksEdit)),
    ("automation_runs", None),
    ("milestone_list", Some(TasksView)),
    ("milestone_write", Some(TasksEdit)),
    ("task_ready_list", Some(TasksView)),
    ("dependency_graph", Some(TasksView)),
    ("ai_suggestions_list", Some(TasksView)),
    ("ai_suggestion_decide", Some(TasksEdit)),
    ("ai_analyze", Some(TasksEdit)),
    ("ai_summarize", Some(TasksView)),
    ("plan_import", Some(TasksCreate)),
    ("task_relate", Some(TasksEdit)),
    ("task_watch", Some(TasksView)),
    ("workflow_update", Some(TasksWorkflow)),
    ("run_start", Some(RunsLaunch)),
    ("task_launch", Some(RunsLaunch)),
    ("run_get", None),
    ("run_list", None),
    ("run_steps", None),
    ("run_usage", None),
    ("agent_profile_list", None),
    ("launch_list", None),
    ("environment_list", None),
    ("repository_list", Some(CodeView)),
    ("pull_request_list", Some(CodeView)),
    ("repository_get", Some(CodeView)),
    ("pull_request_get", Some(CodeView)),
    ("run_request", Some(RunsLaunch)),
    ("approval_get", None),
    ("approval_respond", Some(RunsApprove)),
    ("external_event_poll", None),
];

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
        let tools = super::super::tool_catalog()
            .into_iter()
            .map(|tool| tool.name.to_string())
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
    fn viewers_keep_the_read_tools_they_had() {
        let viewer = PermissionSet::built_in(MembershipRole::Viewer);
        for tool in [
            "task_list",
            "task_open",
            "task_watch",
            "repository_list",
            "workflow_get",
            "project_export",
        ] {
            let need = super::needs(tool).expect("listed");
            assert!(need.is_none_or(|need| viewer.contains(need)), "{tool}");
        }
    }
}
