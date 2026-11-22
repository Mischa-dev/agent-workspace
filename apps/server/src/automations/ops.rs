//! Automation rules as operations (K2.18): list a space's rules, make,
//! change, remove or try one, and read what a rule did. Each calls the
//! rule functions the web routes call (`automations::create` and the
//! rest), as the caller the dispatcher checked; those keep the rule's own
//! checks (who may edit whose rule) and write their own audit rows. The
//! old `automation_write` and `automation_runs` tools are retired.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Action, Conditions, CreateRule, RunNow, Trigger, UpdateRule};
use crate::{
    error::AppError,
    events::Caller,
    mcp::task_commands::principal_for,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<AutomationList>()
        .add::<AutomationCreate>()
        .add::<AutomationUpdate>()
        .add::<AutomationDelete>()
        .add::<AutomationRunStart>()
        .add::<AutomationPreview>()
        .add::<AutomationHistory>();
}

/// The caller as the rule functions read it, in the space the call acts in.
/// Who makes or runs a rule, by the edge the call came through: a person in
/// the browser isn't acting through an app, so their apps' limit doesn't
/// apply to the rule (`automations::through_app`).
fn caller_and_space(call: &OpCall<'_>) -> (Caller, Uuid) {
    let principal = principal_for(call.ctx, None);
    let space = principal.project_id;
    let mut caller = Caller::mcp(&principal);
    if let crate::ops::Edge::Web { session_id } = call.ctx.edge {
        caller.session_id = Some(session_id);
        caller.channel = "browser";
        caller.authentication_class = "password_session";
        caller.client_name = None;
        caller.token_project_id = None;
    }
    (caller, space)
}

fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(Uuid::nil))
}

fn rule_id(value: &str) -> Result<Uuid, OpError> {
    parse_id(value).ok_or(OpError::Invalid {
        field: "ruleId".into(),
        message: "Use the rule's ID.".into(),
    })
}

fn to_value(value: impl Serialize) -> Result<Value, OpError> {
    serde_json::to_value(value).map_err(|_| OpError::Unavailable)
}

/// The rule functions' refusals, keeping the sentence an `Invalid` carries.
fn refused(error: AppError) -> OpError {
    match error {
        AppError::Invalid(message) => OpError::Invalid {
            field: "request".into(),
            message: message.into(),
        },
        other => other.into(),
    }
}

// ---------------------------------------------------------------------------
// automation_list

pub(crate) struct AutomationList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpaceInput {
    /// The space's ID, link or key (such as "AWD").
    pub project_id: String,
}

impl Targeted for SpaceInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for AutomationList {
    const NAME: &'static str = "automation_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List automations";
    const DESCRIPTION: &'static str = "A space's automation rules: each one's trigger, \
        conditions (a saved view's filters), actions, who it acts as, whether it's on and why \
        the server turned it off.";

    type Input = SpaceInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, _input: SpaceInput) -> Result<Value, OpError> {
        let (caller, space) = caller_and_space(call);
        to_value(
            super::list(call.state, &caller, space)
                .await
                .map_err(refused)?,
        )
    }
}

// ---------------------------------------------------------------------------
// automation_create

pub(crate) struct AutomationCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateInput {
    /// The space's ID, link or key.
    pub project_id: String,
    pub name: String,
    pub trigger: Trigger,
    /// Saved view and/or list filters the task must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
    /// One to five actions, run in order.
    pub actions: Vec<Action>,
    /// Start it off; left out, it's on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl Targeted for CreateInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for AutomationCreate {
    const NAME: &'static str = "automation_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Create automation";
    const DESCRIPTION: &'static str = "Make an automation rule in a space. Triggers: \
        task_created, task_matches (a task enters the conditions), run_finished, \
        pull_request_checks_failed, message_posted, schedule, capacity_window. Conditions: a \
        saved view (viewId) and/or task list filters. Actions: launch (with the space's run \
        defaults), set_field, notify, post_message, run_operation; text can use {{task.key}}, \
        {{task.title}}, {{task.url}}, {{pr.url}} and similar. The rule acts as you and stops \
        if you lose access.";
    const EMITS: &'static [&'static str] = &["automation.created"];

    type Input = CreateInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: CreateInput) -> Result<Value, OpError> {
        let (caller, space) = caller_and_space(call);
        let state = call.state;
        let made = super::create(
            state,
            call.tx().await?,
            &caller,
            space,
            CreateRule {
                name: input.name,
                trigger: input.trigger,
                conditions: input.conditions.unwrap_or_default(),
                actions: input.actions,
                enabled: input.enabled,
            },
        )
        .await
        .map_err(refused)?;
        call.audited_by_command();
        to_value(made)
    }
}

// ---------------------------------------------------------------------------
// automation_update

