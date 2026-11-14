//! Moving a task (and its subtasks) to another project, for the web app and
//! for `task_move` (AWD-102; docs/plans/mcp-tools.md, decision 1).
//!
//! A task can't change projects in place: its comments are immutable, and
//! its links and authors belong to its project. So a move carries the task
//! over into the target project (title, description, priority, status and
//! type by name, the assignee if they're there, start and due dates, the
//! estimate, the milestone by name if the target has it, acceptance
//! criteria, subtasks, labels, custom field values that fit, and every
//! comment quoted with its author and date), then archives the original. The
//! original keeps its history, links and every field value, and its key
//! keeps opening the new task (migration 0043).
//!
//! Custom field values carry over only into a compatible field of the
//! target (same name and type) and only when they mean the same thing there:
//! an option with the same name, a person who can hold tasks there, or a
//! link to a task that moves in the same batch (which then points at its new
//! copy). Anything else is an issue the mover settles before confirming:
//! put a value in a compatible target field, or leave it on the original.
//!
//! The flow is a preview, then one confirmation:
//! - `preview` changes nothing. It shows the mover, and only them, what
//!   carries over and what needs a choice, read through the same
//!   field-value projection the task page uses, and returns a snapshot of
//!   everything the plan depends on.
//! - `execute` locks both projects and the task tree, checks access again,
//!   plans again and refuses if the snapshot changed, checks every choice
//!   in the target, then moves everything in one transaction or nothing.
//!   With a request ID the whole request (target, snapshot and choices) is
//!   one digest bound to the sender, kept with the move's original result
//!   for as long as the archived original lasts
//!   (`work_item_move_receipts`): the same request gets that result back,
//!   however late; the same ID with anything changed is a conflict.
//!
//! What stays behind is never named in the target: its history, the
//! provenance comment, audit rows and the stored retry result hold counts
//! and IDs, not the values. The mover reads the details in the preview and
//! once in the confirmation's response.

use std::collections::{HashMap, HashSet};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    fields::{self, FieldDefinition, FieldType, LinkTarget, Normalized},
    idempotency::{IdempotencyKey, RequestHash},
    message_content::MessageContent,
    policy::{Action, Decision},
    work_item_commands::{
        AllocatedNumberMetadata, ChangeFinalization, CommandActor, CreateWorkItemCommand,
        CreateWorkItemOutcome, MAX_SUBTASK_LEVELS, NewTaskFields, ProjectMembership,
        authorize_mutation, create_work_item_with_id, finalize_change, finalize_comment,
        finalize_copied_comment, insert_criteria, lock_membership, read_membership,
        record_audit_for, validate_assignee,
    },
    work_item_fields::{self, FieldValueChange, FieldValueChanges, Writer},
    work_item_labels::{self, LabelLimit, MAX_LABELS_PER_TASK, SkipReason},
};

/// The most tasks (a task and its subtasks) one move carries.
pub const MAX_TASKS: usize = 100;
/// The most choices one confirmation takes.
pub const MAX_RESOLUTIONS: usize = 1_000;
/// The most comments carried over per task; the rest stay on the original.
const MAX_COMMENTS: usize = 200;
/// Comments are at most 20,000 characters; leave room for the quote header.
/// Each carried comment gets the clock time, so they keep their order within
/// the one transaction.
const MAX_QUOTED_CHARACTERS: usize = 19_000;

// ---------------------------------------------------------------------------
// Who moves, and through what

/// Where the move came from, for audit action names: MCP keeps the names
/// `task_move` always wrote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Surface {
    Mcp,
    Web,
}

impl Surface {
    fn moved_in(self) -> &'static str {
        match self {
            Self::Mcp => "work_item.mcp_moved_in",
            Self::Web => "work_item.moved_in",
        }
    }

    fn moved_out(self) -> &'static str {
        match self {
            Self::Mcp => "work_item.mcp_moved_out",
            Self::Web => "work_item.moved_out",
        }
    }

    fn move_comment(self) -> &'static str {
        match self {
            Self::Mcp => "work_item.mcp_move_comment",
            Self::Web => "work_item.move_comment",
        }
    }

    fn label_created(self) -> &'static str {
        match self {
            Self::Mcp => "work_item_label.mcp_created",
            Self::Web => "work_item_label.created",
        }
    }
}

/// The person or agent moving, as each surface authenticated them.
pub struct Mover {
    pub actor_id: Uuid,
    /// An agent's connection, which limits the projects it reaches
    /// (`actor_can_access_project`); None for a person.
    pub external_agent_connection_id: Option<Uuid>,
    /// The app grant a person acts through, for rows that record it.
    pub via_grant: Option<Uuid>,
    /// A project-scoped credential reaches only its own project.
    pub confined_to: Option<Uuid>,
    /// Attribution in the source project.
    pub source: CommandActor,
    /// Attribution in the target project.
    pub target: CommandActor,
    pub surface: Surface,
}

/// Why a move (or its preview) didn't happen. Each surface words these as
/// its own refusals; none of them echoes a value that stays behind.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// What wasn't found, or isn't visible: "That task", "That project".
    NotFound(&'static str),
    Denied(String),
    Invalid {
        field: String,
        message: String,
    },
    Conflict(String),
    /// Tasks is off in the target project.
    ModuleOff,
}

impl Refusal {
    fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            message: message.into(),
        }
    }
}

type Refusable<T> = Result<Result<T, Refusal>, AppError>;

/// Returns the refusal from a `Refusable` step.
macro_rules! refuse {
    ($step:expr) => {
        match $step? {
            Ok(value) => value,
            Err(refusal) => return Ok(Err(refusal)),
        }
    };
}

// ---------------------------------------------------------------------------
// What a caller sends

/// A choice for one issue in a preview.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resolution {
    /// The issue's `id` from the preview.
    pub issue: String,
    /// "leave" keeps the value on the original only. "set" puts a value in
    /// `targetField`: the same value when it fits there (omit `value`), or
    /// the `value` you give, checked as `task_update` checks fields.
    pub action: ResolutionAction,
    /// For "set": a field of the target project, by name or ID, of the same
    /// type as the original field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_field: Option<String>,
    /// For "set": the value in the target, as `task_update` takes it (an
    /// option's name, a person's @handle or ID, a task's key, a page's
    /// link). Omit it to carry the original value as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionAction {
    Leave,
    Set,
}

/// A confirmed move.
pub struct MoveRequest {
    pub task_id: Uuid,
    pub target_project_id: Uuid,
    /// The preview's snapshot. Without it (older `task_move` calls), values
    /// that fit carry over and every issue stays on the original.
    pub snapshot: Option<String>,
    pub resolutions: Vec<Resolution>,
    /// Leave every issue without a resolution on the original.
    pub leave_unresolved: bool,
    pub idempotency_key: Option<IdempotencyKey>,
}

/// Everything one confirmation asks for, as its idempotency fingerprint:
/// the same key with any of this changed is a conflict.
#[derive(Serialize)]
struct Fingerprint<'a> {
    task_id: Uuid,
    target_project_id: Uuid,
    snapshot: Option<&'a str>,
    resolutions: &'a [Resolution],
    leave_unresolved: bool,
}

// ---------------------------------------------------------------------------
// What comes back

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovedTask {
    /// The old key, which now opens the new task.
    pub from: String,
    /// The new key.
    pub to: String,
    pub id: String,
    /// Empty when you can't see tasks in its project.
    pub title: String,
    pub url: String,
}

/// A move's result.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveDone {
    /// The task first, then its subtasks.
    pub moved: Vec<MovedTask>,
    /// Anything that didn't carry over exactly. Only the mover reads these,
    /// once; they aren't kept anywhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// False when the task had already been moved; `moved` says where it
    /// went; this request changed nothing.
    pub created: bool,
    /// True when this is the stored result of the same request, sent again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
    /// How many custom field values stayed on the original only.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub left_behind: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// A confirmed move's receipt (work_item_move_receipts): keys, IDs and a
