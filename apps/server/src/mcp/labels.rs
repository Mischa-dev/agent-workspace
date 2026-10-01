//! Label tools: list a project's labels, and create, rename, recolor,
//! describe, archive or restore one (docs/plans/mcp-tools.md, Phase 2).
//! Labels go on tasks through task_create and task_update; the rules for
//! labels live in `crate::work_item_labels`.
//!
//! `label` is never resolved by the MCP middleware: it is looked up only
//! among the labels of `principal.project_id`, so an ID from another project
//! never matches.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::{
    McpPrincipal,
    common::{ARCHIVED_PROJECT_MESSAGE, ToolResult, insert_audit, mark_via, parse_id},
    talk::can_write_in,
    tasks::command_actor,
};
use crate::{
    AppState,
    error::AppError,
    memory,
    policy::{Action, Decision},
    work_item_commands::{authorize_mutation, lock_membership},
    work_item_labels::{
        self, ChangeLabelOutcome, CreateLabelOutcome, LabelChange, LabelLimit, LabelLookup,
        LabelRow, MAX_ACTIVE_LABELS_PER_PROJECT, NewLabel, echo,
    },
};

const MESSAGES_SPACE_MESSAGE: &str =
    "The Messages space has no tasks, so it has no labels. Pick a project with projects_list.";
const NAME_MESSAGE: &str =
    "Give the label a name of 1 to 50 characters on one line, that isn't an ID.";
const COLOR_MESSAGE: &str = "Use a color like #3b82f6, or one of: red, orange, yellow, green, teal, blue, purple, pink, gray.";
const DESCRIPTION_MESSAGE: &str = "Keep the description to one line of at most 200 characters.";
const NOTHING_TO_CHANGE_MESSAGE: &str =
    "Say what to change: name, color, description, or archived.";
const DENIED_MESSAGE: &str =
    "Only project members can create or change labels here; viewers can see them with label_list.";

// ---------------------------------------------------------------------------
// label_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LabelListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Also list archived labels, which label_write can restore.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LabelLine {
    pub id: String,
    pub name: String,
    /// "#rrggbb".
    pub color: String,
    /// What the label is for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub archived: bool,
    /// Tasks that carry it (not counting archived tasks). For an archived
    /// label, the tasks restoring it brings it back on.
    pub task_count: i64,
    /// Pass as expectedVersion to label_write.
    pub version: i64,
}

