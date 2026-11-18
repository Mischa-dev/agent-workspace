//! Forms as their own definition (T1.28). A form belongs to a collection
//! and says which of its fields to ask, in sections, with conditions and
//! values it sets on every record (`collection_forms::FormConfig`).
//!
//! A public form is filled in by anyone with its link: the link
//! (`bug_report_links`, its token, limits and state) points at the form,
//! and `bug_report_link_write` makes and changes both. An internal form has
//! no link: the space's members fill it in in the app (`form_answer_send`),
//! each answer a record written as them, so it may ask for people and
//! relations a public form never shows.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    collection_forms::{self, Audience, FormConfig, PublicField},
    error::AppError,
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

/// Forms one space keeps, archived ones included.
pub const MAX_FORMS_PER_SPACE: i64 = 200;
const MAX_NAME_CHARACTERS: usize = 80;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FormList>();
    registry.add::<FormGet>();
    registry.add::<FormCreate>();
    registry.add::<FormUpdate>();
    registry.add::<FormAnswerSend>();
}

/// A form to store.
pub struct NewForm<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub collection_id: Uuid,
    pub name: &'a str,
    pub audience: Audience,
    pub definition: &'a FormConfig,
    pub actor_id: Uuid,
}

/// Stores a checked form in the caller's transaction.
pub async fn insert(connection: &mut PgConnection, form: NewForm<'_>) -> Result<(), AppError> {
    let definition =
        serde_json::to_value(form.definition).map_err(|_| AppError::Internal("form definition"))?;
    sqlx::query(
        "INSERT INTO forms (id, workspace_id, space_id, collection_id, name, audience, \
             definition, created_by_actor_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(form.id)
    .bind(form.workspace_id)
    .bind(form.space_id)
    .bind(form.collection_id)
    .bind(form.name)
    .bind(form.audience.as_str())
    .bind(definition)
    .bind(form.actor_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn clean_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(OpError::invalid(
            "name",
            "Give the form a name of 1 to 80 characters, on one line.",
        ));
    }
    Ok(name.to_owned())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    collection_id: Uuid,
    collection_name: String,
    title_name: String,
    name: String,
    audience: String,
    definition: Value,
    version: i64,
    archived: bool,
    link_id: Option<Uuid>,
}

const SELECT: &str = "SELECT form.id, form.space_id, form.collection_id, \
        collection.name AS collection_name, collection.title_name, form.name, form.audience, \
        form.definition, form.version, form.archived_at IS NOT NULL AS archived, \
        link.id AS link_id \
    FROM forms form \
    JOIN collections collection \
      ON collection.project_id = form.space_id AND collection.id = form.collection_id \
    LEFT JOIN bug_report_links link ON link.form_id = form.id";

async fn find(
    connection: &mut PgConnection,
    space: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Option<Row>, OpError> {
    let sql = format!(
        "{SELECT} WHERE form.space_id = $1 AND form.id = $2 \
           AND module_part_on('collection', collection.id){}",
        if lock { " FOR UPDATE OF form" } else { "" }
    );
    Ok(sqlx::query_as::<_, Row>(&sql)
        .bind(space)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(AppError::from)?)
}

fn audience_of(row: &Row) -> Audience {
    Audience::parse(&row.audience).unwrap_or(Audience::Public)
}

fn config_of(row: &Row) -> FormConfig {
    serde_json::from_value(row.definition.clone()).unwrap_or_default()
}

/// A form in a list.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormSummary {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// public (anyone with its link) or internal (the space's members).
    pub audience: Audience,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub collection_name: String,
    /// How many questions it asks, after the title.
    pub questions: usize,
    pub version: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// A public form's link (manage it with bug_report_link_write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub link_id: Option<Uuid>,
}

fn summary(row: &Row) -> FormSummary {
    FormSummary {
        id: row.id,
        name: row.name.clone(),
        audience: audience_of(row),
        collection_id: row.collection_id,
        collection_name: row.collection_name.clone(),
        questions: config_of(row).fields.len(),
        version: row.version,
        archived: row.archived,
        link_id: row.link_id,
    }
}

// ---------------------------------------------------------------------------
// form_list

pub struct FormList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormListInput {
    /// The space (a project): its ID or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Only one collection's forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<IdRef>,
    /// Archived forms too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_archived: bool,
}

impl Targeted for FormListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormListOutput {
    pub forms: Vec<FormSummary>,
}

impl Operation for FormList {
    const NAME: &'static str = "form_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List forms";
    const DESCRIPTION: &'static str = "A space's forms: each adds a record to a collection. \
        Public ones are filled in by anyone with their link, internal ones by the space's \
        members (form_answer_send). form_get shows one form's questions.";

    type Input = FormListInput;
    type Output = FormListOutput;

    async fn run(call: &mut OpCall<'_>, input: FormListInput) -> Result<FormListOutput, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let sql = format!(
            "{SELECT} WHERE form.space_id = $1 \
               AND ($2::uuid IS NULL OR form.collection_id = $2) \
               AND ($3 OR form.archived_at IS NULL) \
               AND module_part_on('collection', collection.id) \
             ORDER BY form.archived_at IS NOT NULL, form.created_at, form.id"
        );
        let rows = sqlx::query_as::<_, Row>(&sql)
            .bind(space)
            .bind(input.collection.map(|collection| collection.id()))
            .bind(input.include_archived)
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?;
        Ok(FormListOutput {
            forms: rows.iter().map(summary).collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// form_get

pub struct FormGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormRefInput {
    /// The form: its id, or a link to it.
    pub form: IdRef,
}

impl Targeted for FormRefInput {
    fn target(&self) -> Target {
        Target::thing("form", self.form.id())
    }
}

/// One form: its settings and its questions as they are asked now.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormView {
    #[serde(flatten)]
    pub summary: FormSummary,
    /// What the record's title is called, such as Name or Summary: always
    /// asked first.
    pub title_name: String,
    /// Its settings, as form_update takes them.
    pub definition: FormConfig,
    /// The questions after the title, in order, with their options,
    /// conditions and section headings; one whose field was archived since
    /// drops out until it's back.
    pub questions: Vec<PublicField>,
    /// Whether you can change it.
    pub can_build: bool,
    /// Whether you can fill it in here (an internal form).
    pub can_answer: bool,
}

async fn view(call: &mut OpCall<'_>, space: Uuid, row: Row) -> Result<FormView, OpError> {
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    let definitions =
        crate::collections::field_definitions(tx, space, row.collection_id, false).await?;
    let config = config_of(&row);
    let audience = audience_of(&row);
    let can_build = crate::access::check(
        tx,
        &caller,
        crate::access::Need::permission(Perm::CollectionsBuild),
        crate::access::Target::Space(space),
    )
    .await?
    .is_ok()
        && caller.can_write;
    let can_edit = crate::access::check(
        tx,
        &caller,
        crate::access::Need::permission(Perm::CollectionsEdit),
        crate::access::Target::Space(space),
    )
    .await?
    .is_ok();
    Ok(FormView {
        questions: collection_forms::questions(&config, &definitions, audience),
        can_answer: audience == Audience::Internal && !row.archived && can_edit && caller.can_write,
        can_build,
        title_name: row.title_name.clone(),
        summary: summary(&row),
        definition: config,
    })
}

impl Operation for FormGet {
    const NAME: &'static str = "form_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get a form";
    const DESCRIPTION: &'static str = "One form: its settings (the definition form_update \
        takes) and its questions as they're asked now, with their options, conditions \
        (showIf: shown only when an earlier answer is one of some values) and section \
        headings. Answer an internal form with form_answer_send.";

    type Input = FormRefInput;
    type Output = FormView;

    async fn run(call: &mut OpCall<'_>, input: FormRefInput) -> Result<FormView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let row = find(tx, space, input.form.id(), false)
            .await?
            .ok_or(OpError::not_found("That form"))?;
        view(call, space, row).await
    }
}

// ---------------------------------------------------------------------------
// form_create

pub struct FormCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormCreateInput {
    /// The collection its answers become records of: its id, or a link.
    pub collection: IdRef,
    pub name: String,
    /// Its questions, sections, conditions and set values: {"intro",
    /// "fields": [{"field", "required", "showIf": {"field", "isOneOf"}}],
    /// "sections": [{"title", "description", "before"}], "sets": [{"field",
    /// "value"}], "thanks"}. Fields by name or id.
    #[serde(default)]
    pub definition: Value,
}

impl Targeted for FormCreateInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

/// The collection a form may be built on, locked, with its fields.
async fn buildable(
    connection: &mut PgConnection,
    space: Uuid,
    collection_id: Uuid,
) -> Result<
    (
        crate::collections::Collection,
        Vec<crate::fields::FieldDefinition>,
    ),
    OpError,
> {
    let collection = crate::collections::lock(connection, space, collection_id)
        .await?
        .ok_or(OpError::not_found("That collection"))?;
    if collection.archived {
        return Err(OpError::Conflict {
            reason: ConflictReason::Archived,
            message: format!(
                "\"{}\" is archived. Restore it before giving it a form.",
                collection.name
            )
            .into(),
            current: None,
        });
    }
    let definitions =
        crate::collections::field_definitions(connection, space, collection.id, false).await?;
    Ok((collection, definitions))
}

/// A definition as given, its fields named or by id, read into a form's
/// settings.
fn read_definition(
    mut definition: Value,
    definitions: &[crate::fields::FieldDefinition],
) -> Result<FormConfig, OpError> {
    let invalid = |message: String| OpError::Invalid {
        field: "definition".into(),
        message: message.into(),
    };
    if definition.is_null() {
        return Ok(FormConfig::default());
    }
    let mut problem = None;
    let mut by_id = |slot: Option<&mut Value>| {
        let Some(slot) = slot else { return };
        let Some(name) = slot.as_str() else { return };
        if Uuid::parse_str(name).is_ok() {
            return;
        }
        match definitions
            .iter()
            .find(|field| !field.archived && field.name.trim().eq_ignore_ascii_case(name.trim()))
        {
            Some(field) => *slot = Value::String(field.id.to_string()),
            None => {
                problem.get_or_insert_with(|| {
                    format!("The collection has no field named \"{name}\".")
                });
            }
        }
    };
    if let Some(fields) = definition.get_mut("fields").and_then(Value::as_array_mut) {
        for asked in fields {
            by_id(asked.get_mut("field"));
            by_id(
                asked
                    .get_mut("showIf")
                    .and_then(|condition| condition.get_mut("field")),
            );
        }
    }
    if let Some(sections) = definition.get_mut("sections").and_then(Value::as_array_mut) {
        for section in sections {
            by_id(section.get_mut("before"));
        }
    }
    if let Some(sets) = definition.get_mut("sets").and_then(Value::as_array_mut) {
        for set in sets {
            by_id(set.get_mut("field"));
        }
    }
    if let Some(problem) = problem {
        return Err(invalid(problem));
    }
    serde_json::from_value(definition)
        .map_err(|error| invalid(format!("The definition doesn't read: {error}.")))
}

fn checked(
    definition: &FormConfig,
    definitions: &[crate::fields::FieldDefinition],
    audience: Audience,
) -> Result<FormConfig, OpError> {
    collection_forms::check_for(definition, definitions, audience).map_err(|message| {
        OpError::Invalid {
            field: "definition".into(),
            message: message.into(),
        }
    })
}

impl Operation for FormCreate {
    const NAME: &'static str = "form_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Create a form";
    const DESCRIPTION: &'static str = "Make an internal form on a collection: the space's \
        members fill it in in the app (form_answer_send) and each answer becomes a record \
        written as them. It may ask for people and relations, split its questions into \
        sections, show a question only after some answer to an earlier one, and set fields \
        on every record. A public form, filled in by anyone with its link, is made with \
        bug_report_link_write.";

    type Input = FormCreateInput;
    type Output = FormView;

    async fn run(call: &mut OpCall<'_>, input: FormCreateInput) -> Result<FormView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let name = clean_name(&input.name)?;
        let tx = call.tx().await?;
        let (collection, definitions) = buildable(tx, space, input.collection.id()).await?;
        if collection.acts_like_task {
            return Err(OpError::Invalid {
                field: "collection".into(),
                message: format!(
                    "\"{}\" makes tasks: give it a public form (bug_report_link_write) or a task template instead.",
                    collection.name
                )
                .into(),
            });
        }
        let definition = checked(
            &read_definition(input.definition, &definitions)?,
            &definitions,
            Audience::Internal,
        )?;
        let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM forms WHERE space_id = $1")
            .bind(space)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
        if count >= MAX_FORMS_PER_SPACE {
            return Err(OpError::Conflict {
                reason: ConflictReason::State { code: "forms_full" },
                message: format!("A space keeps at most {MAX_FORMS_PER_SPACE} forms.").into(),
                current: None,
            });
        }
        let id = Uuid::now_v7();
        insert(
            tx,
            NewForm {
                id,
                workspace_id: workspace,
                space_id: space,
                collection_id: collection.id,
                name: &name,
                audience: Audience::Internal,
                definition: &definition,
                actor_id: actor,
            },
        )
        .await?;
        crate::outbox::enqueue_collection_updated(tx, space, collection.id, &[]).await?;
        call.audit_target(
            "form",
            id,
            serde_json::json!({ "collection": collection.id }),
        );
        let tx = call.tx().await?;
        let row = find(tx, space, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        view(call, space, row).await
    }
}

// ---------------------------------------------------------------------------
// form_update

pub struct FormUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormUpdateInput {
    /// The form: its id, or a link to it.
    pub form: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its whole definition, replacing the one it has (form_get shows it),
    /// fields by name or id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<Value>,
    /// Archive an internal form (it stops taking answers) or bring it back.
    /// A public form stops when its link is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// The version you read; a change made since is a conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for FormUpdateInput {
    fn target(&self) -> Target {
        Target::thing("form", self.form.id())
    }
}

impl Operation for FormUpdate {
    const NAME: &'static str = "form_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Change a form";
    const DESCRIPTION: &'static str = "Rename a form, replace its definition (questions, \
        sections, conditions and set values, as form_get shows it) or archive an internal \
        one. A public form never asks for people or links. Pass expectedVersion from \
        form_get.";

    type Input = FormUpdateInput;
    type Output = FormView;

    async fn run(call: &mut OpCall<'_>, input: FormUpdateInput) -> Result<FormView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let name = input.name.as_deref().map(clean_name).transpose()?;
        let tx = call.tx().await?;
        let row = find(tx, space, input.form.id(), true)
            .await?
            .ok_or(OpError::not_found("That form"))?;
        if let Some(expected) = input.expected_version
            && expected != row.version
        {
            return Err(OpError::Conflict {
                reason: ConflictReason::Version {
                    current_version: row.version,
                },
                message: format!(
                    "Someone changed this form first (now version {}). Read it again, then retry.",
                    row.version
                )
                .into(),
                current: None,
            });
        }
        let audience = audience_of(&row);
        if input.archived.is_some() && audience == Audience::Public {
            return Err(OpError::invalid(
                "archived",
                "A public form stops when its link is deleted (bug_report_link_write).",
            ));
        }
        let definition = match input.definition {
            None => None,
            Some(definition) => {
                let (_, definitions) = buildable(tx, space, row.collection_id).await?;
                Some(checked(
                    &read_definition(definition, &definitions)?,
                    &definitions,
                    audience,
                )?)
            }
        };
        let changed = name.as_ref().is_some_and(|name| *name != row.name)
            || definition
                .as_ref()
                .is_some_and(|definition| *definition != config_of(&row))
            || input
                .archived
                .is_some_and(|archived| archived != row.archived);
        if changed {
            let definition = definition
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| OpError::Unavailable)?;
            sqlx::query(
                "UPDATE forms SET name = coalesce($2, name), \
                     definition = coalesce($3, definition), \
                     archived_at = CASE WHEN $4::boolean IS NULL THEN archived_at \
                         WHEN $4 THEN coalesce(archived_at, now()) ELSE NULL END, \
                     version = version + 1, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(name.as_deref())
            .bind(definition)
            .bind(input.archived)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
            // A public form's link carries its name too.
            if let (Some(name), Some(link)) = (name.as_deref(), row.link_id) {
                sqlx::query(
                    "UPDATE bug_report_links SET name = $2, updated_at = now() WHERE id = $1",
                )
                .bind(link)
                .bind(name)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
            }
            crate::outbox::enqueue_collection_updated(tx, space, row.collection_id, &[]).await?;
        }
        call.audit_target("form", row.id, serde_json::json!({ "changed": changed }));
        let tx = call.tx().await?;
        let row = find(tx, space, row.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        view(call, space, row).await
    }
}

// ---------------------------------------------------------------------------
// form_answer_send

pub struct FormAnswerSend;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormAnswerInput {
    /// The internal form: its id, or a link to it.
    pub form: IdRef,
    /// The answers by field id or name, and the record's title as "title".
    /// A person by id or handle; people and relations as lists.
    pub answers: Map<String, Value>,
}

impl Targeted for FormAnswerInput {
    fn target(&self) -> Target {
        Target::thing("form", self.form.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormAnswered {
    /// The record the answer made.
    #[schemars(with = "String")]
    pub record_id: Uuid,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub number: i64,
    pub title: String,
    /// The form's thank-you note.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thanks: String,
}

impl Operation for FormAnswerSend {
    const NAME: &'static str = "form_answer_send";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["record.created"];
    const TITLE: &'static str = "Answer a form";
    const DESCRIPTION: &'static str = "Fill in one of the space's internal forms: the answer \
        becomes a record of its collection, written as you, with the form's set values. A \
        question whose condition isn't met is skipped, required or not. Pass a \
        clientRequestId so a retry makes one record.";

    type Input = FormAnswerInput;
    type Output = FormAnswered;

    async fn run(call: &mut OpCall<'_>, input: FormAnswerInput) -> Result<FormAnswered, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let actor = call.account_id();
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let row = find(tx, space, input.form.id(), false)
            .await?
            .ok_or(OpError::not_found("That form"))?;
        let closed = |message: &'static str| OpError::Conflict {
            reason: ConflictReason::State {
                code: "form_closed",
            },
            message: message.into(),
            current: None,
        };
        if audience_of(&row) != Audience::Internal {
            return Err(OpError::invalid(
                "form",
                "That's a public form: it's answered at its link.",
            ));
        }
        if row.archived {
            return Err(closed("This form is archived, so it isn't taking answers."));
        }
        let collection = crate::collections::lock(tx, space, row.collection_id)
            .await?
            .ok_or(OpError::not_found("That collection"))?;
        if collection.archived {
            return Err(closed(
                "The form's collection is archived, so it isn't taking answers.",
            ));
        }
        if collection.acts_like_task {
            return Err(closed("This form's collection makes tasks now."));
        }
        let definitions =
            crate::collections::field_definitions(tx, space, collection.id, true).await?;
        let config = config_of(&row);
        let (title, values) = collection_forms::record_from_answer_for(
            &collection,
            &config,
            &definitions,
            &input.answers,
            Audience::Internal,
        )
        .map_err(|message| OpError::Invalid {
            field: "answers".into(),
            message: message.into(),
        })?;
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?;
        // A form's set values may fill fields only builders change.
        let may_build = crate::access::check(
            tx,
            &caller,
            crate::access::Need::permission(Perm::CollectionsBuild),
            crate::access::Target::Space(space),
        )
        .await?
        .is_ok();
        let written = crate::collections::records::write(
            tx,
            space,
            &collection,
            crate::work_item_fields::Writer {
                actor_id: actor,
                project_key: &project_key,
            },
            may_build,
            &definitions,
            "answers",
            &[crate::collections::records::RecordInput {
                record: None,
                title: Some(title),
                values: values.into_iter().collect(),
                archived: None,
                expected_version: None,
            }],
        )
        .await?
        .map_err(|problem| match problem.kind {
            crate::collections::records::ProblemKind::Full => OpError::Conflict {
                reason: ConflictReason::State {
                    code: "collection_full",
                },
                message: problem.message.into(),
                current: None,
            },
            _ => OpError::Invalid {
                field: "answers".into(),
                message: problem.message.into(),
            },
        })?;
        let record = written
            .into_iter()
            .next()
            .ok_or(OpError::Unavailable)?
            .record()
            .clone();
        crate::outbox::enqueue_collection_updated(tx, space, collection.id, &[record.id]).await?;
        call.audit_target("record", record.id, serde_json::json!({ "form": row.id }));
        Ok(FormAnswered {
            record_id: record.id,
            collection_id: collection.id,
            number: record.number,
            title: record.title,
            thanks: config.thanks,
        })
    }
}
