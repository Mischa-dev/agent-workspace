//! Record templates and recurring records (T1.30), for any collection.
//!
//! A template is a record to start from: its title, where "{date}" becomes
//! the day a record is made from it, and field values by name or id, as
//! `record_create` takes them (`record_create` with `template` makes one).
//! A recurring record makes a record from a template on a schedule, the
//! same daily, weekly or monthly schedules recurring tasks keep
//! (`work_items::templates::Schedule`), as whoever set it up or last started
//! it again. Each due time is one job, keyed by the schedule and the time,
//! and the record it makes is a `record_create` through the dispatcher with
//! a request id of that key, so a job run twice makes one record. When the
//! person it acts as can no longer add records there, it pauses and says
//! why. Task templates and recurring tasks stay as they are: they are the
//! Tasks collection's.

use std::time::Duration as StdDuration;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{Acquire, FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob, enqueue},
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
    work_items::templates::{
        Frequency, Schedule, ScheduleRequest, local_time, next_run, read_schedule, weekdays_column,
    },
};

pub(crate) const SWEEP_KIND: &str = "collections.recurring_sweep";
pub(crate) const OCCURRENCE_KIND: &str = "collections.recurring_record";
/// Templates one collection keeps, archived ones included.
const MAX_TEMPLATES_PER_COLLECTION: i64 = 100;
/// Recurring records one space keeps.
const MAX_RECURRING_PER_SPACE: i64 = 200;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<RecordTemplateList>();
    registry.add::<RecordTemplateCreate>();
    registry.add::<RecordTemplateUpdate>();
    registry.add::<RecurringRecordList>();
    registry.add::<RecurringRecordCreate>();
    registry.add::<RecurringRecordUpdate>();
    registry.add::<RecurringRecordDelete>();
}

// ---------------------------------------------------------------------------
// Templates

#[derive(Clone, Debug, FromRow)]
pub(crate) struct TemplateRow {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub name: String,
    pub title: String,
    pub field_values: Value,
    pub version: i64,
    pub archived: bool,
}

const TEMPLATE_SELECT: &str = "SELECT template.id, template.collection_id, template.name, \
        template.title, template.field_values, template.version, \
        template.archived_at IS NOT NULL AS archived \
    FROM record_templates template";

/// One of the space's templates, if its collection's module is on and the
/// caller can see that collection (a restricted one hides its templates).
pub(crate) async fn template(
    connection: &mut PgConnection,
    space: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Option<TemplateRow>, AppError> {
    let sql = format!(
        "{TEMPLATE_SELECT} WHERE template.space_id = $1 AND template.id = $2 \
           AND module_part_on('collection', template.collection_id) \
           AND EXISTS (SELECT 1 FROM collections holder \
                       WHERE holder.id = template.collection_id){}",
        if lock { " FOR UPDATE" } else { "" }
    );
    Ok(sqlx::query_as::<_, TemplateRow>(&sql)
        .bind(space)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?)
}

/// A template's title for a record made on `day`.
pub(crate) fn title_on(title: &str, day: NaiveDate) -> String {
    title.replace("{date}", &day.format("%Y-%m-%d").to_string())
}

/// A template's field values as record_create takes them.
pub(crate) fn values_of(row: &TemplateRow) -> Map<String, Value> {
    row.field_values.as_object().cloned().unwrap_or_default()
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    pub name: String,
    /// The record's title; "{date}" becomes the day it's made.
    pub title: String,
    /// Field values by name or id.
    pub fields: Map<String, Value>,
    pub version: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

fn template_view(row: &TemplateRow) -> TemplateView {
    TemplateView {
        id: row.id,
        collection_id: row.collection_id,
        name: row.name.clone(),
        title: row.title.clone(),
        fields: values_of(row),
        version: row.version,
        archived: row.archived,
    }
}

fn clean_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "name",
            "Give the template a name of 1 to 80 characters, on one line.",
        ));
    }
    Ok(name.to_owned())
}

fn clean_title(title: &str) -> Result<String, OpError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 300 || title.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "title",
            "Give records made from it a title of 1 to 300 characters, on one line.",
        ));
    }
    Ok(title.to_owned())
}

