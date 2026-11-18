//! Collections as operations (K2). `formula_preview` (T1.17) checks a
//! formula as someone writes it and works it out on one record, without
//! saving anything: the web app's formula editor calls it as they type.
//! `button_press` (T1.21) runs a button field's operations on one record as
//! whoever presses it; `button_get` says whether they can, and if not why,
//! so the web app draws the button disabled with the reason.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::computed;
use crate::{
    fields::{
        Accessor, FieldConfig, FieldDefinition, FieldType,
        button::{self, ButtonConfig},
        formula::{self, ResultType},
    },
    ops::{
        DispatchRequest, Idempotency, OpCall, OpError, OpKind, Operation, Registry, Target,
        Targeted, Unkeyed, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FormulaPreview>();
    registry.add::<ButtonGet>();
    registry.add::<ButtonPress>();
}

/// Check a formula and see its value on a record.
pub struct FormulaPreview;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormulaPreviewInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// The formula as written, fields by name in braces: `{Price} * {Qty}`.
    pub formula: String,
    /// The formula field being edited, if it exists already: its own name
    /// can't be used, and the formulas it reads are checked for loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<IdRef>,
    /// The record to work it out on, by number; the first one when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<i64>,
}

impl Targeted for FormulaPreviewInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

/// The record a preview was worked out on.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRecord {
    pub number: i64,
    pub title: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormulaPreviewOutput {
    /// What's wrong with the formula, so it can't be saved yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// What it gives: number, text, boolean, date or list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultType>,
    /// The record it was worked out on; none when the collection has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<PreviewRecord>,
    /// Its value there: null for none, `{"error": ...}` for an error.
    #[serde(default)]
    pub value: Value,
}

impl Operation for FormulaPreview {
    const NAME: &'static str = "formula_preview";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Preview a formula";
    const DESCRIPTION: &'static str = "Check a formula field's formula without saving it, and \
        see its value on one record of the collection (the first, or the record numbered). \
        Answers what to fix when it can't be saved; collection_write saves it.";

