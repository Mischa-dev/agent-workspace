//! task_criteria: change a task's acceptance criteria in one call, all
//! operations or none (docs/plans/mcp-tools.md, Phase 2). The rules live in
//! `work_item_commands::change_criteria`, which the web app's criteria
//! handlers use too, so both write the same rows, activities and versions.
//!
//! `criterion` is never resolved by the MCP middleware: it is looked up only
//! among the criteria of the task `taskId` names, so an ID from another task
//! is "not found" and its existence is never confirmed.

use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, ToolResult, archived_task_message, string_or_number,
        version_conflict_message,
    },
    tasks::{resolve_task, scoped},
    writes::{self, WriteAccess},
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope},
    work_item_commands::{
        CriteriaCommand, CriteriaOperation, CriteriaOutcome, CriteriaPlanError, CriterionTarget,
        MAX_CRITERIA_PER_WORK_ITEM, PlannedCriterionChange, change_criteria,
        normalize_criterion_text, record_audit,
    },
};

/// The most operations one call takes.
const MAX_OPERATIONS: usize = 50;
/// The most criteria a result lists; `total` counts them all.
const MAX_LISTED: i64 = 100;
const REQUEST_ID_MESSAGE: &str = "Use a clientRequestId of 1 to 128 letters, digits, dashes, dots or underscores, and send the same one when you retry.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCriteriaArguments {
    /// The task's ID, link, or key such as AWD-12.
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// The task's version you last read (from task_open or task_list).
    pub expected_version: i64,
    /// 1 to 50 operations, applied in order. Either all apply or none do.
    pub operations: Vec<CriterionOperationArgument>,
    /// A stable ID so retrying never applies the change twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CriterionOperationArgument {
    /// add, edit (new text), check, uncheck, remove, or move (to a position).
    pub op: CriterionOp,
    /// Which criterion (not for add): its ID, or its position (such as 2) in
    /// the version you read, whatever earlier operations in this call do.
    #[serde(default, alias = "criterionId", deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub criterion: Option<String>,
    /// For add and edit: up to 1,000 characters; new lines are fine.
    pub text: Option<String>,
    /// For add (default: the end) and move: where it goes, 1 being first,
    /// counted in the list as it stands after the operations before this one.
    pub position: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CriterionOp {
    Add,
    Edit,
    Check,
    Uncheck,
    Remove,
    Move,
}

impl CriterionOp {
    fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Edit => "edit",
            Self::Check => "check",
            Self::Uncheck => "uncheck",
            Self::Remove => "remove",
            Self::Move => "move",
        }
    }
}