/// The collection a template may be on, locked: not archived, and not one
/// whose records are tasks (those use task templates).
async fn template_collection(
    connection: &mut PgConnection,
    space: Uuid,
    collection_id: Uuid,
) -> Result<crate::collections::Collection, OpError> {
    let collection = crate::collections::lock(connection, space, collection_id)
        .await?
        .ok_or(OpError::not_found("That collection"))?;
    if collection.archived {
        return Err(OpError::Conflict {
            reason: ConflictReason::Archived,
            message: format!("\"{}\" is archived. Restore it first.", collection.name).into(),
            current: None,
        });
    }
    if collection.is_pages() {
        return Err(OpError::invalid(
            "collection",
            crate::collections::pages::RECORDS_MESSAGE,
        ));
    }
    if collection.acts_like_task {
        return Err(OpError::Invalid {
            field: "collection".into(),
            message: format!(
                "Records in \"{}\" are tasks: use a task template (template_create).",
                collection.name
            )
            .into(),
        });
    }
    Ok(collection)
}

/// Whether a record with these values could be written: a write in a
/// savepoint, rolled back, so a template is checked as its records will be.
async fn check_values(
    call: &mut OpCall<'_>,
    space: Uuid,
    collection: &crate::collections::Collection,
    title: &str,
    values: &Map<String, Value>,
) -> Result<(), OpError> {
    let actor = call.account_id();
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    let may_build = crate::access::check(
        tx,
        &caller,
        crate::access::Need::permission(Perm::CollectionsBuild),
        crate::access::Target::Space(space),
    )
    .await?
    .is_ok();
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(space)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
    let definitions = crate::collections::field_definitions(tx, space, collection.id, true).await?;
    let mut savepoint = tx.begin().await.map_err(AppError::from)?;
    let written = super::records::write(
        &mut savepoint,
        space,
        collection,
        crate::work_item_fields::Writer {
            actor_id: actor,
            project_key: &project_key,
        },
        may_build,
        &definitions,
        "fields",
        &[super::records::RecordInput {
            record: None,
            title: Some(title_on(title, Utc::now().date_naive())),
            values: values.clone().into_iter().collect(),
            archived: None,
            expected_version: None,
        }],
    )
    .await?;
    savepoint.rollback().await.map_err(AppError::from)?;
    match written {
        Ok(_) => Ok(()),
        Err(problem) if problem.kind == super::records::ProblemKind::Full => Ok(()),
        Err(problem) => Err(OpError::Invalid {
            field: "fields".into(),
            message: problem.message.into(),
        }),
    }
}

pub struct RecordTemplateList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionRefInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    /// Archived ones too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_archived: bool,
}

impl Targeted for CollectionRefInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateList {
    pub templates: Vec<TemplateView>,
}

impl Operation for RecordTemplateList {
    const NAME: &'static str = "record_template_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List record templates";
    const DESCRIPTION: &'static str = "A collection's record templates: records to start \
        from, each with its title and field values. Make a record from one with record_create \
        (template), or on a schedule with recurring_record_create.";

    type Input = CollectionRefInput;
    type Output = TemplateList;

    async fn run(
        call: &mut OpCall<'_>,
        input: CollectionRefInput,
    ) -> Result<TemplateList, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let sql = format!(
            "{TEMPLATE_SELECT} WHERE template.space_id = $1 AND template.collection_id = $2 \
               AND ($3 OR template.archived_at IS NULL) \
               AND module_part_on('collection', template.collection_id) \
             ORDER BY template.archived_at IS NOT NULL, lower(template.name), template.id"
        );
        let rows = sqlx::query_as::<_, TemplateRow>(&sql)
            .bind(space)
            .bind(input.collection.id())
            .bind(input.include_archived)
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?;
        Ok(TemplateList {
            templates: rows.iter().map(template_view).collect(),
        })
    }
}

pub struct RecordTemplateCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateCreateInput {
    /// The collection: its id, or a link to it.
    pub collection: IdRef,
    pub name: String,
    /// The title of records made from it; "{date}" becomes the day one is
    /// made, such as "Weekly check-in {date}".
    pub title: String,
    /// Values by field name or id, as record_create takes them.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

impl Targeted for TemplateCreateInput {
    fn target(&self) -> Target {
        Target::thing("collection", self.collection.id())
    }
}

impl Operation for RecordTemplateCreate {
    const NAME: &'static str = "record_template_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Create a record template";
    const DESCRIPTION: &'static str = "Save a record to start from in a collection: a title \
        (\"{date}\" becomes the day a record is made) and field values by name. Its values are \
        checked as a record's would be. Records whose collection acts like tasks use task \
        templates instead.";

