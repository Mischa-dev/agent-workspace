//! Collections as operations (K2). `formula_preview` (T1.17) checks a
//! formula as someone writes it and works it out on one record, without
//! saving anything: the web app's formula editor calls it as they type.
//! `button_press` (T1.21) runs a button field's operations on one record as
//! whoever presses it; `button_get` says whether they can, and if not why,
//! so the web app draws the button disabled with the reason.
//! `record_create` (C2.21) adds one record, made from a message if asked:
//! the record links back to it and, with saySo, the conversation says so.

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
        ConflictReason, DispatchRequest, Idempotency, OpCall, OpError, OpKind, Operation, Perm,
        Registry, Target, Targeted, Unkeyed,
        refs::{IdRef, MessageRef},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FormulaPreview>();
    registry.add::<ButtonGet>();
    registry.add::<ButtonPress>();
    registry.add::<RecordCreate>();
    registry.add::<RecordUpdate>();
}

/// Add a record to a collection, made from a message if you like.
pub struct RecordCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordCreateInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// The record's title. Made from a message, its first line when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Values by field name or id.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, Value>,
    /// The message it is made from (its id, or a link), in a conversation of
    /// the collection's space that you can read. The record links back to it.
    #[serde(
        default,
        alias = "sourceMessageId",
        skip_serializing_if = "Option::is_none"
    )]
    pub from_message_id: Option<MessageRef>,
    /// With fromMessageId: also say in that conversation that you made the
    /// message into this record.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub say_so: bool,
    /// A record template of the collection to start from (T1.30): its title
    /// and field values, under what you give.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<IdRef>,
}

impl Targeted for RecordCreateInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordCreateOutput {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub number: i64,
    pub title: String,
}

/// A message's first line, as a record title.
fn title_from(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .take(300)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl Operation for RecordCreate {
    const NAME: &'static str = "record_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["record.created"];
    const TITLE: &'static str = "Create a record";
    const DESCRIPTION: &'static str = "Add one record to a collection, with its title and \
        field values by name. Pass fromMessageId to make it from a message: the record links \
        back to the message, takes its first line as the title unless you give one, and with \
        saySo the conversation says you made it. Pass template to start from a record template \
        (record_template_list). Records of a collection that acts like tasks are tasks: use \
        task_create. record_update_many changes many records at once.";

    type Input = RecordCreateInput;
    type Output = RecordCreateOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RecordCreateInput,
    ) -> Result<RecordCreateOutput, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let actor = call.account_id();
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let Some(collection) = super::lock(tx, space_id, input.collection.id()).await? else {
            return Err(OpError::not_found("collection"));
        };
        if collection.archived {
            return Err(OpError::Conflict {
                reason: ConflictReason::Archived,
                message: format!(
                    "\"{}\" is archived. Restore it before adding records.",
                    collection.name
                )
                .into(),
                current: None,
            });
        }
        if collection.acts_like_task {
            return Err(OpError::Invalid {
                field: "collection".into(),
                message: format!(
                    "Records in \"{}\" are tasks: make them with task_create.",
                    collection.name
                )
                .into(),
            });
        }
        // The message, only from a conversation here the actor can read.
        let source = match input.from_message_id {
            None => None,
            Some(message) => Some(
                sqlx::query_as::<_, (Uuid, Uuid, String)>(
                    "SELECT message.id, message.conversation_id, message.plain_text \
                     FROM messages message \
                     WHERE message.id = $1 AND message.project_id = $2 \
                       AND message.removed_at IS NULL \
                       AND conversation_visible_to(message.conversation_id, $3)",
                )
                .bind(message.id())
                .bind(space_id)
                .bind(actor)
                .fetch_optional(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?
                .ok_or_else(|| {
                    OpError::invalid(
                        "fromMessageId",
                        "That isn't a message you can read in this collection's space.",
                    )
                })?,
            ),
        };
        // A template's title and values, under what the input gives.
        let mut fields = input.fields;
        let mut template_title = None;
        if let Some(template) = input.template {
            let template = super::record_templates::template(tx, space_id, template.id(), false)
                .await?
                .filter(|template| !template.archived && template.collection_id == collection.id)
                .ok_or_else(|| {
                    OpError::invalid("template", "That isn't a template of this collection.")
                })?;
            for (name, value) in super::record_templates::values_of(&template) {
                fields.entry(name).or_insert(value);
            }
            template_title = Some(super::record_templates::title_on(
                &template.title,
                chrono::Utc::now().date_naive(),
            ));
        }
        let title = input
            .title
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty())
            .or(template_title)
            .or_else(|| source.as_ref().map(|(_, _, text)| title_from(text)));
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?;
        let definitions = super::field_definitions(tx, space_id, collection.id, true).await?;
        // Fields only builders change (T1.19).
        let may_build = crate::access::check(
            tx,
            &caller,
            crate::access::Need::permission(Perm::CollectionsBuild),
            crate::access::Target::Space(space_id),
        )
        .await?
        .is_ok();
        let written = super::records::write(
            tx,
            space_id,
            &collection,
            crate::work_item_fields::Writer {
                actor_id: actor,
                project_key: &project_key,
            },
            may_build,
            &definitions,
            "fields",
            &[super::records::RecordInput {
                record: None,
                title,
                values: fields.into_iter().collect(),
                archived: None,
                expected_version: None,
            }],
        )
        .await?
        .map_err(|problem| match problem.kind {
            super::records::ProblemKind::Full => OpError::Conflict {
                reason: ConflictReason::State {
                    code: "collection_full",
                },
                message: problem.message.into(),
                current: None,
            },
            _ => OpError::Invalid {
                field: problem.field.into(),
                message: problem.message.into(),
            },
        })?;
        let record = written
            .into_iter()
            .next()
            .ok_or(OpError::Unavailable)?
            .record()
            .clone();
        if let Some((message_id, conversation_id, _)) = source {
            sqlx::query(
                "UPDATE collection_records SET source_message_id = $1 \
                 WHERE id = $2 AND project_id = $3",
            )
            .bind(message_id)
            .bind(record.id)
            .bind(space_id)
            .execute(&mut **tx)
            .await
            .map_err(crate::error::AppError::from)?;
            crate::conversations::system_entries::made_from_message(
                tx,
                conversation_id,
                actor,
                "record",
                record.id,
                message_id,
                input.say_so,
            )
            .await?;
        }
        crate::outbox::enqueue_collection_updated(tx, space_id, collection.id, &[record.id])
            .await?;
        call.audit_target("record", record.id, serde_json::json!({}));
        Ok(RecordCreateOutput {
            id: record.id,
            collection_id: collection.id,
            number: record.number,
            title: record.title,
        })
    }
}

