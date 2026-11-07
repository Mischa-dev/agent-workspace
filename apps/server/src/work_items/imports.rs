//! Imports and exports (docs/plans/roadmap.md, W11, T9).
//!
//! An import reads Markdown, CSV (Linear and Jira exports map without
//! changes) or a connected repository's GitHub issues into rows, shows a
//! preview of what each row will become, and lets the person change the
//! column mapping before anything is created. Committing queues a job that
//! creates the tasks in chunks, acting as the person who started it and
//! checking each chunk that they still may (principle 9). Every created
//! task is linked to where it came from, so importing the same issues again
//! skips the ones already here, and a job that runs twice creates nothing
//! twice.
//!
//! Export writes a project's tasks as JSON or as CSV that imports back.

use std::collections::{BTreeMap, HashMap};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    authorize_work_item, capabilities,
    import_parse::{
        FIELDS, ImportRow, MAX_ROWS, auto_mapping, csv_rows, date_of, github_rows, markdown_rows,
        parse_csv, priority_of,
    },
    planning::{Started, begin, invalid, refusal, start_write},
    project_membership,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    fields::{self, FieldDefinition, FieldType, FieldView},
    idempotency::resource_bound_scope,
    jobs::{JobContext, JobError, JobRegistry, NewJob, enqueue},
    policy::{
        Action, ActorKind, AgentLimits, Decision, DenyReason, MembershipScope, Permission,
        within_agent_limits,
    },
    work_item_commands::{
        AllocatedNumberMetadata, AuditAttribution, CommandActor, CreateWorkItemCommand,
        CreateWorkItemOutcome, NewTaskFields, authorize_mutation, create_work_item,
        lock_membership, normalize_description, normalize_title, record_audit_for, valid_estimate,
    },
    work_item_fields::{self, FieldValueChanges, Writer},
    work_item_labels::{self, default_color},
    work_item_planning,
};

pub(crate) const FETCH_KIND: &str = "tasks.import_fetch";
pub(crate) const COMMIT_KIND: &str = "tasks.import_commit";
/// The largest import body: 4 MB of text and a little JSON around it.
pub(crate) const MAX_IMPORT_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const PREVIEW_ROWS: usize = 100;
const CHUNK: usize = 100;
const MAX_EXPORT_TASKS: i64 = 20_000;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportOptions {
    /// Markdown headings become milestones.
    #[serde(default)]
    pub headings_as_milestones: bool,
    /// Create labels the project doesn't have yet.
    #[serde(default = "yes")]
    pub create_labels: bool,
    /// Create milestones the project doesn't have yet.
    #[serde(default = "yes")]
    pub create_milestones: bool,
    /// GitHub: "open" (the default) or "all".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_state: Option<String>,
    /// Rows, by place, left out of the import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_rows: Vec<usize>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, FromRow)]
struct ImportRecord {
    id: Uuid,
    project_id: Uuid,
    created_by_actor_id: Uuid,
    created_by_name: Option<String>,
    source: String,
    source_name: String,
    repository_id: Option<Uuid>,
    repository_name: Option<String>,
    mapping: Value,
    options: Value,
    rows: Value,
    status: String,
    row_count: i32,
    processed_count: i32,
    created_count: i32,
    skipped_count: i32,
    error: Option<String>,
    version: i64,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    /// Committed (or, for a GitHub fetch, started) by a person's app or
    /// script: the jobs act within that app's agent limit as it is now.
    made_through_app: bool,
}

const IMPORT_SELECT: &str = r#"
    SELECT import.id, import.project_id, import.created_by_actor_id,
           creator.display_name AS created_by_name, import.source, import.source_name,
           import.repository_id,
           CASE WHEN repository.id IS NULL THEN NULL
                ELSE repository.owner || '/' || repository.name END AS repository_name,
           import.mapping, import.options, import.rows, import.status, import.row_count,
           import.processed_count, import.created_count, import.skipped_count, import.error,
           import.version, import.created_at, import.finished_at, import.made_through_app
    FROM task_imports import
    LEFT JOIN project_memberships creator
      ON creator.project_id = import.project_id AND creator.actor_id = import.created_by_actor_id
    LEFT JOIN repositories repository
      ON repository.project_id = import.project_id AND repository.id = import.repository_id
"#;

impl ImportRecord {
    fn options(&self) -> ImportOptions {
        serde_json::from_value(self.options.clone()).unwrap_or_default()
    }

    fn mapping(&self) -> BTreeMap<String, String> {
        serde_json::from_value(self.mapping.clone()).unwrap_or_default()
    }

    fn rows(&self) -> Vec<ImportRow> {
        serde_json::from_value(self.rows.clone()).unwrap_or_default()
    }

    /// Where created tasks are linked from, for skipping repeats.
    fn source_key(&self) -> String {
        match (self.source.as_str(), self.repository_id) {
            ("github", Some(repository)) => format!("github:{repository}"),
            (source, _) => source.to_owned(),
        }
    }
}

async fn import_row(
    connection: &mut PgConnection,
    project_id: Uuid,
    import_id: Uuid,
    lock: bool,
) -> Result<Option<ImportRecord>, AppError> {
    Ok(sqlx::query_as::<_, ImportRecord>(&format!(
        "{IMPORT_SELECT} WHERE import.project_id = $1 AND import.id = $2{}",
        if lock { " FOR UPDATE OF import" } else { "" }
    ))
    .bind(project_id)
    .bind(import_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Whether `actor_id` sees code in `project_id`: their role there, and an
/// app's agent limit in this call. A GitHub import is a repository's
/// issues, so starting, changing, committing or reading one in full follows
/// "see code" as the repository does.
async fn sees_code(
    connection: &mut PgConnection,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<bool, AppError> {
    let allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships
            WHERE actor_id = $1 AND project_id = $2 AND enabled
              AND custom_role_allows(role_id, $3)
        )
        "#,
    )
    .bind(actor_id)
    .bind(project_id)
    .bind(Permission::CodeView.info().key)
    .fetch_one(&mut *connection)
    .await?;
    Ok(allowed && !crate::policy::agent_limit_hides(actor_id, project_id, Permission::CodeView))
}

// ---------------------------------------------------------------------------
// Matching rows to the project

#[derive(Debug, Default)]
struct ProjectContext {
    project_key: String,
    estimate_unit: String,
    /// id, name, category, is default
    statuses: Vec<(Uuid, String, String, bool)>,
    /// id, name, is default
    types: Vec<(Uuid, String, bool)>,
    labels: Vec<(Uuid, String)>,
    milestones: Vec<(Uuid, String)>,
    /// actor id, display name, handle
    members: Vec<(Uuid, String, Option<String>)>,
    /// external ID to task key, for this import's source
    linked: HashMap<String, i64>,
    /// The project's active custom fields, which columns match by name.
    fields: Vec<FieldDefinition>,
}

async fn load_context(
    connection: &mut PgConnection,
    project_id: Uuid,
    source_key: &str,
) -> Result<ProjectContext, AppError> {
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    let (_, estimate_unit) =
        crate::work_item_workflow::task_settings(connection, project_id).await?;
    let statuses = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "SELECT id, name, semantic_category, is_default FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let types = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, is_default FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let labels = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM project_labels WHERE project_id = $1 AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let milestones = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let members = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        r#"
        SELECT membership.actor_id, membership.display_name, membership.handle
        FROM project_memberships membership
        WHERE membership.project_id = $1 AND membership.enabled
          AND membership.role <> 'viewer'
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let linked = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT link.external_id, item.number
        FROM task_import_links link
        JOIN work_items item ON item.project_id = link.project_id AND item.id = link.work_item_id
        WHERE link.project_id = $1 AND link.source_key = $2
        "#,
    )
    .bind(project_id)
    .bind(source_key)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect();
    let fields = work_item_fields::definitions(&mut *connection, project_id, false).await?;
    Ok(ProjectContext {
        project_key,
        estimate_unit,
        statuses,
        types,
        labels,
        milestones,
        members,
        linked,
        fields,
    })
}

/// What one row will become.
#[derive(Clone, Debug, Default)]
struct Resolved {
    skip: Option<String>,
    existing_key: Option<String>,
    status_id: Option<Uuid>,
    status_name: Option<String>,
    type_id: Option<Uuid>,
    type_name: Option<String>,
    priority: &'static str,
    label_ids: Vec<Uuid>,
    new_labels: Vec<(String, String)>,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    milestone_id: Option<Uuid>,
    new_milestone: Option<String>,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    estimate: Option<f64>,
    notes: Vec<String>,
    /// Custom field values as written, by field name, that passed the type
    /// check; people and links are looked up when the task is made.
    field_inputs: Vec<(String, Value)>,
}

fn resolve(
    context: &ProjectContext,
    options: &ImportOptions,
    index: usize,
    row: &ImportRow,
) -> Resolved {
    let mut resolved = Resolved {
        priority: "no_priority",
        ..Resolved::default()
    };
    if options.excluded_rows.contains(&index) {
        resolved.skip = Some("Left out".to_owned());
    }
    if normalize_title(&row.title).is_err() {
        resolved.skip = Some("No title".to_owned());
    }
    if let Some(number) = row
        .external_id
        .as_ref()
        .and_then(|id| context.linked.get(id))
    {
        let key = format!("{}-{number}", context.project_key);
        resolved.skip = Some(format!("Already imported as {key}"));
        resolved.existing_key = Some(key);
    }

    let status = row.status.as_deref().and_then(|wanted| {
        context
            .statuses
            .iter()
            .find(|status| status.1.eq_ignore_ascii_case(wanted.trim()))
    });
    let status = status.or_else(|| {
        let category = if row.canceled {
            Some("canceled")
        } else if row.done {
            Some("completed")
        } else {
            None
        }?;
        context
            .statuses
            .iter()
            .find(|status| status.2 == category)
            .or_else(|| {
                context
                    .statuses
                    .iter()
                    .find(|status| status.2 == "completed")
            })
    });
    if let Some(status) = status {
        resolved.status_id = Some(status.0);
        resolved.status_name = Some(status.1.clone());
    } else if let Some(wanted) = &row.status {
        resolved
            .notes
            .push(format!("No status “{wanted}”; it starts in the default"));
    }
    if resolved.status_name.is_none() {
        resolved.status_name = context
            .statuses
            .iter()
            .find(|status| status.3)
            .map(|status| status.1.clone());
    }

    if let Some(wanted) = &row.type_name {
        match context
            .types
            .iter()
            .find(|kind| kind.1.eq_ignore_ascii_case(wanted.trim()))
        {
            Some(kind) => {
                resolved.type_id = Some(kind.0);
                resolved.type_name = Some(kind.1.clone());
            }
            None => resolved
                .notes
                .push(format!("No type “{wanted}”; it gets the default")),
        }
    }

    if let Some(wanted) = &row.priority {
        match priority_of(wanted) {
            Some(priority) => resolved.priority = priority,
            None => resolved
                .notes
                .push(format!("Priority “{wanted}” isn't one this app knows")),
        }
    }

    for label in &row.labels {
        let Some(name) = work_item_labels::normalize_name(&label.name) else {
            continue;
        };
        if let Some((id, _)) = context
            .labels
            .iter()
            .find(|(_, existing)| existing.eq_ignore_ascii_case(&name))
        {
            if !resolved.label_ids.contains(id) {
                resolved.label_ids.push(*id);
            }
        } else if options.create_labels {
            if !resolved
                .new_labels
                .iter()
                .any(|(kept, _)| kept.eq_ignore_ascii_case(&name))
            {
                let color = label
                    .color
                    .clone()
                    .unwrap_or_else(|| default_color(&name).to_owned());
                resolved.new_labels.push((name, color));
            }
        } else {
            resolved
                .notes
                .push(format!("Label “{name}” isn't in the project"));
        }
    }
    let total_labels = resolved.label_ids.len() + resolved.new_labels.len();
    if total_labels > work_item_labels::MAX_LABELS_PER_TASK {
        resolved.notes.push(format!(
            "Only the first {} labels are kept",
            work_item_labels::MAX_LABELS_PER_TASK
        ));
    }

    if let Some(wanted) = &row.assignee {
        let wanted = wanted.trim().trim_start_matches('@');
        match context.members.iter().find(|(_, name, handle)| {
            name.eq_ignore_ascii_case(wanted)
                || handle
                    .as_deref()
                    .is_some_and(|handle| handle.eq_ignore_ascii_case(wanted))
        }) {
            Some((id, name, _)) => {
                resolved.assignee_actor_id = Some(*id);
                resolved.assignee_name = Some(name.clone());
            }
            None => resolved
                .notes
                .push(format!("No member “{wanted}”; it stays unassigned")),
        }
    }

    if let Some(wanted) = &row.milestone {
        if let Some((id, _)) = context
            .milestones
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(wanted.trim()))
        {
            resolved.milestone_id = Some(*id);
        } else if options.create_milestones {
            resolved.new_milestone = work_item_planning::normalize_milestone_name(wanted);
        } else {
            resolved
                .notes
                .push(format!("Milestone “{wanted}” isn't in the project"));
        }
    }

    for (value, slot, what) in [
        (&row.start_date, &mut resolved.start_date, "start date"),
        (&row.due_date, &mut resolved.due_date, "due date"),
    ] {
        if let Some(value) = value {
            *slot = date_of(value);
            if slot.is_none() {
                resolved
                    .notes
                    .push(format!("Couldn't read the {what} “{value}”"));
            }
        }
    }
    if let (Some(start), Some(due)) = (resolved.start_date, resolved.due_date)
        && start > due
    {
        resolved.start_date = None;
        resolved
            .notes
            .push("The start date was after the due date, so it's left out".to_owned());
    }
    if let Some(value) = &row.estimate {
        match value.trim().parse::<f64>() {
            Ok(estimate) if valid_estimate(estimate) => resolved.estimate = Some(estimate),
            _ => resolved
                .notes
                .push(format!("Couldn't read the estimate “{value}”")),
        }
    }
    // Columns named after a custom field fill it; other columns are ignored.
    for (header, raw) in &row.fields {
        let Some(field) = column_field(&context.fields, header) else {
            continue;
        };
        let input = if field.field_type == FieldType::MultipleChoice {
            Value::Array(
                raw.split([',', ';', '|'])
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| Value::String(part.to_owned()))
                    .collect(),
            )
        } else {
            Value::String(raw.clone())
        };
        match fields::normalize_value(field, &input) {
            Ok(fields::Normalized::Clear) => {}
            Ok(_) => resolved.field_inputs.push((field.name.clone(), input)),
            Err(message) => resolved
                .notes
                .push(format!("{message} “{raw}” is left out")),
        }
    }
    resolved
}

