//! Old tool names (K2 section 3.6). When a module's slice gives a tool its
//! canonical name, the old name stays callable as an alias: on MCP and the
//! public API v1, with its old arguments and its old result shape, through
//! an adapter. Each call is counted per workspace and day in
//! `operation_alias_usage`, so an alias is removed only once nobody uses
//! it (at least 180 days after the canonical operation ships, and 30 days
//! with no calls). MCP answers carry `_meta.deprecated`; the public API's
//! v1 route answers with `Deprecation` and, once a date is set, `Sunset`.
//! An answer that succeeds is the old tool's value alone, as it was; a
//! refusal keeps the operation's envelope.
//!
//! The rename table is data. A row is inert until its canonical operation
//! exists; the slice that adds the operation removes the hand-written tool
//! of the old name in the same change (a test checks they never coexist).

use std::sync::{OnceLock, RwLock};

use rmcp::model::JsonObject;
use serde_json::Value;

use super::registry;

/// An old name for an operation.
#[derive(Clone, Copy, Debug)]
pub struct Alias {
    /// The old tool name.
    pub name: &'static str,
    /// The operation it runs, chosen from the old arguments (`page_write`
    /// is a create without `pageId` and an update with it).
    pub canonical: fn(&JsonObject) -> &'static str,
    /// The canonical operations it may run, for checks and docs.
    pub runs: &'static [&'static str],
    /// The old arguments as the canonical operation's input.
    pub adapt_in: fn(JsonObject) -> JsonObject,
    /// The canonical operation's value in the old result shape.
    /// Takes the canonical name, the value and whether the answer was a replay.
    pub adapt_out: fn(&str, Value, bool) -> Value,
    /// When it stops being served, once set (a `YYYY-MM-DD` date).
    pub sunset: Option<&'static str>,
}

fn same_in(arguments: JsonObject) -> JsonObject {
    arguments
}

fn same_out(_canonical: &str, value: Value, _replayed: bool) -> Value {
    value
}

/// Tools that already answered in the `{outcome, value}` envelope (A1.3's
/// run tools) keep it.
fn enveloped(_canonical: &str, value: Value, _replayed: bool) -> Value {
    serde_json::json!({ "outcome": "ok", "value": value })
}

/// The layout and invitation tools said `workspace: true` for the whole
/// workspace; `workspace` now names one, so the flag is `wholeWorkspace`.
fn whole_workspace_in(mut arguments: JsonObject) -> JsonObject {
    // The argument named the whole workspace when it was a bool; a string
    // still names which workspace, and the edge reads it.
    const OLD: &str = "workspace";
    match arguments.remove(OLD) {
        Some(Value::Bool(flag)) => {
            arguments.insert("wholeWorkspace".to_owned(), Value::Bool(flag));
        }
        Some(named) => {
            arguments.insert(OLD.to_owned(), named);
        }
        None => {}
    }
    arguments
}

/// `page_write` (and the sheet, collection and record `_write` tools)
/// marked a retried write in its value.
fn page_written(_canonical: &str, mut value: Value, replayed: bool) -> Value {
    if replayed && let Some(object) = value.as_object_mut() {
        object.insert("idempotentReplay".to_owned(), Value::Bool(true));
    }
    serde_json::json!({ "outcome": "ok", "value": value })
}

/// The old task tools took the project as `projectId`; the operations find
/// it from the task.
fn without_project(mut arguments: JsonObject) -> JsonObject {
    arguments.remove("projectId");
    arguments
}

/// `task_mark_blocked`'s `reason` is `task_update`'s `blockedReason`.
fn mark_blocked_in(arguments: JsonObject) -> JsonObject {
    let mut arguments = without_project(arguments);
    if let Some(reason) = arguments.remove("reason") {
        arguments.insert("blockedReason".to_owned(), reason);
    }
    arguments
}

/// `task_mark_ready_for_review` is `task_update` with `readyForReview`.
fn mark_ready_in(arguments: JsonObject) -> JsonObject {
    let mut arguments = without_project(arguments);
    arguments.insert("readyForReview".to_owned(), Value::Bool(true));
    arguments
}