/// count, as the move made them. Titles are read again with the caller's
/// access then; notes aren't kept.
#[derive(Debug, FromRow)]
struct ReceiptRow {
    target_project_id: Uuid,
    request_hash: Vec<u8>,
    moved: sqlx::types::Json<Vec<ReceiptTask>>,
    left_behind: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptTask {
    from: String,
    to: String,
    id: Uuid,
}

/// A preview: private to the mover, and nothing changes.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    pub target_project_id: String,
    pub target_project_key: String,
    /// The task first, then its subtasks, as they would be created.
    pub tasks: Vec<PlannedTask>,
    /// Field values that carry over as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub carried: Vec<CarriedValue>,
    /// Values that need a choice before the move: put them in a target
    /// field, or leave them on the original.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<MoveIssue>,
    /// Everything else that won't carry over exactly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Values in archived fields, which stay on the original.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub archived_field_values: u32,
    /// Pass this back with your choices. A move refuses it if anything the
    /// preview showed has changed since.
    pub snapshot: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlannedTask {
    pub id: String,
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CarriedValue {
    pub task_id: String,
    pub task_key: String,
    pub field: String,
    pub target_field: String,
    /// The value as it reads now.
    pub value: Value,
    /// A link to a task that moves too, which will point at its new copy.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follows_move: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueReason {
    /// The target has no field of this name and type.
    NoMatchingField,
    /// The target's field has no option of this name.
    OptionMissing,
    /// The person can't hold tasks in the target.
    PersonNotInTarget,
    /// A linked task that doesn't move with this one.
    LinkOutsideMove,
    /// A linked page, which stays in its project.
    PageLink,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveIssue {
    /// Pass this as a resolution's `issue`.
    pub id: String,
    pub task_id: String,
    pub task_key: String,
    pub field_id: String,
    pub field: String,
    pub field_type: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_target: Option<LinkTarget>,
    pub reason: IssueReason,
    /// What's wrong, in words.
    pub message: String,
    /// The value as it reads on the original, for you only.
    pub value: Value,
    /// Target fields this could go in: the same type, and for a link the
    /// same kind of record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateField>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CandidateField {
    pub id: String,
    pub name: String,
    /// The original value fits this field as it is ("set" without a value).
    pub takes_value: bool,
    /// A choice field's options, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

// ---------------------------------------------------------------------------
// Access

struct Access {
    target_key: String,
    organization_id: Uuid,
    /// Whether the mover sees tasks (their fields, people and records) in
    /// the target: needed to plan custom fields.
    sees_target: bool,
    /// Repositories and pull requests are named only for a mover who sees
    /// code in the source project.
    sees_code: bool,
}

/// Every check a move takes, for the preview (`lock` false) and the move
/// itself (both projects locked, in one order so opposite moves can't
/// deadlock). A replay goes through it too, so a retry never outlives
/// access.
async fn check_access(
    transaction: &mut Transaction<'_, Postgres>,
    state: &AppState,
    mover: &Mover,
    source_project_id: Uuid,
    target_project_id: Uuid,
    lock: bool,
) -> Refusable<Access> {
    if mover
        .confined_to
        .is_some_and(|project| project != target_project_id)
    {
        return Ok(Err(Refusal::NotFound("That target project")));
    }
    if target_project_id == source_project_id {
        return Ok(Err(Refusal::invalid(
            "toProjectId",
            "The task is already in that project.",
        )));
    }
    // A move locks in the order the other writers that change who may act
    // here do, so none of them waits on another in a circle:
    //
    // 1. Project rows, ascending: the source and target to change, and an
    //    agent's home project (its connection's, which its reach rests on,
    //    agent_connection_can_access) to share. Archiving a project locks
    //    its row first, and changing or removing an agent's connection
    //    locks its home project's row first (external_agents.rs,
    //    mutate_connection: load_access_tx).
    // 2. The agent's connection, then its membership of the home project,
    //    shared: that mutation locks those next (load_connection_tx).
    // 3. Only then the mover's memberships of the source and target,
    //    which also share their workspace membership and actor row: that
    //    mutation updates the actor and memberships last.
    //
    // So a role change, removal or archive either waits for the move, or
    // the move reads it once it has committed.
    let anchor = match mover.external_agent_connection_id {
        Some(connection) => sqlx::query_scalar::<_, Uuid>(
            "SELECT project_id FROM external_agent_connections WHERE id = $1",
        )
        .bind(connection)
        .fetch_optional(&mut **transaction)
        .await?
        .filter(|home| *home != source_project_id && *home != target_project_id),
        None => None,
    };
    let mut order = vec![source_project_id, target_project_id];
    order.sort();
    if lock {
        let mut rows = order.clone();
        rows.extend(anchor);
        rows.sort();
        for project in rows {
            let statement = if Some(project) == anchor {
                "SELECT 1 FROM projects WHERE id = $1 FOR SHARE"
            } else {
                "SELECT 1 FROM projects WHERE id = $1 FOR UPDATE"
            };
            sqlx::query(statement)
                .bind(project)
                .execute(&mut **transaction)
                .await?;
        }
        if let Some(connection) = mover.external_agent_connection_id {
            sqlx::query("SELECT 1 FROM external_agent_connections WHERE id = $1 FOR SHARE")
                .bind(connection)
                .execute(&mut **transaction)
                .await?;
        }
        if let Some(home) = anchor {
            sqlx::query(
                "SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2 FOR SHARE",
            )
            .bind(home)
            .bind(mover.actor_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    let mut memberships: HashMap<Uuid, ProjectMembership> = HashMap::new();
    for project in order {
        let membership = if lock {
            lock_membership(transaction, mover.actor_id, project).await?
        } else {
            read_membership(transaction, mover.actor_id, project).await?
        };
        let Some(membership) = membership else {
            return Ok(Err(Refusal::NotFound("That project")));
        };
        if membership.project_archived_at.is_some() || !membership.enabled {
            return Ok(Err(Refusal::NotFound("That project")));
        }
        memberships.insert(project, membership);
    }
    // Membership alone isn't reach: an agent's connection limits the
    // projects it may act in. Read after the locks, so a connection changed
    // while this waited is read as it is now. Checked on every call, a
    // replay's included: a caller that has lost either project gets nothing.
    let reach = sqlx::query_as::<_, (Uuid, bool, String, Uuid)>(
        r#"
        SELECT id, actor_can_access_project($1, $2, id) AND kind = 'standard',
               project_key, organization_id
        FROM projects WHERE id = ANY($3)
        "#,
    )
    .bind(mover.actor_id)
    .bind(mover.external_agent_connection_id)
    .bind(vec![source_project_id, target_project_id])
    .fetch_all(&mut **transaction)
    .await?;
    let reachable = |project: Uuid| reach.iter().find(|row| row.0 == project && row.1);
    if reachable(source_project_id).is_none() {
        return Ok(Err(Refusal::NotFound("That task")));
    }
    let Some((_, _, target_key, organization_id)) = reachable(target_project_id).cloned() else {
        return Ok(Err(Refusal::NotFound("That target project")));
    };
    let allows = |actor: &CommandActor, project: Uuid, action: Action| -> Result<bool, AppError> {
        let membership = memberships
            .get(&project)
            .ok_or(AppError::Internal("move membership"))?;
        Ok(matches!(
            authorize_mutation(actor, membership, action, None)?,
            Decision::Allow
        ))
    };
    // A move archives the source and creates in the target. An app or agent
    // also needs to be let write through MCP in both.
    let mcp = mover.surface == Surface::Mcp;
    for (actor, project, what, also) in [
        (
            &mover.source,
            source_project_id,
            "this task's project",
            Action::WorkItemArchive,
        ),
        (
            &mover.target,
            target_project_id,
            "the target project",
            Action::WorkItemCreate,
        ),
    ] {
        if (mcp && !allows(actor, project, Action::McpWrite)?) || !allows(actor, project, also)? {
            return Ok(Err(Refusal::Denied(format!(
                "You can't change tasks in {what}."
            ))));
        }
    }
    // A move copies the task's description, criteria, comments and notes
    // into the target: only for someone who sees tasks where it is now
    // (their role, and an app's agent limit).
    if !allows(&mover.source, source_project_id, Action::WorkItemView)? {
        return Ok(Err(Refusal::Denied(
            "You can't see tasks in this task's project, so you can't move them.".to_owned(),
        )));
    }
    // AWD-75: Tasks must be on where the move creates.
    for project in [source_project_id, target_project_id] {
        if !crate::modules::state::for_project(transaction, project, mover.actor_id)
            .await?
            .unwrap_or_default()
            .contains("tasks")
        {
            return Ok(Err(Refusal::ModuleOff));
        }
    }
    let sees_target = allows(&mover.target, target_project_id, Action::WorkItemView)?;
    let sees_code = !crate::search::projects_allowing(
        &state.pool,
        mover.actor_id,
        &[source_project_id],
        crate::policy::Permission::CodeView,
    )
    .await?
    .is_empty();
    Ok(Ok(Access {
        target_key,
        organization_id,
        sees_target,
        sees_code,
    }))
}

fn public_url(state: &AppState, path: &str) -> String {
    format!("{}{path}", crate::invitations::public_origin(state))
}

// ---------------------------------------------------------------------------
// The tree and its plan

#[derive(Debug, FromRow)]
struct SourceTask {
    id: Uuid,
    parent_id: Option<Uuid>,
    key: String,
    number: i64,
    title: String,
    description: String,
    priority: String,
    type_name: String,
    status_name: String,
    status_category: String,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    blocked_reason: Option<String>,
    version: i64,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
    estimate: Option<f64>,
    milestone_name: Option<String>,
    field_values: Value,
}

/// The task and its live subtasks, parents first. `lock` holds them until
/// the move commits.
async fn load_tree(
    transaction: &mut Transaction<'_, Postgres>,
    source_project_id: Uuid,
    task_id: Uuid,
    lock: bool,
) -> Result<Vec<SourceTask>, AppError> {
    let query = format!(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id, 0 AS depth FROM work_items
            WHERE project_id = $1 AND id = $2
            UNION ALL
            SELECT child.id, tree.depth + 1
            FROM task_rows child
            JOIN tree ON child.parent_work_item_id = tree.id
            WHERE child.project_id = $1 AND child.archived_at IS NULL AND tree.depth < $3
        )
        SELECT item.id,
               CASE WHEN item.id = $2 THEN NULL ELSE item.parent_work_item_id END AS parent_id,
               project.project_key || '-' || item.number AS key,
               item.number, item.title, item.description, item.priority,
               item_type.name AS type_name, status.name AS status_name,
               status.semantic_category AS status_category,
               item.assignee_actor_id, assignee.display_name AS assignee_name,
               item.blocked_reason, item.version, item.start_date, item.due_date,
               item.estimate, milestone.name AS milestone_name, item.field_values
        FROM tree
        JOIN task_rows item ON item.id = tree.id
        LEFT JOIN project_milestones milestone
          ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_types item_type
          ON item_type.project_id = item.project_id AND item_type.id = item.type_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id
         AND assignee.actor_id = item.assignee_actor_id
        ORDER BY tree.depth, item.number
        {}
        "#,
        if lock { "FOR UPDATE OF item" } else { "" }
    );
    Ok(sqlx::query_as::<_, SourceTask>(&query)
        .bind(source_project_id)
        .bind(task_id)
        // Depth 0 is the task itself, so this walks every level a tree can have.
        .bind(i32::try_from(MAX_SUBTASK_LEVELS - 1).unwrap_or(i32::MAX))
        .fetch_all(&mut **transaction)
        .await?)
}

/// Where a task's own settings land in the target.
struct TaskSettings {
    status_id: Option<Uuid>,
    type_id: Option<Uuid>,
    assignee: Option<Uuid>,
    milestone_id: Option<Uuid>,
    estimate: Option<f64>,
}

/// A value as it will be stored in the target.
#[derive(Clone, Debug, PartialEq, Serialize)]
enum Carry {
    Stored(Value),
    /// A link to a task in the tree: its new copy's ID once known.
    MovedTask(Uuid),
}

#[derive(Clone, Debug)]
// A few per move's plan, read once: boxing the large variant buys nothing.
#[allow(clippy::large_enum_variant)]
enum ValuePlan {
    Carry {
        target_field: FieldDefinition,
        carry: Carry,
    },
    Issue {
        reason: IssueReason,
    },
}

struct PlannedValue {
    task_index: usize,
    source_field: FieldDefinition,
    value: Value,
    plan: ValuePlan,
}

struct Plan {
    tasks: Vec<SourceTask>,
    settings: Vec<TaskSettings>,
    values: Vec<PlannedValue>,
    /// Values per task that stay without a choice: archived fields, or all
    /// of them when the mover can't see the target's fields.
    quiet: Vec<u32>,
    notes: Vec<String>,
    source_definitions: Vec<FieldDefinition>,
    target_definitions: Vec<FieldDefinition>,
    snapshot: String,
}

fn issue_id(task_id: Uuid, field_id: Uuid) -> String {
    format!("{}:{}", task_id.simple(), field_id.simple())
}

/// Whether `target` can hold values of `source`'s kind.
fn compatible(source: &FieldDefinition, target: &FieldDefinition) -> bool {
    !target.archived
        && source.field_type == target.field_type
        && (source.field_type != FieldType::Link
            || source.config.target.unwrap_or(LinkTarget::Task)
                == target.config.target.unwrap_or(LinkTarget::Task))
}

fn active_option_by_name(field: &FieldDefinition, name: &str) -> Option<Uuid> {
    let wanted = name.trim().to_lowercase();
    field
        .config
        .options
        .iter()
        .find(|option| !option.archived && option.name.to_lowercase() == wanted)
        .map(|option| option.id)
}

/// `value` from `source` as `target` would store it, or None when it
/// doesn't mean the same thing there.
async fn carry_value(
    connection: &mut PgConnection,
    target_project_id: Uuid,
    tree: &HashSet<Uuid>,
    source: &FieldDefinition,
    target: &FieldDefinition,
    value: &Value,
) -> Result<Option<Carry>, AppError> {
    if !compatible(source, target) {
        return Ok(None);
    }
    Ok(match source.field_type {
        // Types only manifests declare are never stored, so never carried.
        FieldType::People | FieldType::Labels => None,
        // The record's own columns: nothing stored to carry.
        FieldType::CreatedTime
        | FieldType::EditedTime
        | FieldType::CreatedBy
        | FieldType::EditedBy
        | FieldType::AutoId => None,
        FieldType::Text
        | FieldType::LongText
        | FieldType::Number
        | FieldType::Date
        | FieldType::Url
        | FieldType::Email
        | FieldType::Phone
        | FieldType::Checkbox => match fields::normalize_value(target, value) {
            Ok(Normalized::Value(stored)) => Some(Carry::Stored(stored)),
            _ => None,
        },
        FieldType::SingleChoice | FieldType::Status => value
            .as_str()
            .and_then(|id| source.option(id))
            .and_then(|option| active_option_by_name(target, &option.name))
            .map(|id| Carry::Stored(Value::String(id.to_string()))),
        FieldType::MultipleChoice => {
            let mut chosen = HashSet::new();
            for id in value.as_array().map(Vec::as_slice).unwrap_or_default() {
                let Some(found) = id
                    .as_str()
                    .and_then(|id| source.option(id))
                    .and_then(|option| active_option_by_name(target, &option.name))
                else {
                    return Ok(None);
                };
                chosen.insert(found);
            }
            if chosen.is_empty() {
                return Ok(None);
            }
            Some(Carry::Stored(Value::Array(
                target
                    .config
                    .options
                    .iter()
                    .filter(|option| chosen.contains(&option.id))
                    .map(|option| Value::String(option.id.to_string()))
                    .collect(),
            )))
        }
        FieldType::Person => match value.as_str().and_then(|id| id.parse::<Uuid>().ok()) {
            Some(person)
                if validate_assignee(connection, target_project_id, Some(person)).await? =>
            {
                Some(Carry::Stored(Value::String(person.to_string())))
            }
            _ => None,
        },
        FieldType::Link => match (
            source.config.target.unwrap_or(LinkTarget::Task),
            value.as_str().and_then(|id| id.parse::<Uuid>().ok()),
        ) {
            (LinkTarget::Task, Some(task)) if tree.contains(&task) => Some(Carry::MovedTask(task)),
            _ => None,
        },
    })
}

fn issue_reason(source: &FieldDefinition, same_name: Option<&FieldDefinition>) -> IssueReason {
    if same_name.is_none() {
        return IssueReason::NoMatchingField;
    }
    match (source.field_type, source.config.target) {
        (FieldType::SingleChoice | FieldType::MultipleChoice | FieldType::Status, _) => {
            IssueReason::OptionMissing
        }
        (FieldType::Person, _) => IssueReason::PersonNotInTarget,
        (FieldType::Link, Some(LinkTarget::Page)) => IssueReason::PageLink,
        (FieldType::Link, _) => IssueReason::LinkOutsideMove,
        _ => IssueReason::NoMatchingField,
    }
}

/// Plans the move of `tasks` into the target: where each task's settings
/// land, and what happens to each custom field value. Reads only.
#[allow(clippy::too_many_arguments)]
async fn plan(
    transaction: &mut Transaction<'_, Postgres>,
    mover: &Mover,
    source_project_id: Uuid,
    target_project_id: Uuid,
    access: &Access,
    tasks: Vec<SourceTask>,
) -> Result<Plan, AppError> {
    let target_key = &access.target_key;
    let (_, source_unit) =
        crate::work_item_workflow::task_settings(transaction, source_project_id).await?;
    let (_, target_unit) =
        crate::work_item_workflow::task_settings(transaction, target_project_id).await?;
    let same_unit = source_unit == target_unit;

    let mut notes = Vec::new();
    // A task with no matching status or type takes the target's default,
    // resolved here (not when it's created) so the snapshot holds the very
    // record it lands on, and a default changed since the preview is a
    // change.
    let default_status = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1 AND enabled AND is_default",
    )
    .bind(target_project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let default_type = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled AND is_default",
    )
    .bind(target_project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let mut settings = Vec::with_capacity(tasks.len());
    for task in &tasks {
        let status_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM work_item_statuses
            WHERE project_id = $1 AND enabled
              AND (lower(name) = lower($2) OR semantic_category = $3)
            ORDER BY lower(name) = lower($2) DESC, position
            LIMIT 1
            "#,
        )
        .bind(target_project_id)
        .bind(&task.status_name)
        .bind(&task.status_category)
        .fetch_optional(&mut **transaction)
        .await?;
        if status_id.is_none() {
            notes.push(format!(
                "{target_key} has no status named {} and none in its group ({}), so {} starts in {target_key}'s default status.",
                task.status_name, task.status_category, task.key
            ));
        }
        let type_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled AND lower(name) = lower($2)",
        )
        .bind(target_project_id)
        .bind(&task.type_name)
        .fetch_optional(&mut **transaction)
        .await?;
        let assignee = match task.assignee_actor_id {
            Some(assignee)
                if validate_assignee(transaction, target_project_id, Some(assignee)).await? =>
            {
                Some(assignee)
            }
            Some(_) => {
                notes.push(format!(
                    "{} isn't in {target_key}, so the task moved from {} is unassigned.",
                    task.assignee_name.as_deref().unwrap_or("Its assignee"),
                    task.key
                ));
                None
            }
            None => None,
        };
        let milestone_id = match &task.milestone_name {
            Some(name) => {
                let found = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL AND lower(name) = lower($2)",
                )
                .bind(target_project_id)
                .bind(name)
                .fetch_optional(&mut **transaction)
                .await?;
                if found.is_none() {
                    notes.push(format!(
                        "{target_key} has no milestone called \"{name}\", so the task moved from {} isn't in one.",
                        task.key
                    ));
                }
                found
            }
            None => None,
        };
        if let (Some(estimate), false) = (task.estimate, same_unit) {
            notes.push(format!(
                "{target_key} estimates in {target_unit}, not {source_unit}, so the estimate of {estimate} {source_unit} on {} wasn't carried over.",
                task.key
            ));
        }
        settings.push(TaskSettings {
            status_id: status_id.or(default_status),
            type_id: type_id.or(default_type),
            assignee,
            milestone_id,
            estimate: if same_unit { task.estimate } else { None },
        });
    }

    // A task of a collection that acts like a task has that collection's
    // fields; they carry over by name like the rest.
    let source_definitions =
        work_item_fields::all_task_definitions(transaction, source_project_id, true).await?;
    let target_definitions = if access.sees_target {
        work_item_fields::definitions(&mut **transaction, target_project_id, true).await?
    } else {
        Vec::new()
    };
    let tree = tasks.iter().map(|task| task.id).collect::<HashSet<_>>();
    let mut values = Vec::new();
    let mut quiet = vec![0_u32; tasks.len()];
    let mut unseen = 0_u32;
    for (task_index, task) in tasks.iter().enumerate() {
        let Some(stored) = task.field_values.as_object() else {
            continue;
        };
        // In the source's field order, so a preview reads like the task.
        for source_field in &source_definitions {
            let Some(value) = stored.get(&source_field.id.to_string()) else {
                continue;
            };
            if source_field.archived {
                quiet[task_index] += 1;
                continue;
            }
            // Planning needs the target's fields, people and records; a
            // mover who can't see them there carries no values over.
            if !access.sees_target {
                quiet[task_index] += 1;
                unseen += 1;
                continue;
            }
            let same_name = target_definitions.iter().find(|field| {
                field.name.to_lowercase() == source_field.name.to_lowercase()
                    && compatible(source_field, field)
            });
            let carried = match same_name {
                Some(target_field) => carry_value(
                    transaction,
                    target_project_id,
                    &tree,
                    source_field,
                    target_field,
                    value,
                )
                .await?
                .map(|carry| (target_field.clone(), carry)),
                None => None,
            };
            let plan = match carried {
                Some((target_field, carry)) => ValuePlan::Carry {
                    target_field,
                    carry,
                },
                None => ValuePlan::Issue {
                    reason: issue_reason(source_field, same_name),
                },
            };
            values.push(PlannedValue {
                task_index,
                source_field: source_field.clone(),
                value: value.clone(),
                plan,
            });
        }
        // Values of fields the project no longer has stay where they are.
        let known = source_definitions
            .iter()
            .map(|field| field.id.to_string())
            .collect::<HashSet<_>>();
        quiet[task_index] +=
            u32::try_from(stored.keys().filter(|key| !known.contains(*key)).count())
                .unwrap_or(u32::MAX);
    }
    match unseen {
        0 => {}
        1 => notes.push(format!(
            "1 custom field value stayed on the original: you can't see tasks in {target_key}, so it couldn't be matched there."
        )),
        count => notes.push(format!(
            "{count} custom field values stayed on the original: you can't see tasks in {target_key}, so they couldn't be matched there."
        )),
    }

    // Labels go by name at the move; the target's labels of those names
    // (and whether they're archived) decide what each becomes.
    let mut label_names = Vec::new();
    for task in &tasks {
        for (name, _) in work_item_labels::active_labels_with_colors(
            &mut **transaction,
            source_project_id,
            task.id,
        )
        .await?
        {
            label_names.push(name.to_lowercase());
        }
    }
    label_names.sort();
    label_names.dedup();
    let target_labels = sqlx::query_as::<_, (Uuid, String, bool)>(
        r#"
        SELECT id, lower(name), archived_at IS NOT NULL FROM project_labels
        WHERE project_id = $1 AND lower(name) = ANY($2)
        ORDER BY lower(name), id
        "#,
    )
    .bind(target_project_id)
    .bind(&label_names)
    .fetch_all(&mut **transaction)
    .await?;

    let snapshot = snapshot_of(
        mover,
        source_project_id,
        target_project_id,
        access,
        &tasks,
        &settings,
        &target_labels,
        &source_definitions,
        &target_definitions,
        &values,
        &quiet,
        &notes,
    )?;
    Ok(Plan {
        tasks,
        settings,
        values,
        quiet,
        notes,
        source_definitions,
        target_definitions,
        snapshot,
    })
}

/// A digest of everything a plan rests on: who moves, the tree's versions,
/// where each task's status, type, assignee, milestone and estimate land
/// (by the target's IDs, so a record replaced by another of the same name
/// counts as a change), the target's labels of the names that move, both
/// projects' field definitions, every decision and note. A move whose own
/// plan digests differently refuses the preview as out of date. It holds
/// IDs and versions, never names or values in the clear.
#[allow(clippy::too_many_arguments)]
fn snapshot_of(
    mover: &Mover,
    source_project_id: Uuid,
    target_project_id: Uuid,
    access: &Access,
    tasks: &[SourceTask],
    settings: &[TaskSettings],
    target_labels: &[(Uuid, String, bool)],
    source_definitions: &[FieldDefinition],
    target_definitions: &[FieldDefinition],
    values: &[PlannedValue],
    quiet: &[u32],
    notes: &[String],
) -> Result<String, AppError> {
    let decisions = values
        .iter()
        .map(|planned| {
            let task = tasks[planned.task_index].id;
            match &planned.plan {
                ValuePlan::Carry {
                    target_field,
                    carry,
                } => serde_json::json!([
                    task,
                    planned.source_field.id,
                    "carry",
                    target_field.id,
                    carry
                ]),
                ValuePlan::Issue { reason } => {
                    serde_json::json!([
                        task,
                        planned.source_field.id,
                        "issue",
                        reason,
                        planned.value
                    ])
                }
            }
        })
        .collect::<Vec<_>>();
    let digest = serde_json::json!({
        "actor": mover.actor_id,
        "source": source_project_id,
        "target": target_project_id,
        "seesTarget": access.sees_target,
        "tasks": tasks.iter().map(|task| (task.id, task.version, task.parent_id)).collect::<Vec<_>>(),
        "settings": settings.iter().map(|setting| (
            setting.status_id,
            setting.type_id,
            setting.assignee,
            setting.milestone_id,
            setting.estimate,
        )).collect::<Vec<_>>(),
        "targetLabels": target_labels,
        "sourceFields": source_definitions.iter().map(|field| (field.id, field.version)).collect::<Vec<_>>(),
        "targetFields": target_definitions.iter().map(|field| (field.id, field.version)).collect::<Vec<_>>(),
        "decisions": decisions,
        "quiet": quiet,
        "notes": notes,
    });
    let encoded =
        serde_json::to_vec(&digest).map_err(|_| AppError::Internal("serialize move snapshot"))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

// ---------------------------------------------------------------------------
// Preview

/// What would happen, for the mover only. Writes nothing and takes no locks.
pub async fn preview(
    state: &AppState,
    mover: &Mover,
    source_project_id: Uuid,
    task_id: Uuid,
    target_project_id: Uuid,
) -> Refusable<MovePlan> {
    let mut transaction = state.pool.begin().await?;
    let access = refuse!(
        check_access(
            &mut transaction,
            state,
            mover,
            source_project_id,
            target_project_id,
            false,
        )
        .await
    );
    if !access.sees_target {
        return Ok(Err(Refusal::Denied(format!(
            "You can't see tasks in {}, so a preview can't show its fields. Ask for access there, or move without a preview: field values then stay on the original.",
            access.target_key
        ))));
    }
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE project_id = $1 AND id = $2",
    )
    .bind(source_project_id)
    .bind(task_id)
    .fetch_optional(&mut *transaction)
    .await?;
    match archived {
        None => return Ok(Err(Refusal::NotFound("That task"))),
        Some(true) => {
            return Ok(Err(Refusal::Conflict(
                "That task is archived, or was moved already, so there's nothing to move."
                    .to_owned(),
            )));
        }
        Some(false) => {}
    }
    let tasks = load_tree(&mut transaction, source_project_id, task_id, false).await?;
    if let Some(refusal) = too_many(&tasks) {
        return Ok(Err(refusal));
    }
    let plan = plan(
        &mut transaction,
        mover,
        source_project_id,
        target_project_id,
        &access,
        tasks,
    )
    .await?;
    let view = plan_view(
        &mut transaction,
        source_project_id,
        target_project_id,
        &access,
        &plan,
    )
    .await?;
    transaction.rollback().await?;
    Ok(Ok(view))
}

/// A project the task could move into.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Destination {
    pub id: String,
    pub key: String,
    pub name: String,
}

/// The projects this task could move into, for the person asking: the ones
/// a preview of the move would accept, by the same checks (reach, an open
/// ordinary project, their role's permission to add and see tasks there,
/// and Tasks on for them), read from their actual role, custom or not.
/// It grants nothing and writes nothing; the move checks everything again.
pub async fn destinations(
    state: &AppState,
    mover: &Mover,
    source_project_id: Uuid,
    task_id: Uuid,
) -> Refusable<Vec<Destination>> {
    let mut transaction = state.pool.begin().await?;
    // The task, and the right to move it out of its project.
    let Some(source) = read_membership(&mut transaction, mover.actor_id, source_project_id).await?
    else {
        return Ok(Err(Refusal::NotFound("That task")));
    };
    let reaches = |project: Uuid| {
        sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, $2, $3)")
            .bind(mover.actor_id)
            .bind(mover.external_agent_connection_id)
            .bind(project)
    };
    if source.project_archived_at.is_some()
        || !source.enabled
        || !reaches(source_project_id)
            .fetch_one(&mut *transaction)
            .await?
    {
        return Ok(Err(Refusal::NotFound("That task")));
    }
    let open = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NULL FROM task_rows WHERE project_id = $1 AND id = $2",
    )
    .bind(source_project_id)
    .bind(task_id)
    .fetch_optional(&mut *transaction)
    .await?;
    match open {
        None => return Ok(Err(Refusal::NotFound("That task"))),
        Some(false) => {
            return Ok(Err(Refusal::Conflict(
                "That task is archived, or was moved already, so there's nothing to move."
                    .to_owned(),
            )));
        }
        Some(true) => {}
    }
    let allows = |actor: &CommandActor, membership: &ProjectMembership, action: Action| {
        authorize_mutation(actor, membership, action, None)
            .map(|decision| matches!(decision, Decision::Allow))
    };
    let mcp = mover.surface == Surface::Mcp;
    if !allows(&mover.source, &source, Action::WorkItemArchive)?
        || !allows(&mover.source, &source, Action::WorkItemView)?
        || (mcp && !allows(&mover.source, &source, Action::McpWrite)?)
    {
        return Ok(Err(Refusal::Denied(
            "You can't move tasks out of this project.".to_owned(),
        )));
    }
    if !crate::modules::state::for_project(&mut transaction, source_project_id, mover.actor_id)
        .await?
        .unwrap_or_default()
        .contains("tasks")
    {
        return Ok(Err(Refusal::ModuleOff));
    }