    type Input = TemplateCreateInput;
    type Output = TemplateView;

    async fn run(
        call: &mut OpCall<'_>,
        input: TemplateCreateInput,
    ) -> Result<TemplateView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let name = clean_name(&input.name)?;
        let title = clean_title(&input.title)?;
        let tx = call.tx().await?;
        let collection = template_collection(tx, space, input.collection.id()).await?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM record_templates WHERE space_id = $1 AND collection_id = $2",
        )
        .bind(space)
        .bind(collection.id)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if count >= MAX_TEMPLATES_PER_COLLECTION {
            return Err(OpError::Conflict {
                reason: ConflictReason::State {
                    code: "templates_full",
                },
                message: format!(
                    "A collection keeps at most {MAX_TEMPLATES_PER_COLLECTION} templates."
                )
                .into(),
                current: None,
            });
        }
        check_values(call, space, &collection, &title, &input.fields).await?;
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        sqlx::query(
            "INSERT INTO record_templates (id, workspace_id, space_id, collection_id, name, \
                 title, field_values, created_by_actor_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(workspace)
        .bind(space)
        .bind(collection.id)
        .bind(&name)
        .bind(&title)
        .bind(Value::Object(input.fields))
        .bind(actor)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        crate::outbox::enqueue_collection_updated(tx, space, collection.id, &[]).await?;
        call.audit_target(
            "record_template",
            id,
            json!({ "collection": collection.id }),
        );
        let tx = call.tx().await?;
        let row = template(tx, space, id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        Ok(template_view(&row))
    }
}

pub struct RecordTemplateUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateUpdateInput {
    /// The template: its id.
    pub template: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// All its field values, replacing the ones it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Map<String, Value>>,
    /// Archive it (its recurring records pause) or bring it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for TemplateUpdateInput {
    fn target(&self) -> Target {
        Target::thing("record_template", self.template.id())
    }
}

fn version_conflict(what: &str, current: i64) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::Version {
            current_version: current,
        },
        message: format!(
            "Someone changed this {what} first (now version {current}). Read it again, then retry."
        )
        .into(),
        current: None,
    }
}