/// The old task write answer: the write's fields with `"outcome": "applied"`.
fn task_written(_canonical: &str, mut value: Value, _replayed: bool) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("outcome".to_owned(), Value::String("applied".to_owned()));
    }
    value
}

/// A tool renamed when it became an operation that already answered in
/// the envelope (A1.3's run tools, K2.7's message tools), answering as it
/// did.
macro_rules! run_renamed {
    ($old:literal => $new:literal) => {
        Alias {
            name: $old,
            canonical: |_| $new,
            runs: &[$new],
            adapt_in: same_in,
            adapt_out: enveloped,
            sunset: None,
        }
    };
}

macro_rules! renamed {
    ($old:literal => $new:literal) => {
        Alias {
            name: $old,
            canonical: |_| $new,
            runs: &[$new],
            adapt_in: same_in,
            adapt_out: same_out,
            sunset: None,
        }
    };
}

/// `page_write` and the other `_write` tools: a create without the thing's
/// id, an update with it.
macro_rules! written {
    ($old:literal, $id:literal => $create:literal, $update:literal) => {
        Alias {
            name: $old,
            canonical: |arguments| {
                if arguments.contains_key($id) {
                    $update
                } else {
                    $create
                }
            },
            runs: &[$create, $update],
            adapt_in: same_in,
            adapt_out: enveloped,
            sunset: None,
        }
    };
}

/// `record_write` added the entries that name no record and changed the
/// rest; a batch that names any is an update, which adds the others too.
fn record_write_runs(arguments: &JsonObject) -> &'static str {
    let names_one = arguments
        .get("records")
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records
                .iter()
                .any(|entry| entry.get("record").is_some_and(|record| !record.is_null()))
        });
    if names_one {
        "record_update_many"
    } else {
        "record_create_many"
    }
}

/// The old workspace tools named a project as `projectId`; the operations
/// take the space as `spaceId`.
fn project_as_space(mut arguments: JsonObject) -> JsonObject {
    if let Some(project) = arguments.remove("projectId") {
        arguments.insert("spaceId".to_owned(), project);
    }
    arguments
}

/// `task_link_pull_request` took `expectedVersion`, `pullRequestId` and a
/// `projectId`; `task_link` finds the space from the task.
fn link_pull_request_in(arguments: JsonObject) -> JsonObject {
    without_project(arguments)
}

/// `task_link_pull_request` answered a task write (`"outcome": "applied"`).
fn link_pull_request_out(_canonical: &str, value: Value, replayed: bool) -> Value {
    let changed = value["changed"].as_bool().unwrap_or(false);
    serde_json::json!({
        "outcome": "applied",
        "workItemId": value["taskId"],
        "version": value["version"],
        "changedFields": if changed { vec!["pullRequests"] } else { Vec::new() },
        "resultId": value["targetId"],
        "idempotentReplay": replayed,
        "key": value["key"],
        "url": value["taskUrl"],
    })
}

/// The argument a call names its workspace with (not the module).
const NAMED_WORKSPACE: &str = "workspace";

/// What every edge takes beside an operation's own arguments: the workspace
/// the call names, its request id (so a retry through the old name is
/// answered once) and a preview.
fn edge_argument(key: &str) -> bool {
    matches!(key, NAMED_WORKSPACE | "clientRequestId" | "preview")
}

/// `automation_write`'s `operation` names the automation operation it runs.
fn automation_write_runs(arguments: &JsonObject) -> &'static str {
    match arguments.get("operation").and_then(Value::as_str) {
        Some("update") => "automation_update",
        Some("delete") => "automation_delete",
        Some("run") => "automation_run_start",
        _ => "automation_create",
    }
}

/// `automation_write` took every field for every operation; each operation
/// takes only its own.
fn automation_write_in(mut arguments: JsonObject) -> JsonObject {
    let keep: &[&str] = match automation_write_runs(&arguments) {
        "automation_update" => &[
            "projectId",
            "ruleId",
            "expectedVersion",
            "name",
            "trigger",
            "conditions",
            "actions",
            "enabled",
        ],
        "automation_delete" => &["projectId", "ruleId"],
        "automation_run_start" => &["projectId", "ruleId", "taskId"],
        _ => &[
            "projectId",
            "name",
            "trigger",
            "conditions",
            "actions",
            "enabled",
        ],
    };
    // What the edge reads (the workspace, request id, preview) stays.
    arguments.retain(|key, value| {
        (keep.contains(&key.as_str()) || edge_argument(key)) && !value.is_null()
    });
    arguments
}