    let candidates = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT project.id, project.project_key, project.name
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1 AND membership.enabled
        WHERE project.id <> $3 AND project.kind = 'standard' AND project.archived_at IS NULL
          AND actor_can_access_project($1, $2, project.id)
          AND ($4::uuid IS NULL OR project.id = $4)
        ORDER BY lower(project.name), project.id
        "#,
    )
    .bind(mover.actor_id)
    .bind(mover.external_agent_connection_id)
    .bind(source_project_id)
    .bind(mover.confined_to)
    .fetch_all(&mut *transaction)
    .await?;
    let mut found = Vec::new();
    for (project, key, name) in candidates {
        let Some(membership) = read_membership(&mut transaction, mover.actor_id, project).await?
        else {
            continue;
        };
        // The same attribution the move would use there.
        let actor = CommandActor {
            project_id: project,
            ..mover.source.clone()
        };
        // A preview needs to see tasks there; the move needs to add them.
        if !membership.enabled
            || membership.project_archived_at.is_some()
            || !allows(&actor, &membership, Action::WorkItemCreate)?
            || !allows(&actor, &membership, Action::WorkItemView)?
            || (mcp && !allows(&actor, &membership, Action::McpWrite)?)
        {
            continue;
        }
        if !crate::modules::state::for_project(&mut transaction, project, mover.actor_id)
            .await?
            .unwrap_or_default()
            .contains("tasks")
        {
            continue;
        }
        found.push(Destination {
            id: project.to_string(),
            key,
            name,
        });
    }
    transaction.rollback().await?;
    Ok(Ok(found))
}

