//! Collections as operations (K2). `formula_preview` (T1.17) checks a
//! formula as someone writes it and works it out on one record, without
//! saving anything: the web app's formula editor calls it as they type.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::computed;
use crate::{
    fields::{
        Accessor, FieldConfig, FieldDefinition, FieldType,
        formula::{self, ResultType},
    },
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Registry, Target, Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FormulaPreview>();
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
    const KIND: OpKind = OpKind::Describe;
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