/// `environment_write` archived or checked with a flag, changed with an
/// `environment` and added without one.
fn environment_write_runs(arguments: &JsonObject) -> &'static str {
    let flag = |name: &str| arguments.get(name).and_then(Value::as_bool) == Some(true);
    if flag("archive") {
        "environment_archive"
    } else if flag("check") {
        "environment_check"
    } else if arguments.contains_key("environment") {
        "environment_update"
    } else {
        "environment_create"
    }
}

/// `environment_write`'s flags name the operation; archive and check took
/// nothing else (the operations refuse what they don't take).
fn environment_write_in(mut arguments: JsonObject) -> JsonObject {
    let canonical = environment_write_runs(&arguments);
    arguments.retain(|key, value| !value.is_null() && key != "archive" && key != "check");
    if canonical == "environment_create" {
        arguments.remove("expectedVersion");
    }
    arguments
}

/// `repository_write` prepared or archived the repository it named, and
/// added one otherwise.
fn repository_write_runs(arguments: &JsonObject) -> &'static str {
    let flag = |name: &str| arguments.get(name).and_then(Value::as_bool) == Some(true);
    if arguments.contains_key("repository") && flag("archive") {
        "repository_archive"
    } else if arguments.contains_key("repository") && flag("prepare") {
        "repository_prepare"
    } else {
        "repository_create"
    }
}

fn repository_write_in(arguments: JsonObject) -> JsonObject {
    let keep: &[&str] = match repository_write_runs(&arguments) {
        "repository_create" => &[
            "projectId",
            "owner",
            "name",
            "githubRepositoryId",
            "installationId",
            "defaultBranch",
            "trustAcknowledged",
            // Refused by the operation, as repository_write refused them.
            "repository",
            "prepare",
            "archive",
        ],
        _ => &["projectId", "repository", "expectedVersion"],
    };
    keep_only(arguments, keep)
}

/// `github_integration_write` linked an installation or switched the
/// automation.
fn github_integration_write_runs(arguments: &JsonObject) -> &'static str {
    if arguments.contains_key("installationId") || arguments.contains_key("repositories") {
        "github_installation_link"
    } else {
        "github_automation_update"
    }
}

fn github_integration_write_in(arguments: JsonObject) -> JsonObject {
    // Mixing the two was refused; the operation refuses the other's fields.
    let _ = github_integration_write_runs(&arguments);
    keep_only(
        arguments,
        &[
            "projectId",
            "installationId",
            "repositories",
            "pullRequestOpen",
            "pullRequestMerge",
            "expectedVersion",
        ],
    )
}

/// The arguments one operation takes, and what the edge reads.
fn keep_only(mut arguments: JsonObject, keep: &[&str]) -> JsonObject {
    arguments.retain(|key, value| {
        (keep.contains(&key.as_str()) || edge_argument(key)) && !value.is_null()
    });
    arguments
}