impl Operation for RecordTemplateUpdate {
    const NAME: &'static str = "record_template_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Change a record template";
    const DESCRIPTION: &'static str = "Rename a record template, change its title or replace \
        its field values (checked as a record's would be), or archive it: its recurring \
        records pause until it's back.";

    type Input = TemplateUpdateInput;
    type Output = TemplateView;

    async fn run(
        call: &mut OpCall<'_>,
        input: TemplateUpdateInput,
    ) -> Result<TemplateView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let name = input.name.as_deref().map(clean_name).transpose()?;
        let title = input.title.as_deref().map(clean_title).transpose()?;
        let tx = call.tx().await?;
        let row = template(tx, space, input.template.id(), true)
            .await?
            .ok_or(OpError::not_found("That template"))?;
        if let Some(expected) = input.expected_version
            && expected != row.version
        {
            return Err(version_conflict("template", row.version));
        }
        if title.is_some() || input.fields.is_some() {
            let collection = template_collection(tx, space, row.collection_id).await?;
            let values = input.fields.clone().unwrap_or_else(|| values_of(&row));
            let title = title.clone().unwrap_or_else(|| row.title.clone());
            check_values(call, space, &collection, &title, &values).await?;
        }
        let tx = call.tx().await?;
        sqlx::query(
            "UPDATE record_templates SET name = coalesce($2, name), title = coalesce($3, title), \
                 field_values = coalesce($4, field_values), \
                 archived_at = CASE WHEN $5::boolean IS NULL THEN archived_at \
                     WHEN $5 THEN coalesce(archived_at, now()) ELSE NULL END, \
                 version = version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(name.as_deref())
        .bind(title.as_deref())
        .bind(input.fields.map(Value::Object))
        .bind(input.archived)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if input.archived == Some(true) && !row.archived {
            sqlx::query(
                "UPDATE recurring_records SET enabled = false, next_run_at = NULL, \
                     paused_reason = 'Paused because its template was archived.', \
                     version = version + 1, updated_at = now() \
                 WHERE space_id = $1 AND template_id = $2 AND enabled",
            )
            .bind(space)
            .bind(row.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
        crate::outbox::enqueue_collection_updated(tx, space, row.collection_id, &[]).await?;
        call.audit_target("record_template", row.id, json!({}));
        let tx = call.tx().await?;
        let row = template(tx, space, row.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        Ok(template_view(&row))
    }
}

// ---------------------------------------------------------------------------
// Recurring records

#[derive(Clone, Debug, FromRow)]
struct RecurringRow {
    id: Uuid,
    workspace_id: Uuid,
    space_id: Uuid,
    template_id: Uuid,
    template_name: String,
    collection_id: Uuid,
    created_by_actor_id: Uuid,
    external_agent_connection_id: Option<Uuid>,
    via_oauth_grant_id: Option<Uuid>,
    owner_name: Option<String>,
    frequency: String,
    repeat_every: i32,
    weekdays: Vec<i16>,
    month_day: Option<i16>,
    time_of_day: NaiveTime,
    timezone: String,
    starts_on: NaiveDate,
    enabled: bool,
    paused_reason: Option<String>,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    last_record_id: Option<Uuid>,
    version: i64,
}

impl RecurringRow {
    fn schedule(&self) -> Schedule {
        Schedule {
            frequency: Frequency::parse(&self.frequency).unwrap_or(Frequency::Weekly),
            every: u32::try_from(self.repeat_every).unwrap_or(1),
            weekdays: self
                .weekdays
                .iter()
                .filter_map(|day| u32::try_from(*day).ok())
                .collect(),
            month_day: self.month_day.and_then(|day| u32::try_from(day).ok()),
            time: self.time_of_day,
            starts_on: self.starts_on,
        }
    }
}

const RECURRING_SELECT: &str = "SELECT recurring.id, recurring.workspace_id, recurring.space_id, \
        recurring.template_id, template.name AS template_name, template.collection_id, \
        recurring.created_by_actor_id, recurring.external_agent_connection_id, \
        recurring.via_oauth_grant_id, owner.display_name AS owner_name, \
        recurring.frequency, recurring.repeat_every, recurring.weekdays, recurring.month_day, \
        recurring.time_of_day, recurring.timezone, recurring.starts_on, recurring.enabled, \
        recurring.paused_reason, recurring.next_run_at, recurring.last_run_at, \
        recurring.last_record_id, recurring.version \
    FROM recurring_records recurring \
    JOIN record_templates template \
      ON template.space_id = recurring.space_id AND template.id = recurring.template_id \
    LEFT JOIN project_memberships owner \
      ON owner.project_id = recurring.space_id AND owner.actor_id = recurring.created_by_actor_id";

async fn recurring(
    connection: &mut PgConnection,
    space: Option<Uuid>,
    id: Uuid,
    lock: bool,
) -> Result<Option<RecurringRow>, AppError> {
    let sql = format!(
        "{RECURRING_SELECT} WHERE recurring.id = $2 \
           AND ($1::uuid IS NULL OR recurring.space_id = $1){}",
        if lock { " FOR UPDATE OF recurring" } else { "" }
    );
    Ok(sqlx::query_as::<_, RecurringRow>(&sql)
        .bind(space)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?)
}

/// When a recurring record runs.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleInput {
    /// daily, weekly or monthly.
    pub frequency: String,
    /// Every this many days, weeks or months; 1 when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    /// Weekly: ISO weekdays, 1 (Monday) to 7 (Sunday).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<u32>,
    /// Monthly: the day of the month; a shorter month uses its last day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_day: Option<u32>,
    /// HH:MM, local; 09:00 when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<String>,
    /// An IANA zone such as Europe/Amsterdam; UTC when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The first day it may run; today when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on: Option<NaiveDate>,
}

async fn schedule_of(
    connection: &mut PgConnection,
    input: &ScheduleInput,
) -> Result<(Schedule, String), OpError> {
    let request = ScheduleRequest {
        frequency: input.frequency.trim().to_ascii_lowercase(),
        every: input.every.unwrap_or(1),
        weekdays: input.weekdays.clone(),
        month_day: input.month_day,
        time_of_day: input
            .time_of_day
            .clone()
            .unwrap_or_else(|| "09:00".to_owned()),
        timezone: input.timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
        starts_on: input.starts_on,
    };
    read_schedule(connection, &request)
        .await?
        .map_err(|(field, message)| OpError::invalid(field, message))
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecurringView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub template_id: Uuid,
    pub template_name: String,
    #[schemars(with = "String")]
    pub collection_id: Uuid,
    /// Who its records are made as.
    #[schemars(with = "String")]
    pub owner_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    pub frequency: String,
    pub every: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_day: Option<i16>,
    pub time_of_day: String,
    pub timezone: String,
    pub starts_on: String,
    pub enabled: bool,
    /// Why it stopped, when it paused itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub last_record_id: Option<Uuid>,
    pub version: i64,
}

fn recurring_view(row: &RecurringRow) -> RecurringView {
    RecurringView {
        id: row.id,
        template_id: row.template_id,
        template_name: row.template_name.clone(),
        collection_id: row.collection_id,
        owner_id: row.created_by_actor_id,
        owner_name: row.owner_name.clone(),
        frequency: row.frequency.clone(),
        every: row.repeat_every,
        weekdays: row.weekdays.clone(),
        month_day: row.month_day,
        time_of_day: row.time_of_day.format("%H:%M").to_string(),
        timezone: row.timezone.clone(),
        starts_on: row.starts_on.to_string(),
        enabled: row.enabled,
        paused_reason: row.paused_reason.clone(),
        next_run_at: row.next_run_at.map(|at| at.to_rfc3339()),
        last_run_at: row.last_run_at.map(|at| at.to_rfc3339()),
        last_record_id: row.last_record_id,
        version: row.version,
    }
}

pub struct RecurringRecordList;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecurringList {
    pub recurring: Vec<RecurringView>,
}

impl Operation for RecurringRecordList {
    const NAME: &'static str = "recurring_record_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List recurring records";
    const DESCRIPTION: &'static str = "A collection's recurring records: each makes a record \
        from a template on a schedule, as the person who set it up. A paused one says why.";

    type Input = CollectionRefInput;
    type Output = RecurringList;

    async fn run(
        call: &mut OpCall<'_>,
        input: CollectionRefInput,
    ) -> Result<RecurringList, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let sql = format!(
            "{RECURRING_SELECT} WHERE recurring.space_id = $1 AND template.collection_id = $2 \
               AND module_part_on('collection', template.collection_id) \
             ORDER BY recurring.created_at, recurring.id"
        );
        let rows = sqlx::query_as::<_, RecurringRow>(&sql)
            .bind(space)
            .bind(input.collection.id())
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?;
        Ok(RecurringList {
            recurring: rows.iter().map(recurring_view).collect(),
        })
    }
}