    type Input = FormulaPreviewInput;
    type Output = FormulaPreviewOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: FormulaPreviewInput,
    ) -> Result<FormulaPreviewOutput, OpError> {
        let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let collection_id = input.collection.id();
        let tx = call.tx().await?;
        let Some((title_name, acts_like_task)) = sqlx::query_as::<_, (String, bool)>(
            "SELECT title_name, acts_like_task OR builtin IS NOT NULL FROM collections \
             WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(collection_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?
        else {
            return Err(OpError::not_found("collection"));
        };
        let refuse = |problem: String| FormulaPreviewOutput {
            problem: Some(problem),
            result: None,
            record: None,
            value: Value::Null,
        };
        if acts_like_task {
            return Ok(refuse(
                "Formula fields are for collections whose records aren't tasks, for now."
                    .to_owned(),
            ));
        }
        let definitions = super::field_definitions(tx, project_id, collection_id, true).await?;
        let own = input.field.map(IdRef::id);
        let config = match formula::compile(
            &input.formula,
            &formula::Scope {
                fields: &definitions,
                title_name: &title_name,
                own,
            },
        ) {
            Ok(config) => config,
            Err(problem) => return Ok(refuse(problem)),
        };
        let result = config.result;
        let field = FieldDefinition {
            id: own.unwrap_or_else(Uuid::now_v7),
            name: String::new(),
            description: String::new(),
            field_type: FieldType::Formula,
            config: FieldConfig {
                formula: Some(config),
                ..FieldConfig::default()
            },
            position: i32::MAX,
            archived: false,
            version: 1,
            accessor: Accessor::Computed,
        };
        let found = computed::preview(
            tx,
            project_id,
            collection_id,
            &definitions,
            field,
            input.record,
        )
        .await?;
        Ok(match found {
            Some((number, title, value)) => FormulaPreviewOutput {
                problem: None,
                result: Some(result),
                record: Some(PreviewRecord { number, title }),
                value,
            },
            None => FormulaPreviewOutput {
                problem: None,
                result: Some(result),
                record: None,
                value: Value::Null,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Buttons (T1.21)

/// A button field of a collection, found for a call: the collection's
/// space comes from the dispatcher.
async fn button_field(
    call: &mut OpCall<'_>,
    collection_id: Uuid,
    field_id: Uuid,
) -> Result<(Uuid, String, Vec<FieldDefinition>, ButtonConfig), OpError> {
    let project_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let tx = call.tx().await?;
    let Some(title_name) = sqlx::query_scalar::<_, String>(
        "SELECT title_name FROM collections WHERE project_id = $1 AND id = $2 \
         AND archived_at IS NULL",
    )
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?
    else {
        return Err(OpError::not_found("collection"));
    };
    let definitions = super::field_definitions(tx, project_id, collection_id, true).await?;
    let Some(config) = definitions
        .iter()
        .find(|field| field.id == field_id && !field.archived)
        .and_then(|field| field.config.button.clone())
    else {
        return Err(OpError::not_found("button"));
    };
    Ok((project_id, title_name, definitions, config))
}

/// Whether the caller may run every step, by the checks each one's call
/// makes: the first refusal.
async fn may_press(
    call: &OpCall<'_>,
    collection_id: Uuid,
    config: &ButtonConfig,
) -> Result<(), OpError> {
    let target = Target::thing("collection", collection_id);
    for step in &config.steps {
        crate::ops::may_run(
            call.state,
            &call.ctx.caller,
            Some(call.ctx.workspace_id),
            &step.operation,
            &target,
        )
        .await?;
    }
    Ok(())
}

/// Which button.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonGetInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// The button field's id.
    pub field: IdRef,
}

impl Targeted for ButtonGetInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ButtonGetOutput {
    pub button: button::ButtonView,
    /// Whether you can press it here.
    pub can_press: bool,
    /// Why not, when you can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A button field, and whether the caller can press it.
pub struct ButtonGet;

impl Operation for ButtonGet {
    const NAME: &'static str = "button_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See a button";
    const DESCRIPTION: &'static str = "A button field's label and the operations pressing it \
        runs, and whether you can press it, with the reason when you can't. button_press \
        presses it on a record.";

    type Input = ButtonGetInput;
    type Output = ButtonGetOutput;

    async fn run(call: &mut OpCall<'_>, input: ButtonGetInput) -> Result<ButtonGetOutput, OpError> {
        let collection_id = input.collection.id();
        let (_, _, _, config) = button_field(call, collection_id, input.field.id()).await?;
        let reason = may_press(call, collection_id, &config)
            .await
            .err()
            .map(|error| error.message().into_owned());
        Ok(ButtonGetOutput {
            button: button::ButtonView::from(&config),
            can_press: reason.is_none(),
            reason,
        })
    }
}

/// A press: which button, on which record.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonPressInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// The button field's id.
    pub field: IdRef,
    /// The record: its number (`12` or `#12`) or id.
    pub record: String,
}

impl Targeted for ButtonPressInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

/// One operation a press ran.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PressedStep {
    pub operation: String,
    /// What the operation answered.
    pub value: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ButtonPressOutput {
    /// The record it was pressed on.
    pub record: PreviewRecord,
    /// Each operation it ran, in order.
    pub steps: Vec<PressedStep>,
}

/// The record a press names, with what its placeholders read.
#[derive(Debug, sqlx::FromRow)]
struct PressedRecord {
    id: Uuid,
    number: i64,
    title: String,
    field_values: Value,
    computed_values: Value,
    created_by_actor_id: Uuid,
    updated_by_actor_id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// What `{{name}}` fills in on `record` (`name` as stored).
fn fill(
    name: &str,
    record: &PressedRecord,
    fields: &HashMap<Uuid, &FieldDefinition>,
    collection_id: Uuid,
    project_id: Uuid,
    presser: Uuid,
) -> Value {
    match name {
        "record.id" => return Value::String(record.id.to_string()),
        "record.number" => return Value::from(record.number),
        "record.title" => return Value::String(record.title.clone()),
        "collection.id" => return Value::String(collection_id.to_string()),
        "space.id" => return Value::String(project_id.to_string()),
        "me" => return Value::String(presser.to_string()),
        "today" => return Value::String(Utc::now().date_naive().to_string()),
        _ => {}
    }
    let Some(field) = button::field_of(name).and_then(|id| fields.get(&id)) else {
        return Value::Null;
    };
    let key = field.id.to_string();
    let option_name = |id: &Value| {
        id.as_str()
            .and_then(|id| id.parse::<Uuid>().ok())
            .and_then(|id| field.config.options.iter().find(|option| option.id == id))
            .map_or(Value::Null, |option| Value::String(option.name.clone()))
    };
    match field.field_type {
        FieldType::CreatedTime => Value::String(record.created_at.to_rfc3339()),
        FieldType::EditedTime => Value::String(record.updated_at.to_rfc3339()),
        FieldType::CreatedBy => Value::String(record.created_by_actor_id.to_string()),
        FieldType::EditedBy => Value::String(record.updated_by_actor_id.to_string()),
        FieldType::AutoId => Value::from(record.number),
        FieldType::Formula => match record.computed_values.get(&key) {
            Some(Value::Object(_)) | None => Value::Null,
            Some(value) => value.clone(),
        },
        FieldType::SingleChoice | FieldType::Status => record
            .field_values
            .get(&key)
            .map_or(Value::Null, option_name),
        FieldType::MultipleChoice => match record.field_values.get(&key) {
            Some(Value::Array(ids)) => Value::Array(ids.iter().map(option_name).collect()),
            _ => Value::Null,
        },
        _ => record
            .field_values
            .get(&key)
            .cloned()
            .unwrap_or(Value::Null),
    }
}

/// The request id a step is held to: the press's, numbered.
fn step_request_id(press: Option<&str>, index: usize) -> Option<String> {
    press.map(|id| format!("{}.{}", id.chars().take(120).collect::<String>(), index + 1))
}

/// Press a button field on a record.
pub struct ButtonPress;

impl Operation for ButtonPress {
    const NAME: &'static str = "button_press";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    /// Buttons are pressed by people and agents, not by rules.
    const AUTOMATION: bool = false;
    /// Besides what each operation it runs emits.
    const EMITS: &'static [&'static str] = &["record.button_pressed"];
    const TITLE: &'static str = "Press a button";
    const DESCRIPTION: &'static str = "Press a button field on a record: runs the operations \
        the button names, in order, as you, with their arguments filled in from the record. \
        Stops at the first one refused and says which; the ones before it stay done. \
        button_get says whether you can press it.";

    type Input = ButtonPressInput;
    type Output = ButtonPressOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ButtonPressInput,
    ) -> Result<ButtonPressOutput, OpError> {
        let collection_id = input.collection.id();
        let (project_id, _, definitions, config) =
            button_field(call, collection_id, input.field.id()).await?;
        let reference = input.record.trim();
        let number = reference
            .strip_prefix('#')
            .unwrap_or(reference)
            .parse::<i64>()
            .ok();
        let id = crate::ops::refs::parse_id(reference);
        if number.is_none() && id.is_none() {
            return Err(OpError::invalid(
                "record",
                "Name the record by its number (#12) or id.",
            ));
        }
        let tx = call.tx().await?;
        let Some(record) = sqlx::query_as::<_, PressedRecord>(
            "SELECT id, number, title, field_values, computed_values, created_by_actor_id, \
             updated_by_actor_id, created_at, updated_at FROM collection_records \
             WHERE project_id = $1 AND collection_id = $2 AND archived_at IS NULL \
               AND (number = $3 OR id = $4)",
        )
        .bind(project_id)
        .bind(collection_id)
        .bind(number)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?
        else {
            return Err(OpError::not_found("record"));
        };
        // Refused before anything runs when any step would be.
        may_press(call, collection_id, &config).await?;
        let fields = definitions
            .iter()
            .map(|field| (field.id, field))
            .collect::<HashMap<_, _>>();
        let presser = call.ctx.caller.account_id;
        let registry = crate::ops::registry();
        let mut steps = Vec::with_capacity(config.steps.len());
        for (index, step) in config.steps.iter().enumerate() {
            let arguments = button::render(&step.arguments, &|name| {
                fill(name, &record, &fields, collection_id, project_id, presser)
            });
            let request = DispatchRequest {
                caller: call.ctx.caller.clone(),
                edge: call.ctx.edge.clone(),
                trace_id: call.ctx.trace_id.clone(),
                workspace: Some(call.ctx.workspace_id),
                request_id: step_request_id(call.ctx.request_id.as_deref(), index),
                unkeyed: Unkeyed::RunOnce,
                preview: false,
                renamed_from: None,
                // A button's steps run in its press, one after another.
                defer_long: false,
            };
            let answer =
                crate::ops::dispatch(call.state, request, &step.operation, arguments).await;
            if !answer.is_ok() {
                let title = registry
                    .get(&step.operation)
                    .map_or(step.operation.as_str(), |entry| entry.meta.title);
                let why = answer.body["message"]
                    .as_str()
                    .unwrap_or("it was refused")
                    .chars()
                    .take(300)
                    .collect::<String>();
                let before = if index == 0 {
                    String::new()
                } else {
                    format!(" The {index} step(s) before it went through.")
                };
                return Err(OpError::Invalid {
                    field: "button".into(),
                    message: format!(
                        "Step {} of the button, {title}, didn't go through: {why}{before}",
                        index + 1
                    )
                    .into(),
                });
            }
            // A secret a step shows once (a connection's webhook secret)
            // isn't passed on: the press's answer is kept for replays.
            let mut value = answer.body["value"].clone();
            if let (Some(entry), Some(fields)) =
                (registry.get(&step.operation), value.as_object_mut())
            {
                for field in entry.meta.shown_once {
                    fields.remove(*field);
                }
            }
            steps.push(PressedStep {
                operation: step.operation.clone(),
                value,
            });
        }
        call.audit_target(
            "collection_record",
            record.id,
            serde_json::json!({"field": input.field.id(), "steps": steps.len()}),
        );
        Ok(ButtonPressOutput {
            record: PreviewRecord {
                number: record.number,
                title: record.title,
            },
            steps,
        })
    }
}