/// The rename plan of K2 section 3.6. Adapters beyond the identity land
/// with each row's canonical operation, in its module's slice.
pub const ALIASES: &[Alias] = &[
    // K2.15: the read tools answered in the envelope already.
    run_renamed!("projects_list" => "project_list"),
    run_renamed!("project_open" => "project_get"),
    run_renamed!("task_open" => "task_get"),
    run_renamed!("my_work" => "my_work_get"),
    run_renamed!("dependency_graph" => "dependency_graph_get"),
    run_renamed!("search" => "workspace_search"),
    renamed!("conversation_open" => "conversation_get"),
    renamed!("thing_open" => "thing_get"),
    Alias {
        name: "page_write",
        canonical: |arguments| {
            if arguments.contains_key("pageId") {
                "page_update"
            } else {
                "page_create"
            }
        },
        runs: &["page_create", "page_update"],
        adapt_in: same_in,
        adapt_out: page_written,
        sunset: None,
    },
    // K2.10: the page tools answered in the envelope.
    run_renamed!("page_edit" => "page_update_blocks"),
    run_renamed!("page_blocks" => "page_blocks_get"),
    run_renamed!("page_comments" => "page_comment_list"),
    run_renamed!("page_comment" => "page_comment_create"),
    run_renamed!("page_suggestions" => "page_suggestion_list"),
    // K2.11: the sheet and collection tools answered in the envelope.
    run_renamed!("sheet_read" => "sheet_get"),
    Alias {
        name: "sheet_write",
        canonical: |arguments| {
            if arguments.contains_key("sheet") {
                "sheet_update"
            } else {
                "sheet_create"
            }
        },
        runs: &["sheet_create", "sheet_update"],
        adapt_in: same_in,
        adapt_out: page_written,
        sunset: None,
    },
    run_renamed!("collection_describe" => "collection_get"),
    Alias {
        name: "collection_write",
        canonical: |arguments| {
            if arguments.contains_key("collection") {
                "collection_update"
            } else {
                "collection_create"
            }
        },
        runs: &["collection_create", "collection_update"],
        adapt_in: same_in,
        adapt_out: page_written,
        sunset: None,
    },
    run_renamed!("record_query" => "record_list"),
    Alias {
        name: "record_write",
        canonical: record_write_runs,
        runs: &["record_create_many", "record_update_many"],
        adapt_in: same_in,
        adapt_out: page_written,
        sunset: None,
    },
    written!("view_write", "view" => "view_create", "view_update"),
    written!("label_write", "label" => "label_create", "label_update"),
    written!("field_write", "field" => "field_create", "field_update"),
    written!("milestone_write", "milestoneId" => "milestone_create", "milestone_update"),
    written!("template_write", "template" => "template_create", "template_update"),
    // K2.14: recurring_write names the schedule as `recurring`.
    written!("recurring_write", "recurring" => "recurring_create", "recurring_update"),
    // K2.18: automation_write chose by `operation`; automation_runs is
    // automation_history. Both answered in the envelope already.
    Alias {
        name: "automation_write",
        canonical: automation_write_runs,
        runs: &[
            "automation_create",
            "automation_update",
            "automation_delete",
            "automation_run_start",
        ],
        adapt_in: automation_write_in,
        adapt_out: enveloped,
        sunset: None,
    },
    run_renamed!("automation_runs" => "automation_history"),
    // K2.18: repository_write and github_integration_write chose by their
    // fields; both answered in the envelope already.
    Alias {
        name: "repository_write",
        canonical: repository_write_runs,
        runs: &[
            "repository_create",
            "repository_prepare",
            "repository_archive",
        ],
        adapt_in: repository_write_in,
        adapt_out: enveloped,
        sunset: None,
    },
    Alias {
        name: "github_integration_write",
        canonical: github_integration_write_runs,
        runs: &["github_installation_link", "github_automation_update"],
        adapt_in: github_integration_write_in,
        adapt_out: enveloped,
        sunset: None,
    },
    // K2.18: environment_write chose by its fields; it answered in the
    // envelope already.
    Alias {
        name: "environment_write",
        canonical: environment_write_runs,
        runs: &[
            "environment_create",
            "environment_update",
            "environment_archive",
            "environment_check",
        ],
        adapt_in: environment_write_in,
        adapt_out: enveloped,
        sunset: None,
    },
    written!("bug_report_link_write", "linkId" => "bug_report_link_create", "bug_report_link_update"),
    // K2.12: whiteboard_edit answered in the envelope already.
    run_renamed!("whiteboard_edit" => "whiteboard_update_elements"),
    // K2.7: the message tools answered in the envelope already.
    run_renamed!("message_edit" => "message_update"),
    // K2.13: the comment-edit and criteria tools answered in the envelope
    // already; task_comment and the two mark tools answered a task write
    // (`"outcome": "applied"`), and took a projectId the operations find
    // from the task.
    run_renamed!("task_comment_edit" => "task_comment_update"),
    run_renamed!("task_criteria" => "task_criteria_update"),
    Alias {
        name: "task_comment",
        canonical: |_| "task_comment_create",
        runs: &["task_comment_create"],
        adapt_in: without_project,
        adapt_out: task_written,
        sunset: None,
    },
    Alias {
        name: "task_mark_blocked",
        canonical: |_| "task_update",
        runs: &["task_update"],
        adapt_in: mark_blocked_in,
        adapt_out: task_written,
        sunset: None,
    },
    Alias {
        name: "task_mark_ready_for_review",
        canonical: |_| "task_update",
        runs: &["task_update"],
        adapt_in: mark_ready_in,
        adapt_out: task_written,
        sunset: None,
    },
    // K2.14: these answered in the envelope already; task_link takes the
    // arguments of each.
    run_renamed!("task_watch" => "task_watch_set"),
    run_renamed!("task_depend" => "task_link"),
    run_renamed!("task_relate" => "task_link"),
    Alias {
        name: "task_link_pull_request",
        canonical: |_| "task_link",
        runs: &["task_link"],
        adapt_in: link_pull_request_in,
        adapt_out: link_pull_request_out,
        sunset: None,
    },
    run_renamed!("mark_read" => "conversation_mark_read"),
    // K2.8: answered in the envelope already.
    run_renamed!("conversation_members" => "conversation_members_update"),
    // K2.9: the reply tools answered in an envelope of their own; the
    // operations' refusals carry a message too.
    run_renamed!("reply_progress" => "reply_progress_report"),
    run_renamed!("briefing" => "briefing_get"),
    // K2.17: the settings tools named the project as projectId.
    Alias {
        name: "settings_get",
        canonical: |_| "setting_list",
        runs: &["setting_list"],
        adapt_in: project_as_space,
        adapt_out: enveloped,
        sunset: None,
    },
    Alias {
        name: "settings_set",
        canonical: |_| "setting_update",
        runs: &["setting_update"],
        adapt_in: project_as_space,
        adapt_out: enveloped,
        sunset: None,
    },
    renamed!("project_member_set" => "member_update"),
    // K2.17: the layout tool answered in the envelope.
    Alias {
        name: "project_layout_set",
        canonical: |_| "layout_update",
        runs: &["layout_update"],
        adapt_in: whole_workspace_in,
        adapt_out: enveloped,
        sunset: None,
    },
    Alias {
        name: "invitation_write",
        canonical: |arguments| {
            if arguments.contains_key("invitation") {
                "invitation_update"
            } else {
                "invitation_create"
            }
        },
        runs: &["invitation_create", "invitation_update"],
        adapt_in: whole_workspace_in,
        adapt_out: enveloped,
        sunset: None,
    },
    written!("project_template_write", "template" => "project_template_create", "project_template_update"),
    written!("layout_preset_write", "preset" => "layout_preset_create", "layout_preset_update"),
    renamed!("ai_summarize" => "conversation_summarize"),
    renamed!("ai_analyze" => "thing_analyze"),
    run_renamed!("run_open" => "run_create"),
    run_renamed!("run_activity" => "run_activity_report"),
    run_renamed!("run_result" => "run_result_report"),
    // K2.19: the run and settings tools answered in the envelope.
    run_renamed!("run_dismiss_failure" => "run_failure_hide"),
    run_renamed!("agent_profile_write" => "agent_profile_set"),
    run_renamed!("model_catalog_write" => "model_catalog_set"),
    run_renamed!("run_steps" => "run_step_list"),
    run_renamed!("run_usage" => "run_usage_list"),
    run_renamed!("usage_windows" => "usage_window_list"),
    run_renamed!("usage_batch" => "usage_batch_preview"),
];

