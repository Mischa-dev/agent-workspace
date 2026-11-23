//! Old tool names (K2.23). Tools that became operations under new names
//! kept their old names as aliases until the one deploy (ruling: one user,
//! one deploy, no migration window). Now an old name is refused with what
//! to use instead, so an agent that still calls it learns the new name and
//! can switch; it no longer runs anything. A guard written with an old name
//! matches the operations it ran.

/// An old tool name and the operations that replaced it.
#[derive(Clone, Copy, Debug)]
pub struct Retired {
    pub name: &'static str,
    /// What to call instead: one operation, or the few the old name chose
    /// between by its arguments (a create or an update).
    pub use_instead: &'static [&'static str],
}

const fn retired(name: &'static str, use_instead: &'static [&'static str]) -> Retired {
    Retired { name, use_instead }
}

/// Every retired name, in the order the operations took them over.
pub const RETIRED: &[Retired] = &[
    retired("projects_list", &["project_list"]),
    retired("project_open", &["project_get"]),
    retired("task_open", &["task_get"]),
    retired("my_work", &["my_work_get"]),
    retired("dependency_graph", &["dependency_graph_get"]),
    retired("search", &["workspace_search"]),
    retired("page_write", &["page_create", "page_update"]),
    retired("page_edit", &["page_update_blocks"]),
    retired("page_blocks", &["page_blocks_get"]),
    retired("page_comments", &["page_comment_list"]),
    retired("page_comment", &["page_comment_create"]),
    retired("page_suggestions", &["page_suggestion_list"]),
    retired("sheet_read", &["sheet_get"]),
    retired("sheet_write", &["sheet_create", "sheet_update"]),
    retired("collection_describe", &["collection_get"]),
    retired(
        "collection_write",
        &["collection_create", "collection_update"],
    ),
    retired("record_query", &["record_list"]),
    retired(
        "record_write",
        &["record_create_many", "record_update_many"],
    ),
    retired("view_write", &["view_create", "view_update"]),
    retired("label_write", &["label_create", "label_update"]),
    retired("field_write", &["field_create", "field_update"]),
    retired("milestone_write", &["milestone_create", "milestone_update"]),
    retired("template_write", &["template_create", "template_update"]),
    retired("recurring_write", &["recurring_create", "recurring_update"]),
    retired(
        "automation_write",
        &[
            "automation_create",
            "automation_update",
            "automation_delete",
            "automation_run_start",
        ],
    ),
    retired("automation_runs", &["automation_history"]),
    retired(
        "repository_write",
        &[
            "repository_create",
            "repository_prepare",
            "repository_archive",
        ],
    ),
    retired(
        "github_integration_write",
        &["github_installation_link", "github_automation_update"],
    ),
    retired(
        "environment_write",
        &[
            "environment_create",
            "environment_update",
            "environment_archive",
            "environment_check",
        ],
    ),
    retired(
        "bug_report_link_write",
        &["bug_report_link_create", "bug_report_link_update"],
    ),
    retired("whiteboard_edit", &["whiteboard_update_elements"]),
    retired("message_edit", &["message_update"]),
    retired("task_comment_edit", &["task_comment_update"]),
    retired("task_criteria", &["task_criteria_update"]),
    retired("task_comment", &["task_comment_create"]),
    retired("task_mark_blocked", &["task_update"]),
    retired("task_mark_ready_for_review", &["task_update"]),
    retired("task_watch", &["task_watch_set"]),
    retired("task_depend", &["task_link"]),
    retired("task_relate", &["task_link"]),
    retired("task_link_pull_request", &["task_link"]),
    retired("mark_read", &["conversation_mark_read"]),
    retired("conversation_members", &["conversation_members_update"]),
    retired("reply_progress", &["reply_progress_report"]),
    retired("briefing", &["briefing_get"]),
    retired("settings_get", &["setting_list"]),
    retired("settings_set", &["setting_update"]),
    retired("project_layout_set", &["layout_update"]),
    retired(
        "invitation_write",
        &["invitation_create", "invitation_update"],
    ),
    retired(
        "project_template_write",
        &["project_template_create", "project_template_update"],
    ),
    retired(
        "layout_preset_write",
        &["layout_preset_create", "layout_preset_update"],
    ),
    retired("run_open", &["run_create"]),
    retired("run_activity", &["run_activity_report"]),
    retired("run_result", &["run_result_report"]),
    retired("run_dismiss_failure", &["run_failure_hide"]),
    retired("agent_profile_write", &["agent_profile_set"]),
    retired("model_catalog_write", &["model_catalog_set"]),
    retired("run_steps", &["run_step_list"]),
    retired("run_usage", &["run_usage_list"]),
    retired("usage_windows", &["usage_window_list"]),
    retired("usage_batch", &["usage_batch_preview"]),
    retired("external_event_poll", &["event_list"]),
];

/// The retired name `name`, unless an operation of that name exists now.
pub fn find(name: &str) -> Option<Retired> {
    if super::registry().contains(name) {
        return None;
    }
    RETIRED.iter().copied().find(|old| old.name == name)
}

impl Retired {
    /// What a call through the old name is told.
    pub fn message(&self) -> String {
        let instead = match self.use_instead {
            [one] => format!("`{one}`"),
            several => {
                let names = several
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>();
                let (last, rest) = names.split_last().unwrap_or((&names[0], &[]));
                format!("{} or {last}", rest.join(", "))
            }
        };
        format!(
            "`{}` was renamed: use {instead} (see tools/list).",
            self.name
        )
    }

    /// The answer to a call through the old name: not found, saying what to
    /// use, with the names in `useInstead`.
    pub fn refusal(&self) -> serde_json::Value {
        let mut body = super::OpError::Missing {
            message: self.message().into(),
        }
        .to_json();
        body["useInstead"] = serde_json::json!(self.use_instead);
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retired_names_are_well_formed() {
        let mut names = std::collections::BTreeSet::new();
        for old in RETIRED {
            assert!(names.insert(old.name), "{} twice", old.name);
            assert!(!old.use_instead.is_empty(), "{}", old.name);
            for instead in old.use_instead {
                assert_ne!(*instead, old.name, "{} renames to itself", old.name);
                assert!(
                    super::super::registry().contains(instead),
                    "{} points at {instead}, which isn't an operation",
                    old.name
                );
            }
            assert!(
                !crate::mcp::has_legacy_tool(old.name),
                "{} is retired and still a hand-written tool",
                old.name
            );
        }
    }

    #[test]
    fn an_old_name_says_what_to_use() {
        let page_write = find("page_write").expect("page_write");
        assert_eq!(
            page_write.message(),
            "`page_write` was renamed: use `page_create` or `page_update` (see tools/list)."
        );
        let automation_write = find("automation_write").expect("automation_write");
        assert_eq!(
            automation_write.message(),
            "`automation_write` was renamed: use `automation_create`, `automation_update`, `automation_delete` or `automation_run_start` (see tools/list)."
        );
        assert!(find("page_create").is_none());
    }
}