/// An imported row's custom field values. A person or link the project
/// can't use is left out rather than losing the row.
async fn import_field_values(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ProjectContext,
    actor: &CommandActor,
    inputs: &[(String, Value)],
) -> Result<Option<FieldValueChanges>, AppError> {
    if inputs.is_empty() {
        return Ok(None);
    }
    let writer = Writer {
        actor_id: actor.actor_id,
        project_key: &context.project_key,
    };
    if let Ok(values) = work_item_fields::resolve(
        transaction,
        actor.project_id,
        writer,
        &context.fields,
        "fields",
        inputs,
    )
    .await?
    {
        return Ok(Some(values));
    }
    let mut kept = FieldValueChanges::default();
    for input in inputs {
        if let Ok(values) = work_item_fields::resolve(
            transaction,
            actor.project_id,
            writer,
            &context.fields,
            "fields",
            std::slice::from_ref(input),
        )
        .await?
        {
            kept.entries.extend(values.entries);
        }
    }
    Ok((!kept.is_empty()).then_some(kept))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRow {
    index: usize,
    external_id: Option<String>,
    title: String,
    status: Option<String>,
    #[serde(rename = "type")]
    type_name: Option<String>,
    priority: &'static str,
    labels: Vec<String>,
    assignee: Option<String>,
    milestone: Option<String>,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    estimate: Option<f64>,
    parent: Option<usize>,
    /// Why it won't be created, when it won't.
    skip: Option<String>,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    total: usize,
    will_create: usize,
    already_imported: usize,
    without_title: usize,
    left_out: usize,
    new_labels: Vec<String>,
    new_milestones: Vec<String>,
    /// Source values with no match, and how many rows have each.
    unmatched: BTreeMap<String, BTreeMap<String, usize>>,
    rows: Vec<PreviewRow>,
    rows_shown: usize,
}

fn preview(context: &ProjectContext, options: &ImportOptions, rows: &[ImportRow]) -> Preview {
    let mut preview = Preview {
        total: rows.len(),
        ..Preview::default()
    };
    let mut new_labels = BTreeMap::<String, String>::new();
    let mut new_milestones = BTreeMap::<String, String>::new();
    for (index, row) in rows.iter().enumerate() {
        let resolved = resolve(context, options, index, row);
        match resolved.skip.as_deref() {
            None => preview.will_create += 1,
            Some("No title") => preview.without_title += 1,
            Some("Left out") => preview.left_out += 1,
            Some(_) => preview.already_imported += 1,
        }
        if resolved.skip.is_none() {
            for (name, _) in &resolved.new_labels {
                new_labels.insert(name.to_lowercase(), name.clone());
            }
            if let Some(name) = &resolved.new_milestone {
                new_milestones.insert(name.to_lowercase(), name.clone());
            }
            let mut unmatched = |kind: &str, value: &Option<String>, matched: bool| {
                if let Some(value) = value
                    && !matched
                {
                    *preview
                        .unmatched
                        .entry(kind.to_owned())
                        .or_default()
                        .entry(value.trim().to_owned())
                        .or_default() += 1;
                }
            };
            let status_matched = row.status.as_deref().is_none_or(|wanted| {
                context
                    .statuses
                    .iter()
                    .any(|status| status.1.eq_ignore_ascii_case(wanted.trim()))
            });
            unmatched("status", &row.status, status_matched);
            unmatched("type", &row.type_name, resolved.type_id.is_some());
            unmatched(
                "assignee",
                &row.assignee,
                resolved.assignee_actor_id.is_some(),
            );
        }
        if preview.rows.len() < PREVIEW_ROWS {
            let mut labels = resolved
                .label_ids
                .iter()
                .filter_map(|id| {
                    context
                        .labels
                        .iter()
                        .find(|(label, _)| label == id)
                        .map(|(_, name)| name.clone())
                })
                .collect::<Vec<_>>();
            labels.extend(resolved.new_labels.iter().map(|(name, _)| name.clone()));
            preview.rows.push(PreviewRow {
                index,
                external_id: row.external_id.clone(),
                title: row.title.clone(),
                status: resolved.status_name.clone(),
                type_name: resolved.type_name.clone(),
                priority: resolved.priority,
                labels,
                assignee: resolved.assignee_name.clone(),
                milestone: resolved
                    .milestone_id
                    .and_then(|id| {
                        context
                            .milestones
                            .iter()
                            .find(|(milestone, _)| *milestone == id)
                            .map(|(_, name)| name.clone())
                    })
                    .or(resolved.new_milestone.clone()),
                start_date: resolved.start_date,
                due_date: resolved.due_date,
                estimate: resolved.estimate,
                parent: row.parent,
                skip: resolved.skip,
                notes: resolved.notes,
            });
        }
    }
    preview.new_labels = new_labels.into_values().collect();
    preview.new_milestones = new_milestones.into_values().collect();
    preview.rows_shown = preview.rows.len();
    preview
}

// ---------------------------------------------------------------------------
// Views

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportView {
    id: Uuid,
    source: String,
    source_name: String,
    repository_id: Option<Uuid>,
    repository_name: Option<String>,
    status: String,
    row_count: i32,
    processed_count: i32,
    created_count: i32,
    skipped_count: i32,
    error: Option<String>,
    created_by_name: Option<String>,
    version: i64,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    options: ImportOptions,
    mapping: BTreeMap<String, String>,
    /// CSV column names, for changing the mapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
    fields: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<Preview>,
    /// Where the created tasks can be found.
    href: String,
}

async fn import_view(
    connection: &mut PgConnection,
    record: ImportRecord,
    with_preview: bool,
    sees_code: bool,
) -> Result<ImportView, AppError> {
    // Without "see code", a GitHub import shows no repository, issues or
    // fetch error: only that it exists and how far it got. Only a GitHub
    // import names a repository (older Markdown or CSV ones may have stored
    // one they were passed).
    let github = record.source == "github";
    let hidden = github && !sees_code;
    let repository = github && sees_code;
    let preview = if with_preview && !hidden && record.status == "draft" {
        let context = load_context(connection, record.project_id, &record.source_key()).await?;
        Some(preview(&context, &record.options(), &record.rows()))
    } else {
        None
    };
    let columns = if with_preview && record.source == "csv" && record.status == "draft" {
        let text = sqlx::query_scalar::<_, Option<String>>(
            "SELECT raw_text FROM task_imports WHERE id = $1",
        )
        .bind(record.id)
        .fetch_one(&mut *connection)
        .await?;
        text.and_then(|text| parse_csv(&text).ok())
            .and_then(|records| records.into_iter().next())
    } else {
        None
    };
    Ok(ImportView {
        href: format!("/projects/{}/work-items?sort=updated", record.project_id),
        options: record.options(),
        mapping: record.mapping(),
        id: record.id,
        source: record.source,
        source_name: record.source_name,
        repository_id: record.repository_id.filter(|_| repository),
        repository_name: record.repository_name.filter(|_| repository),
        status: record.status,
        row_count: record.row_count,
        processed_count: record.processed_count,
        created_count: record.created_count,
        skipped_count: record.skipped_count,
        error: if hidden { None } else { record.error },
        created_by_name: record.created_by_name,
        version: record.version,
        created_at: record.created_at,
        finished_at: record.finished_at,
        columns,
        fields: &FIELDS,
        preview,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportsResponse {
    imports: Vec<ImportView>,
    can_import: bool,
}

pub(super) async fn list_imports(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ImportsResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let mut connection = state.pool.acquire().await?;
    let records = sqlx::query_as::<_, ImportRecord>(&format!(
        "{IMPORT_SELECT} WHERE import.project_id = $1 ORDER BY import.created_at DESC, import.id DESC LIMIT 20"
    ))
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let code = sees_code(&mut connection, principal.actor_id, project_id).await?;
    let mut imports = Vec::with_capacity(records.len());
    for record in records {
        imports.push(import_view(&mut connection, record, false, code).await?);
    }
    Ok(Json(ImportsResponse {
        imports,
        can_import: capabilities(&membership)?.edit && membership.project_archived_at.is_none(),
    }))
}

pub(super) async fn get_import(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<ImportView>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let mut connection = state.pool.acquire().await?;
    let record = import_row(&mut connection, project_id, import_id, false)
        .await?
        .ok_or(AppError::NotFound("import"))?;
    let code = sees_code(&mut connection, principal.actor_id, project_id).await?;
    Ok(Json(
        import_view(&mut connection, record, true, code).await?,
    ))
}

// ---------------------------------------------------------------------------
// Starting and changing an import

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateImportRequest {
    pub source: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub repository_id: Option<Uuid>,
    #[serde(default)]
    pub options: Option<ImportOptions>,
    #[serde(default)]
    pub mapping: Option<BTreeMap<String, String>>,
}

/// A refusal: the field and what to change.
pub(crate) type Refusal = (&'static str, String);

fn refuse<T>(field: &'static str, message: impl Into<String>) -> Result<T, Refusal> {
    Err((field, message.into()))
}

/// Rows read from text, with the mapping used for CSV.
fn read_text(
    source: &str,
    text: &str,
    mapping: Option<BTreeMap<String, String>>,
    options: &ImportOptions,
) -> Result<(Vec<ImportRow>, BTreeMap<String, String>), Refusal> {
    match source {
        "csv" => {
            let records = parse_csv(text).map_err(|message| ("text", message))?;
            let Some(headers) = records.first() else {
                return refuse("text", "The file is empty.");
            };
            let mapping = match mapping {
                Some(mapping) => {
                    if let Some(field) = mapping
                        .keys()
                        .find(|field| !FIELDS.contains(&field.as_str()))
                    {
                        return refuse("mapping", format!("“{field}” isn't a task field."));
                    }
                    if let Some(column) = mapping.values().find(|column| {
                        !headers
                            .iter()
                            .any(|header| header.trim().eq_ignore_ascii_case(column.trim()))
                    }) {
                        return refuse("mapping", format!("The file has no column “{column}”."));
                    }
                    mapping
                }
                None => auto_mapping(headers),
            };
            let rows = csv_rows(&records, &mapping).map_err(|message| ("mapping", message))?;
            Ok((rows, mapping))
        }
        "markdown" => {
            let rows = markdown_rows(text, options.headings_as_milestones)
                .map_err(|message| ("text", message))?;
            if rows.is_empty() {
                return refuse(
                    "text",
                    "No tasks found. Write each task as a list item, such as “- [ ] Ship it”.",
                );
            }
            Ok((rows, BTreeMap::new()))
        }
        _ => refuse("source", "Use markdown, csv or github."),
    }
}

fn check_options(options: &ImportOptions) -> Result<(), Refusal> {
    if let Some(state) = &options.github_state
        && !matches!(state.as_str(), "open" | "all")
    {
        return refuse("options.githubState", "Use open or all.");
    }
    if options.excluded_rows.len() > MAX_ROWS {
        return refuse("options.excludedRows", "Too many rows left out.");
    }
    Ok(())
}

/// What an import will start with, read and checked before anything is
/// locked.
pub(crate) struct Prepared {
    options: ImportOptions,
    source_name: String,
    rows: Vec<ImportRow>,
    mapping: BTreeMap<String, String>,
    text: Option<String>,
}

pub(crate) fn prepare(request: &CreateImportRequest) -> Result<Prepared, Refusal> {
    let options = request.options.clone().unwrap_or(ImportOptions {
        create_labels: true,
        create_milestones: true,
        ..ImportOptions::default()
    });
    check_options(&options)?;
    let source_name = request
        .file_name
        .as_deref()
        .map(|name| name.trim().chars().take(200).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            match request.source.as_str() {
                "github" => "GitHub issues",
                "csv" => "CSV",
                _ => "Markdown plan",
            }
            .to_owned()
        });
    if request.source == "github" {
        if request.repository_id.is_none() {
            return refuse("repositoryId", "Pick a repository to import issues from.");
        }
        return Ok(Prepared {
            options,
            source_name,
            rows: Vec::new(),
            mapping: BTreeMap::new(),
            text: None,
        });
    }
    let Some(text) = request.text.as_deref() else {
        return refuse("text", "Paste or upload what to import.");
    };
    if text.len() > MAX_TEXT_BYTES {
        return refuse("text", "Import at most 4 MB at a time.");
    }
    let (rows, mapping) = read_text(&request.source, text, request.mapping.clone(), &options)?;
    Ok(Prepared {
        options,
        source_name,
        rows,
        mapping,
        text: Some(text.to_owned()),
    })
}

async fn organization_of(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// Records a new import. A GitHub one starts fetching its issues. The
/// caller has locked the project and checked the actor may create tasks.
pub(crate) async fn insert_import(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    request: &CreateImportRequest,
    prepared: Prepared,
) -> Result<Result<Uuid, Refusal>, AppError> {
    let project_id = actor.project_id;
    let status = if request.source == "github" {
        if !sees_code(transaction, actor.actor_id, project_id).await? {
            return Ok(refuse(
                "repositoryId",
                "You can't see this project's code, so you can't import its GitHub issues.",
            ));
        }
        let usable = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM repositories WHERE project_id = $1 AND id = $2 AND enabled AND archived_at IS NULL)",
        )
        .bind(project_id)
        .bind(request.repository_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !usable {
            return Ok(refuse(
                "repositoryId",
                "Pick one of the project's repositories.",
            ));
        }
        "fetching"
    } else {
        "draft"
    };
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO task_imports (
            id, project_id, created_by_actor_id, source, source_name, repository_id,
            raw_text, mapping, options, rows, status, row_count, made_through_app
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(actor.actor_id)
    .bind(&request.source)
    .bind(&prepared.source_name)
    // Only a GitHub import has a repository; one passed with Markdown or CSV
    // is dropped.
    .bind(request.repository_id.filter(|_| request.source == "github"))
    .bind(&prepared.text)
    .bind(json!(prepared.mapping))
    .bind(json!(prepared.options))
    .bind(json!(prepared.rows))
    .bind(status)
    .bind(prepared.rows.len() as i32)
    .bind(through_app(actor))
    .execute(&mut **transaction)
    .await?;
    if status == "fetching" {
        let organization_id = organization_of(transaction, project_id).await?;
        enqueue(
            &mut **transaction,
            NewJob::new(FETCH_KIND)
                .project(organization_id, project_id)
                .payload(ImportJob { import_id: id })
                .unique(format!("task-import-fetch:{id}")),
        )
        .await?;
    }
    record_audit_for(
        transaction,
        actor,
        "task_import",
        "task_import.created",
        id,
        json!({ "source": request.source, "rows": prepared.rows.len() }),
    )
    .await?;
    Ok(Ok(id))
}

/// One import as callers see it, with a preview while it's a draft.
pub(crate) async fn import_by_id(
    connection: &mut PgConnection,
    project_id: Uuid,
    import_id: Uuid,
    with_preview: bool,
    viewer: Uuid,
) -> Result<Option<ImportView>, AppError> {
    match import_row(connection, project_id, import_id, false).await? {
        Some(record) => {
            let code = sees_code(connection, viewer, project_id).await?;
            Ok(Some(
                import_view(connection, record, with_preview, code).await?,
            ))
        }
        None => Ok(None),
    }
}

pub(super) async fn create_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateImportRequest>,
) -> Result<Response, AppError> {
    let prepared = match prepare(&request) {
        Ok(prepared) => prepared,
        Err((field, message)) => return Ok(invalid(field, message)),
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("projects", project_id, "task_import_create")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    let id = match insert_import(&mut write.transaction, &actor, &request, prepared).await? {
        Ok(id) => id,
        Err((field, message)) => return Ok(invalid(field, message)),
    };
    let view = import_by_id(&mut write.transaction, project_id, id, true, actor.actor_id)
        .await?
        .ok_or(AppError::Internal("import vanished"))?;
    write.finish(StatusCode::CREATED, &view).await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateImportRequest {
    expected_version: i64,
    #[serde(default)]
    mapping: Option<BTreeMap<String, String>>,
    #[serde(default)]
    options: Option<ImportOptions>,
    /// Stops the import: a draft is dropped, a queued or running one stops
    /// after the chunk it's on. Tasks already made stay.
    #[serde(default)]
    cancel: bool,
}

fn version_conflict(current: i64) -> Response {
    refusal(
        StatusCode::CONFLICT,
        "version_conflict",
        "This import changed since you loaded it.".to_owned(),
        json!({ "currentVersion": current }),
    )
}

pub(super) async fn update_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateImportRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("task_imports", import_id, "update")?,
            &request,
        )
        .await
    );
    let record = import_row(&mut write.transaction, project_id, import_id, true)
        .await?
        .ok_or(AppError::NotFound("import"))?;
    let code = sees_code(&mut write.transaction, write.actor.actor_id, project_id).await?;
    if record.source == "github" && !code {
        return Err(AppError::NotFound("import"));
    }
    if record.version != request.expected_version {
        return Ok(version_conflict(record.version));
    }
    if request.cancel {
        if !matches!(
            record.status.as_str(),
            "fetching" | "draft" | "queued" | "running"
        ) {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "import_finished",
                "This import already finished.".to_owned(),
                json!({ "status": record.status }),
            ));
        }
        sqlx::query(
            r#"
            UPDATE task_imports
            SET status = 'canceled', raw_text = NULL, finished_at = now(),
                version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(import_id)
        .execute(&mut *write.transaction)
        .await?;
    } else {
        if record.status != "draft" {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "import_not_draft",
                "Only an import that hasn't started can change.".to_owned(),
                json!({ "status": record.status }),
            ));
        }
        let options = request.options.clone().unwrap_or_else(|| record.options());
        if let Err((field, message)) = check_options(&options) {
            return Ok(invalid(field, message));
        }
        let reread = record.source != "github"
            && (request.mapping.is_some()
                || options.headings_as_milestones != record.options().headings_as_milestones);
        let (rows, mapping) = if reread {
            let text = sqlx::query_scalar::<_, Option<String>>(
                "SELECT raw_text FROM task_imports WHERE id = $1",
            )
            .bind(import_id)
            .fetch_one(&mut *write.transaction)
            .await?
            .unwrap_or_default();
            match read_text(
                &record.source,
                &text,
                request
                    .mapping
                    .clone()
                    .or_else(|| (record.source == "csv").then(|| record.mapping())),
                &options,
            ) {
                Ok(read) => read,
                Err((field, message)) => return Ok(invalid(field, message)),
            }
        } else {
            (record.rows(), record.mapping())
        };
        sqlx::query(
            r#"
            UPDATE task_imports
            SET mapping = $2, options = $3, rows = $4, row_count = $5,
                version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(import_id)
        .bind(json!(mapping))
        .bind(json!(options))
        .bind(json!(rows))
        .bind(rows.len() as i32)
        .execute(&mut *write.transaction)
        .await?;
    }
    let record = import_row(&mut write.transaction, project_id, import_id, false)
        .await?
        .ok_or(AppError::Internal("import vanished"))?;
    let view = import_view(&mut write.transaction, record, true, code).await?;
    write.finish(StatusCode::OK, &view).await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CommitImportRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportJob {
    import_id: Uuid,
}

/// Why an import can't be committed.
pub(crate) enum CommitRefusal {
    NotFound,
    VersionConflict(i64),
    NotDraft(String),
    Empty,
}

/// Queues the job that creates an import's tasks, as `actor`. The caller
/// has locked the project and checked the actor may create tasks.
pub(crate) async fn queue_commit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    import_id: Uuid,
    expected_version: Option<i64>,
) -> Result<Result<(), CommitRefusal>, AppError> {
    let project_id = actor.project_id;
    let Some(record) = import_row(transaction, project_id, import_id, true).await? else {
        return Ok(Err(CommitRefusal::NotFound));
    };
    // Committing turns the issues into tasks: only for whoever sees code.
    if record.source == "github" && !sees_code(transaction, actor.actor_id, project_id).await? {
        return Ok(Err(CommitRefusal::NotFound));
    }
    if expected_version.is_some_and(|expected| expected != record.version) {
        return Ok(Err(CommitRefusal::VersionConflict(record.version)));
    }
    if record.status != "draft" {
        return Ok(Err(CommitRefusal::NotDraft(record.status)));
    }
    if record.row_count == 0 {
        return Ok(Err(CommitRefusal::Empty));
    }
    // Whoever commits is who the tasks are made by, and through what: an
    // app's commit stays within the app's limit while the chunks run.
    sqlx::query(
        r#"
        UPDATE task_imports
        SET status = 'queued', created_by_actor_id = $2, made_through_app = $3,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(import_id)
    .bind(actor.actor_id)
    .bind(through_app(actor))
    .execute(&mut **transaction)
    .await?;
    let organization_id = organization_of(transaction, project_id).await?;
    enqueue(
        &mut **transaction,
        NewJob::new(COMMIT_KIND)
            .project(organization_id, project_id)
            .payload(ImportJob { import_id })
            .unique(format!("task-import-commit:{import_id}")),
    )
    .await?;
    record_audit_for(
        transaction,
        actor,
        "task_import",
        "task_import.committed",
        import_id,
        json!({ "rows": record.row_count }),
    )
    .await?;
    Ok(Ok(()))
}

pub(super) async fn commit_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CommitImportRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("task_imports", import_id, "commit")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    match queue_commit(
        &mut write.transaction,
        &actor,
        import_id,
        Some(request.expected_version),
    )
    .await?
    {
        Ok(()) => {}
        Err(CommitRefusal::NotFound) => return Err(AppError::NotFound("import")),
        Err(CommitRefusal::VersionConflict(current)) => return Ok(version_conflict(current)),
        Err(CommitRefusal::NotDraft(status)) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "import_not_draft",
                "This import has already started.".to_owned(),
                json!({ "status": status }),
            ));
        }
        Err(CommitRefusal::Empty) => return Ok(invalid("rows", "There's nothing to import.")),
    }
    let view = import_by_id(
        &mut write.transaction,
        project_id,
        import_id,
        false,
        actor.actor_id,
    )
    .await?
    .ok_or(AppError::Internal("import vanished"))?;
    write.finish(StatusCode::OK, &view).await
}