/// task_criteria's result. It is stored for replays, so it deserializes too.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CriteriaValue {
    pub task_id: String,
    /// The task's key, such as AWD-12.
    pub key: String,
    /// The task's version now; pass it as expectedVersion next time.
    pub version: i64,
    pub url: String,
    /// What each operation did, in the order given.
    pub operations: Vec<CriterionOperationOutcome>,
    /// The criteria after the change, in order (the first 100).
    pub criteria: Vec<CriterionValue>,
    /// How many criteria the task has.
    pub total: i64,
    /// How many of them are checked.
    pub completed: i64,
    /// True when this answers a retry of a call that already applied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CriterionOperationOutcome {
    pub op: CriterionOp,
    /// The criterion it changed; for add, the new criterion's ID.
    pub criterion_id: String,
    /// False when it was already that way, such as checking a checked
    /// criterion.
    pub changed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct CriterionValue {
    pub id: String,
    pub position: i32,
    pub text: String,
    pub completed: bool,
    /// Who checked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// One operation, checked and normalized: what the shared command runs, and
/// what the request hash covers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckedOperation {
    op: CriterionOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    criterion: Option<CriterionReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i32>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum CriterionReference {
    Id(Uuid),
    Position(i32),
}

impl CriterionReference {
    fn target(self) -> CriterionTarget {
        match self {
            Self::Id(id) => CriterionTarget::Id(id),
            Self::Position(position) => CriterionTarget::Position(position),
        }
    }

    fn describe(self) -> String {
        match self {
            Self::Id(id) => id.to_string(),
            Self::Position(position) => format!("#{position}"),
        }
    }
}

impl CheckedOperation {
    fn command(&self) -> CriteriaOperation {
        let target = self
            .criterion
            .map_or(CriterionTarget::Position(0), CriterionReference::target);
        match self.op {
            CriterionOp::Add => CriteriaOperation::Add {
                text: self.text.clone().unwrap_or_default(),
                position: self.position,
            },
            CriterionOp::Edit => CriteriaOperation::Update {
                target,
                text: self.text.clone(),
                position: None,
                completed: None,
            },
            CriterionOp::Check | CriterionOp::Uncheck => CriteriaOperation::Update {
                target,
                text: None,
                position: None,
                completed: Some(self.op == CriterionOp::Check),
            },
            CriterionOp::Move => CriteriaOperation::Update {
                target,
                text: None,
                position: self.position,
                completed: None,
            },
            CriterionOp::Remove => CriteriaOperation::Remove { target },
        }
    }
}

/// The field at fault and what would work.
type Problem = (String, String);

/// Checks every operation before anything is read: the fields each op needs
/// and takes, the text, and how criteria and positions are written.
fn check(arguments: &TaskCriteriaArguments) -> Result<Vec<CheckedOperation>, Problem> {
    if arguments.operations.is_empty() || arguments.operations.len() > MAX_OPERATIONS {
        return Err((
            "operations".to_owned(),
            format!("Pass 1 to {MAX_OPERATIONS} operations."),
        ));
    }
    if arguments.expected_version < 1 {
        return Err((
            "expectedVersion".to_owned(),
            "Pass the version you last read (from task_open or task_list).".to_owned(),
        ));
    }
    let mut checked = Vec::with_capacity(arguments.operations.len());
    for (index, operation) in arguments.operations.iter().enumerate() {
        let number = index + 1;
        let name = operation.op.name();
        let field = |name: &str| format!("operations[{index}].{name}");
        let (needs_criterion, takes_text, takes_position) = match operation.op {
            CriterionOp::Add => (false, true, true),
            CriterionOp::Edit => (true, true, false),
            CriterionOp::Check | CriterionOp::Uncheck | CriterionOp::Remove => (true, false, false),
            CriterionOp::Move => (true, false, true),
        };
        let needs_text = takes_text;
        let needs_position = operation.op == CriterionOp::Move;
        if operation.criterion.is_some() && !needs_criterion {
            return Err((
                field("criterion"),
                format!(
                    "Operation {number} ({name}) doesn't take a criterion: a new criterion gets its ID when it's added."
                ),
            ));
        }
        if needs_criterion && operation.criterion.is_none() {
            return Err((
                field("criterion"),
                format!(
                    "Operation {number} ({name}) needs criterion: its ID or its position, such as 2 (task_open lists them)."
                ),
            ));
        }
        if operation.text.is_some() && !takes_text {
            return Err((
                field("text"),
                format!(
                    "Operation {number} ({name}) doesn't take text. Use edit to change the wording."
                ),
            ));
        }
        if needs_text && operation.text.is_none() {
            return Err((
                field("text"),
                format!("Operation {number} ({name}) needs text."),
            ));
        }
        if operation.position.is_some() && !takes_position {
            return Err((
                field("position"),
                format!(
                    "Operation {number} ({name}) doesn't take a position. Use move to change where it goes."
                ),
            ));
        }
        if needs_position && operation.position.is_none() {
            return Err((
                field("position"),
                format!(
                    "Operation {number} ({name}) needs position: where it goes, 1 being first."
                ),
            ));
        }
        let text = match operation.text.as_deref() {
            Some(text) => Some(normalize_criterion_text(text).map_err(|_| {
                (
                    field("text"),
                    format!(
                        "Operation {number}: criterion text must be 1 to 1,000 characters, with no control characters except new lines and tabs."
                    ),
                )
            })?),
            None => None,
        };
        let criterion = match operation.criterion.as_deref() {
            Some(value) => Some(parse_reference(value).ok_or_else(|| {
                (
                    field("criterion"),
                    format!(
                        "Operation {number}: name the criterion by its ID or its position, such as 2 (task_open lists them)."
                    ),
                )
            })?),
            None => None,
        };
        if let Some(position) = operation.position
            && !(1..=MAX_CRITERIA_PER_WORK_ITEM as i32).contains(&position)
        {
            return Err((
                field("position"),
                format!("Operation {number}: use a position from 1 to 1,000, where 1 is first."),
            ));
        }
        checked.push(CheckedOperation {
            op: operation.op,
            criterion,
            text,
            position: operation.position,
        });
    }
    Ok(checked)
}

/// A criterion as agents name one: its ID, or its position such as `2` or
/// `#2`.
fn parse_reference(value: &str) -> Option<CriterionReference> {
    let value = value.trim();
    if let Ok(id) = value.parse::<Uuid>() {
        return Some(CriterionReference::Id(id));
    }
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i32>()
        .ok()
        .filter(|position| (1..=MAX_CRITERIA_PER_WORK_ITEM as i32).contains(position))
        .map(CriterionReference::Position)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Plain words for an operation that can't apply to the criteria as they
/// are. Nothing was written.
fn plan_refusal(
    error: CriteriaPlanError,
    criteria: usize,
    key: &str,
    version: i64,
    operations: &[CheckedOperation],
) -> ToolResult<CriteriaValue> {
    match error {
        CriteriaPlanError::UnknownCriterion { operation_index } => {
            let reference = operations
                .get(operation_index)
                .and_then(|operation| operation.criterion)
                .map_or_else(|| "that".to_owned(), CriterionReference::describe);
            ToolResult::invalid(
                &format!("operations[{operation_index}].criterion"),
                format!(
                    "Operation {}: {key} has no criterion {reference} at version {version}. It has {}; use an ID or position from task_open.",
                    operation_index + 1,
                    plural(criteria, "criterion", "criteria"),
                ),
            )
        }
        CriteriaPlanError::RemovedEarlier {
            operation_index,
            removed_by,
        } => ToolResult::invalid(
            &format!("operations[{operation_index}].criterion"),
            format!(
                "Operation {} changes a criterion that operation {} removed.",
                operation_index + 1,
                removed_by + 1
            ),
        ),
        CriteriaPlanError::Position {
            operation_index,
            max,
        } => {
            let position = operations
                .get(operation_index)
                .and_then(|operation| operation.position)
                .map_or_else(String::new, |position| format!(" {position}"));
            ToolResult::invalid(
                &format!("operations[{operation_index}].position"),
                format!(
                    "Operation {}: position{position} is out of range. Use 1 to {max}, counting the list after the operations before it.",
                    operation_index + 1
                ),
            )
        }
        CriteriaPlanError::TooMany { count } => ToolResult::invalid(
            "operations",
            format!("A task can have at most 1,000 acceptance criteria; this would make {count}."),
        ),
    }
}

pub(super) async fn task_criteria(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCriteriaArguments,
) -> ToolResult<CriteriaValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let operations = match check(&arguments) {
        Ok(operations) => operations,
        Err((field, message)) => return ToolResult::invalid(&field, message),
    };
    let key = match arguments.client_request_id.as_deref() {
        Some(value) => match writes::parse_request_id(value) {
            Some(key) => key,
            None => return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE),
        },
        None => match writes::parse_request_id(&Uuid::now_v7().to_string()) {
            Some(key) => key,
            None => return ToolResult::unavailable(),
        },
    };
    let result = async {
        let Some((task_id, project_id)) =
            resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        let scoped = scoped(principal, project_id);
        let mut transaction = state.pool.begin().await?;
        // Marks the app a person acts through, takes the project lock every
        // task write takes first, and checks the role (Action::McpWrite).
        match writes::check_write_access(&mut transaction, &scoped).await? {
            WriteAccess::Allowed => {}
            WriteAccess::ReadOnly => return Ok(ToolResult::read_only()),
            WriteAccess::NotFound => return Ok(ToolResult::not_found("That task")),
            WriteAccess::ProjectArchived => {
                return Ok(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE));
            }
            WriteAccess::Denied => {
                return Ok(ToolResult::denied(
                    "You can't change tasks in that project.",
                ));
            }
        }
        let scope = resource_bound_scope("mcp_work_items", task_id, "criteria")?;
        let request_hash =
            RequestHash::for_value(&(task_id, arguments.expected_version, &operations))?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await
        {
            Ok(IdempotencyOutcome::Replay(replay)) => {
                let mut value = replay.decode::<CriteriaValue>()?;
                transaction.commit().await?;
                value.idempotent_replay = true;
                return Ok(ToolResult::ok(value));
            }
            Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
            Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                return Ok(ToolResult::invalid(
                    "clientRequestId",
                    "That clientRequestId was already used for a different change. Use a new one.",
                ));
            }
            Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                return Ok(ToolResult::conflict(
                    "The same change is still being applied. Try again in a moment.",
                ));
            }
            Err(error) => return Err(error),
        };
        let (task_key, url) = writes::locate(&mut transaction, state, project_id, task_id).await?;
        let actor = writes::mcp_actor(&scoped, key.as_str().to_owned());
        let commands = operations
            .iter()
            .map(CheckedOperation::command)
            .collect::<Vec<_>>();
        let outcome = change_criteria(
            &mut transaction,
            &actor,
            task_id,
            CriteriaCommand {
                expected_version: arguments.expected_version,
                operations: &commands,
                audit_action: "work_item.mcp_criteria_changed",
            },
        )
        .await?;
        // Anything but a change (or none needed) returns before commit, so
        // the transaction rolls back and the clientRequestId stays free.
        let (version, changes) = match outcome {
            CriteriaOutcome::Applied {
                version, changes, ..
            } => (version, changes),
            CriteriaOutcome::Unchanged { current, changes } => {
                record_audit(
                    &mut transaction,
                    &actor,
                    "work_item.mcp_criteria_unchanged",
                    task_id,
                    serde_json::json!({"changed": false}),
                )
                .await?;
                (current.version, changes)
            }
            // An archived task refuses everything but a restore, whatever
            // version the call names.
            CriteriaOutcome::Conflict(current) if current.archived => {
                return Ok(ToolResult::conflict(archived_task_message(&task_key)));
            }
            CriteriaOutcome::Conflict(current) => {
                return Ok(ToolResult::conflict(version_conflict_message(
                    &task_key,
                    current.version,
                )));
            }
            CriteriaOutcome::NotFound => return Ok(ToolResult::not_found("That task")),
            CriteriaOutcome::Plan { error, criteria } => {
                return Ok(plan_refusal(
                    error,
                    criteria,
                    &task_key,
                    arguments.expected_version,
                    &operations,
                ));
            }
        };
        let value = criteria_value(
            &mut transaction,
            CriteriaValueParts {
                task_id,
                project_id,
                key: task_key,
                url,
                version,
            },
            &operations,
            &changes,
        )
        .await?;
        complete(&mut transaction, &reservation, StatusCode::OK, &value).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_criteria"))
}