pub struct RecurringRecordCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringCreateInput {
    /// The record template it makes records from.
    pub template: IdRef,
    #[serde(flatten)]
    pub schedule: ScheduleInput,
}

impl Targeted for RecurringCreateInput {
    fn target(&self) -> Target {
        Target::thing("record_template", self.template.id())
    }
}

impl Operation for RecurringRecordCreate {
    const NAME: &'static str = "recurring_record_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Make records on a schedule";
    const DESCRIPTION: &'static str = "Make a record from a template on a schedule: daily, \
        weekly on some weekdays (1 is Monday) or monthly on a day, at a local time in a time \
        zone, every so many days, weeks or months. Records are made as you; it pauses, saying \
        why, if you can no longer add them.";

    type Input = RecurringCreateInput;
    type Output = RecurringView;

    async fn run(
        call: &mut OpCall<'_>,
        input: RecurringCreateInput,
    ) -> Result<RecurringView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        // The agent connection or connected app it acts through.
        let through = (call.ctx.caller.connection_id, call.ctx.caller.via_grant_id);
        let tx = call.tx().await?;
        let row = template(tx, space, input.template.id(), false)
            .await?
            .ok_or(OpError::not_found("That template"))?;
        if row.archived {
            return Err(OpError::Conflict {
                reason: ConflictReason::Archived,
                message: "That template is archived. Bring it back first.".into(),
                current: None,
            });
        }
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM recurring_records WHERE space_id = $1",
        )
        .bind(space)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if count >= MAX_RECURRING_PER_SPACE {
            return Err(OpError::Conflict {
                reason: ConflictReason::State {
                    code: "recurring_full",
                },
                message: format!(
                    "A space keeps at most {MAX_RECURRING_PER_SPACE} recurring records."
                )
                .into(),
                current: None,
            });
        }
        let (schedule, timezone) = schedule_of(tx, &input.schedule).await?;
        let next = next_run(tx, &schedule, &timezone, Utc::now()).await?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO recurring_records (id, workspace_id, space_id, template_id, \
                 created_by_actor_id, external_agent_connection_id, via_oauth_grant_id, \
                 frequency, repeat_every, weekdays, month_day, time_of_day, \
                 timezone, starts_on, next_run_at) \
             VALUES ($1, $2, $3, $4, $5, $14, $15, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(id)
        .bind(workspace)
        .bind(space)
        .bind(row.id)
        .bind(actor)
        .bind(schedule.frequency.as_str())
        .bind(i32::try_from(schedule.every).unwrap_or(1))
        .bind(weekdays_column(&schedule))
        .bind(schedule.month_day.and_then(|day| i16::try_from(day).ok()))
        .bind(schedule.time)
        .bind(&timezone)
        .bind(schedule.starts_on)
        .bind(next)
        .bind(through.0)
        .bind(through.1)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        crate::outbox::enqueue_collection_updated(tx, space, row.collection_id, &[]).await?;
        call.audit_target("recurring_record", id, json!({ "template": row.id }));
        let tx = call.tx().await?;
        let made = recurring(tx, Some(space), id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        Ok(recurring_view(&made))
    }
}