// ---------------------------------------------------------------------------
// Jobs

pub(crate) fn register_jobs(registry: &mut JobRegistry) {
    registry.register(FETCH_KIND, |context: JobContext| async move {
        fetch_issues(&context).await
    });
    registry.register(COMMIT_KIND, |context: JobContext| async move {
        commit_rows(&context).await
    });
}

async fn fail(
    transaction: &mut Transaction<'_, Postgres>,
    import_id: Uuid,
    message: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE task_imports
        SET status = 'failed', error = $2, raw_text = NULL, finished_at = now(),
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(import_id)
    .bind(message)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The person an import acts as, if they still may create tasks there.
async fn acting_actor(
    transaction: &mut Transaction<'_, Postgres>,
    record: &ImportRecord,
    job_id: Uuid,
) -> Result<Result<CommandActor, String>, AppError> {
    let who = record
        .created_by_name
        .clone()
        .unwrap_or_else(|| "The person who started it".to_owned());
    let stopped = || format!("Stopped because {who} can no longer create tasks in this project.");
    let Some(membership) =
        lock_membership(transaction, record.created_by_actor_id, record.project_id).await?
    else {
        return Ok(Err(stopped()));
    };
    if membership.project_archived_at.is_some() {
        return Ok(Err("Stopped because the project is archived.".to_owned()));
    }
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
        .bind(record.created_by_actor_id)
        .fetch_one(&mut **transaction)
        .await?;
    let actor_kind = match kind.as_str() {
        "human" => ActorKind::Human,
        "external_agent" => ActorKind::ExternalAgent,
        _ => return Ok(Err(stopped())),
    };
    let actor = CommandActor {
        actor_id: record.created_by_actor_id,
        actor_kind,
        project_id: record.project_id,
        audit: AuditAttribution::Job {
            job_id,
            source: "task_import",
        },
    };
    let decision = authorize_mutation(&actor, &membership, Action::WorkItemCreate, None)?;
    // A GitHub import reads the repository's issues: the fetch and each
    // chunk also need "see code", by role and (inside within_agent_limits)
    // by the app's limit as it is now.
    if matches!(decision, Decision::Allow)
        && record.source == "github"
        && !sees_code(transaction, actor.actor_id, actor.project_id).await?
    {
        return Ok(Err(format!(
            "Stopped because {who}, or the app it was started from, can no longer see this project's code. Tasks already made are kept."
        )));
    }
    match decision {
        Decision::Allow => Ok(Ok(actor)),
        Decision::Deny(DenyReason::AgentLimit) => Ok(Err(
            "Stopped because the app it was started from may no longer create tasks in this project. Tasks already made are kept."
                .to_owned(),
        )),
        Decision::Deny(_) => Ok(Err(stopped())),
    }
}

/// Whether `actor` is a person's app or script in this call: the MCP layer
/// (and so the public API) sets the app's agent limit for the call.
fn through_app(actor: &CommandActor) -> bool {
    actor.actor_kind == ActorKind::Human
        && crate::policy::current_agent_limit(
            actor.actor_id,
            MembershipScope::Project(actor.project_id),
        )
        .is_some()
}

/// For an import a person's app made: that app's agent limits as they are
/// now, for the job to act within. None otherwise.
async fn app_limits(
    transaction: &mut Transaction<'_, Postgres>,
    record: &ImportRecord,
) -> Result<Option<AgentLimits>, AppError> {
    if !record.made_through_app {
        return Ok(None);
    }
    let human = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM actors WHERE id = $1 AND kind = 'human')",
    )
    .bind(record.created_by_actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !human {
        return Ok(None);
    }
    Ok(Some(
        crate::roles::app_access(transaction, record.created_by_actor_id)
            .await?
            .limits,
    ))
}