struct CriteriaValueParts {
    task_id: Uuid,
    project_id: Uuid,
    key: String,
    url: String,
    version: i64,
}

/// The result: what each operation did, and the criteria as they now are,
/// read inside the transaction so they match the version returned.
async fn criteria_value(
    transaction: &mut Transaction<'_, Postgres>,
    parts: CriteriaValueParts,
    operations: &[CheckedOperation],
    changes: &[PlannedCriterionChange],
) -> Result<CriteriaValue, AppError> {
    let criteria = sqlx::query_as::<_, CriterionValue>(
        r#"
        SELECT criterion.id::text AS id, criterion.position, criterion.text,
               criterion.completed, completer.display_name AS completed_by,
               criterion.completed_at::text AS completed_at
        FROM work_item_acceptance_criteria criterion
        LEFT JOIN project_memberships completer
          ON completer.project_id = criterion.project_id
         AND completer.actor_id = criterion.completed_by_actor_id
        WHERE criterion.project_id = $1 AND criterion.work_item_id = $2
        ORDER BY criterion.position
        LIMIT $3
        "#,
    )
    .bind(parts.project_id)
    .bind(parts.task_id)
    .bind(MAX_LISTED)
    .fetch_all(&mut **transaction)
    .await?;
    let (total, completed) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT count(*), count(*) FILTER (WHERE completed)
        FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        "#,
    )
    .bind(parts.project_id)
    .bind(parts.task_id)
    .fetch_one(&mut **transaction)
    .await?;
    // The plan has exactly one change per operation, in order.
    let operations = operations
        .iter()
        .zip(changes)
        .map(|(operation, change)| CriterionOperationOutcome {
            op: operation.op,
            criterion_id: change.criterion_id.to_string(),
            changed: change.changed,
        })
        .collect();
    Ok(CriteriaValue {
        task_id: parts.task_id.to_string(),
        key: parts.key,
        version: parts.version,
        url: parts.url,
        operations,
        criteria,
        total,
        completed,
        idempotent_replay: false,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{CriterionOp, CriterionReference, TaskCriteriaArguments, check, parse_reference};

    fn arguments(operations: serde_json::Value) -> TaskCriteriaArguments {
        serde_json::from_value(json!({
            "taskId": "AWD-12",
            "expectedVersion": 3,
            "operations": operations,
        }))
        .expect("arguments")
    }

    fn refused(operations: serde_json::Value) -> (String, String) {
        match check(&arguments(operations)) {
            Ok(checked) => panic!("accepted: {checked:?}"),
            Err(problem) => problem,
        }
    }

    #[test]
    fn criteria_are_named_by_id_or_position() {
        let id = Uuid::now_v7();
        assert!(matches!(
            parse_reference(&id.to_string()),
            Some(CriterionReference::Id(found)) if found == id
        ));
        for (written, position) in [("2", 2), (" #3 ", 3), ("1000", 1000)] {
            assert!(
                matches!(
                    parse_reference(written),
                    Some(CriterionReference::Position(found)) if found == position
                ),
                "{written}"
            );
        }
        for refused in ["0", "1001", "-1", "#", "two", "2.5", ""] {
            assert!(parse_reference(refused).is_none(), "{refused}");
        }
        // Agents write positions as numbers too.
        let checked =
            check(&arguments(json!([{"op": "check", "criterion": 2}]))).expect("number position");
        assert!(matches!(
            checked[0].criterion,
            Some(CriterionReference::Position(2))
        ));
        let checked = check(&arguments(json!([{"op": "remove", "criterionId": "#1"}])))
            .expect("criterionId alias");
        assert_eq!(checked[0].op, CriterionOp::Remove);
    }

    #[test]
    fn each_operation_takes_only_its_own_fields() {
        assert_eq!(refused(json!([])).0, "operations");
        assert_eq!(
            refused(json!(vec![json!({"op": "add", "text": "x"}); 51])).0,
            "operations"
        );
        let (field, message) = refused(json!([
            {"op": "add", "text": "fine"},
            {"op": "edit", "criterion": "1"}
        ]));
        assert_eq!(field, "operations[1].text");
        assert_eq!(message, "Operation 2 (edit) needs text.");
        let (field, message) = refused(json!([
            {"op": "add", "text": "a"},
            {"op": "add", "text": "b"},
            {"op": "check", "criterion": "1", "text": "c"}
        ]));
        assert_eq!(field, "operations[2].text");
        assert!(
            message.contains("Use edit to change the wording"),
            "{message}"
        );
        assert_eq!(
            refused(json!([{"op": "add", "text": "a", "criterion": "1"}])).0,
            "operations[0].criterion"
        );
        assert_eq!(
            refused(json!([{"op": "remove"}])).0,
            "operations[0].criterion"
        );
        assert_eq!(
            refused(json!([{"op": "move", "criterion": "1"}])).0,
            "operations[0].position"
        );
        assert_eq!(
            refused(json!([{"op": "uncheck", "criterion": "1", "position": 2}])).0,
            "operations[0].position"
        );
        assert_eq!(
            refused(json!([{"op": "add", "text": "   "}])).0,
            "operations[0].text"
        );
        assert_eq!(
            refused(json!([{"op": "add", "text": "a", "position": 0}])).0,
            "operations[0].position"
        );
        assert_eq!(
            refused(json!([{"op": "check", "criterion": "first"}])).0,
            "operations[0].criterion"
        );
        let checked = check(&arguments(json!([
            {"op": "add", "text": "  Works offline\n(on 3G) ", "position": 1},
            {"op": "move", "criterion": "2", "position": 1},
            {"op": "uncheck", "criterion": "1"}
        ])))
        .expect("valid");
        assert_eq!(checked[0].text.as_deref(), Some("Works offline\n(on 3G)"));
        assert!(
            serde_json::from_value::<TaskCriteriaArguments>(json!({
                "taskId": "AWD-1", "expectedVersion": 1,
                "operations": [{"op": "delete", "criterion": "1"}]
            }))
            .is_err()
        );
    }
}