pub struct RecurringRecordUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringUpdateInput {
    /// The recurring record: its id.
    pub recurring: IdRef,
    /// A new schedule, replacing the one it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ScheduleInput>,
    /// Pause it, or start it again: started again, it makes records as you.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for RecurringUpdateInput {
    fn target(&self) -> Target {
        Target::thing("recurring_record", self.recurring.id())
    }
}

impl Operation for RecurringRecordUpdate {
    const NAME: &'static str = "recurring_record_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Change a recurring record";
    const DESCRIPTION: &'static str = "Change when a recurring record runs, pause it, or start \
        it again (it then makes records as you).";

    type Input = RecurringUpdateInput;
    type Output = RecurringView;

    async fn run(
        call: &mut OpCall<'_>,
        input: RecurringUpdateInput,
    ) -> Result<RecurringView, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let actor = call.account_id();
        // The agent connection or connected app it acts through.
        let through = (call.ctx.caller.connection_id, call.ctx.caller.via_grant_id);
        let tx = call.tx().await?;
        let row = recurring(tx, Some(space), input.recurring.id(), true)
            .await?
            .ok_or(OpError::not_found("That recurring record"))?;
        if let Some(expected) = input.expected_version
            && expected != row.version
        {
            return Err(version_conflict("recurring record", row.version));
        }
        let (schedule, timezone) = match &input.schedule {
            Some(schedule) => schedule_of(tx, schedule).await?,
            None => (row.schedule(), row.timezone.clone()),
        };
        let enabled = input.enabled.unwrap_or(row.enabled);
        if enabled && input.enabled == Some(true) && !row.enabled {
            let template = template(tx, space, row.template_id, false)
                .await?
                .ok_or(OpError::not_found("That template"))?;
            if template.archived {
                return Err(OpError::Conflict {
                    reason: ConflictReason::Archived,
                    message: "Its template is archived. Bring it back first.".into(),
                    current: None,
                });
            }
        }
        let next = if enabled {
            next_run(tx, &schedule, &timezone, Utc::now()).await?
        } else {
            None
        };
        // Started again, it acts as whoever started it, through what they
        // called with.
        let (owner, through) = if input.enabled == Some(true) && !row.enabled {
            (actor, through)
        } else {
            (
                row.created_by_actor_id,
                (row.external_agent_connection_id, row.via_oauth_grant_id),
            )
        };
        sqlx::query(
            "UPDATE recurring_records SET frequency = $2, repeat_every = $3, weekdays = $4, \
                 month_day = $5, time_of_day = $6, timezone = $7, starts_on = $8, enabled = $9, \
                 next_run_at = $10, created_by_actor_id = $11, \
                 external_agent_connection_id = $12, via_oauth_grant_id = $13, \
                 paused_reason = CASE WHEN $9 THEN NULL ELSE paused_reason END, \
                 version = version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(schedule.frequency.as_str())
        .bind(i32::try_from(schedule.every).unwrap_or(1))
        .bind(weekdays_column(&schedule))
        .bind(schedule.month_day.and_then(|day| i16::try_from(day).ok()))
        .bind(schedule.time)
        .bind(&timezone)
        .bind(schedule.starts_on)
        .bind(enabled)
        .bind(next)
        .bind(owner)
        .bind(through.0)
        .bind(through.1)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        crate::outbox::enqueue_collection_updated(tx, space, row.collection_id, &[]).await?;
        call.audit_target("recurring_record", row.id, json!({ "enabled": enabled }));
        let tx = call.tx().await?;
        let changed = recurring(tx, Some(space), row.id, false)
            .await?
            .ok_or(OpError::Unavailable)?;
        Ok(recurring_view(&changed))
    }
}