fn too_many(tasks: &[SourceTask]) -> Option<Refusal> {
    (tasks.len() > MAX_TASKS).then(|| {
        Refusal::invalid(
            "taskId",
            format!(
                "That task has {} subtasks; one move carries at most {MAX_TASKS} tasks. Move some subtasks first.",
                tasks.len() - 1
            ),
        )
    })
}

/// The plan as the mover reads it. Values read through the task page's own
/// projection of field values (work_item_fields::references, under the
/// "see tasks" the move already requires), so a preview shows no more than
/// the task does.
async fn plan_view(
    transaction: &mut Transaction<'_, Postgres>,
    source_project_id: Uuid,
    target_project_id: Uuid,
    access: &Access,
    plan: &Plan,
) -> Result<MovePlan, AppError> {
    let references = work_item_fields::references(
        transaction,
        source_project_id,
        &plan.source_definitions,
        plan.tasks.iter().map(|task| &task.field_values),
    )
    .await?;
    let shown = |field: &FieldDefinition, value: &Value| {
        fields::display_value(field, value, &|id| {
            work_item_fields::reference_text(&references, id).map(|text| {
                match id.parse::<Uuid>().ok().and_then(|id| references.get(&id)) {
                    Some(reference) if reference.kind == "task" => {
                        format!("{text} {}", reference.label)
                    }
                    _ => text,
                }
            })
        })
    };
    let tree = plan
        .tasks
        .iter()
        .map(|task| task.id)
        .collect::<HashSet<_>>();
    let mut carried = Vec::new();
    let mut issues = Vec::new();
    for planned in &plan.values {
        let task = &plan.tasks[planned.task_index];
        match &planned.plan {
            ValuePlan::Carry {
                target_field,
                carry,
            } => carried.push(CarriedValue {
                task_id: task.id.to_string(),
                task_key: task.key.clone(),
                field: planned.source_field.name.clone(),
                target_field: target_field.name.clone(),
                value: shown(&planned.source_field, &planned.value),
                follows_move: matches!(carry, Carry::MovedTask(_)),
            }),
            ValuePlan::Issue { reason } => {
                let mut candidates = Vec::new();
                for field in plan
                    .target_definitions
                    .iter()
                    .filter(|field| compatible(&planned.source_field, field))
                {
                    let takes_value = carry_value(
                        transaction,
                        target_project_id,
                        &tree,
                        &planned.source_field,
                        field,
                        &planned.value,
                    )
                    .await?
                    .is_some();
                    candidates.push(CandidateField {
                        id: field.id.to_string(),
                        name: field.name.clone(),
                        takes_value,
                        options: field
                            .config
                            .options
                            .iter()
                            .filter(|option| !option.archived)
                            .map(|option| option.name.clone())
                            .collect(),
                    });
                }
                issues.push(MoveIssue {
                    id: issue_id(task.id, planned.source_field.id),
                    task_id: task.id.to_string(),
                    task_key: task.key.clone(),
                    field_id: planned.source_field.id.to_string(),
                    field: planned.source_field.name.clone(),
                    field_type: planned.source_field.field_type,
                    link_target: (planned.source_field.field_type == FieldType::Link).then(|| {
                        planned
                            .source_field
                            .config
                            .target
                            .unwrap_or(LinkTarget::Task)
                    }),
                    reason: *reason,
                    message: issue_message(*reason, &planned.source_field, &access.target_key),
                    value: shown(&planned.source_field, &planned.value),
                    candidates,
                });
            }
        }
    }
    Ok(MovePlan {
        target_project_id: target_project_id.to_string(),
        target_project_key: access.target_key.clone(),
        tasks: plan
            .tasks
            .iter()
            .map(|task| PlannedTask {
                id: task.id.to_string(),
                key: task.key.clone(),
                title: task.title.clone(),
                parent_id: task.parent_id.map(|id| id.to_string()),
            })
            .collect(),
        carried,
        issues,
        notes: plan.notes.clone(),
        archived_field_values: plan.quiet.iter().sum(),
        snapshot: plan.snapshot.clone(),
    })
}