async fn job_record(
    transaction: &mut Transaction<'_, Postgres>,
    context: &JobContext,
) -> Result<Option<ImportRecord>, JobError> {
    let payload: ImportJob = context.job.payload_as()?;
    let Some(project_id) = context.job.project_id else {
        return Err(JobError::permanent("import job without a project"));
    };
    Ok(import_row(transaction, project_id, payload.import_id, true).await?)
}

async fn fetch_issues(context: &JobContext) -> Result<(), JobError> {
    let pool = &context.state.pool;
    let mut transaction = pool.begin().await?;
    let Some(record) = job_record(&mut transaction, context).await? else {
        return Ok(());
    };
    if record.status != "fetching" {
        return Ok(());
    }
    let limits = app_limits(&mut transaction, &record).await?;
    if let Err(reason) = within_agent_limits(
        limits,
        acting_actor(&mut transaction, &record, context.job.id),
    )
    .await?
    {
        fail(&mut transaction, record.id, &reason).await?;
        transaction.commit().await?;
        return Ok(());
    }
    let Some(repository_id) = record.repository_id else {
        fail(&mut transaction, record.id, "The repository is gone.").await?;
        transaction.commit().await?;
        return Ok(());
    };
    // Don't hold the import's lock across calls to GitHub.
    transaction.commit().await?;
    let issue_state = record
        .options()
        .github_state
        .unwrap_or_else(|| "open".to_owned());
    let fetched = crate::github::repository_issues(
        &context.state,
        record.project_id,
        repository_id,
        &issue_state,
        MAX_ROWS * 2,
    )
    .await?;
    let mut transaction = pool.begin().await?;
    let Some(current) = job_record(&mut transaction, context).await? else {
        return Ok(());
    };
    if current.status != "fetching" {
        return Ok(());
    }
    // Access may have gone while GitHub answered: check again before
    // keeping the issues.
    let limits = app_limits(&mut transaction, &current).await?;
    if let Err(reason) = within_agent_limits(
        limits,
        acting_actor(&mut transaction, &current, context.job.id),
    )
    .await?
    {
        fail(&mut transaction, record.id, &reason).await?;
        transaction.commit().await?;
        return Ok(());
    }
    match fetched {
        Err(unavailable) => fail(&mut transaction, record.id, unavailable.message()).await?,
        Ok(issues) => {
            let rows = github_rows(issues);
            sqlx::query(
                r#"
                UPDATE task_imports
                SET status = 'draft', rows = $2, row_count = $3,
                    version = version + 1, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(record.id)
            .bind(json!(rows))
            .bind(rows.len() as i32)
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

/// Creates the rows in chunks of 100, one transaction each, so a long
/// import shows progress and a restart picks up where it stopped.
async fn commit_rows(context: &JobContext) -> Result<(), JobError> {
    loop {
        let mut transaction = context.state.pool.begin().await?;
        let Some(record) = job_record(&mut transaction, context).await? else {
            return Ok(());
        };
        if !matches!(record.status.as_str(), "queued" | "running") {
            return Ok(());
        }
        // Each chunk checks again: the person's role, and for an import their
        // app committed, the app's agent limit as it is now. Tasks earlier
        // chunks made are kept.
        let limits = app_limits(&mut transaction, &record).await?;
        let actor = match within_agent_limits(
            limits.clone(),
            acting_actor(&mut transaction, &record, context.job.id),
        )
        .await?
        {
            Ok(actor) => actor,
            Err(reason) => {
                fail(&mut transaction, record.id, &reason).await?;
                transaction.commit().await?;
                return Ok(());
            }
        };
        let rows = record.rows();
        let start = usize::try_from(record.processed_count).unwrap_or(0);
        let end = (start + CHUNK).min(rows.len());
        let (created, skipped) = within_agent_limits(
            limits,
            create_chunk(&mut transaction, &actor, &record, &rows, start..end),
        )
        .await?;
        let finished = end >= rows.len();
        sqlx::query(
            r#"
            UPDATE task_imports
            SET status = CASE WHEN $5 THEN 'completed' ELSE 'running' END,
                processed_count = $2,
                created_count = created_count + $3,
                skipped_count = skipped_count + $4,
                raw_text = CASE WHEN $5 THEN NULL ELSE raw_text END,
                finished_at = CASE WHEN $5 THEN now() ELSE finished_at END,
                version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(record.id)
        .bind(end as i32)
        .bind(created as i32)
        .bind(skipped as i32)
        .bind(finished)
        .execute(&mut *transaction)
        .await?;
        if finished {
            record_audit_for(
                &mut transaction,
                &actor,
                "task_import",
                "task_import.completed",
                record.id,
                json!({
                    "created": record.created_count as usize + created,
                    "skipped": record.skipped_count as usize + skipped,
                }),
            )
            .await?;
        }
        transaction.commit().await?;
        if finished {
            return Ok(());
        }
    }
}

/// The link key for a row: its external ID, or its place in this import.
fn link_key(record: &ImportRecord, index: usize, row: &ImportRow) -> (String, String) {
    match &row.external_id {
        Some(id) => (record.source_key(), id.clone()),
        None => (format!("import:{}", record.id), format!("row:{index}")),
    }
}

async fn linked_work_item(
    connection: &mut PgConnection,
    project_id: Uuid,
    (source_key, external_id): &(String, String),
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT work_item_id FROM task_import_links WHERE project_id = $1 AND source_key = $2 AND external_id = $3",
    )
    .bind(project_id)
    .bind(source_key)
    .bind(external_id)
    .fetch_optional(&mut *connection)
    .await?)
}

async fn create_chunk(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    record: &ImportRecord,
    rows: &[ImportRow],
    range: std::ops::Range<usize>,
) -> Result<(usize, usize), AppError> {
    let options = record.options();
    let mut context = load_context(transaction, record.project_id, &record.source_key()).await?;
    let (mut created, mut skipped) = (0, 0);
    for index in range {
        let row = &rows[index];
        let key = link_key(record, index, row);
        if linked_work_item(transaction, record.project_id, &key)
            .await?
            .is_some()
        {
            skipped += 1;
            continue;
        }
        let resolved = resolve(&context, &options, index, row);
        if resolved.skip.is_some() {
            skipped += 1;
            continue;
        }
        let mut label_ids = resolved.label_ids.clone();
        if !resolved.new_labels.is_empty() {
            let ensured = work_item_labels::ensure_by_name(
                transaction,
                record.project_id,
                actor.actor_id,
                &resolved.new_labels,
            )
            .await?;
            for created_label in &ensured.created {
                context
                    .labels
                    .push((created_label.id, created_label.name.clone()));
            }
            for id in ensured.ids {
                if !label_ids.contains(&id) {
                    label_ids.push(id);
                }
            }
        }
        label_ids.truncate(work_item_labels::MAX_LABELS_PER_TASK);
        let mut milestone_id = resolved.milestone_id;
        if let Some(name) = &resolved.new_milestone {
            match work_item_planning::create_milestone(transaction, actor, name, "", None).await {
                Ok((id, _)) => {
                    context.milestones.push((id, name.clone()));
                    milestone_id = Some(id);
                }
                Err(AppError::Conflict(..)) => {}
                Err(error) => return Err(error),
            }
        }
        let parent_work_item_id = match row.parent {
            Some(parent) if parent < rows.len() => {
                linked_work_item(
                    transaction,
                    record.project_id,
                    &link_key(record, parent, &rows[parent]),
                )
                .await?
            }
            _ => None,
        };
        let description = normalize_description(&row.description).unwrap_or_default();
        let title = normalize_title(&row.title).unwrap_or_else(|_| row.title.clone());
        let mut assignee = resolved.assignee_actor_id;
        let mut parent = parent_work_item_id;
        let mut fields = NewTaskFields {
            milestone_id,
            start_date: resolved.start_date,
            due_date: resolved.due_date,
            estimate: resolved.estimate,
        };
        let field_values =
            import_field_values(transaction, &context, actor, &resolved.field_inputs).await?;
        // A value the project refuses is dropped rather than losing the row.
        let mut attempts = 0;
        let work_item_id = loop {
            attempts += 1;
            let outcome = create_work_item(
                transaction,
                actor,
                CreateWorkItemCommand {
                    field_values: field_values.as_ref(),
                    title: &title,
                    description: &description,
                    type_id: resolved.type_id,
                    status_id: resolved.status_id,
                    priority: resolved.priority,
                    assignee_actor_id: assignee,
                    parent_work_item_id: parent,
                    label_ids: &label_ids,
                    acceptance_criteria: &[],
                    board_placement: None,
                    fields: NewTaskFields {
                        milestone_id: fields.milestone_id,
                        start_date: fields.start_date,
                        due_date: fields.due_date,
                        estimate: fields.estimate,
                    },
                    activity_summary: "Work Item created",
                    activity_metadata: json!({ "importId": record.id }),
                    activity_number_metadata: AllocatedNumberMetadata::Omit,
                    audit_action: "work_item.created",
                    audit_metadata: json!({ "importId": record.id, "source": record.source }),
                },
            )
            .await?;
            match outcome {
                CreateWorkItemOutcome::Created { work_item_id, .. } => break Some(work_item_id),
                _ if attempts > 4 => break None,
                CreateWorkItemOutcome::InvalidAssignee => assignee = None,
                CreateWorkItemOutcome::InvalidMilestone => fields.milestone_id = None,
                CreateWorkItemOutcome::InvalidDates => fields.start_date = None,
                CreateWorkItemOutcome::ParentNotFound
                | CreateWorkItemOutcome::ParentArchived(_)
                | CreateWorkItemOutcome::ParentTooDeep
                | CreateWorkItemOutcome::InvalidPlacement(_) => parent = None,
            }
        };
        let Some(work_item_id) = work_item_id else {
            skipped += 1;
            continue;
        };
        sqlx::query(
            r#"
            INSERT INTO task_import_links (
                id, project_id, source_key, external_id, work_item_id, import_id
            ) VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(record.project_id)
        .bind(&key.0)
        .bind(&key.1)
        .bind(work_item_id)
        .bind(record.id)
        .execute(&mut **transaction)
        .await?;
        created += 1;
    }
    Ok((created, skipped))
}

// ---------------------------------------------------------------------------
// Export

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExportQuery {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportTask {
    key: String,
    number: i64,
    title: String,
    description: String,
    status: String,
    status_category: String,
    #[serde(rename = "type")]
    type_name: String,
    priority: String,
    labels: Vec<String>,
    assignee: Option<String>,
    milestone: Option<String>,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    estimate: Option<f64>,
    parent: Option<String>,
    blocked_by: Vec<String>,
    acceptance_criteria: Vec<String>,
    archived: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(skip)]
    field_values: Value,
    /// Custom field values by field name, as task_open reads them.
    #[sqlx(skip)]
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    fields: serde_json::Map<String, Value>,
}

async fn export_tasks(
    connection: &mut PgConnection,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<ExportTask>, AppError> {
    let tasks = sqlx::query_as::<_, ExportTask>(
        r#"
        SELECT project.project_key || '-' || item.number AS key, item.number, item.title,
               item.description, status.name AS status,
               status.semantic_category AS status_category, item_type.name AS type_name,
               item.priority,
               COALESCE((
                   SELECT array_agg(label.name ORDER BY lower(label.name))
                   FROM work_item_active_labels label
                   WHERE label.project_id = item.project_id AND label.work_item_id = item.id
               ), ARRAY[]::text[]) AS labels,
               assignee.display_name AS assignee, milestone.name AS milestone,
               item.start_date, item.due_date, item.estimate,
               CASE WHEN parent.id IS NULL THEN NULL
                    ELSE project.project_key || '-' || parent.number END AS parent,
               COALESCE((
                   SELECT array_agg(project.project_key || '-' || blocker.number ORDER BY blocker.number)
                   FROM work_item_dependencies dependency
                   JOIN work_items blocker
                     ON blocker.project_id = dependency.project_id
                    AND blocker.id = dependency.blocking_work_item_id
                   WHERE dependency.project_id = item.project_id
                     AND dependency.blocked_work_item_id = item.id
               ), ARRAY[]::text[]) AS blocked_by,
               COALESCE((
                   SELECT array_agg(criterion.text ORDER BY criterion.position, criterion.id)
                   FROM work_item_acceptance_criteria criterion
                   WHERE criterion.project_id = item.project_id
                     AND criterion.work_item_id = item.id
               ), ARRAY[]::text[]) AS acceptance_criteria,
               item.archived_at IS NOT NULL AS archived, item.created_at, item.updated_at,
               item.field_values
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        JOIN work_item_types item_type
          ON item_type.project_id = item.project_id AND item_type.id = item.type_id
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
        LEFT JOIN project_milestones milestone
          ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
        LEFT JOIN work_items parent
          ON parent.project_id = item.project_id AND parent.id = item.parent_work_item_id
        WHERE item.project_id = $1 AND ($2 OR item.archived_at IS NULL)
        ORDER BY item.number
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(include_archived)
    .bind(MAX_EXPORT_TASKS)
    .fetch_all(&mut *connection)
    .await?;
    Ok(tasks)
}

/// Each task's custom field values by name, for the export.
async fn name_field_values(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    tasks: &mut [ExportTask],
) -> Result<(), AppError> {
    if definitions.is_empty() {
        return Ok(());
    }
    let mut references = work_item_fields::references(
        connection,
        project_id,
        definitions,
        tasks.iter().map(|task| &task.field_values),
    )
    .await?;
    // A page link keeps its page's link beside the title, so an import of
    // this file can find the page again; tasks go by key, people by @handle.
    for reference in references.values_mut() {
        if reference.kind == "page" {
            reference.label =
                work_item_fields::exported_page(project_id, reference.id, &reference.label);
        }
    }
    for task in tasks {
        task.fields = work_item_fields::named_values(definitions, &task.field_values, &references)
            .into_iter()
            .map(|(field, value)| (field.name, value))
            .collect();
    }
    Ok(())
}

fn csv_field(value: &str) -> String {
    // A leading = + - @ would run as a formula in a spreadsheet.
    let value = if value.starts_with(['=', '+', '-', '@']) && value.parse::<f64>().is_err() {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

/// The CSV columns, named so an import maps them back without changes.
const CSV_HEADERS: [&str; 15] = [
    "ID",
    "Title",
    "Description",
    "Status",
    "Priority",
    "Type",
    "Labels",
    "Assignee",
    "Milestone",
    "Start Date",
    "Due Date",
    "Estimate",
    "Parent",
    "Created",
    "Updated",
];

/// How a custom field's CSV column starts, so a field named like a built-in
/// column (Priority, Status) or one of its aliases isn't read as that column.
const FIELD_COLUMN_PREFIX: &str = "Field: ";

/// The custom field a CSV column names: by what follows "Field:" first, as
/// exports write it, else by the whole header (other tools, and exports
/// from before the prefix). So with fields "Risk" and "Field: Risk", the
/// columns "Field: Risk" and "Field: Field: Risk" fill one each. An older
/// export's bare "Field: Risk" column now reads as the prefix too, filling
/// "Risk" when the project has it.
fn column_field<'a>(
    definitions: &'a [FieldDefinition],
    header: &str,
) -> Option<&'a FieldDefinition> {
    let header = header.trim();
    let prefix = FIELD_COLUMN_PREFIX.trim();
    header
        .split_at_checked(prefix.len())
        .filter(|(head, _)| head.eq_ignore_ascii_case(prefix))
        .and_then(|(_, name)| fields::find(definitions, name))
        .or_else(|| fields::find(definitions, header))
}

/// The CSV: the built-in columns, then one per custom field ("Field: " and
/// its name), so importing it into a project with the same fields fills them.
fn tasks_csv(tasks: &[ExportTask], definitions: &[FieldDefinition]) -> String {
    let mut out = String::from("\u{feff}");
    let headers = CSV_HEADERS
        .iter()
        .map(|header| (*header).to_owned())
        .chain(
            definitions
                .iter()
                .map(|field| csv_field(&format!("{FIELD_COLUMN_PREFIX}{}", field.name))),
        )
        .collect::<Vec<_>>();
    out.push_str(&headers.join(","));
    out.push_str("\r\n");
    for task in tasks {
        let date =
            |value: Option<NaiveDate>| value.map(|date| date.to_string()).unwrap_or_default();
        let cells = [
            task.key.clone(),
            task.title.clone(),
            task.description.clone(),
            task.status.clone(),
            task.priority.replace('_', " "),
            task.type_name.clone(),
            task.labels.join(", "),
            task.assignee.clone().unwrap_or_default(),
            task.milestone.clone().unwrap_or_default(),
            date(task.start_date),
            date(task.due_date),
            task.estimate
                .map(|value| value.to_string())
                .unwrap_or_default(),
            task.parent.clone().unwrap_or_default(),
            task.created_at.to_rfc3339(),
            task.updated_at.to_rfc3339(),
        ];
        let custom = definitions.iter().map(|field| {
            task.fields
                .get(&field.name)
                .map(fields::display_text)
                .unwrap_or_default()
        });
        out.push_str(
            &cells
                .into_iter()
                .chain(custom)
                .map(|cell| csv_field(&cell))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
    }
    out
}

pub(crate) async fn export_json(
    connection: &mut PgConnection,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Value, AppError> {
    let context = load_context(connection, project_id, "").await?;
    let mut tasks = export_tasks(connection, project_id, include_archived).await?;
    name_field_values(connection, project_id, &context.fields, &mut tasks).await?;
    let truncated = tasks.len() as i64 >= MAX_EXPORT_TASKS;
    Ok(json!({
        "project": { "key": context.project_key },
        "exportedAt": Utc::now(),
        "estimateUnit": context.estimate_unit,
        "statuses": context.statuses.iter().map(|status| json!({
            "name": status.1, "category": status.2, "isDefault": status.3,
        })).collect::<Vec<_>>(),
        "types": context.types.iter().map(|kind| json!({ "name": kind.1, "isDefault": kind.2 })).collect::<Vec<_>>(),
        "labels": context.labels.iter().map(|label| label.1.clone()).collect::<Vec<_>>(),
        "milestones": context.milestones.iter().map(|milestone| milestone.1.clone()).collect::<Vec<_>>(),
        "fields": context.fields.iter().map(FieldView::from).collect::<Vec<_>>(),
        "tasks": tasks,
        "truncated": truncated,
    }))
}

pub(super) async fn export_project(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ExportQuery>,
) -> Result<Response, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let mut connection = state.pool.acquire().await?;
    let key = sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&mut *connection)
        .await?;
    let (body, content_type, extension) = match query.format.as_deref().unwrap_or("json") {
        "json" => (
            serde_json::to_string_pretty(
                &export_json(&mut connection, project_id, query.include_archived).await?,
            )
            .map_err(|_| AppError::Internal("serialize export"))?,
            "application/json",
            "json",
        ),
        "csv" => {
            let definitions =
                work_item_fields::definitions(&mut *connection, project_id, false).await?;
            let mut tasks =
                export_tasks(&mut connection, project_id, query.include_archived).await?;
            name_field_values(&mut connection, project_id, &definitions, &mut tasks).await?;
            (
                tasks_csv(&tasks, &definitions),
                "text/csv; charset=utf-8",
                "csv",
            )
        }
        _ => return Ok(invalid("format", "Use json or csv.")),
    };
    let disposition = format!(
        "attachment; filename=\"{}-tasks.{extension}\"",
        key.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "")
    );
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ProjectContext {
        let todo = Uuid::now_v7();
        ProjectContext {
            project_key: "AW".to_owned(),
            estimate_unit: "points".to_owned(),
            statuses: vec![
                (todo, "Todo".to_owned(), "unstarted".to_owned(), true),
                (
                    Uuid::now_v7(),
                    "Done".to_owned(),
                    "completed".to_owned(),
                    false,
                ),
                (
                    Uuid::now_v7(),
                    "Canceled".to_owned(),
                    "canceled".to_owned(),
                    false,
                ),
            ],
            types: vec![(Uuid::now_v7(), "Bug".to_owned(), false)],
            labels: vec![(Uuid::now_v7(), "frontend".to_owned())],
            milestones: vec![],
            members: vec![(Uuid::now_v7(), "Ana".to_owned(), Some("ana".to_owned()))],
            linked: HashMap::from([("7".to_owned(), 12)]),
            fields: Vec::new(),
        }
    }

    fn options() -> ImportOptions {
        ImportOptions {
            create_labels: true,
            create_milestones: true,
            ..ImportOptions::default()
        }
    }

    #[test]
    fn rows_match_the_project_and_say_what_doesnt() {
        let context = context();
        let row = ImportRow {
            title: "Fix it".to_owned(),
            status: Some("in review".to_owned()),
            labels: vec![
                crate::work_items::import_parse::LabelValue {
                    name: "Frontend".to_owned(),
                    color: None,
                },
                crate::work_items::import_parse::LabelValue {
                    name: "new one".to_owned(),
                    color: Some("#123456".to_owned()),
                },
            ],
            assignee: Some("@ana".to_owned()),
            type_name: Some("bug".to_owned()),
            milestone: Some("Launch".to_owned()),
            priority: Some("P1".to_owned()),
            due_date: Some("2026-11-01".to_owned()),
            estimate: Some("3".to_owned()),
            ..ImportRow::default()
        };
        let resolved = resolve(&context, &options(), 0, &row);
        assert!(resolved.skip.is_none());
        assert_eq!(resolved.label_ids.len(), 1);
        assert_eq!(
            resolved.new_labels,
            vec![("new one".to_owned(), "#123456".to_owned())]
        );
        assert!(resolved.assignee_actor_id.is_some());
        assert!(resolved.type_id.is_some());
        assert_eq!(resolved.new_milestone.as_deref(), Some("Launch"));
        assert_eq!(resolved.priority, "high");
        assert_eq!(resolved.status_name.as_deref(), Some("Todo"));
        assert!(resolved.notes.iter().any(|note| note.contains("in review")));
    }

    #[test]
    fn done_closed_and_repeats_are_handled() {
        let context = context();
        let done = ImportRow {
            title: "a".to_owned(),
            done: true,
            ..ImportRow::default()
        };
        assert_eq!(
            resolve(&context, &options(), 0, &done)
                .status_name
                .as_deref(),
            Some("Done")
        );
        let canceled = ImportRow {
            title: "a".to_owned(),
            canceled: true,
            ..ImportRow::default()
        };
        assert_eq!(
            resolve(&context, &options(), 0, &canceled)
                .status_name
                .as_deref(),
            Some("Canceled")
        );
        let repeat = ImportRow {
            title: "a".to_owned(),
            external_id: Some("7".to_owned()),
            ..ImportRow::default()
        };
        assert_eq!(
            resolve(&context, &options(), 0, &repeat)
                .existing_key
                .as_deref(),
            Some("AW-12")
        );
        let untitled = ImportRow::default();
        assert_eq!(
            resolve(&context, &options(), 0, &untitled).skip.as_deref(),
            Some("No title")
        );
        let excluded = ImportOptions {
            excluded_rows: vec![3],
            ..options()
        };
        assert_eq!(
            resolve(&context, &excluded, 3, &done).skip.as_deref(),
            Some("Left out")
        );
    }

    #[test]
    fn exported_csv_reads_back() {
        let task = ExportTask {
            key: "AW-1".to_owned(),
            number: 1,
            title: "=SUM(A1), \"quoted\"".to_owned(),
            description: "two\nlines".to_owned(),
            status: "Todo".to_owned(),
            status_category: "unstarted".to_owned(),
            type_name: "Task".to_owned(),
            priority: "no_priority".to_owned(),
            labels: vec!["a".to_owned(), "b".to_owned()],
            assignee: None,
            milestone: Some("Launch".to_owned()),
            start_date: None,
            due_date: NaiveDate::from_ymd_opt(2026, 11, 1),
            estimate: Some(2.5),
            parent: None,
            blocked_by: vec![],
            acceptance_criteria: vec![],
            archived: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            field_values: Value::Null,
            fields: serde_json::Map::from_iter([
                ("Risk".to_owned(), json!("High")),
                ("Field: Risk".to_owned(), json!("Low")),
                ("Priority".to_owned(), json!("Customer first")),
            ]),
        };
        let text_field = |name: &str| FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type: FieldType::Text,
            config: fields::FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
        };
        // A custom field named like a built-in column keeps its own column,
        // and so does one whose name starts with the prefix itself.
        let definitions = vec![
            text_field("Risk"),
            text_field("Field: Risk"),
            text_field("Priority"),
        ];
        let text = tasks_csv(&[task], &definitions);
        let records = parse_csv(&text).expect("test input is valid");
        let mapping = auto_mapping(&records[0]);
        let rows = csv_rows(&records, &mapping).expect("test input is valid");
        assert_eq!(rows[0].title, "'=SUM(A1), \"quoted\"");
        assert_eq!(rows[0].description, "two\nlines");
        assert_eq!(rows[0].labels.len(), 2);
        assert_eq!(rows[0].milestone.as_deref(), Some("Launch"));
        assert_eq!(rows[0].external_id.as_deref(), Some("AW-1"));
        assert_eq!(
            priority_of(rows[0].priority.as_deref().expect("test input is valid")),
            Some("no_priority")
        );
        // A custom field's column comes back by name and fills the field.
        assert_eq!(
            rows[0].fields.get("Field: Risk").map(String::as_str),
            Some("High")
        );
        assert_eq!(
            rows[0].fields.get("Field: Field: Risk").map(String::as_str),
            Some("Low")
        );
        let mut project = context();
        project.fields = definitions;
        let resolved = resolve(&project, &ImportOptions::default(), 0, &rows[0]);
        // Each value lands in its own field: "Field: Risk" and "Risk" don't
        // trade or overwrite each other.
        assert_eq!(
            resolved.field_inputs,
            vec![
                ("Field: Risk".to_owned(), json!("Low")),
                ("Priority".to_owned(), json!("Customer first")),
                ("Risk".to_owned(), json!("High")),
            ]
        );
        let named =
            |header: &str| column_field(&project.fields, header).map(|field| field.name.clone());
        // A column named after the field alone still fills it, as other
        // tools and earlier exports write it; the prefix reading comes first,
        // so an older bare "Field: Risk" column now fills "Risk".
        assert_eq!(named("risk").as_deref(), Some("Risk"));
        assert_eq!(named("Field: Risk").as_deref(), Some("Risk"));
        assert_eq!(
            named("field:   Field: Risk").as_deref(),
            Some("Field: Risk")
        );
        assert_eq!(named("Fields: Risk"), None);
    }
}