pub struct RecurringRecordDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringDeleteInput {
    /// The recurring record: its id.
    pub recurring: IdRef,
}

impl Targeted for RecurringDeleteInput {
    fn target(&self) -> Target {
        Target::thing("recurring_record", self.recurring.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecurringDeleted {
    #[schemars(with = "String")]
    pub id: Uuid,
}

impl Operation for RecurringRecordDelete {
    const NAME: &'static str = "recurring_record_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "collections";
    const PERMISSION: Option<Perm> = Some(Perm::CollectionsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["collection.updated"];
    const TITLE: &'static str = "Stop a recurring record";
    const DESCRIPTION: &'static str = "Remove a recurring record's schedule. The records it \
        made stay.";

    type Input = RecurringDeleteInput;
    type Output = RecurringDeleted;

    async fn run(
        call: &mut OpCall<'_>,
        input: RecurringDeleteInput,
    ) -> Result<RecurringDeleted, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let row = recurring(tx, Some(space), input.recurring.id(), true)
            .await?
            .ok_or(OpError::not_found("That recurring record"))?;
        sqlx::query("DELETE FROM recurring_records WHERE id = $1")
            .bind(row.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        crate::outbox::enqueue_collection_updated(tx, space, row.collection_id, &[]).await?;
        call.audit_target("recurring_record", row.id, json!({}));
        Ok(RecurringDeleted { id: row.id })
    }
}

// ---------------------------------------------------------------------------
// The jobs

#[derive(Debug, Deserialize, Serialize)]
struct OccurrencePayload {
    recurring_record_id: Uuid,
    occurrence: DateTime<Utc>,
}

pub(crate) fn register_jobs(registry: &mut JobRegistry) {
    registry.register(SWEEP_KIND, |context: JobContext| async move {
        queue_due(&context.state.pool).await?;
        Ok(())
    });
    registry.every(SWEEP_KIND, StdDuration::from_secs(60));
    registry.register(OCCURRENCE_KIND, |context: JobContext| async move {
        make_occurrence(&context).await
    });
}

/// Queues one job for each recurring record that is due; the unique key
/// holds one job per occurrence however often this runs.
pub(crate) async fn queue_due(pool: &PgPool) -> Result<usize, AppError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid, Uuid, DateTime<Utc>)>(
        "SELECT id, space_id, workspace_id, next_run_at FROM recurring_records \
         WHERE enabled AND next_run_at <= now() ORDER BY next_run_at LIMIT 500",
    )
    .fetch_all(crate::db::scoped(pool))
    .await?;
    let count = due.len();
    for (id, space, workspace, occurrence) in due {
        enqueue(
            pool,
            NewJob::new(OCCURRENCE_KIND)
                .project(workspace, space)
                .payload(OccurrencePayload {
                    recurring_record_id: id,
                    occurrence,
                })
                .unique(occurrence_key(id, occurrence)),
        )
        .await?;
    }
    Ok(count)
}

/// One occurrence's job key and request id (a request id takes no colons).
fn occurrence_key(id: Uuid, occurrence: DateTime<Utc>) -> String {
    format!("recurring-record.{id}.{}", occurrence.timestamp_micros())
}

async fn pause(pool: &PgPool, id: Uuid, reason: &str) -> Result<(), AppError> {
    let reason: String = reason.chars().take(300).collect();
    sqlx::query(
        "UPDATE recurring_records SET enabled = false, next_run_at = NULL, paused_reason = $2, \
             version = version + 1, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(())
}

/// Makes one occurrence's record as the recurring record's owner, through
/// the dispatcher with the occurrence as its request id, then moves the
/// schedule on.
async fn make_occurrence(context: &JobContext) -> Result<(), JobError> {
    let payload: OccurrencePayload = context.job.payload_as()?;
    let state = &context.state;
    let pool = &state.pool;
    let row = {
        let mut connection = crate::db::scoped_tx(pool).await?;
        let row = recurring(&mut connection, None, payload.recurring_record_id, false).await?;
        connection.commit().await?;
        row
    };
    let Some(row) = row else {
        return Ok(());
    };
    if context.job.project_id != Some(row.space_id) {
        return Err(JobError::permanent("the job names another space"));
    }
    // Moved, paused or already done by an earlier attempt.
    if !row.enabled || row.next_run_at != Some(payload.occurrence) {
        return Ok(());
    }
    let template = {
        let mut connection = crate::db::scoped_tx(pool).await?;
        let template = template(&mut connection, row.space_id, row.template_id, false).await?;
        connection.commit().await?;
        template
    };
    let Some(template) = template.filter(|template| !template.archived) else {
        pause(pool, row.id, "Paused because its template was archived.").await?;
        return Ok(());
    };
    let (kind_key, app_gone, limits, local) = {
        let mut connection = crate::db::scoped_tx(pool).await?;
        let (kind, app_gone) = sqlx::query_as::<_, (String, bool)>(
            "SELECT actor.kind, $2::uuid IS NOT NULL AND NOT EXISTS ( \
                 SELECT 1 FROM mcp_oauth_grants grant_row \
                 WHERE grant_row.id = $2 AND grant_row.revoked_at IS NULL) \
             FROM actors actor WHERE actor.id = $1",
        )
        .bind(row.created_by_actor_id)
        .bind(row.via_oauth_grant_id)
        .fetch_one(&mut *connection)
        .await?;
        // Through a connected app, the app's limit holds as it did when set up.
        let limits = match row.via_oauth_grant_id {
            Some(_) => Some(
                crate::roles::app_access(&mut connection, row.created_by_actor_id)
                    .await?
                    .limits,
            ),
            None => None,
        };
        let local = local_time(&mut connection, payload.occurrence, &row.timezone).await?;
        connection.commit().await?;
        (kind, app_gone, limits, local)
    };
    let who = row
        .owner_name
        .clone()
        .unwrap_or_else(|| "Its owner".to_owned());
    // The account's kind only says how its caller is built.
    let Ok(kind) = crate::accounts::actor_kind(&kind_key) else {
        pause(
            pool,
            row.id,
            &format!("Paused because {who} can't add records."),
        )
        .await?;
        return Ok(());
    };
    if app_gone {
        pause(
            pool,
            row.id,
            "Paused because the app it was set up through was disconnected. Start it again to have it act as you.",
        )
        .await?;
        return Ok(());
    }
    let mut caller = crate::access::Caller::person(row.created_by_actor_id, kind);
    caller.connection_id = row.external_agent_connection_id;
    caller.via_grant_id = row.via_oauth_grant_id;
    caller.limits = limits;
    let request = crate::ops::DispatchRequest {
        caller,
        edge: crate::ops::Edge::Job {
            job_id: context.job.id,
            source: "recurring_record",
        },
        trace_id: None,
        workspace: Some(row.workspace_id),
        on_screen: None,
        request_id: Some(occurrence_key(row.id, payload.occurrence)),
        unkeyed: crate::ops::Unkeyed::Refuse("clientRequestId"),
        preview: false,
        defer_long: false,
    };
    let answer = crate::ops::dispatch(
        state,
        request,
        "record_create",
        json!({
            "collection": template.collection_id.to_string(),
            "template": template.id.to_string(),
            "title": title_on(&template.title, local.date()),
        }),
    )
    .await;
    if !answer.is_ok() {
        if answer.outcome() == "temporarily_unavailable" || answer.outcome() == "rate_limited" {
            return Err(JobError::retry("record_create was unavailable"));
        }
        let reason = match answer.outcome() {
            "permission_denied" | "not_found" => format!(
                "Paused because {who} can no longer add records here. Start it again to have it act as you."
            ),
            _ => format!(
                "Paused: {}",
                answer.body["message"]
                    .as_str()
                    .unwrap_or("the record couldn't be made.")
            ),
        };
        pause(pool, row.id, &reason).await?;
        return Ok(());
    }
    let record = answer.body["value"]["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok());
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let next = next_run(
        &mut transaction,
        &row.schedule(),
        &row.timezone,
        payload.occurrence,
    )
    .await?;
    sqlx::query(
        "UPDATE recurring_records SET next_run_at = $3, last_run_at = $2, \
             last_record_id = coalesce($4, last_record_id), updated_at = now() \
         WHERE id = $1 AND next_run_at = $2",
    )
    .bind(row.id)
    .bind(payload.occurrence)
    .bind(next)
    .bind(record)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}