fn issue_message(reason: IssueReason, field: &FieldDefinition, target_key: &str) -> String {
    let name = &field.name;
    match reason {
        IssueReason::NoMatchingField => format!(
            "{target_key} has no {} field called \"{name}\".",
            field.field_type.as_str().replace('_', " ")
        ),
        IssueReason::OptionMissing => {
            format!("{target_key}'s \"{name}\" doesn't have this option.")
        }
        IssueReason::PersonNotInTarget => {
            "This person can't hold tasks in the target project.".to_owned()
        }
        IssueReason::LinkOutsideMove => {
            "The linked task stays where it is: it isn't one of the tasks moving.".to_owned()
        }
        IssueReason::PageLink => "The linked page stays in its own project.".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// The move

/// Moves the task and its subtasks, all or nothing.
pub async fn execute(
    state: &AppState,
    mover: &Mover,
    source_project_id: Uuid,
    request: MoveRequest,
) -> Refusable<MoveDone> {
    if request.resolutions.len() > MAX_RESOLUTIONS {
        return Ok(Err(Refusal::invalid(
            "resolutions",
            format!("Send at most {MAX_RESOLUTIONS} choices."),
        )));
    }
    let mut resolutions = request.resolutions.clone();
    for resolution in &mut resolutions {
        resolution.issue = resolution.issue.trim().to_ascii_lowercase();
        resolution.target_field = resolution
            .target_field
            .as_ref()
            .map(|field| field.trim().to_owned());
    }
    resolutions.sort_by(|a, b| a.issue.cmp(&b.issue));
    if resolutions
        .windows(2)
        .any(|pair| pair[0].issue == pair[1].issue)
    {
        return Ok(Err(Refusal::invalid(
            "resolutions",
            "Each issue takes one choice.",
        )));
    }
    let snapshot = request.snapshot.as_deref().map(str::trim);
    if snapshot.is_none() && (!resolutions.is_empty() || request.leave_unresolved) {
        return Ok(Err(Refusal::invalid(
            "snapshot",
            "Preview the move first, then send its snapshot with your choices.",
        )));
    }

    let mut transaction = state.pool.begin().await?;
    crate::domain::set_via_grant(&mut transaction, mover.via_grant).await?;
    let access = refuse!(
        check_access(
            &mut transaction,
            state,
            mover,
            source_project_id,
            request.target_project_id,
            true,
        )
        .await
    );

    // One digest of the whole request; with the request ID's digest and the
    // sender, it names this move for good (work_item_move_receipts).
    let request_hash = RequestHash::for_value(&Fingerprint {
        task_id: request.task_id,
        target_project_id: request.target_project_id,
        snapshot,
        resolutions: &resolutions,
        leave_unresolved: request.leave_unresolved,
    })?;
    let key_hash = request
        .idempotency_key
        .as_ref()
        .map(|key| Sha256::digest(key.as_str().as_bytes()).to_vec());

    // The task row's lock orders identical requests: the second waits here,
    // then finds the first one's receipt.
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE project_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(source_project_id)
    .bind(request.task_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(archived) = archived else {
        return Ok(Err(Refusal::NotFound("That task")));
    };
    if archived {
        // The same sender's same request ID gets the move's original result
        // (access was checked again above), or a conflict if anything else
        // in the request differs.
        if let Some(key_hash) = &key_hash
            && let Some(receipt) = sqlx::query_as::<_, ReceiptRow>(
                r#"
                SELECT target_project_id, request_hash, moved, left_behind
                FROM work_item_move_receipts
                WHERE source_work_item_id = $1 AND actor_id = $2 AND request_key_hash = $3
                "#,
            )
            .bind(request.task_id)
            .bind(mover.actor_id)
            .bind(key_hash)
            .fetch_optional(&mut *transaction)
            .await?
        {
            transaction.rollback().await?;
            if receipt.request_hash.as_slice() != request_hash.as_bytes() {
                return Ok(Err(Refusal::Conflict(
                    "That request ID was already used for a move of this task with a different target or different choices, so this request wasn't carried out. Use a new request ID for a new request."
                        .to_owned(),
                )));
            }
            return Ok(Ok(replayed(state, mover, receipt).await?));
        }
        // Any other request for a moved task returns where it went, so a
        // retry never makes a second copy; it changes nothing.
        let moved = already_moved(&mut transaction, state, mover, request.task_id).await?;
        transaction.rollback().await?;
        return Ok(match moved {
            Some(moved) => Ok(MoveDone {
                moved,
                notes: if snapshot.is_some() {
                    vec![
                        "This task had already been moved, so this request wasn't carried out and its choices weren't applied. moved says where the task went."
                            .to_owned(),
                    ]
                } else {
                    Vec::new()
                },
                created: false,
                replayed: false,
                left_behind: 0,
            }),
            None => Err(Refusal::Conflict(
                "That task is archived. Restore it before moving it.".to_owned(),
            )),
        });
    }

    let tasks = load_tree(&mut transaction, source_project_id, request.task_id, true).await?;
    if let Some(refusal) = too_many(&tasks) {
        return Ok(Err(refusal));
    }
    let plan = plan(
        &mut transaction,
        mover,
        source_project_id,
        request.target_project_id,
        &access,
        tasks,
    )
    .await?;
    if let Some(snapshot) = snapshot {
        if !access.sees_target {
            return Ok(Err(Refusal::Denied(format!(
                "You can't see tasks in {} any more, so these choices can't be checked there. This request wasn't carried out.",
                access.target_key
            ))));
        }
        if snapshot != plan.snapshot {
            return Ok(Err(Refusal::Conflict(
                "Something changed since the preview (the tasks, either project's fields, or who can hold tasks there). This request wasn't carried out; preview again."
                    .to_owned(),
            )));
        }
    }
    let chosen = refuse!(
        choices(
            &mut transaction,
            mover,
            request.target_project_id,
            &access,
            &plan,
            &resolutions,
            snapshot.is_none() || request.leave_unresolved,
        )
        .await
    );

    let done = carry_over(
        &mut transaction,
        state,
        mover,
        source_project_id,
        request.target_project_id,
        &access,
        &plan,
        &chosen,
    )
    .await?;
    // The receipt holds what the move made, as it made it; later moves and
    // renames don't rewrite it. Only for a request with an ID.
    if let Some(key_hash) = &key_hash {
        let moved = done
            .moved
            .iter()
            .map(|task| {
                Ok(ReceiptTask {
                    from: task.from.clone(),
                    to: task.to.clone(),
                    id: task
                        .id
                        .parse()
                        .map_err(|_| AppError::Internal("moved task id"))?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        sqlx::query(
            r#"
            INSERT INTO work_item_move_receipts (
                id, organization_id, source_project_id, source_work_item_id,
                target_project_id, actor_id, request_key_hash, request_hash,
                moved, left_behind
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(access.organization_id)
        .bind(source_project_id)
        .bind(request.task_id)
        .bind(request.target_project_id)
        .bind(mover.actor_id)
        .bind(key_hash)
        .bind(request_hash.as_bytes().as_slice())
        .bind(sqlx::types::Json(&moved))
        .bind(i32::try_from(done.left_behind).unwrap_or(i32::MAX))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    let mut done = done;
    // As before: titles only for someone whose role shows tasks there.
    if crate::search::projects_showing_tasks(&state.pool, mover.actor_id, &[source_project_id])
        .await?
        .is_empty()
    {
        for task in &mut done.moved {
            task.title = String::new();
        }
    }
    Ok(Ok(done))
}

/// What each value becomes: put in a target field, or left on the original.
struct Chosen {
    /// Per task (by index): the values set on its new copy.
    values: Vec<Vec<(FieldDefinition, Carry)>>,
    /// Per task: how many values stay on the original only.
    left: Vec<u32>,
    /// For the mover only.
    notes: Vec<String>,
}

/// Checks every choice against the target as it is now, inside the move's
/// transaction: a field of the same kind, a value `task_update` would take
/// there, and a page only where the mover reads pages.
async fn choices(
    transaction: &mut Transaction<'_, Postgres>,
    mover: &Mover,
    target_project_id: Uuid,
    access: &Access,
    plan: &Plan,
    resolutions: &[Resolution],
    leave_unresolved: bool,
) -> Refusable<Chosen> {
    let mut values = vec![Vec::new(); plan.tasks.len()];
    let mut left = plan.quiet.clone();
    let mut notes = Vec::new();
    let by_issue = resolutions
        .iter()
        .enumerate()
        .map(|(index, resolution)| (resolution.issue.as_str(), (index, resolution)))
        .collect::<HashMap<_, _>>();
    let tree = plan
        .tasks
        .iter()
        .map(|task| task.id)
        .collect::<HashSet<_>>();
    let mut used = HashSet::new();
    let mut unresolved = Vec::new();
    let mut known = HashSet::new();
    // Values that carry as they are hold their target fields first, so a
    // choice can't put a second value in one.
    for planned in &plan.values {
        if let ValuePlan::Carry {
            target_field,
            carry,
        } = &planned.plan
        {
            used.insert((planned.task_index, target_field.id));
            values[planned.task_index].push((target_field.clone(), carry.clone()));
        }
    }
    for planned in &plan.values {
        let task = &plan.tasks[planned.task_index];
        let id = issue_id(task.id, planned.source_field.id);
        if !matches!(planned.plan, ValuePlan::Issue { .. }) {
            continue;
        }
        known.insert(id.clone());
        let Some((index, resolution)) = by_issue.get(id.as_str()) else {
            if leave_unresolved {
                left[planned.task_index] += 1;
                notes.push(format!(
                    "\"{}\" on {} stayed on the original.",
                    planned.source_field.name, task.key
                ));
            } else {
                unresolved.push(format!("\"{}\" on {}", planned.source_field.name, task.key));
            }
            continue;
        };
        let at = format!("resolutions[{index}]");
        match resolution.action {
            ResolutionAction::Leave => {
                if resolution.target_field.is_some() || resolution.value.is_some() {
                    return Ok(Err(Refusal::invalid(
                        &at,
                        "\"leave\" takes no targetField or value.",
                    )));
                }
                left[planned.task_index] += 1;
                notes.push(format!(
                    "\"{}\" on {} stayed on the original.",
                    planned.source_field.name, task.key
                ));
            }
            ResolutionAction::Set => {
                let Some(target_field) = resolution
                    .target_field
                    .as_deref()
                    .and_then(|reference| fields::find(&plan.target_definitions, reference))
                    .filter(|field| compatible(&planned.source_field, field))
                else {
                    return Ok(Err(Refusal::invalid(
                        format!("{at}.targetField"),
                        format!(
                            "Pick an active {} field of {} for \"{}\" (see the preview's candidates), or leave it.",
                            planned.source_field.field_type.as_str().replace('_', " "),
                            access.target_key,
                            planned.source_field.name
                        ),
                    )));
                };
                if !used.insert((planned.task_index, target_field.id)) {
                    return Ok(Err(Refusal::invalid(
                        format!("{at}.targetField"),
                        format!(
                            "\"{}\" on {} would be set twice. Put each value in its own field.",
                            target_field.name, task.key
                        ),
                    )));
                }
                let carry = match &resolution.value {
                    None => carry_value(
                        transaction,
                        target_project_id,
                        &tree,
                        &planned.source_field,
                        target_field,
                        &planned.value,
                    )
                    .await?
                    .ok_or_else(|| {
                        Refusal::invalid(
                            format!("{at}.value"),
                            format!(
                                "The original value doesn't fit {}'s \"{}\". Give a value, or leave it.",
                                access.target_key, target_field.name
                            ),
                        )
                    }),
                    Some(value) => {
                        if target_field.field_type == FieldType::Link
                            && target_field.config.target == Some(LinkTarget::Page)
                            && !crate::modules::state::for_project(
                                transaction,
                                target_project_id,
                                mover.actor_id,
                            )
                            .await?
                            .unwrap_or_default()
                            .contains("pages")
                        {
                            return Ok(Err(Refusal::Denied(format!(
                                "You can't see pages in {}, so you can't link one there.",
                                access.target_key
                            ))));
                        }
                        let resolved = work_item_fields::resolve(
                            transaction,
                            target_project_id,
                            Writer {
                                actor_id: mover.actor_id,
                                project_key: &access.target_key,
                            },
                            &plan.target_definitions,
                            &format!("{at}.value"),
                            &[(target_field.id.to_string(), value.clone())],
                        )
                        .await?;
                        match resolved {
                            Ok(changes) => match changes.entries.into_iter().next() {
                                Some(FieldValueChange {
                                    value: Some(stored),
                                    ..
                                }) => Ok(Carry::Stored(stored)),
                                _ => Err(Refusal::invalid(
                                    format!("{at}.value"),
                                    "That clears the value. To keep it off the new task, leave it.",
                                )),
                            },
                            Err(problem) => Err(Refusal::invalid(problem.field, problem.message)),
                        }
                    }
                };
                match carry {
                    Ok(carry) => values[planned.task_index].push((target_field.clone(), carry)),
                    Err(refusal) => return Ok(Err(refusal)),
                }
            }
        }
    }
    if let Some(carried) = resolutions.iter().position(|resolution| {
        plan.values.iter().any(|planned| {
            matches!(planned.plan, ValuePlan::Carry { .. })
                && issue_id(plan.tasks[planned.task_index].id, planned.source_field.id)
                    == resolution.issue
        })
    }) {
        return Ok(Err(Refusal::invalid(
            format!("resolutions[{carried}].issue"),
            "That value carries over as it is; it needs no choice.",
        )));
    }
    if let Some(index) = resolutions
        .iter()
        .position(|resolution| !known.contains(&resolution.issue))
    {
        return Ok(Err(Refusal::invalid(
            format!("resolutions[{index}].issue"),
            "That isn't one of this preview's issues.",
        )));
    }
    if !unresolved.is_empty() {
        return Ok(Err(Refusal::invalid(
            "resolutions",
            format!(
                "{} still need a choice: {}. Set or leave each one, or pass leaveUnresolved.",
                unresolved.len(),
                unresolved.join(", ")
            ),
        )));
    }
    Ok(Ok(Chosen {
        values,
        left,
        notes,
    }))
}

/// The tree's new copies, then the archived originals.
#[allow(clippy::too_many_arguments)]
async fn carry_over(
    transaction: &mut Transaction<'_, Postgres>,
    state: &AppState,
    mover: &Mover,
    source_project_id: Uuid,
    target_project_id: Uuid,
    access: &Access,
    plan: &Plan,
    chosen: &Chosen,
) -> Result<MoveDone, AppError> {
    let target_key = &access.target_key;
    let mut notes = plan.notes.clone();
    notes.extend(chosen.notes.iter().cloned());
    // Every copy's ID up front, so links between moving tasks land on the
    // copies as each is created.
    let new_ids = plan
        .tasks
        .iter()
        .map(|task| (task.id, Uuid::now_v7()))
        .collect::<HashMap<_, _>>();
    let mover_name = mover_name(transaction, source_project_id, mover.actor_id).await?;
    let mut moved = Vec::new();
    for (index, task) in plan.tasks.iter().enumerate() {
        let settings = &plan.settings[index];
        let new_id = new_ids[&task.id];
        let parent = task
            .parent_id
            .and_then(|parent| new_ids.get(&parent).copied());
        // Labels go by name: one the target already has keeps its color
        // there, and a missing one is created with the source's color.
        let mut wanted = work_item_labels::active_labels_with_colors(
            &mut **transaction,
            source_project_id,
            task.id,
        )
        .await?;
        let over_limit = wanted.split_off(wanted.len().min(MAX_LABELS_PER_TASK));
        let labels = work_item_labels::ensure_by_name(
            transaction,
            target_project_id,
            mover.actor_id,
            &wanted,
        )
        .await?;
        for label in &labels.created {
            record_audit_for(
                transaction,
                &mover.target,
                "work_item_label",
                mover.surface.label_created(),
                label.id,
                serde_json::json!({
                    "changedFields": ["label"],
                    "name": label.name,
                    "movedWith": task.key,
                }),
            )
            .await?;
        }
        let field_values = FieldValueChanges {
            entries: chosen.values[index]
                .iter()
                .map(|(field, carry)| {
                    let stored = match carry {
                        Carry::Stored(value) => value.clone(),
                        Carry::MovedTask(source) => Value::String(
                            new_ids.get(source).copied().unwrap_or_default().to_string(),
                        ),
                    };
                    FieldValueChange {
                        field: field.clone(),
                        shown: fields::display_value(field, &stored, &|_| None),
                        value: Some(stored),
                    }
                })
                .collect(),
        };
        let summary = format!("Work Item moved from {}", task.key);
        let outcome = create_work_item_with_id(
            transaction,
            &mover.target,
            new_id,
            CreateWorkItemCommand {
                collection_id: None,
                field_values: Some(&field_values),
                title: &task.title,
                description: &task.description,
                type_id: settings.type_id,
                status_id: settings.status_id,
                priority: &task.priority,
                assignee_actor_id: settings.assignee,
                parent_work_item_id: parent,
                label_ids: &labels.ids,
                acceptance_criteria: &[],
                board_placement: None,
                fields: NewTaskFields {
                    milestone_id: settings.milestone_id,
                    start_date: task.start_date,
                    due_date: task.due_date,
                    estimate: settings.estimate,
                },
                activity_summary: &summary,
                activity_metadata: serde_json::json!({ "movedFrom": task.key }),
                activity_number_metadata: AllocatedNumberMetadata::Include,
                audit_action: mover.surface.moved_in(),
                audit_metadata: serde_json::json!({
                    "moved_from": task.key,
                    "source_work_item_id": task.id,
                }),
            },
        )
        .await?;
        let CreateWorkItemOutcome::Created { number, .. } = outcome else {
            return Err(AppError::Internal("moved task could not be created"));
        };
        let new_key = format!("{target_key}-{number}");
        for (name, reason) in &labels.skipped {
            notes.push(match reason {
                SkipReason::Archived => format!(
                    "\"{name}\" is archived in {target_key}, so it wasn't put on {new_key}."
                ),
                SkipReason::ProjectFull(LabelLimit::Active) => format!(
                    "{target_key} already has {} labels, so \"{name}\" wasn't carried to {new_key}.",
                    work_item_labels::MAX_ACTIVE_LABELS_PER_PROJECT
                ),
                SkipReason::ProjectFull(LabelLimit::Total) => format!(
                    "{target_key} has no room for more labels, so \"{name}\" wasn't carried to {new_key}."
                ),
            });
        }
        for (name, _) in &over_limit {
            notes.push(format!(
                "{} had more than {MAX_LABELS_PER_TASK} labels, so \"{name}\" wasn't carried to {new_key}.",
                task.key
            ));
        }

        carry_criteria(
            transaction,
            mover.actor_id,
            source_project_id,
            target_project_id,
            task.id,
            new_id,
        )
        .await?;
        let links = links_left_behind(
            transaction,
            source_project_id,
            task.id,
            mover.actor_id,
            access.sees_code,
        )
        .await?;
        let mut provenance = format!(
            "Moved from {} by {mover_name}. The original is archived with its full history.",
            task.key,
        );
        if let Some(reason) = &task.blocked_reason {
            provenance.push_str(&format!("\n\nIt was blocked: {reason}"));
        }
        // Only a count: readers here may not be able to see what stayed.
        match chosen.left[index] {
            0 => {}
            1 => provenance.push_str(&format!("\n\n1 custom field value stayed on {}.", task.key)),
            count => provenance.push_str(&format!(
                "\n\n{count} custom field values stayed on {}.",
                task.key
            )),
        }
        if !links.is_empty() {
            provenance.push_str(&format!("\n\nStill linked on {}:", task.key));
            provenance.push_str(&links_left_behind_text(&links));
        }
        add_comment(
            transaction,
            &mover.target,
            mover.surface,
            new_id,
            &provenance,
            Notify::Yes,
        )
        .await?;
        let comments = sqlx::query_as::<_, SourceComment>(
            r#"
            SELECT COALESCE(author.display_name, 'Someone') AS author,
                   to_char(comment.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI "UTC"') AS created_at,
                   comment.plain_text
            FROM work_item_comments comment
            LEFT JOIN project_memberships author
              ON author.project_id = comment.project_id
             AND author.actor_id = comment.author_actor_id
            WHERE comment.project_id = $1 AND comment.work_item_id = $2
              AND comment.removed_at IS NULL
            ORDER BY comment.created_at, comment.id
            LIMIT $3
            "#,
        )
        .bind(source_project_id)
        .bind(task.id)
        .bind(MAX_COMMENTS as i64)
        .fetch_all(&mut **transaction)
        .await?;
        let comment_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM work_item_comments WHERE project_id = $1 AND work_item_id = $2 AND removed_at IS NULL",
        )
        .bind(source_project_id)
        .bind(task.id)
        .fetch_one(&mut **transaction)
        .await?;
        for comment in &comments {
            let text = comment
                .plain_text
                .chars()
                .take(MAX_QUOTED_CHARACTERS)
                .collect::<String>();
            let quoted = format!(
                "{} wrote on {} (on {}):\n\n{}",
                comment.author, comment.created_at, task.key, text
            );
            add_comment(
                transaction,
                &mover.target,
                mover.surface,
                new_id,
                &quoted,
                Notify::No,
            )
            .await?;
        }
        if comment_count > MAX_COMMENTS as i64 {
            notes.push(format!(
                "{} has {} comments; the first {MAX_COMMENTS} were carried over and the rest are on the original.",
                task.key, comment_count
            ));
        }

        // Handoff notes go with the task, so whoever claims it in its new
        // project still reads where the last holder left it.
        sqlx::query(
            r#"
            INSERT INTO work_item_handoffs (
                id, project_id, work_item_id, actor_id, via_oauth_grant_id, note, created_at
            )
            SELECT gen_random_uuid(), $3, $4, handoff.actor_id, handoff.via_oauth_grant_id,
                   handoff.note, handoff.created_at
            FROM work_item_handoffs handoff
            WHERE handoff.project_id = $1 AND handoff.work_item_id = $2
            "#,
        )
        .bind(source_project_id)
        .bind(task.id)
        .bind(target_project_id)
        .bind(new_id)
        .execute(&mut **transaction)
        .await?;

        // Point the original at its new home, then archive it. Its field
        // values stay as they were.
        add_comment(
            transaction,
            &mover.source,
            mover.surface,
            task.id,
            &format!(
                "Moved to {new_key}: {}",
                public_url(
                    state,
                    &format!("/projects/{target_project_id}/work-items/{new_id}")
                )
            ),
            Notify::Yes,
        )
        .await?;
        archive(transaction, &mover.source, mover.surface, task, &new_key).await?;
        sqlx::query(
            r#"
            INSERT INTO work_item_key_aliases (
                id, organization_id, project_key, number, source_work_item_id,
                work_item_id, moved_by_actor_id
            )
            SELECT $1, $2, project.project_key, $4, $5, $6, $7
            FROM projects project WHERE project.id = $3
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(access.organization_id)
        .bind(source_project_id)
        .bind(task.number)
        .bind(task.id)
        .bind(new_id)
        .bind(mover.actor_id)
        .execute(&mut **transaction)
        .await?;
        // Anything that pointed at the old key follows it, including keys
        // the original had itself inherited from an earlier move.
        sqlx::query("UPDATE work_item_key_aliases SET work_item_id = $2 WHERE work_item_id = $1")
            .bind(task.id)
            .bind(new_id)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1")
            .bind(task.id)
            .execute(&mut **transaction)
            .await?;
        moved.push(MovedTask {
            from: task.key.clone(),
            to: new_key,
            id: new_id.to_string(),
            title: task.title.clone(),
            url: public_url(
                state,
                &format!("/projects/{target_project_id}/work-items/{new_id}"),
            ),
        });
    }
    // The titles came from the source, which the move requires seeing.
    Ok(MoveDone {
        moved,
        notes,
        created: true,
        replayed: false,
        left_behind: chosen.left.iter().sum(),
    })
}

/// A retried request: its first result. The keys, IDs and count are the
/// receipt's, as the move made them; only the titles are read now, under the
/// caller's access now (checked again before this), so a rename or a later
/// move changes what's shown, never the receipt. No notes.
async fn replayed(
    state: &AppState,
    mover: &Mover,
    receipt: ReceiptRow,
) -> Result<MoveDone, AppError> {
    let ids = receipt.moved.iter().map(|task| task.id).collect::<Vec<_>>();
    let showing = !crate::search::projects_showing_tasks(
        &state.pool,
        mover.actor_id,
        &[receipt.target_project_id],
    )
    .await?
    .is_empty();
    let titles = if showing {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, title FROM task_rows WHERE project_id = $1 AND id = ANY($2)",
        )
        .bind(receipt.target_project_id)
        .bind(&ids)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    Ok(MoveDone {
        moved: receipt
            .moved
            .0
            .into_iter()
            .map(|task| MovedTask {
                title: titles.get(&task.id).cloned().unwrap_or_default(),
                url: public_url(
                    state,
                    &format!(
                        "/projects/{}/work-items/{}",
                        receipt.target_project_id, task.id
                    ),
                ),
                id: task.id.to_string(),
                from: task.from,
                to: task.to,
            })
            .collect(),
        notes: Vec::new(),
        created: true,
        replayed: true,
        left_behind: u32::try_from(receipt.left_behind).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Pieces of the move (unchanged from task_move)

#[derive(Debug, FromRow)]
struct SourceComment {
    author: String,
    created_at: String,
    plain_text: String,
}

#[derive(Debug, FromRow)]
struct SourceCriterion {
    text: String,
    completed: bool,
}

#[derive(Debug, FromRow)]
struct LinkLine {
    kind: String,
    label: String,
    /// Whether every member of the source project can see it. Private ones
    /// are only counted, never named, because the list is written into a
    /// comment that everyone in the target project reads.
    shared: bool,
}

async fn mover_name(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_else(|| "someone".to_owned()))
}

/// Copy acceptance criteria in order. Whoever moves the task is recorded as
/// creating them, and as completing the ones that were done, since the
/// original authors may not be in the target project.
async fn carry_criteria(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    source_project_id: Uuid,
    target_project_id: Uuid,
    source_id: Uuid,
    target_id: Uuid,
) -> Result<(), AppError> {
    let criteria = sqlx::query_as::<_, SourceCriterion>(
        r#"
        SELECT text, completed FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY position
        "#,
    )
    .bind(source_project_id)
    .bind(source_id)
    .fetch_all(&mut **transaction)
    .await?;
    let criteria = criteria
        .into_iter()
        .map(|criterion| (criterion.text, criterion.completed))
        .collect::<Vec<_>>();
    insert_criteria(
        transaction,
        target_project_id,
        target_id,
        actor_id,
        1,
        &criteria,
    )
    .await
}

/// The lines listing what stays linked on the original. Shared links are
/// named; private conversations and pull requests from private threads are
/// only counted, since readers of the new task may not be able to see them.
fn links_left_behind_text(links: &[LinkLine]) -> String {
    let mut text = String::new();
    for link in links.iter().filter(|link| link.shared) {
        text.push_str(&format!("\n- {}: {}", link.kind, link.label));
    }
    let private_conversations = links
        .iter()
        .filter(|link| !link.shared && link.kind == "Conversation")
        .count();
    let private_pull_requests = links
        .iter()
        .filter(|link| !link.shared && link.kind == "Pull request")
        .count();
    for (count, singular, plural) in [
        (
            private_conversations,
            "private conversation",
            "private conversations",
        ),
        (
            private_pull_requests,
            "private pull request",
            "private pull requests",
        ),
    ] {
        match count {
            0 => {}
            1 => text.push_str(&format!("\n- and 1 {singular}")),
            n => text.push_str(&format!("\n- and {n} {plural}")),
        }
    }
    text
}

/// Threads, repositories and pull requests linked to the original. They stay
/// in its project, so the new task lists them. The list is a comment every
/// target-project member reads, so only links every source-project member
/// can see are marked shared (and named); the rest are counted, and only if
/// the mover can see them.
async fn links_left_behind(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
    sees_code: bool,
) -> Result<Vec<LinkLine>, AppError> {
    Ok(sqlx::query_as::<_, LinkLine>(
        r#"
        SELECT 'Conversation' AS kind, conversation.name AS label,
               COALESCE(parent.visibility, conversation.visibility) = 'project' AS shared
        FROM work_item_conversation_links link
        JOIN conversations conversation ON conversation.id = link.conversation_id
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND conversation_visible_to(conversation.id, $3)
        UNION ALL
        SELECT 'Repository', repository.owner || '/' || repository.name, TRUE
        FROM work_item_repository_links link
        JOIN repositories repository ON repository.id = link.repository_id
        WHERE link.project_id = $1 AND link.work_item_id = $2 AND $4
        UNION ALL
        -- Same rule as pull_request_visible_to, for a reader who belongs to
        -- no private conversation: no run_artifact link, or one whose
        -- thread is project-visible.
        SELECT 'Pull request', pull_request.html_url,
               NOT EXISTS (
                   SELECT 1 FROM github_pull_request_conversation_links artifact
                   WHERE artifact.github_pull_request_id = pull_request.id
                     AND artifact.source = 'run_artifact')
               OR EXISTS (
                   SELECT 1 FROM github_pull_request_conversation_links artifact
                   JOIN conversations thread ON thread.id = artifact.conversation_id
                   LEFT JOIN conversations parent ON parent.id = thread.parent_channel_id
                   WHERE artifact.github_pull_request_id = pull_request.id
                     AND artifact.source = 'run_artifact'
                     AND COALESCE(parent.visibility, thread.visibility) = 'project')
        FROM github_pull_request_work_item_links link
        JOIN github_pull_requests pull_request ON pull_request.id = link.github_pull_request_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND pull_request_visible_to(pull_request.id, $3) AND $4
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(viewer)
    .bind(sees_code)
    .fetch_all(&mut **transaction)
    .await?)
}

/// Whether adding a comment tells watchers and the assignee. Copied history
/// doesn't: the move's own comment already did, once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Notify {
    Yes,
    No,
}

async fn add_comment(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    surface: Surface,
    work_item_id: Uuid,
    text: &str,
    notify: Notify,
) -> Result<(), AppError> {
    let validated = MessageContent::from_plain_text(text.to_owned())
        .map_err(|_| AppError::Validation("comment"))?;
    let content = serde_json::to_value(validated.content())
        .map_err(|_| AppError::Internal("serialize moved comment"))?;
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(actor.project_id)
    .bind(work_item_id)
    .fetch_one(&mut **transaction)
    .await?;
    let comment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_item_comments (
            id, project_id, work_item_id, author_actor_id, content_version, content, plain_text,
            created_at
        ) VALUES ($1, $2, $3, $4, 1, $5, $6, clock_timestamp())
        "#,
    )
    .bind(comment_id)
    .bind(actor.project_id)
    .bind(work_item_id)
    .bind(actor.actor_id)
    .bind(content)
    .bind(text)
    .execute(&mut **transaction)
    .await?;
    if notify == Notify::No {
        return finalize_copied_comment(
            transaction,
            actor,
            work_item_id,
            comment_id,
            surface.move_comment(),
        )
        .await;
    }
    finalize_comment(
        transaction,
        actor,
        work_item_id,
        comment_id,
        version,
        surface.move_comment(),
    )
    .await?;
    Ok(())
}

async fn archive(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    surface: Surface,
    task: &SourceTask,
    new_key: &str,
) -> Result<(), AppError> {
    // Archive state is the record's (crate::task_records); the version is
    // the task's.
    if !crate::task_records::write_shared(
        transaction,
        actor.project_id,
        task.id,
        actor.actor_id,
        crate::task_records::SharedChange {
            archived: Some(true),
            ..Default::default()
        },
    )
    .await?
    {
        return Err(AppError::Internal("a moved task was already archived"));
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE work_items
        SET version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        RETURNING version
        "#,
    )
    .bind(actor.project_id)
    .bind(task.id)
    .fetch_one(&mut **transaction)
    .await?;
    let summary = format!("Work Item moved to {new_key}");
    finalize_change(
        transaction,
        actor,
        task.id,
        ChangeFinalization {
            activity_kind: "work_item_archived",
            summary: &summary,
            audit_action: surface.moved_out(),
            activity_payload: serde_json::json!({ "movedTo": new_key, "previousVersion": task.version }),
            audit_metadata: serde_json::json!({ "moved_to": new_key }),
            version,
        },
    )
    .await?;
    Ok(())
}

/// Where an archived task went, with any subtasks that moved with it, by
/// the caller's reach now.
async fn already_moved(
    transaction: &mut Transaction<'_, Postgres>,
    state: &AppState,
    mover: &Mover,
    source_id: Uuid,
) -> Result<Option<Vec<MovedTask>>, AppError> {
    let rows = sqlx::query_as::<_, (String, String, Uuid, Uuid, String)>(
        // The moved subtasks have aliases of their own, keyed by their own
        // source IDs, so the whole source tree is walked.
        r#"
        WITH RECURSIVE source_tree(id, depth) AS (
            SELECT $1::uuid, 0
            UNION ALL
            SELECT child.id, source_tree.depth + 1
            FROM task_rows child
            JOIN source_tree ON child.parent_work_item_id = source_tree.id
            WHERE source_tree.depth < 50
        )
        SELECT alias.project_key || '-' || alias.number,
               project.project_key || '-' || item.number,
               item.id, item.project_id, item.title
        FROM source_tree
        JOIN work_item_key_aliases alias ON alias.source_work_item_id = source_tree.id
        JOIN task_rows item ON item.id = alias.work_item_id
        JOIN projects project ON project.id = item.project_id
        WHERE actor_can_access_project($2, $3, item.project_id)
          AND ($4::uuid IS NULL OR item.project_id = $4)
        ORDER BY source_tree.depth, alias.number
        "#,
    )
    .bind(source_id)
    .bind(mover.actor_id)
    .bind(mover.external_agent_connection_id)
    .bind(mover.confined_to)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    // Each title is the task's current one, where it is now; it is shown
    // only with "see tasks" in that project, as task_open would.
    let mut projects = rows.iter().map(|row| row.3).collect::<Vec<_>>();
    projects.sort_unstable();
    projects.dedup();
    let showing =
        crate::search::projects_showing_tasks(&state.pool, mover.actor_id, &projects).await?;
    Ok(Some(
        rows.into_iter()
            .map(|(from, to, id, project_id, title)| MovedTask {
                from,
                to,
                id: id.to_string(),
                title: if showing.contains(&project_id) {
                    title
                } else {
                    String::new()
                },
                url: public_url(state, &format!("/projects/{project_id}/work-items/{id}")),
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{ChoiceOption, FieldConfig};

    fn field(name: &str, field_type: FieldType, options: &[&str]) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig {
                options: options
                    .iter()
                    .map(|name| ChoiceOption {
                        id: Uuid::now_v7(),
                        name: (*name).to_owned(),
                        color: "#336699".to_owned(),
                        archived: false,
                        category: None,
                    })
                    .collect(),
                target: (field_type == FieldType::Link).then_some(LinkTarget::Task),
                collection: None,
            },
            position: 0,
            archived: false,
            version: 1,
            accessor: crate::fields::Accessor::Stored,
        }
    }

    #[test]
    fn only_fields_of_the_same_kind_are_compatible() {
        let text = field("Notes", FieldType::Text, &[]);
        let other_text = field("Summary", FieldType::Text, &[]);
        let number = field("Notes", FieldType::Number, &[]);
        let mut page = field("Spec", FieldType::Link, &[]);
        page.config.target = Some(LinkTarget::Page);
        let task = field("Spec", FieldType::Link, &[]);
        assert!(compatible(&text, &other_text));
        assert!(!compatible(&text, &number));
        assert!(!compatible(&task, &page));
        let mut archived = other_text.clone();
        archived.archived = true;
        assert!(!compatible(&text, &archived));
    }

    #[test]
    fn options_match_by_active_name_only() {
        let mut target = field("Risk", FieldType::SingleChoice, &["Low", "High"]);
        assert!(active_option_by_name(&target, " high ").is_some());
        target.config.options[1].archived = true;
        assert!(active_option_by_name(&target, "High").is_none());
    }

    #[test]
    fn issues_are_named_by_task_and_field() {
        let task = Uuid::now_v7();
        let field = Uuid::now_v7();
        let id = issue_id(task, field);
        assert_eq!(id, format!("{}:{}", task.simple(), field.simple()));
        assert_eq!(id, id.to_ascii_lowercase());
    }

    #[test]
    fn reasons_follow_the_field_kind() {
        let choice = field("Risk", FieldType::SingleChoice, &["Low"]);
        assert_eq!(issue_reason(&choice, None), IssueReason::NoMatchingField);
        assert_eq!(
            issue_reason(&choice, Some(&choice)),
            IssueReason::OptionMissing
        );
        let person = field("Owner", FieldType::Person, &[]);
        assert_eq!(
            issue_reason(&person, Some(&person)),
            IssueReason::PersonNotInTarget
        );
        let task = field("Parent", FieldType::Link, &[]);
        assert_eq!(
            issue_reason(&task, Some(&task)),
            IssueReason::LinkOutsideMove
        );
        let mut page = field("Spec", FieldType::Link, &[]);
        page.config.target = Some(LinkTarget::Page);
        assert_eq!(issue_reason(&page, Some(&page)), IssueReason::PageLink);
    }
}