/// Change one record, or propose the change (T1.41).
pub struct RecordUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordUpdateInput {
    /// The record: its id, or a link to it.
    pub record: IdRef,
    /// A new title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Values by field name or id; null clears one.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, Value>,
    /// true archives the record, false restores it (not proposable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// The version you last read: refused if it changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// When it becomes a proposal: a note for whoever decides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Targeted for RecordUpdateInput {
    fn target(&self) -> Target {
        Target::thing("record", self.record.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordUpdateOutput {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub number: i64,
    pub title: String,
    /// The record's version (unchanged when it became a proposal).
    pub version: i64,
    /// What changed: "title", "archived" and field names. Empty for a
    /// proposal.
    pub changed: Vec<String>,
    /// Set when your role only suggests here: the change waits as this
    /// proposal for someone who edits the record (proposal_decide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub proposal_id: Option<Uuid>,
    /// The fields the proposal sets, each with how it read before and after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed: Vec<Value>,
}

impl Operation for RecordUpdate {
    const NAME: &'static str = "record_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    // Suggest is the floor: `run` asks `access::write_mode` whether the
    // change lands (edit) or waits as a proposal (suggest only).
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsSuggest);
    const PROPOSABLE: bool = true;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &[
        "record.updated",
        "record.proposals_changed",
        "request.raised",
    ];
    const TITLE: &'static str = "Update a record";
    const DESCRIPTION: &'static str = "Change one record: its title, field values by name \
        (null clears one), or archive it. All of it is one change in the record's history. Pass \
        the version you read as expectedVersion. If your role only suggests on records, the \
        change becomes a proposal (proposalId) that someone who edits the record accepts or \
        rejects, from its request card or with proposal_decide; it is then written as yours. \
        record_update_many changes many records at once.";

    type Input = RecordUpdateInput;
    type Output = RecordUpdateOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RecordUpdateInput,
    ) -> Result<RecordUpdateOutput, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let actor = call.account_id();
        let caller = call.ctx.caller.clone();
        let via_grant_id = caller.via_grant_id;
        let id = input.record.id();
        let tx = call.tx().await?;
        let (collection_id, creator) = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT collection_id, created_by_actor_id FROM collection_records \
             WHERE id = $1 AND project_id = $2",
        )
        .bind(id)
        .bind(space_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?
        .ok_or_else(|| OpError::not_found("record"))?;
        let Some(collection) = super::lock(tx, space_id, collection_id).await? else {
            return Err(OpError::not_found("record"));
        };
        if collection.archived {
            return Err(OpError::Conflict {
                reason: ConflictReason::Archived,
                message: format!(
                    "\"{}\" is archived. Restore it before changing its records.",
                    collection.name
                )
                .into(),
                current: None,
            });
        }
        if collection.acts_like_task {
            return Err(OpError::Invalid {
                field: "record".into(),
                message: format!(
                    "Records in \"{}\" are tasks: change them with task_update.",
                    collection.name
                )
                .into(),
            });
        }
        let mode = crate::access::write_mode_locked(
            tx,
            &caller,
            "record",
            space_id,
            creator == actor,
            Some("collections"),
        )
        .await?
        .map_err(|_| OpError::from(crate::error::AppError::Forbidden))?
        .mode;
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?;
        let may_build = crate::access::check(
            tx,
            &caller,
            crate::access::Need::permission(Perm::CollectionsBuild),
            crate::access::Target::Space(space_id),
        )
        .await?
        .is_ok();
        let writer = crate::work_item_fields::Writer {
            actor_id: actor,
            project_key: &project_key,
        };
        let record_input = super::records::RecordInput {
            record: Some(super::records::RecordRef::Id(id)),
            title: input.title.clone(),
            values: input.fields.clone().into_iter().collect(),
            archived: input.archived,
            expected_version: input.expected_version,
        };
        let refused = |problem: super::records::WriteProblem| match problem.kind {
            super::records::ProblemKind::Conflict => OpError::Conflict {
                reason: ConflictReason::State {
                    code: "record_changed",
                },
                message: problem.message.into(),
                current: None,
            },
            super::records::ProblemKind::NotFound => OpError::not_found("record"),
            _ => OpError::Invalid {
                field: problem.field.into(),
                message: problem.message.into(),
            },
        };
        if mode == crate::access::WriteMode::Propose {
            let record = super::records::find(
                &mut **tx,
                space_id,
                collection.id,
                super::records::RecordRef::Id(id),
                false,
            )
            .await?
            .ok_or_else(|| OpError::not_found("record"))?;
            if let Some(expected) = input.expected_version
                && expected != record.version
            {
                return Err(OpError::Conflict {
                    reason: ConflictReason::State {
                        code: "record_changed",
                    },
                    message: format!(
                        "#{} changed since it was read (now version {}). Read it again and retry.",
                        record.number, record.version
                    )
                    .into(),
                    current: None,
                });
            }
            let note = input
                .note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty());
            if note.is_some_and(|note| note.chars().count() > 500) {
                return Err(OpError::invalid("note", "Keep a note to 500 characters."));
            }
            let proposed = super::record_proposals::propose(
                tx,
                writer,
                via_grant_id,
                &collection,
                &record,
                may_build,
                record_input,
                note,
            )
            .await?;
            return match proposed {
                super::record_proposals::Proposed::Made {
                    id: proposal,
                    changes,
                } => {
                    let name = if record.title.trim().is_empty() {
                        format!("#{}", record.number)
                    } else {
                        record.title.clone()
                    };
                    crate::proposals::raise_request(
                        tx,
                        proposal,
                        &format!("Accept a change to \"{name}\" in {}?", collection.name),
                        Some(creator),
                    )
                    .await?;
                    call.audit_target("proposal", proposal, serde_json::json!({ "record": id }));
                    Ok(RecordUpdateOutput {
                        id,
                        collection_id: collection.id,
                        number: record.number,
                        title: record.title,
                        version: record.version,
                        changed: Vec::new(),
                        proposal_id: Some(proposal),
                        proposed: changes
                            .into_iter()
                            .map(|mut change| {
                                if let Some(object) = change.as_object_mut() {
                                    object.remove("was");
                                    object.remove("is");
                                }
                                change
                            })
                            .collect(),
                    })
                }
                super::record_proposals::Proposed::Unchanged => Ok(RecordUpdateOutput {
                    id,
                    collection_id: collection.id,
                    number: record.number,
                    title: record.title,
                    version: record.version,
                    changed: Vec::new(),
                    proposal_id: None,
                    proposed: Vec::new(),
                }),
                super::record_proposals::Proposed::Problem(problem) => Err(refused(problem)),
                super::record_proposals::Proposed::Refused(message) => Err(OpError::Denied {
                    reason: crate::ops::DenyReason::Rule,
                    message: message.into(),
                }),
            };
        }
        let definitions = super::field_definitions(tx, space_id, collection.id, true).await?;
        let written = super::records::write(
            tx,
            space_id,
            &collection,
            writer,
            may_build,
            &definitions,
            "fields",
            &[record_input],
        )
        .await?
        .map_err(refused)?;
        let (record, changed) = match written.into_iter().next() {
            Some(super::records::Written::Changed { record, changed }) => (record, changed),
            Some(
                super::records::Written::Unchanged(record)
                | super::records::Written::Created(record),
            ) => (record, Vec::new()),
            None => return Err(OpError::Unavailable),
        };
        if !changed.is_empty() {
            crate::outbox::enqueue_collection_updated(tx, space_id, collection.id, &[record.id])
                .await?;
        }
        call.audit_target(
            "record",
            record.id,
            serde_json::json!({ "changed": changed }),
        );
        Ok(RecordUpdateOutput {
            id: record.id,
            collection_id: collection.id,
            number: record.number,
            title: record.title,
            version: record.version,
            changed,
            proposal_id: None,
            proposed: Vec::new(),
        })
    }
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
        Answers what to fix when it can't be saved; collection_create saves it.";

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
            "SELECT title_name, acts_like_task FROM collections \
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
        FieldType::Formula | FieldType::Rollup => match record.computed_values.get(&key) {
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