/// Aliases added by tests outside the crate.
fn extras() -> &'static RwLock<Vec<Alias>> {
    static EXTRAS: OnceLock<RwLock<Vec<Alias>>> = OnceLock::new();
    EXTRAS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Adds an alias for the tests (integration tests can't extend the table).
pub fn register_alias_for_tests(alias: Alias) {
    if let Ok(mut extras) = extras().write() {
        extras.retain(|existing| existing.name != alias.name);
        extras.push(alias);
    }
}

/// The alias of this name whose canonical operations exist, if any.
pub fn active(name: &str) -> Option<Alias> {
    let registry = registry();
    let live =
        |alias: &Alias| alias.name == name && alias.runs.iter().all(|runs| registry.contains(runs));
    ALIASES.iter().copied().find(live).or_else(|| {
        extras()
            .read()
            .ok()
            .and_then(|extras| extras.iter().copied().find(live))
    })
}

impl Alias {
    /// The `Sunset` header's value (RFC 8594: an HTTP date), once set.
    pub fn sunset_header(&self) -> Option<String> {
        let date = chrono::NaiveDate::parse_from_str(self.sunset?, "%Y-%m-%d").ok()?;
        Some(date.format("%a, %d %b %Y 00:00:00 GMT").to_string())
    }
}

/// What an MCP answer through an alias says about it.
pub fn deprecation(alias: &Alias, canonical: &str) -> String {
    match alias.sunset {
        Some(date) => format!("use {canonical}; {} stops working after {date}", alias.name),
        None => format!("use {canonical}"),
    }
}

/// Counts one call through an alias (`operation_alias_usage`).
pub(super) async fn count(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    workspace_id: uuid::Uuid,
    alias: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO operation_alias_usage (workspace_id, alias, day, calls)
        VALUES ($1, $2, current_date, 1)
        ON CONFLICT (workspace_id, alias, day)
        DO UPDATE SET calls = operation_alias_usage.calls + 1
        "#,
    )
    .bind(workspace_id)
    .bind(alias)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rename_table_is_well_formed() {
        let mut names = std::collections::BTreeSet::new();
        for alias in ALIASES {
            assert!(names.insert(alias.name), "{} twice", alias.name);
            assert!(!alias.runs.is_empty(), "{}", alias.name);
            for canonical in alias.runs {
                assert_ne!(*canonical, alias.name, "{} renames to itself", alias.name);
                super::super::conventions::check_name(canonical, None)
                    .unwrap_or_else(|problem| panic!("{}: {problem}", alias.name));
            }
            // The adapters' pick is one of the operations it runs.
            let picked = (alias.canonical)(&JsonObject::new());
            assert!(alias.runs.contains(&picked), "{}", alias.name);
        }
    }

    #[test]
    fn an_active_alias_has_no_hand_written_tool() {
        for alias in ALIASES {
            if active(alias.name).is_some() {
                assert!(
                    !crate::mcp::has_legacy_tool(alias.name),
                    "{} is an alias of {:?} and still a hand-written tool: remove the tool",
                    alias.name,
                    alias.runs
                );
            }
        }
    }

    #[test]
    fn a_write_alias_picks_create_or_update() {
        let page_write = ALIASES
            .iter()
            .find(|alias| alias.name == "page_write")
            .expect("page_write");
        let mut arguments = JsonObject::new();
        assert_eq!((page_write.canonical)(&arguments), "page_create");
        arguments.insert("pageId".into(), Value::String("p".into()));
        assert_eq!((page_write.canonical)(&arguments), "page_update");
    }

    #[test]
    fn an_alias_answers_in_its_old_shape() {
        // A test alias with real adapters, as a module's slice writes them:
        // the old name took `name` and answered `{ thing: { name } }`.
        let alias = Alias {
            name: "thing_write",
            canonical: |_| "thing_create",
            runs: &["thing_create"],
            adapt_in: |mut arguments| {
                if let Some(name) = arguments.remove("name") {
                    arguments.insert("title".into(), name);
                }
                arguments
            },
            adapt_out: |_, value, _| serde_json::json!({ "thing": { "name": value["title"] } }),
            sunset: Some("2027-06-01"),
        };
        let mut old = JsonObject::new();
        old.insert("name".into(), Value::String("Kept".into()));
        let input = (alias.adapt_in)(old);
        assert_eq!(Value::Object(input), serde_json::json!({ "title": "Kept" }));
        let answered = (alias.adapt_out)(
            "thing_create",
            serde_json::json!({ "id": "t-1", "title": "Kept" }),
            false,
        );
        assert_eq!(
            serde_json::to_string(&answered).expect("JSON"),
            r#"{"thing":{"name":"Kept"}}"#
        );
        assert_eq!(
            deprecation(&alias, "thing_create"),
            "use thing_create; thing_write stops working after 2027-06-01"
        );
        assert_eq!(
            alias.sunset_header().as_deref(),
            Some("Tue, 01 Jun 2027 00:00:00 GMT")
        );
    }
}