impl From<LabelRow> for LabelLine {
    fn from(row: LabelRow) -> Self {
        Self {
            id: row.id.to_string(),
            name: row.name,
            color: row.color,
            description: row.description,
            archived: row.archived,
            task_count: row.task_count,
            version: row.version,
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LabelListValue {
    /// Active labels by name, then archived ones when asked for.
    pub labels: Vec<LabelLine>,
}

pub(super) async fn label_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LabelListArguments,
) -> ToolResult<LabelListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    // The middleware turned projectId into the project it checked the caller
    // can reach; anything else is a project it couldn't resolve.
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        let messages_space = memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            Action::WorkItemView,
        )
        .await?;
        if messages_space {
            return Ok(ToolResult::invalid("projectId", MESSAGES_SPACE_MESSAGE));
        }
        let labels = work_item_labels::list(
            &state.pool,
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(LabelListValue {
            labels: labels.into_iter().map(LabelLine::from).collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "label_list"))
}

// ---------------------------------------------------------------------------
// label_write

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LabelWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The label to change, by ID or current name. Leave it out to create one.
    pub label: Option<String>,
    /// The name for a new label, or a new name.
    pub name: Option<String>,
    /// "#rrggbb", "#rgb", or red, orange, yellow, green, teal, blue, purple,
    /// pink, gray. Left out on create, one is picked from the name.
    pub color: Option<String>,
    /// What the label is for, on one line. "" clears it.
    pub description: Option<String>,
    /// true archives it (hidden from every task until restored), false
    /// restores it.
    pub archived: Option<bool>,
    /// The version from label_list. Leave it out to change the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LabelWriteValue {
    pub label: LabelLine,
    /// False when it already existed, or nothing needed to change.
    pub created: bool,
    /// "label" on create; otherwise any of name, color, description,
    /// archived. Empty when nothing changed.
    pub changed_fields: Vec<String>,
    /// Something worth knowing, such as how many tasks an archive hides it
    /// from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// label_write's arguments, checked and normalized before anything is read.
struct Checked {
    label: Option<String>,
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
    archived: Option<bool>,
    expected_version: Option<i64>,
}

/// The field at fault and what would work.
type Problem = (&'static str, &'static str);

fn check(arguments: LabelWriteArguments) -> Result<Checked, Problem> {
    let name = match arguments.name.as_deref() {
        Some(value) => Some(work_item_labels::normalize_name(value).ok_or(("name", NAME_MESSAGE))?),
        None => None,
    };
    let color = match arguments.color.as_deref() {
        Some(value) => {
            Some(work_item_labels::normalize_color(value).ok_or(("color", COLOR_MESSAGE))?)
        }
        None => None,
    };
    let description = match arguments.description.as_deref() {
        Some(value) => Some(
            work_item_labels::normalize_label_description(value)
                .ok_or(("description", DESCRIPTION_MESSAGE))?,
        ),
        None => None,
    };
    if arguments
        .expected_version
        .is_some_and(|version| version < 1)
    {
        return Err((
            "expectedVersion",
            "Pass the version label_list shows for the label.",
        ));
    }
    let label = match arguments.label.as_deref().map(str::trim) {
        Some("") => {
            return Err((
                "label",
                "Pass the label's name or ID, or leave label out to create one.",
            ));
        }
        Some(label) => Some(label.to_owned()),
        None => None,
    };
    if label.is_none() {
        if name.is_none() {
            return Err((
                "name",
                "Give the new label a name, or pass label to change one.",
            ));
        }
        if arguments.archived == Some(true) {
            return Err(("archived", "A new label can't start archived."));
        }
        if arguments.expected_version.is_some() {
            return Err((
                "expectedVersion",
                "expectedVersion is for changing a label; leave it out to create one.",
            ));
        }
    } else if name.is_none()
        && color.is_none()
        && description.is_none()
        && arguments.archived.is_none()
    {
        return Err(("label", NOTHING_TO_CHANGE_MESSAGE));
    }
    Ok(Checked {
        label,
        name,
        color,
        description,
        archived: arguments.archived,
        expected_version: arguments.expected_version,
    })
}

fn full_message(key: &str, limit: LabelLimit) -> String {
    match limit {
        LabelLimit::Active => format!(
            "{key} already has {MAX_ACTIVE_LABELS_PER_PROJECT} labels, the most a project can have. Archive ones you no longer use (label_list shows how many tasks carry each), then try again."
        ),
        LabelLimit::Total => format!(
            "{key} keeps {} labels counting archived ones, the most a project can. Restore and rename an archived label (label_list with includeArchived) instead.",
            work_item_labels::MAX_LABEL_ROWS_PER_PROJECT
        ),
    }
}

fn tasks_phrase(count: i64) -> String {
    if count == 1 {
        "1 task".to_owned()
    } else {
        format!("{count} tasks")
    }
}

pub(super) async fn label_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LabelWriteArguments,
) -> ToolResult<LabelWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let checked = match check(arguments) {
        Ok(checked) => checked,
        Err((field, message)) => return ToolResult::invalid(field, message),
    };
    let project_id = principal.project_id;
    let result = async {
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied(DENIED_MESSAGE));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let Some(membership) =
            lock_membership(&mut transaction, principal.actor_id, project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !membership.enabled {
            return Ok(ToolResult::not_found("That project"));
        }
        if membership.project_archived_at.is_some() {
            return Ok(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE));
        }
        let decision = authorize_mutation(
            &command_actor(principal, project_id),
            &membership,
            Action::WorkItemEdit,
            None,
        )?;
        if !matches!(decision, Decision::Allow) {
            return Ok(ToolResult::denied(DENIED_MESSAGE));
        }
        let (kind, key) = sqlx::query_as::<_, (String, String)>(
            "SELECT kind, project_key FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if kind == "direct" {
            return Ok(ToolResult::invalid("projectId", MESSAGES_SPACE_MESSAGE));
        }

        let Some(reference) = checked.label.as_deref() else {
            // Create.
            let name = checked.name.as_deref().unwrap_or_default();
            let outcome = work_item_labels::create_label(
                &mut transaction,
                project_id,
                principal.actor_id,
                NewLabel {
                    name,
                    color: checked.color.as_deref(),
                    description: checked.description.as_deref().unwrap_or_default(),
                },
            )
            .await?;
            let value = match outcome {
                CreateLabelOutcome::Created(row) => {
                    insert_audit(
                        &mut transaction,
                        principal,
                        project_id,
                        "work_item_label.mcp_created",
                        "work_item_label",
                        row.id,
                        serde_json::json!({
                            "changedFields": ["label"],
                            "name": row.name,
                            "color": row.color,
                        }),
                    )
                    .await?;
                    LabelWriteValue {
                        label: row.into(),
                        created: true,
                        changed_fields: vec!["label".to_owned()],
                        note: None,
                    }
                }
                CreateLabelOutcome::Exists(row) => LabelWriteValue {
                    note: Some(format!(
                        "A label called \"{0}\" already exists, so nothing changed. To change it, pass label: \"{0}\".",
                        row.name
                    )),
                    label: row.into(),
                    created: false,
                    changed_fields: Vec::new(),
                },
                CreateLabelOutcome::ArchivedNameTaken(row) => {
                    return Ok(ToolResult::conflict(format!(
                        "An archived label called \"{}\" exists in {key}. Restore it with label_write {{label: \"{}\", archived: false}}, or pick another name.",
                        row.name, row.id
                    )));
                }
                CreateLabelOutcome::ProjectFull(limit) => {
                    return Ok(ToolResult::conflict(full_message(&key, limit)));
                }
            };
            transaction.commit().await?;
            return Ok(ToolResult::ok(value));
        };

        // Change.
        let label_id = match work_item_labels::find(&mut transaction, project_id, reference).await? {
            LabelLookup::Found(id) => id,
            LabelLookup::NotFound => {
                return Ok(ToolResult::NotFound {
                    message: format!(
                        "No label called \"{}\" in {key}. label_list shows its labels; leave out label to create one.",
                        echo(reference)
                    ),
                });
            }
            LabelLookup::AmbiguousArchived => {
                return Ok(ToolResult::invalid(
                    "label",
                    format!(
                        "Several archived labels are called \"{}\". Pass the ID from label_list (includeArchived: true).",
                        echo(reference)
                    ),
                ));
            }
        };
        let restoring = checked.archived == Some(false);
        let outcome = work_item_labels::change_label(
            &mut transaction,
            project_id,
            label_id,
            principal.actor_id,
            LabelChange {
                name: checked.name.as_deref(),
                color: checked.color.as_deref(),
                description: checked.description.as_deref(),
                archived: checked.archived,
                expected_version: checked.expected_version,
            },
        )
        .await?;
        let value = match outcome {
            ChangeLabelOutcome::Changed {
                label,
                previous,
                changed_fields,
            } => {
                let action = if label.archived != previous.archived {
                    if label.archived {
                        "work_item_label.mcp_archived"
                    } else {
                        "work_item_label.mcp_restored"
                    }
                } else {
                    "work_item_label.mcp_updated"
                };
                insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    action,
                    "work_item_label",
                    label.id,
                    serde_json::json!({
                        "changedFields": changed_fields,
                        "previous": {
                            "name": previous.name,
                            "color": previous.color,
                            "description": previous.description,
                        },
                        "previousVersion": previous.version,
                    }),
                )
                .await?;
                let note = if label.archived == previous.archived || label.task_count == 0 {
                    None
                } else if label.archived {
                    Some(format!(
                        "Hidden from {} until it's restored (label_write with archived: false).",
                        tasks_phrase(label.task_count)
                    ))
                } else {
                    Some(format!("Back on {}.", tasks_phrase(label.task_count)))
                };
                LabelWriteValue {
                    label: label.into(),
                    created: false,
                    changed_fields: changed_fields.into_iter().map(str::to_owned).collect(),
                    note,
                }
            }
            ChangeLabelOutcome::Unchanged(row) => LabelWriteValue {
                label: row.into(),
                created: false,
                changed_fields: Vec::new(),
                note: None,
            },
            ChangeLabelOutcome::NotFound => {
                return Ok(ToolResult::not_found("That label"));
            }
            ChangeLabelOutcome::VersionConflict(row) => {
                return Ok(ToolResult::conflict(format!(
                    "Someone changed this label first (now version {}). Read it again with label_list, then retry.",
                    row.version
                )));
            }
            ChangeLabelOutcome::NameTaken { other } => {
                return Ok(ToolResult::conflict(if restoring {
                    format!(
                        "Another active label is called \"{other}\" now. Pass a new name as you restore it, or archive the other one first."
                    )
                } else {
                    format!(
                        "There's already a label called \"{other}\" in {key}. Pick another name, or archive that one first."
                    )
                }));
            }
            ChangeLabelOutcome::ProjectFull(limit) => {
                return Ok(ToolResult::conflict(full_message(&key, limit)));
            }
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "label_write"))
}