pub(crate) struct AutomationUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateInput {
    /// The space's ID, link or key.
    pub project_id: String,
    pub rule_id: String,
    /// The version you last read.
    pub expected_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<Action>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl Targeted for UpdateInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for AutomationUpdate {
    const NAME: &'static str = "automation_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update automation";
    const DESCRIPTION: &'static str = "Change an automation rule: its name, trigger, \
        conditions, actions, or whether it's on. Pass the version you last read.";
    const EMITS: &'static [&'static str] = &["automation.updated"];

    type Input = UpdateInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: UpdateInput) -> Result<Value, OpError> {
        let rule = rule_id(&input.rule_id)?;
        let (caller, space) = caller_and_space(call);
        let changed = super::update(
            call.state,
            &caller,
            space,
            rule,
            UpdateRule {
                expected_version: input.expected_version,
                name: input.name,
                trigger: input.trigger,
                conditions: input.conditions,
                actions: input.actions,
                enabled: input.enabled,
            },
        )
        .await
        .map_err(refused)?;
        call.audited_by_command();
        to_value(changed)
    }
}

// ---------------------------------------------------------------------------
// automation_delete

pub(crate) struct AutomationDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuleInput {
    /// The space's ID, link or key.
    pub project_id: String,
    pub rule_id: String,
}

impl Targeted for RuleInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for AutomationDelete {
    const NAME: &'static str = "automation_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove automation";
    const DESCRIPTION: &'static str = "Remove an automation rule. Your own, or any with the \
        right to manage automations; what it already did stays.";
    const EMITS: &'static [&'static str] = &["automation.removed"];

    type Input = RuleInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: RuleInput) -> Result<Value, OpError> {
        let rule = rule_id(&input.rule_id)?;
        let (caller, space) = caller_and_space(call);
        super::delete(call.state, &caller, space, rule)
            .await
            .map_err(refused)?;
        call.audited_by_command();
        Ok(json!({ "deleted": true }))
    }
}

// ---------------------------------------------------------------------------
// automation_run_start

pub(crate) struct AutomationRunStart;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunStartInput {
    /// The space's ID, link or key.
    pub project_id: String,
    pub rule_id: String,
    /// Try it on this task (ID, link or key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl Targeted for RunStartInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for AutomationRunStart {
    const NAME: &'static str = "automation_run_start";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Try automation";
    const DESCRIPTION: &'static str = "Fire an automation rule now, to try it, optionally on \
        one task. automation_history shows what it did.";
    const EMITS: &'static [&'static str] = &["automation.ran"];

    type Input = RunStartInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: RunStartInput) -> Result<Value, OpError> {
        let rule = rule_id(&input.rule_id)?;
        let work_item_id = match input.task_id.as_deref() {
            Some(value) => Some(parse_id(value).ok_or(OpError::Invalid {
                field: "taskId".into(),
                message: "Use the task's ID or link.".into(),
            })?),
            None => None,
        };
        let (caller, space) = caller_and_space(call);
        // Boxed: a firing runs its actions, which can run operations.
        let ran = Box::pin(super::run_now(
            call.state,
            &caller,
            space,
            rule,
            RunNow { work_item_id },
        ))
        .await
        .map_err(refused)?;
        to_value(ran)
    }
}

// ---------------------------------------------------------------------------
// automation_preview

pub(crate) struct AutomationPreview;

impl Operation for AutomationPreview {
    const NAME: &'static str = "automation_preview";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Preview automation";
    const DESCRIPTION: &'static str = "What an automation rule you made would do if it fired \
        now, optionally on one task: the tasks it would act on and each action with its text \
        filled in. It sends, changes and records nothing; automation_run_start fires it for real.";

    type Input = RunStartInput;
    type Output = super::PreviewView;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunStartInput,
    ) -> Result<super::PreviewView, OpError> {
        let rule = rule_id(&input.rule_id)?;
        let work_item_id = match input.task_id.as_deref() {
            Some(value) => Some(parse_id(value).ok_or(OpError::Invalid {
                field: "taskId".into(),
                message: "Use the task's ID or link.".into(),
            })?),
            None => None,
        };
        let (caller, space) = caller_and_space(call);
        super::preview(call.state, &caller, space, rule, RunNow { work_item_id })
            .await
            .map_err(refused)
    }
}

// ---------------------------------------------------------------------------
// automation_history

pub(crate) struct AutomationHistory;

impl Operation for AutomationHistory {
    const NAME: &'static str = "automation_history";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Automation history";
    const DESCRIPTION: &'static str = "What an automation rule did each time it fired, newest \
        first: the cause, each action's result, and why a firing was skipped.";

    type Input = RuleInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: RuleInput) -> Result<Value, OpError> {
        let rule = rule_id(&input.rule_id)?;
        let (caller, space) = caller_and_space(call);
        to_value(
            super::runs(call.state, &caller, space, rule)
                .await
                .map_err(refused)?,
        )
    }
}
