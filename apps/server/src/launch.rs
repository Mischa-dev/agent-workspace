//! Launch (C4, C5, C6): one click on a task starts a run with the project's
//! defaults, in its environment, from a prompt built from the task; several
//! tasks, or a later start, go through the launch queue.
//!
//! A launch belongs to the person who asked for it. Whenever it starts (now,
//! or later from the queue) it is checked again as them: their project
//! access, their provider account and the environment (principle 9). One
//! that fails is cancelled with the reason in plain words.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgConnection, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    api::{RunActor, StartRunRequest},
    auth::{AuthenticatedActor, require_authenticated_write},
    environments::{self, EnvironmentRow, EnvironmentView},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    jobs::{self, JobRegistry, NewJob},
    memory,
    outbox::{
        ConversationCreatedPayload, OutboxConversationKind, OutboxEventPayload, enqueue_typed,
    },
    policy::{Action, ActorKind},
    provider::{manifest::AdapterManifest, run_config::RunConfig},
    settings,
};

pub const DRAIN_KIND: &str = "launch.drain";
pub const TAG_KIND: &str = "launch.tag";
const DRAIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// The most tasks one launch request may start or queue.
pub const MAX_TASKS_PER_LAUNCH: usize = 50;
/// A rendered prompt stays inside one message.
const MAX_PROMPT_CHARS: usize = 19_000;
const MAX_DESCRIPTION_CHARS: usize = 8_000;
const MAX_GUIDE_CHARS: usize = 4_000;
/// The skills' instructions written into a prompt, together. Skills past it
/// are listed by name and link only.
const MAX_SKILLS_CHARS: usize = 4_000;
const ACTIVE_RUN_STATES: &str =
    "('created', 'queued', 'starting', 'working', 'blocked', 'recovering')";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/launch",
            get(launch_plan),
        )
        .route(
            "/api/v1/projects/{project_id}/launches",
            get(list_launches).post(create_launches),
        )
        .route(
            "/api/v1/projects/{project_id}/launches/{launch_id}/cancel",
            post(cancel_launch),
        )
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(DRAIN_KIND, |context| async move {
        drain(&context.state).await?;
        Ok(())
    });
    registry.every(DRAIN_KIND, DRAIN_INTERVAL);
    registry.register(TAG_KIND, |context| async move {
        let payload: TagPayload = context.job.payload_as()?;
        launch_from_tag(&context.state, payload).await?;
        Ok(())
    });
}

/// Ask for a drain soon, such as after a launch is queued or a run ends.
pub(crate) async fn kick(executor: impl PgExecutor<'_>) -> Result<(), AppError> {
    jobs::enqueue(
        executor,
        NewJob::new(DRAIN_KIND).unique("launch.drain.kick"),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Prompt templates (C6)

/// Fills a launch template. `{{field}}` is replaced by the field's value
/// (an unknown field stays as written), `{{#if field}}…{{/if}}` keeps its
/// part only when the field has a value, and `{{#harness kind}}…{{/harness}}`
/// only when the run uses that harness (several kinds may be listed). A
/// block tag swallows the line break after it, so a template reads like the
/// Markdown it makes.
pub(crate) fn render(template: &str, fields: &BTreeMap<&str, String>, harness: &str) -> String {
    let mut output = String::new();
    let mut shown: Vec<bool> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let visible = shown.iter().all(|shown| *shown);
        if visible {
            output.push_str(&rest[..start]);
        }
        let after = &rest[start..];
        let Some(end) = after.find("}}") else {
            if visible {
                output.push_str(after);
            }
            rest = "";
            break;
        };
        let tag = after[2..end].trim();
        let mut next = &after[end + 2..];
        let block = if let Some(field) = tag.strip_prefix("#if ") {
            shown.push(
                fields
                    .get(field.trim())
                    .is_some_and(|value| !value.trim().is_empty()),
            );
            true
        } else if let Some(kinds) = tag.strip_prefix("#harness ") {
            shown.push(
                kinds
                    .split([',', ' '])
                    .any(|kind| !kind.is_empty() && kind == harness),
            );
            true
        } else if tag == "/if" || tag == "/harness" {
            shown.pop();
            true
        } else {
            if visible {
                match fields.get(tag) {
                    Some(value) => output.push_str(value),
                    None => output.push_str(&after[..end + 2]),
                }
            }
            false
        };
        if block {
            next = next
                .strip_prefix("\r\n")
                .or_else(|| next.strip_prefix('\n'))
                .unwrap_or(next);
        }
        rest = next;
    }
    if shown.iter().all(|shown| *shown) {
        output.push_str(rest);
    }
    let mut collapsed = String::with_capacity(output.len());
    let mut newlines = 0;
    for character in output.trim().chars() {
        if character == '\n' {
            newlines += 1;
            if newlines > 2 {
                continue;
            }
        } else if character != '\r' {
            newlines = 0;
        }
        collapsed.push(character);
    }
    if collapsed.chars().count() > MAX_PROMPT_CHARS {
        collapsed = collapsed.chars().take(MAX_PROMPT_CHARS).collect();
    }
    collapsed
}

fn shorten(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        let mut short = value.chars().take(max_chars).collect::<String>();
        short.push_str("\n\n(cut short)");
        short
    }
}

fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}

#[derive(FromRow)]
struct TaskRow {
    id: Uuid,
    key: String,
    title: String,
    description: String,
    priority: String,
    type_name: String,
    project_name: String,
    project_key: String,
    archived: bool,
}

/// The fields a template can use, for one task.
pub(crate) async fn task_fields(
    state: &AppState,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<BTreeMap<&'static str, String>, AppError> {
    let task = load_task(&state.pool, project_id, work_item_id).await?;
    let criteria = sqlx::query_as::<_, (String, bool)>(
        r#"
        SELECT text, completed FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY position
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&state.pool)
    .await?;
    let labels = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM work_item_active_labels
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY lower(name)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&state.pool)
    .await?;
    let pages = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT page.id, page.title
        FROM work_item_links link
        JOIN project_pages page
          ON page.id = link.target_id AND page.project_id = link.project_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND link.target_kind = 'page' AND page.archived_at IS NULL
        ORDER BY link.created_at, page.id
        LIMIT 20
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&state.pool)
    .await?;
    let guide = sqlx::query_scalar::<_, String>(
        r#"
        SELECT body FROM project_pages
        WHERE project_id = $1 AND is_guide AND archived_at IS NULL
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or_default();
    let origin = public_origin(state);
    let mut fields = BTreeMap::new();
    fields.insert("task.key", task.key);
    fields.insert("task.title", task.title);
    fields.insert(
        "task.description",
        shorten(&task.description, MAX_DESCRIPTION_CHARS),
    );
    fields.insert(
        "task.criteria",
        criteria
            .iter()
            .map(|(text, done)| format!("- [{}] {text}", if *done { "x" } else { " " }))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    fields.insert(
        "task.url",
        format!("{origin}/projects/{project_id}/work-items/{}", task.id),
    );
    fields.insert(
        "task.priority",
        if task.priority == "no_priority" {
            String::new()
        } else {
            task.priority
        },
    );
    fields.insert("task.type", task.type_name);
    fields.insert("task.labels", labels.join(", "));
    fields.insert(
        "task.pages",
        pages
            .iter()
            .map(|(id, title)| format!("- {title}: {origin}/projects/{project_id}/pages/{id}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    fields.insert("project.name", task.project_name);
    fields.insert("project.key", task.project_key);
    fields.insert("project.guide", shorten(&guide, MAX_GUIDE_CHARS));
    let skills = sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT id, title, skill_description, body FROM project_pages
        WHERE project_id = $1 AND skill_description IS NOT NULL AND archived_at IS NULL
        ORDER BY lower(title), id
        LIMIT 200
        "#,
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    fields.insert(
        "project.skills",
        skills_field(&skills, &format!("{origin}/projects/{project_id}/pages")),
    );
    Ok(fields)
}

/// The project's skills for a prompt: each one's name and when to use it,
/// with its instructions, all within `MAX_SKILLS_CHARS` (descriptions and
/// links count too, so many skills can't push the task out of the prompt).
/// Past what fits in full, a skill is its name and link; past that, a count
/// and the pages link. A run may not be connected to Agent Workspace, so the
/// instructions travel with it while they fit.
fn skills_field(skills: &[(Uuid, String, String, String)], pages_url: &str) -> String {
    let mut budget = MAX_SKILLS_CHARS;
    let mut parts: Vec<String> = Vec::new();
    for (index, (page_id, title, description, body)) in skills.iter().enumerate() {
        let name = crate::memory::skill_name(title);
        let body = body.trim();
        let full = format!("### {name}\n\n{description}\n\n{body}");
        let linked = format!("### {name}\n\n{description} Instructions: {pages_url}/{page_id}");
        let bare = format!("### {name}\n\nInstructions: {pages_url}/{page_id}");
        let chosen = [(!body.is_empty()).then_some(full), Some(linked), Some(bare)]
            .into_iter()
            .flatten()
            .find(|part| part.chars().count() + 2 <= budget);
        match chosen {
            Some(part) => {
                budget -= part.chars().count() + 2;
                parts.push(part);
            }
            None => {
                parts.push(format!("{} more skills: {pages_url}", skills.len() - index));
                break;
            }
        }
    }
    parts.join("\n\n")
}

async fn load_task(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<TaskRow, AppError> {
    sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT item.id,
               project.project_key || '-' || item.number::text AS key,
               item.title, item.description, item.priority,
               item_type.name AS type_name,
               project.name AS project_name, project.project_key,
               item.archived_at IS NOT NULL AS archived
        FROM task_rows item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_types item_type
          ON item_type.project_id = item.project_id AND item_type.id = item.type_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("work_item"))
}

// ---------------------------------------------------------------------------
// Run defaults (S2 settings applied to runs)

/// The run settings that apply to one person in one project, as far as
/// someone chose them (built-in defaults are left to the harness).
pub(crate) struct RunDefaults {
    pub organization_id: Uuid,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub speed: Option<String>,
    pub attempts: Option<u8>,
    pub repository_id: Option<Uuid>,
    /// The project's default environment, while it is active.
    pub environment: Option<EnvironmentRow>,
    pub template: String,
    pub effective: settings::EffectiveSettings,
}

impl RunDefaults {
    /// The harness runs use unless a launch names one: the default
    /// environment's, else the harness setting.
    pub(crate) fn preferred_harness(&self) -> Option<String> {
        self.environment
            .as_ref()
            .map(|environment| environment.harness.clone())
            .or_else(|| self.harness.clone())
    }
}

fn chosen<T: serde::de::DeserializeOwned>(
    effective: &settings::EffectiveSettings,
    key: &str,
) -> Option<T> {
    effective
        .get(key)
        .filter(|resolved| resolved.source != settings::Source::Default)?;
    effective.value(key)
}

pub(crate) async fn run_defaults(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<RunDefaults, AppError> {
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(AppError::NotFound("project"))?;
    let effective = settings::resolve(
        &mut *connection,
        settings::SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: Some(actor_id),
        },
        &BTreeMap::new(),
    )
    .await?;
    let environment = match effective
        .string("run.environment")
        .and_then(|id| Uuid::parse_str(&id).ok())
    {
        Some(id) => environments::active_environment(&mut *connection, project_id, id).await?,
        None => None,
    };
    let not_default = |value: Option<String>| value.filter(|value| value != "default");
    Ok(RunDefaults {
        organization_id,
        harness: chosen(&effective, "run.harness"),
        model: chosen(&effective, "run.model"),
        reasoning: not_default(chosen(&effective, "run.reasoning")),
        speed: not_default(chosen(&effective, "run.speed")),
        attempts: chosen(&effective, "run.attempts"),
        repository_id: chosen::<String>(&effective, "run.repository")
            .and_then(|id| Uuid::parse_str(&id).ok()),
        environment,
        template: effective.string("launch.template").unwrap_or_default(),
        effective,
    })
}

fn schema_has(manifest: &AdapterManifest, property: &str) -> bool {
    manifest
        .options_schema
        .get("properties")
        .and_then(|properties| properties.get(property))
        .is_some()
}

/// The options an environment gives a run on its harness.
pub(crate) fn environment_options(
    environment: &EnvironmentRow,
    manifest: &AdapterManifest,
) -> Map<String, Value> {
    let mut options = Map::new();
    if environment.harness != manifest.kind {
        return options;
    }
    if let Some(id) = &environment.provider_environment_id
        && schema_has(manifest, "environmentId")
    {
        options.insert("environmentId".to_owned(), json!(id));
    }
    if let Some(branch) = &environment.branch
        && schema_has(manifest, "branch")
    {
        options.insert("branch".to_owned(), json!(branch));
    }
    options
}

/// The account's saved run defaults with the person's and project's
/// settings over them, as the lenient defaults `RunConfig::resolve` takes.
pub(crate) fn merged_defaults(
    account_defaults: &Value,
    defaults: &RunDefaults,
    manifest: &AdapterManifest,
) -> Value {
    let mut config = RunConfig::from_value(account_defaults);
    if let Some(model) = &defaults.model {
        config.model = Some(model.clone());
    }
    if let Some(reasoning) = &defaults.reasoning {
        config.reasoning = Some(reasoning.clone());
    }
    if let Some(speed) = &defaults.speed {
        config.speed = Some(speed.clone());
    }
    if let Some(attempts) = defaults.attempts {
        config.attempts = Some(attempts.min(manifest.run_options.max_attempts.max(1)));
    }
    if config
        .attempts
        .is_some_and(|attempts| attempts > manifest.run_options.max_attempts.max(1))
    {
        config.attempts = None;
    }
    if let Some(environment) = &defaults.environment {
        config
            .options
            .extend(environment_options(environment, manifest));
    }
    serde_json::to_value(config).unwrap_or_else(|_| account_defaults.clone())
}

// ---------------------------------------------------------------------------
// Who launches

/// Whoever a launch runs as: the person, through the browser, a connected
/// app, or later from the queue.
#[derive(Clone, Debug)]
pub(crate) struct LaunchCaller {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    pub via_grant_id: Option<Uuid>,
    pub mcp: bool,
    pub session_id: Option<Uuid>,
    pub request_id: Option<String>,
}

impl LaunchCaller {
    pub(crate) fn browser(
        principal: &AuthenticatedActor,
        headers: &HeaderMap,
    ) -> Result<Self, AppError> {
        Ok(Self {
            actor_id: principal.actor_id,
            actor_kind: environments::principal_kind(principal)?,
            via_grant_id: None,
            mcp: false,
            session_id: Some(principal.session_id),
            request_id: headers
                .get(crate::REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= 128)
                .map(str::to_owned),
        })
    }

    fn authentication_class(&self) -> &'static str {
        if self.mcp {
            "mcp_bearer"
        } else if self.session_id.is_some() {
            "password_session"
        } else {
            "system"
        }
    }
}

impl RunActor for LaunchCaller {
    fn actor_id(&self) -> Uuid {
        self.actor_id
    }
    fn actor_kind(&self) -> Result<ActorKind, AppError> {
        Ok(self.actor_kind)
    }
    fn is_mcp(&self) -> bool {
        self.mcp
    }
}

/// The caller of a launch's start, with where the launch came from: a run
/// started for a launch a person's app queued records it, so it stays
/// within that app's limit if it waits for a slot (`scheduler`). Only the
/// record changes; who may start the run is the caller's as before.
struct LaunchStarter<'a> {
    caller: &'a LaunchCaller,
    through_app: bool,
}

impl RunActor for LaunchStarter<'_> {
    fn actor_id(&self) -> Uuid {
        self.caller.actor_id
    }
    fn actor_kind(&self) -> Result<ActorKind, AppError> {
        Ok(self.caller.actor_kind)
    }
    fn is_mcp(&self) -> bool {
        self.caller.mcp
    }
    fn made_through_app(&self) -> bool {
        self.through_app
    }
}

async fn audit(
    executor: impl PgExecutor<'_>,
    caller: &LaunchCaller,
    project_id: Uuid,
    action: &'static str,
    launch_id: Uuid,
    metadata: Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'launch', $7, 'succeeded', $8, $9)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(caller.actor_id)
    .bind(project_id)
    .bind(caller.session_id)
    .bind(caller.authentication_class())
    .bind(action)
    .bind(launch_id)
    .bind(&caller.request_id)
    .bind(metadata)
    .execute(executor)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Launch records

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LaunchOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u8>,
}

impl LaunchOverrides {
    fn check(&self) -> Result<(), AppError> {
        RunConfig {
            harness: self.harness.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
            speed: self.speed.clone(),
            attempts: self.attempts,
            ..Default::default()
        }
        .check_shape()
        .map_err(|error| AppError::Validation(error.0))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StartWhen {
    #[default]
    Now,
    At,
    WindowReset,
    BlockersDone,
}

impl StartWhen {
    fn as_str(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::At => "at",
            Self::WindowReset => "window_reset",
            Self::BlockersDone => "blockers_done",
        }
    }
}

#[derive(Clone, Debug, FromRow)]
struct LaunchRow {
    id: Uuid,
    project_id: Uuid,
    work_item_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    requested_by_actor_id: Uuid,
    via_oauth_grant_id: Option<Uuid>,
    environment_id: Option<Uuid>,
    overrides: Value,
    prompt: Option<String>,
    provider_account_id: Option<Uuid>,
    status_reason: Option<String>,
    agent_profile_id: Option<Uuid>,
    source: String,
    made_through_app: bool,
}

impl LaunchRow {
    /// Queued by a person's app or script (directly, in a batch, from a
    /// message it sent or through an automation it made): the run it starts
    /// stays within that app's limit while it waits for a slot. Source mcp
    /// covers rows from before the flag was set on every insert, as in
    /// `drain`.
    fn through_app(&self) -> bool {
        self.made_through_app || self.source == "mcp"
    }
}

const LAUNCH_ROW: &str = r#"
    id, project_id, work_item_id, conversation_id,
    requested_by_actor_id, via_oauth_grant_id, environment_id,
    overrides, prompt, provider_account_id, status_reason, agent_profile_id,
    source, made_through_app
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchTask {
    pub id: Uuid,
    pub key: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchView {
    pub id: Uuid,
    pub work_item: Option<LaunchTask>,
    pub conversation_id: Option<Uuid>,
    pub requested_by_actor_id: Uuid,
    pub requested_by_name: String,
    /// button, dialog, queue, tag or mcp.
    pub source: String,
    pub environment_id: Option<Uuid>,
    pub environment_name: Option<String>,
    pub harness: Option<String>,
    pub harness_name: Option<String>,
    /// now, at, window_reset or blockers_done.
    pub start_when: String,
    pub start_at: Option<DateTime<Utc>>,
    /// queued, started, cancelled or failed.
    pub state: String,
    /// Why it waits, or why it was cancelled or failed.
    pub status_reason: Option<String>,
    pub agent_run_id: Option<Uuid>,
    pub run_state: Option<String>,
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub can_cancel: bool,
}

#[derive(FromRow)]
struct LaunchViewRow {
    id: Uuid,
    work_item_id: Option<Uuid>,
    work_item_key: Option<String>,
    work_item_title: Option<String>,
    conversation_id: Option<Uuid>,
    requested_by_actor_id: Uuid,
    requested_by_name: String,
    source: String,
    environment_id: Option<Uuid>,
    environment_name: Option<String>,
    harness: Option<String>,
    start_when: String,
    start_at: Option<DateTime<Utc>>,
    state: String,
    status_reason: Option<String>,
    agent_run_id: Option<Uuid>,
    run_state: Option<String>,
    position: i64,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
}

const LAUNCH_VIEW_SELECT: &str = r#"
    SELECT launch.id, launch.work_item_id,
           project.project_key || '-' || item.number::text AS work_item_key,
           item.title AS work_item_title,
           launch.conversation_id, launch.requested_by_actor_id,
           COALESCE(membership.display_name, 'Someone') AS requested_by_name,
           launch.source, launch.environment_id, environment.name AS environment_name,
           COALESCE(launch.overrides ->> 'harness', environment.harness) AS harness,
           launch.start_when, launch.start_at, launch.state, launch.status_reason,
           launch.agent_run_id, run.state AS run_state, launch.position,
           launch.created_at, launch.started_at
    FROM launches launch
    JOIN projects project ON project.id = launch.project_id
    LEFT JOIN task_rows item
      ON item.project_id = launch.project_id AND item.id = launch.work_item_id
    LEFT JOIN project_memberships membership
      ON membership.project_id = launch.project_id
     AND membership.actor_id = launch.requested_by_actor_id
    LEFT JOIN project_environments environment
      ON environment.project_id = launch.project_id AND environment.id = launch.environment_id
    LEFT JOIN agent_runs run ON run.id = launch.agent_run_id
"#;

fn launch_view(
    state: &AppState,
    row: LaunchViewRow,
    viewer_id: Uuid,
    can_manage: bool,
) -> LaunchView {
    let harness_name = row
        .harness
        .as_deref()
        .map(|harness| crate::agent_sessions::harness_name(state, harness));
    LaunchView {
        can_cancel: row.state == "queued" && (row.requested_by_actor_id == viewer_id || can_manage),
        id: row.id,
        work_item: match (row.work_item_id, row.work_item_key, row.work_item_title) {
            (Some(id), Some(key), Some(title)) => Some(LaunchTask { id, key, title }),
            _ => None,
        },
        conversation_id: row.conversation_id,
        requested_by_actor_id: row.requested_by_actor_id,
        requested_by_name: row.requested_by_name,
        source: row.source,
        environment_id: row.environment_id,
        environment_name: row.environment_name,
        harness: row.harness,
        harness_name,
        start_when: row.start_when,
        start_at: row.start_at,
        state: row.state,
        status_reason: row.status_reason,
        agent_run_id: row.agent_run_id,
        run_state: row.run_state,
        position: row.position,
        created_at: row.created_at,
        started_at: row.started_at,
    }
}

async fn can_manage(state: &AppState, caller: &LaunchCaller, project_id: Uuid) -> bool {
    memory::project_access(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::SettingsManage,
    )
    .await
    .is_ok()
}

/// Whether the caller may see the launched tasks' keys and titles here: their
/// role and, through an app, its current limit. Launch views leave the task
/// out otherwise; the launch itself stays visible.
async fn sees_tasks(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        !crate::search::projects_showing_tasks(&state.pool, caller.actor_id, &[project_id])
            .await?
            .is_empty(),
    )
}

/// Launch views by id, for every response that returns launches (cancel,
/// a launch's result and its replay, a task's launch plan), with the task
/// left out for someone who may not see tasks here, as in `list_for`.
async fn views_by_id(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<LaunchView>, AppError> {
    let manage = can_manage(state, caller, project_id).await;
    let sees_tasks = sees_tasks(state, caller, project_id).await?;
    let rows = sqlx::query_as::<_, LaunchViewRow>(&format!(
        "{LAUNCH_VIEW_SELECT} WHERE launch.project_id = $1 AND launch.id = ANY($2) \
         ORDER BY launch.position"
    ))
    .bind(project_id)
    .bind(ids)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut view = launch_view(state, row, caller.actor_id, manage);
            if !sees_tasks {
                view.work_item = None;
            }
            view
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Blockers, accounts and capacity

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Blocker {
    pub id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub title: String,
}

/// The tasks blocking this one that aren't done yet.
pub(crate) async fn open_blockers(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<Blocker>, AppError> {
    Ok(sqlx::query_as::<_, Blocker>(
        r#"
        SELECT blocker.id, blocker.project_id,
               project.project_key || '-' || blocker.number::text AS key,
               blocker.title
        FROM work_item_dependencies dependency
        JOIN task_rows blocker
          ON blocker.project_id = dependency.blocking_project_id
         AND blocker.id = dependency.blocking_work_item_id
        JOIN projects project ON project.id = blocker.project_id
        JOIN work_item_statuses status
          ON status.project_id = blocker.project_id AND status.id = blocker.status_id
        WHERE dependency.project_id = $1 AND dependency.blocked_work_item_id = $2
          AND blocker.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
        ORDER BY blocker.project_id, blocker.number
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(executor)
    .await?)
}

#[derive(Clone, Debug, FromRow)]
pub(crate) struct LaunchAccount {
    pub id: Uuid,
    pub name: String,
    pub adapter_kind: String,
    pub home_path: String,
    pub max_concurrent_runs: i16,
}

impl LaunchAccount {
    fn handle(&self) -> crate::provider::AccountHandle {
        crate::provider::AccountHandle {
            id: self.id,
            home: std::path::PathBuf::from(&self.home_path),
            max_concurrent_runs: u16::try_from(self.max_concurrent_runs).unwrap_or(1),
        }
    }
}

/// The signed-in account a run for this person would use: the one they
/// picked, else their preferred profile's, as `select_run_setup_for` picks.
pub(crate) async fn launch_account(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    actor_id: Uuid,
    harness: Option<&str>,
    account_id: Option<Uuid>,
) -> Result<Option<LaunchAccount>, AppError> {
    Ok(sqlx::query_as::<_, LaunchAccount>(
        r#"
        SELECT account.id, account.name, account.adapter_kind, account.home_path,
               account.max_concurrent_runs
        FROM execution_profiles profile
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        JOIN provider_connections connection
          ON connection.id = profile.provider_connection_id
         AND connection.project_id = profile.project_id
         AND connection.enabled
        WHERE profile.project_id = $1 AND profile.owner_actor_id = $2
          AND profile.status = 'available' AND profile.account_state = 'authenticated'
          AND ($3::text IS NULL OR account.adapter_kind = $3)
          AND ($4::uuid IS NULL OR account.id = $4)
        ORDER BY profile.preferred DESC, profile.created_at, profile.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .bind(harness)
    .bind(account_id)
    .fetch_optional(executor)
    .await?)
}

/// The person's signed-in accounts for runs in this project.
async fn launch_accounts(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<Vec<LaunchAccount>, AppError> {
    Ok(sqlx::query_as::<_, LaunchAccount>(
        r#"
        SELECT DISTINCT ON (account.id)
               account.id, account.name, account.adapter_kind, account.home_path,
               account.max_concurrent_runs
        FROM execution_profiles profile
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        JOIN provider_connections connection
          ON connection.id = profile.provider_connection_id
         AND connection.project_id = profile.project_id
         AND connection.enabled
        WHERE profile.project_id = $1 AND profile.owner_actor_id = $2
          AND profile.status = 'available' AND profile.account_state = 'authenticated'
        ORDER BY account.id
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_all(executor)
    .await?)
}

/// Runs holding or waiting for one of the account's slots.
pub(crate) async fn account_load(
    executor: impl PgExecutor<'_>,
    account_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        r#"
        SELECT count(*)
        FROM agent_runs run
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        WHERE profile.provider_account_id = $1 AND run.state IN {ACTIVE_RUN_STATES}
        "#
    ))
    .bind(account_id)
    .fetch_one(executor)
    .await?)
}

async fn thread_busy(
    executor: impl PgExecutor<'_>,
    conversation_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS (SELECT 1 FROM agent_runs WHERE conversation_id = $1 AND state IN {ACTIVE_RUN_STATES})"
    ))
    .bind(conversation_id)
    .fetch_one(executor)
    .await?)
}

/// When the account's usage window resets, from the harness.
async fn window_reset(
    state: &AppState,
    account: &LaunchAccount,
) -> Result<Option<DateTime<Utc>>, AppError> {
    let Ok(mut session) = state
        .supervisors
        .acquire_control(&account.adapter_kind, account.handle())
        .await
    else {
        return Ok(None);
    };
    let limits = session.read_rate_limits().await.ok();
    let windows = limits
        .into_iter()
        .flat_map(|limits| [limits.primary, limits.secondary])
        .flatten()
        .filter(|window| window.resets_at.is_some())
        .max_by_key(|window| window.used_percent);
    Ok(windows
        .and_then(|window| window.resets_at)
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0)))
}

// ---------------------------------------------------------------------------
// The launch plan (what the dialog shows)

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanQuery {
    environment_id: Option<Uuid>,
    harness: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelChoice {
    id: String,
    display_name: String,
    reasoning_levels: Vec<String>,
    speeds: Vec<String>,
    is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountChoice {
    id: Uuid,
    name: String,
    harness: String,
    running: i64,
    max_concurrent_runs: i16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessPlan {
    /// A connected runner: launches go to its agent and need no account.
    connected_runner: bool,
    /// The runner agent's name in this project, for a connected runner.
    runner_name: Option<String>,
    kind: String,
    display_name: String,
    runs_on: crate::provider::manifest::RunLocation,
    max_attempts: u8,
    models: Vec<ModelChoice>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingChoice {
    value: Value,
    source: settings::Source,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchPlan {
    work_item: LaunchTask,
    blockers: Vec<Blocker>,
    environments: Vec<EnvironmentView>,
    default_environment_id: Option<Uuid>,
    environment_id: Option<Uuid>,
    harness: Option<String>,
    harnesses: Vec<HarnessPlan>,
    accounts: Vec<AccountChoice>,
    /// run.model, run.reasoning, run.speed and run.attempts with where each
    /// came from.
    settings: BTreeMap<&'static str, SettingChoice>,
    prompt: String,
    template_source: settings::Source,
    /// Started or queued launches of this task, newest first.
    launches: Vec<LaunchView>,
    thread_id: Option<Uuid>,
}

async fn launch_plan(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<PlanQuery>,
) -> Result<Json<LaunchPlan>, AppError> {
    let caller = LaunchCaller::browser(&principal, &HeaderMap::new())?;
    Ok(Json(
        plan_for(&state, &caller, project_id, work_item_id, query).await?,
    ))
}

async fn plan_for(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
    work_item_id: Uuid,
    query: PlanQuery,
) -> Result<LaunchPlan, AppError> {
    environments::authorize_project(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::WorkItemView,
    )
    .await?;
    let task = load_task(&state.pool, project_id, work_item_id).await?;
    let defaults = {
        let mut connection = state.pool.acquire().await?;
        run_defaults(&mut connection, project_id, caller.actor_id).await?
    };
    let list =
        environments::list_for(state, caller.actor_id, caller.actor_kind, project_id).await?;
    let environment_id = query
        .environment_id
        .filter(|id| {
            list.environments
                .iter()
                .any(|environment| environment.id == *id)
        })
        .or(list.default_environment_id);
    let environment = list
        .environments
        .iter()
        .find(|environment| Some(environment.id) == environment_id);
    let harness = query
        .harness
        .clone()
        .or_else(|| environment.map(|environment| environment.harness.clone()))
        .or_else(|| defaults.harness.clone());
    let mut harnesses = Vec::new();
    for manifest in state.supervisors.manifests_in(defaults.organization_id) {
        let catalog =
            crate::provider::catalog::run_catalog(&state.pool, defaults.organization_id, &manifest)
                .await?;
        let runner = state.supervisors.runner_for(&manifest.kind);
        let runner_name = match runner {
            Some(actor_id) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2 AND enabled",
                )
                .bind(project_id)
                .bind(actor_id)
                .fetch_optional(&state.pool)
                .await?
            }
            None => None,
        };
        harnesses.push(HarnessPlan {
            connected_runner: runner.is_some(),
            runner_name,
            kind: manifest.kind.clone(),
            display_name: manifest.display_name.clone(),
            runs_on: manifest.runs_on,
            max_attempts: manifest.run_options.max_attempts.max(1),
            models: catalog
                .into_iter()
                .filter(|model| !model.hidden)
                .map(|model| ModelChoice {
                    id: model.id,
                    display_name: model.display_name,
                    reasoning_levels: model.reasoning_levels,
                    speeds: model.speeds.into_iter().map(|speed| speed.id).collect(),
                    is_default: model.is_default,
                })
                .collect(),
        });
    }
    let mut accounts = Vec::new();
    for account in launch_accounts(&state.pool, project_id, caller.actor_id).await? {
        accounts.push(AccountChoice {
            running: account_load(&state.pool, account.id).await?,
            id: account.id,
            name: account.name,
            harness: account.adapter_kind,
            max_concurrent_runs: account.max_concurrent_runs,
        });
    }
    let mut settings = BTreeMap::new();
    for key in ["run.model", "run.reasoning", "run.speed", "run.attempts"] {
        if let Some(resolved) = defaults.effective.get(key) {
            settings.insert(
                resolved.key,
                SettingChoice {
                    value: resolved.value.clone(),
                    source: resolved.source,
                },
            );
        }
    }
    let mut fields = task_fields(state, project_id, work_item_id).await?;
    let harness_kind = harness.clone().unwrap_or_default();
    fields.insert(
        "harness",
        harness
            .as_deref()
            .map(|harness| crate::agent_sessions::harness_name(state, harness))
            .unwrap_or_default(),
    );
    fields.insert(
        "environment",
        environment
            .map(|environment| environment.name.clone())
            .unwrap_or_default(),
    );
    let prompt = render(&defaults.template, &fields, &harness_kind);
    let template_source = defaults
        .effective
        .get("launch.template")
        .map_or(settings::Source::Default, |resolved| resolved.source);
    let launch_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM launches
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY created_at DESC
        LIMIT 5
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&state.pool)
    .await?;
    let mut launches = views_by_id(state, caller, project_id, &launch_ids).await?;
    launches.sort_by_key(|launch| std::cmp::Reverse(launch.created_at));
    let thread_id =
        reusable_thread(&state.pool, project_id, work_item_id, caller.actor_id, None).await?;
    Ok(LaunchPlan {
        blockers: open_blockers(&state.pool, project_id, work_item_id).await?,
        work_item: LaunchTask {
            id: task.id,
            key: task.key,
            title: task.title,
        },
        environments: list.environments,
        default_environment_id: list.default_environment_id,
        environment_id,
        harness,
        harnesses,
        accounts,
        settings,
        prompt,
        template_source,
        launches,
        thread_id,
    })
}

// ---------------------------------------------------------------------------
// Creating launches

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LaunchRequest {
    pub work_item_ids: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<Uuid>,
    #[serde(default)]
    pub overrides: LaunchOverrides,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_account_id: Option<Uuid>,
    /// An edited prompt, for a single task. Left out, the template builds it
    /// when the launch starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub start_when: StartWhen,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    /// Queue even a single task that could start now.
    #[serde(default)]
    pub queue: bool,
    /// Whether the person used the dialog (for the record).
    #[serde(default)]
    pub dialog: bool,
    /// The agent (A9) that does the work, with its harness, environment,
    /// model settings and instructions; left out, the project's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchResult {
    pub launches: Vec<LaunchView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredLaunchResult {
    launch_ids: Vec<Uuid>,
}

async fn create_launches(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<LaunchRequest>,
) -> Result<(StatusCode, Json<LaunchResult>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = LaunchCaller::browser(&principal, &headers)?;
    let source = if request.dialog { "dialog" } else { "button" };
    let result = launch_tasks(
        &state,
        &caller,
        project_id,
        request,
        source,
        IdempotencyKey::from_headers(&headers),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// Starts or queues a launch for each task. A single task with no start
/// condition starts at once; anything else waits in the queue.
pub(crate) async fn launch_tasks(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
    mut request: LaunchRequest,
    source: &'static str,
    idempotency_key: Result<IdempotencyKey, AppError>,
) -> Result<LaunchResult, AppError> {
    let organization_id = environments::authorize_project(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::WorkerInvoke,
    )
    .await?;
    // A launch names its thread after the task and writes the task's title,
    // description and criteria into the run's prompt, which the thread
    // shows: only for someone who sees tasks here (their role, and an app's
    // agent limit in its call).
    environments::authorize_project(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::WorkItemView,
    )
    .await?;
    if request.work_item_ids.is_empty() || request.work_item_ids.len() > MAX_TASKS_PER_LAUNCH {
        return Err(AppError::Validation("workItemIds"));
    }
    let mut seen = std::collections::HashSet::new();
    request.work_item_ids.retain(|id| seen.insert(*id));
    request.overrides.check()?;
    if let Some(prompt) = &mut request.prompt {
        *prompt = prompt.trim().to_owned();
        if request.work_item_ids.len() != 1
            || prompt.is_empty()
            || prompt.chars().count() > MAX_PROMPT_CHARS
            || prompt
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(AppError::Validation("prompt"));
        }
    }
    match (request.start_when, request.start_at) {
        (StartWhen::At, Some(at)) => {
            if at > Utc::now() + chrono::Duration::days(30) {
                return Err(AppError::Validation("startAt"));
            }
        }
        (StartWhen::At, None) => return Err(AppError::Validation("startAt")),
        (_, Some(_)) => return Err(AppError::Validation("startAt")),
        _ => {}
    }
    let idempotency_key = idempotency_key?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = idempotency::resource_bound_scope("project", project_id, "launch_create")?;

    // Everything a launch starts with is checked now, so a bad choice is
    // refused here rather than failing later in the queue.
    let defaults = {
        let mut connection = state.pool.acquire().await?;
        run_defaults(&mut connection, project_id, caller.actor_id).await?
    };
    let profile = match request.agent_profile_id {
        Some(profile_id) => Some(
            crate::agent_profiles::profile_settings(&state.pool, profile_id)
                .await?
                .ok_or(AppError::NotFound("agent_profile"))?,
        ),
        None => None,
    };
    if let Some(profile) = &profile
        && sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM agent_profiles WHERE id = $1")
            .bind(profile.id)
            .fetch_one(&state.pool)
            .await?
            != project_id
    {
        return Err(AppError::NotFound("agent_profile"));
    }
    let environment = match request
        .environment_id
        .or_else(|| profile.as_ref().and_then(|profile| profile.environment_id))
    {
        Some(id) => Some(
            environments::active_environment(&state.pool, project_id, id)
                .await?
                .ok_or(AppError::NotFound("environment"))?,
        ),
        None if profile
            .as_ref()
            .is_some_and(|profile| profile.harness.is_some()) =>
        {
            None
        }
        None => defaults.environment.clone(),
    };
    if request.overrides.harness.is_none()
        && let Some(harness) = profile.as_ref().and_then(|profile| profile.harness.clone())
    {
        request.overrides.harness = Some(harness);
    }
    if let (Some(environment), Some(harness)) = (&environment, &request.overrides.harness)
        && &environment.harness != harness
    {
        return Err(AppError::Invalid(
            "the environment runs on a different harness; pick one or the other",
        ));
    }
    let harness = request.overrides.harness.clone().or_else(|| {
        environment
            .as_ref()
            .map(|environment| environment.harness.clone())
    });
    if let Some(harness) = &harness
        && state
            .supervisors
            .manifest_in(defaults.organization_id, harness)
            .is_none()
    {
        return Err(AppError::Validation("harness"));
    }
    let start_at = match request.start_when {
        StartWhen::At => request.start_at,
        StartWhen::WindowReset => {
            let account = launch_account(
                &state.pool,
                project_id,
                caller.actor_id,
                harness.as_deref().or(defaults.harness.as_deref()),
                request.provider_account_id,
            )
            .await?
            .ok_or(AppError::ExecutionProfileRequired)?;
            Some(
                window_reset(state, &account)
                    .await?
                    .ok_or(AppError::Invalid(
                        "that account doesn't say when its usage resets; pick a time instead",
                    ))?,
            )
        }
        _ => None,
    };
    if let Some(account_id) = request.provider_account_id
        && launch_account(
            &state.pool,
            project_id,
            caller.actor_id,
            None,
            Some(account_id),
        )
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("provider_account"));
    }

    let mut transaction = state.pool.begin().await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        caller.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let stored: StoredLaunchResult = replay.decode()?;
            transaction.commit().await?;
            return Ok(LaunchResult {
                launches: views_by_id(state, caller, project_id, &stored.launch_ids).await?,
            });
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("launches:{project_id}"))
        .execute(&mut *transaction)
        .await?;
    let immediate =
        request.work_item_ids.len() == 1 && request.start_when == StartWhen::Now && !request.queue;
    let mut launch_ids = Vec::new();
    for work_item_id in &request.work_item_ids {
        let task = load_task(&mut *transaction, project_id, *work_item_id).await?;
        if task.archived {
            return Err(AppError::Conflict("work_item", "archived"));
        }
        let id = Uuid::now_v7();
        let reason = if immediate {
            None
        } else {
            Some("Waiting in the queue.")
        };
        sqlx::query(
            r#"
            INSERT INTO launches (
                id, organization_id, project_id, work_item_id, requested_by_actor_id,
                via_oauth_grant_id, source, environment_id, overrides, prompt,
                start_when, start_at, provider_account_id, status_reason, position,
                agent_profile_id, made_through_app
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                (SELECT COALESCE(max(position), 0) + 1 FROM launches WHERE project_id = $3),
                $15, $16
            )
            "#,
        )
        .bind(id)
        .bind(organization_id)
        .bind(project_id)
        .bind(task.id)
        .bind(caller.actor_id)
        .bind(caller.via_grant_id)
        .bind(if immediate || request.work_item_ids.len() == 1 {
            source
        } else {
            "queue"
        })
        .bind(environment.as_ref().map(|environment| environment.id))
        .bind(
            serde_json::to_value(&request.overrides)
                .map_err(|_| AppError::Internal("launch overrides"))?,
        )
        .bind(&request.prompt)
        .bind(request.start_when.as_str())
        .bind(start_at)
        .bind(request.provider_account_id)
        .bind(reason)
        .bind(request.agent_profile_id)
        // A person's app or script queued it (whatever the display source),
        // directly or through an automation it made, whose actions run
        // within the app's limit: it starts only within that limit when the
        // queue drains.
        .bind(
            caller.actor_kind == ActorKind::Human
                && (caller.mcp
                    || crate::policy::current_agent_limit(
                        caller.actor_id,
                        crate::policy::MembershipScope::Project(project_id),
                    )
                    .is_some()),
        )
        .execute(&mut *transaction)
        .await?;
        audit(
            &mut *transaction,
            caller,
            project_id,
            "launch.created",
            id,
            json!({
                "workItemId": task.id,
                "environmentId": environment.as_ref().map(|environment| environment.id),
                "startWhen": request.start_when.as_str(),
            }),
        )
        .await?;
        launch_ids.push(id);
    }
    if !immediate {
        kick(&mut *transaction).await?;
    }
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::CREATED,
        &StoredLaunchResult {
            launch_ids: launch_ids.clone(),
        },
    )
    .await?;
    transaction.commit().await?;

    if immediate && let Some(launch_id) = launch_ids.first() {
        let launch = load_launch(&state.pool, *launch_id).await?;
        start_or_record(state, caller, &launch).await?;
    }
    Ok(LaunchResult {
        launches: views_by_id(state, caller, project_id, &launch_ids).await?,
    })
}

async fn load_launch(
    executor: impl PgExecutor<'_>,
    launch_id: Uuid,
) -> Result<LaunchRow, AppError> {
    sqlx::query_as::<_, LaunchRow>(&format!("SELECT {LAUNCH_ROW} FROM launches WHERE id = $1"))
        .bind(launch_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("launch"))
}

// ---------------------------------------------------------------------------
// Starting a launch

/// A linked thread of the task the person can see, with no run going, that
/// isn't bound to another repository.
async fn reusable_thread(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    repository_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(&format!(
        r#"
        SELECT conversation.id
        FROM work_item_conversation_links link
        JOIN conversations conversation
          ON conversation.project_id = link.project_id
         AND conversation.id = link.conversation_id
         AND conversation.kind = 'thread'
         AND conversation.archived_at IS NULL
        JOIN conversations parent
          ON parent.id = conversation.parent_channel_id
         AND parent.kind = 'channel'
         AND parent.archived_at IS NULL
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND conversation_visible_to(conversation.id, $3)
          AND ($4::uuid IS NULL OR conversation.repository_id IS NULL
               OR conversation.repository_id = $4)
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs run
              WHERE run.conversation_id = conversation.id
                AND run.state IN {ACTIVE_RUN_STATES}
          )
        ORDER BY link.created_at DESC
        LIMIT 1
        "#
    ))
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .bind(repository_id)
    .fetch_optional(executor)
    .await?)
}

/// The task's thread for this launch: a linked one that's free, or a new one
/// in the channel the task's threads live in (else the project's first
/// channel the person can see).
async fn task_thread(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
    work_item_id: Uuid,
    repository_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if let Some(thread_id) = reusable_thread(
        &state.pool,
        project_id,
        work_item_id,
        caller.actor_id,
        repository_id,
    )
    .await?
    {
        return Ok(thread_id);
    }
    memory::project_access(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::ThreadCreate,
    )
    .await?;
    let mut transaction = state.pool.begin().await?;
    let task = load_task(&mut *transaction, project_id, work_item_id).await?;
    let channel_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT channel.id
        FROM conversations channel
        LEFT JOIN LATERAL (
            SELECT max(link.created_at) AS linked_at
            FROM work_item_conversation_links link
            JOIN conversations thread
              ON thread.id = link.conversation_id AND thread.parent_channel_id = channel.id
            WHERE link.project_id = $1 AND link.work_item_id = $2
        ) linked ON true
        WHERE channel.project_id = $1 AND channel.kind = 'channel'
          AND channel.archived_at IS NULL
          AND conversation_visible_to(channel.id, $3)
        ORDER BY linked.linked_at DESC NULLS LAST, channel.created_at, channel.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(caller.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Invalid(
        "the project needs a channel you can see before a task can launch",
    ))?;
    let thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, $4)
        "#,
    )
    .bind(thread_id)
    .bind(project_id)
    .bind(channel_id)
    .bind(crate::work_items::worker_thread_name(
        &task.key,
        &task.title,
        thread_id,
    ))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        ) VALUES ($1, $2, $3, 0)
        "#,
    )
    .bind(project_id)
    .bind(thread_id)
    .bind(caller.actor_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO work_item_conversation_links (
            project_id, work_item_id, conversation_id, created_by_actor_id
        ) VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(thread_id)
    .bind(caller.actor_id)
    .execute(&mut *transaction)
    .await?;
    enqueue_typed(
        &mut transaction,
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: thread_id,
            kind: OutboxConversationKind::Thread,
            version: 1,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(thread_id)
}

/// What a person reads about a launch that couldn't start.
pub(crate) fn failure_reason(error: &AppError) -> String {
    match error {
        AppError::ExecutionProfileRequired => {
            "No signed-in provider account for this harness; sign in under your accounts and launch again.".to_owned()
        }
        AppError::RepositoryChoiceRequired { .. } => {
            "The project has several repositories; pick one in the environment or the project's run settings.".to_owned()
        }
        AppError::RepositoryBindingConflict => {
            "The task's thread works in a different repository than the environment.".to_owned()
        }
        AppError::NotFound("project") | AppError::Forbidden => {
            "The person who launched it can no longer start runs in this project.".to_owned()
        }
        AppError::NotFound(what) => format!("The {} is gone.", what.replace('_', " ")),
        AppError::Conflict(_, message) | AppError::Invalid(message) => {
            let mut message = message.to_string();
            if let Some(first) = message.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            format!("{message}.")
        }
        AppError::Validation(field) => format!("The run settings were refused ({field})."),
        AppError::ProjectArchived => "The project was archived.".to_owned(),
        _ => "The run couldn't start.".to_owned(),
    }
}

/// Starts a launch; whatever stops it is written on the launch instead of
/// returned, except errors worth retrying.
async fn start_or_record(
    state: &AppState,
    caller: &LaunchCaller,
    launch: &LaunchRow,
) -> Result<Option<Uuid>, AppError> {
    match start_launch(state, caller, launch).await {
        Ok(run_id) => Ok(Some(run_id)),
        Err(error @ (AppError::Database(_) | AppError::Internal(_))) => Err(error),
        Err(error) => {
            finish_launch(
                &state.pool,
                launch.id,
                "failed",
                &failure_reason(&error),
                None,
            )
            .await?;
            Ok(None)
        }
    }
}

async fn finish_launch(
    executor: impl PgExecutor<'_>,
    launch_id: Uuid,
    state: &str,
    reason: &str,
    cancelled_by: Option<Uuid>,
) -> Result<bool, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE launches
        SET state = $2, status_reason = $3, cancelled_by_actor_id = $4, updated_at = now()
        WHERE id = $1 AND state = 'queued'
        "#,
    )
    .bind(launch_id)
    .bind(state)
    .bind(reason.chars().take(500).collect::<String>())
    .bind(cancelled_by)
    .execute(executor)
    .await?
    .rows_affected()
        == 1)
}

async fn set_reason(
    executor: impl PgExecutor<'_>,
    launch: &LaunchRow,
    reason: &str,
) -> Result<(), AppError> {
    if launch.status_reason.as_deref() == Some(reason) {
        return Ok(());
    }
    sqlx::query(
        "UPDATE launches SET status_reason = $2, updated_at = now() WHERE id = $1 AND state = 'queued'",
    )
    .bind(launch.id)
    .bind(reason.chars().take(500).collect::<String>())
    .execute(executor)
    .await?;
    Ok(())
}

/// Builds the run from the launch and starts it in the task's thread (or
/// the thread a tag came from) as the person who launched it.
async fn start_launch(
    state: &AppState,
    caller: &LaunchCaller,
    launch: &LaunchRow,
) -> Result<Uuid, AppError> {
    let overrides: LaunchOverrides =
        serde_json::from_value(launch.overrides.clone()).unwrap_or_default();
    // An agent profile (A9) gives the launch its harness, model settings
    // and instructions wherever the launch itself didn't pick them.
    let profile = match launch.agent_profile_id {
        Some(profile_id) => Some(
            crate::agent_profiles::profile_settings(&state.pool, profile_id)
                .await?
                .ok_or(AppError::Invalid("that agent was turned off"))?,
        ),
        None => None,
    };
    let environment = match launch.environment_id {
        Some(id) => Some(
            environments::active_environment(&state.pool, launch.project_id, id)
                .await?
                .ok_or(AppError::NotFound("environment"))?,
        ),
        None => None,
    };
    let harness = overrides
        .harness
        .clone()
        .or_else(|| profile.as_ref().and_then(|profile| profile.harness.clone()))
        .or_else(|| {
            environment
                .as_ref()
                .map(|environment| environment.harness.clone())
        });
    let manifest = harness
        .as_deref()
        .and_then(|harness| state.supervisors.manifest(harness));
    let mut options = Map::new();
    if let (Some(environment), Some(manifest)) = (&environment, &manifest) {
        options = environment_options(environment, manifest);
    }
    if let (Some(profile), Some(manifest)) = (&profile, &manifest) {
        options.extend(profile.options(manifest));
    }
    let repository_id = environment
        .as_ref()
        .and_then(|environment| environment.repository_id);
    let thread_id = match (launch.conversation_id, launch.work_item_id) {
        (Some(thread_id), _) => thread_id,
        (None, Some(work_item_id)) => {
            task_thread(
                state,
                caller,
                launch.project_id,
                work_item_id,
                repository_id,
            )
            .await?
        }
        (None, None) => return Err(AppError::NotFound("work_item")),
    };
    let prompt = match (&launch.prompt, launch.work_item_id) {
        (Some(prompt), _) => prompt.clone(),
        (None, Some(work_item_id)) => {
            let template = {
                let mut connection = state.pool.acquire().await?;
                run_defaults(&mut connection, launch.project_id, caller.actor_id)
                    .await?
                    .template
            };
            let mut fields = task_fields(state, launch.project_id, work_item_id).await?;
            fields.insert(
                "harness",
                harness
                    .as_deref()
                    .map(|harness| crate::agent_sessions::harness_name(state, harness))
                    .unwrap_or_default(),
            );
            fields.insert(
                "environment",
                environment
                    .as_ref()
                    .map(|environment| environment.name.clone())
                    .unwrap_or_default(),
            );
            render(&template, &fields, harness.as_deref().unwrap_or_default())
        }
        (None, None) => return Err(AppError::Validation("prompt")),
    };
    if prompt.trim().is_empty() {
        return Err(AppError::Validation("prompt"));
    }
    let prompt = match &profile {
        Some(profile) => profile.prompt(manifest.as_deref(), &prompt),
        None => prompt,
    };
    if let Some(runner) = harness
        .as_deref()
        .and_then(|harness| state.supervisors.runner_for(harness))
    {
        return hand_off(
            state, caller, launch, thread_id, runner, &prompt, &overrides,
        )
        .await;
    }
    let config = RunConfig {
        harness,
        model: overrides
            .model
            .or_else(|| profile.as_ref().and_then(|profile| profile.model())),
        reasoning: overrides
            .reasoning
            .or_else(|| profile.as_ref().and_then(|profile| profile.reasoning())),
        speed: overrides
            .speed
            .or_else(|| profile.as_ref().and_then(|profile| profile.speed())),
        attempts: overrides.attempts,
        options,
        ..Default::default()
    };
    let started = crate::api::start_run_in(
        state,
        &LaunchStarter {
            caller,
            through_app: launch.through_app(),
        },
        launch.via_oauth_grant_id,
        thread_id,
        StartRunRequest {
            prompt,
            repository_id,
            config: Some(config).filter(|config| *config != RunConfig::default()),
            provider_account_id: launch.provider_account_id,
            agent_profile_id: launch.agent_profile_id,
        },
        IdempotencyKey::parse(&format!("launch-{}", launch.id)),
    )
    .await;
    let run_id = match started {
        Ok((_, response)) => response.run_id,
        // A thread that's busy gets a thread of its own next time.
        Err(error) => return Err(error),
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE launches
        SET state = 'started', agent_run_id = $2, conversation_id = $3,
            status_reason = NULL, started_at = now(), updated_at = now()
        WHERE id = $1 AND state = 'queued'
        "#,
    )
    .bind(launch.id)
    .bind(run_id)
    .bind(thread_id)
    .execute(&mut *transaction)
    .await?;
    audit(
        &mut *transaction,
        caller,
        launch.project_id,
        "launch.started",
        launch.id,
        json!({ "agentRunId": run_id, "conversationId": thread_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(run_id)
}

/// A launch to a connected runner (X2): the task goes to the runner agent
/// as a tag from the person who launched it, with the picked settings on
/// the last line, as docs/harness-guide.md describes. Returns the message.
async fn hand_off(
    state: &AppState,
    caller: &LaunchCaller,
    launch: &LaunchRow,
    thread_id: Uuid,
    runner: Uuid,
    prompt: &str,
    overrides: &LaunchOverrides,
) -> Result<Uuid, AppError> {
    let settings = [
        ("model", overrides.model.as_deref()),
        ("reasoning", overrides.reasoning.as_deref()),
        ("speed", overrides.speed.as_deref()),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| format!("{key}={value}")))
    .collect::<Vec<_>>();
    let mut text = format!(" {}", prompt.trim());
    if !settings.is_empty() {
        text.push_str(&format!("\n\nSettings: {}", settings.join(" ")));
    }
    let actor_kind = match caller.actor_kind {
        ActorKind::Human => "human",
        ActorKind::ExternalAgent => "external_agent",
        ActorKind::ExecutionWorker => "execution_worker",
        ActorKind::Integration => "integration",
        ActorKind::System => "system",
    };
    let outcome = crate::conversations::append_message_command(
        state,
        crate::conversations::MessageCommandActor {
            actor_id: caller.actor_id,
            actor_kind: actor_kind.to_owned(),
            mcp_credential: None,
            via_grant_id: launch.via_oauth_grant_id,
        },
        thread_id,
        None,
        IdempotencyKey::parse(&format!("launch-handoff-{}", launch.id))?,
        json!({
            "version": 1,
            "nodes": [
                { "type": "mention", "actorId": runner },
                { "type": "text", "text": text }
            ]
        }),
        None,
    )
    .await?;
    let message_id = outcome.body.message.id;
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE launches
        SET state = 'handed_off', handoff_message_id = $2, conversation_id = $3,
            status_reason = NULL, started_at = now(), updated_at = now()
        WHERE id = $1 AND state = 'queued'
        "#,
    )
    .bind(launch.id)
    .bind(message_id)
    .bind(thread_id)
    .execute(&mut *transaction)
    .await?;
    audit(
        &mut *transaction,
        caller,
        launch.project_id,
        "launch.handed_off",
        launch.id,
        json!({ "runnerActorId": runner, "conversationId": thread_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(message_id)
}

// ---------------------------------------------------------------------------
// The queue (C5)

#[derive(FromRow)]
struct QueuedLaunch {
    id: Uuid,
    project_id: Uuid,
    work_item_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    requested_by_actor_id: Uuid,
    actor_kind: String,
    actor_name: String,
    environment_id: Option<Uuid>,
    overrides: Value,
    provider_account_id: Option<Uuid>,
    source: String,
    made_through_app: bool,
}

fn actor_kind(value: &str) -> Option<ActorKind> {
    match value {
        "human" => Some(ActorKind::Human),
        "external_agent" => Some(ActorKind::ExternalAgent),
        "execution_worker" => Some(ActorKind::ExecutionWorker),
        "integration" => Some(ActorKind::Integration),
        "system" => Some(ActorKind::System),
        _ => None,
    }
}

/// Starts what the queue can start: due launches in order, each only once
/// its blockers are done, its account has a free slot and its thread is
/// free, rechecking the person's access as it goes. Blocked launches wait,
/// so they start last.
pub async fn drain(state: &AppState) -> Result<usize, AppError> {
    // One drain at a time; the lock goes with the transaction, so a crash
    // can't leave it held.
    let mut guard = state.pool.begin().await?;
    let locked = sqlx::query_scalar::<_, bool>(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('launch.drain', 0))",
    )
    .fetch_one(&mut *guard)
    .await?;
    if !locked {
        return Ok(0);
    }
    let due = sqlx::query_as::<_, QueuedLaunch>(
        r#"
        SELECT launch.id, launch.project_id, launch.work_item_id, launch.conversation_id,
               launch.requested_by_actor_id, actor.kind AS actor_kind,
               COALESCE(membership.display_name, 'The person who queued it') AS actor_name,
               launch.environment_id, launch.overrides, launch.provider_account_id, launch.source,
               launch.made_through_app
        FROM launches launch
        JOIN actors actor ON actor.id = launch.requested_by_actor_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = launch.project_id
         AND membership.actor_id = launch.requested_by_actor_id
        WHERE launch.state = 'queued' AND (launch.start_at IS NULL OR launch.start_at <= now())
        ORDER BY launch.position, launch.id
        LIMIT 500
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    let mut started = 0;
    for queued in due {
        let launch = load_launch(&state.pool, queued.id).await?;
        let Some(kind) = actor_kind(&queued.actor_kind) else {
            continue;
        };
        // Principle 9: the person who queued it must still be able to start
        // runs here, and a launch their app or script queued (made_through_app:
        // directly, in a batch or through an app-made automation) must still
        // be within that app's agent limit as it is now. Source mcp covers
        // rows queued before the flag was set on every insert.
        let limits =
            if (queued.source == "mcp" || queued.made_through_app) && kind == ActorKind::Human {
                let mut connection = state.pool.acquire().await?;
                Some(
                    crate::roles::app_access(&mut connection, queued.requested_by_actor_id)
                        .await?
                        .limits,
                )
            } else {
                None
            };
        let access = memory::project_access(
            &state.pool,
            queued.requested_by_actor_id,
            kind,
            queued.project_id,
            Action::WorkerInvoke,
        );
        let access = match limits.clone() {
            Some(limits) => crate::policy::with_agent_limits(limits, access).await,
            None => access.await,
        };
        if access.is_err() {
            cancel_from_queue(
                state,
                &launch,
                &format!(
                    "{} can no longer start runs in this project, so it was cancelled.",
                    queued.actor_name
                ),
            )
            .await?;
            continue;
        }
        // A task's launch writes the task into the run's prompt: whoever
        // queued it must still see tasks here, by role and app limit.
        let sees_tasks = if queued.work_item_id.is_some() {
            let view = memory::project_access(
                &state.pool,
                queued.requested_by_actor_id,
                kind,
                queued.project_id,
                Action::WorkItemView,
            );
            match limits {
                Some(limits) => crate::policy::with_agent_limits(limits, view).await,
                None => view.await,
            }
            .is_ok()
        } else {
            true
        };
        if !sees_tasks {
            cancel_from_queue(
                state,
                &launch,
                &format!(
                    "{} can no longer see tasks in this project, so it was cancelled.",
                    queued.actor_name
                ),
            )
            .await?;
            continue;
        }
        if let Some(work_item_id) = queued.work_item_id {
            match load_task(&state.pool, queued.project_id, work_item_id).await {
                Ok(task) if !task.archived => {}
                _ => {
                    cancel_from_queue(state, &launch, "The task was archived.").await?;
                    continue;
                }
            }
            let blockers = open_blockers(&state.pool, queued.project_id, work_item_id).await?;
            if !blockers.is_empty() {
                let keys = blockers
                    .iter()
                    .take(5)
                    .map(|blocker| blocker.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                set_reason(
                    &state.pool,
                    &launch,
                    &format!("Waiting for {keys} to finish."),
                )
                .await?;
                continue;
            }
        }
        let environment = match queued.environment_id {
            Some(id) => {
                match environments::active_environment(&state.pool, queued.project_id, id).await? {
                    Some(environment) => Some(environment),
                    None => {
                        cancel_from_queue(state, &launch, "Its environment was archived.").await?;
                        continue;
                    }
                }
            }
            None => None,
        };
        if let Some(thread_id) = queued.conversation_id
            && thread_busy(&state.pool, thread_id).await?
        {
            set_reason(
                &state.pool,
                &launch,
                "Waiting for the run already working in its thread.",
            )
            .await?;
            continue;
        }
        let overrides: LaunchOverrides =
            serde_json::from_value(queued.overrides.clone()).unwrap_or_default();
        let harness = match overrides
            .harness
            .clone()
            .or_else(|| environment.map(|environment| environment.harness))
        {
            Some(harness) => Some(harness),
            None => {
                let mut connection = state.pool.acquire().await?;
                run_defaults(
                    &mut connection,
                    queued.project_id,
                    queued.requested_by_actor_id,
                )
                .await?
                .preferred_harness()
            }
        };
        // A connected runner needs no account here: the task goes to it.
        if harness
            .as_deref()
            .and_then(|harness| state.supervisors.runner_for(harness))
            .is_some()
        {
            let caller = LaunchCaller {
                actor_id: queued.requested_by_actor_id,
                actor_kind: kind,
                via_grant_id: launch.via_oauth_grant_id,
                mcp: false,
                session_id: None,
                request_id: None,
            };
            if start_or_record(state, &caller, &launch).await?.is_some() {
                started += 1;
            }
            continue;
        }
        let account = match launch_account(
            &state.pool,
            queued.project_id,
            queued.requested_by_actor_id,
            harness.as_deref(),
            queued.provider_account_id,
        )
        .await?
        {
            Some(account) => account,
            None => {
                cancel_from_queue(
                    state,
                    &launch,
                    &format!(
                        "{} has no signed-in account for {}, so it was cancelled.",
                        queued.actor_name,
                        harness
                            .as_deref()
                            .map(|harness| crate::agent_sessions::harness_name(state, harness))
                            .unwrap_or_else(|| "this project's runs".to_owned())
                    ),
                )
                .await?;
                continue;
            }
        };
        // Runs this pass started already count: they are queued runs.
        if account_load(&state.pool, account.id).await?
            >= i64::from(account.max_concurrent_runs.max(1))
        {
            set_reason(
                &state.pool,
                &launch,
                &format!("Waiting for a free slot on {}.", account.name),
            )
            .await?;
            continue;
        }
        let caller = LaunchCaller {
            actor_id: queued.requested_by_actor_id,
            actor_kind: kind,
            via_grant_id: launch.via_oauth_grant_id,
            mcp: false,
            session_id: None,
            request_id: None,
        };
        if start_or_record(state, &caller, &launch).await?.is_some() {
            started += 1;
        }
    }
    guard.rollback().await?;
    Ok(started)
}

async fn cancel_from_queue(
    state: &AppState,
    launch: &LaunchRow,
    reason: &str,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    if finish_launch(&mut *transaction, launch.id, "cancelled", reason, None).await? {
        let caller = LaunchCaller {
            actor_id: launch.requested_by_actor_id,
            actor_kind: ActorKind::System,
            via_grant_id: None,
            mcp: false,
            session_id: None,
            request_id: None,
        };
        audit(
            &mut *transaction,
            &caller,
            launch.project_id,
            "launch.cancelled_by_queue",
            launch.id,
            json!({ "reason": reason }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Listing and cancelling

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchList {
    /// Waiting to start, in queue order.
    pub queued: Vec<LaunchView>,
    /// The latest that started, failed or were cancelled.
    pub recent: Vec<LaunchView>,
}

pub(crate) async fn list_for(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
) -> Result<LaunchList, AppError> {
    environments::authorize_project(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let manage = can_manage(state, caller, project_id).await;
    let visible =
        "(launch.conversation_id IS NULL OR conversation_visible_to(launch.conversation_id, $2))";
    let queued = sqlx::query_as::<_, LaunchViewRow>(&format!(
        "{LAUNCH_VIEW_SELECT} WHERE launch.project_id = $1 AND launch.state = 'queued' AND {visible} \
         ORDER BY launch.position LIMIT 200"
    ))
    .bind(project_id)
    .bind(caller.actor_id)
    .fetch_all(&state.pool)
    .await?;
    let recent = sqlx::query_as::<_, LaunchViewRow>(&format!(
        "{LAUNCH_VIEW_SELECT} WHERE launch.project_id = $1 AND launch.state <> 'queued' AND {visible} \
         ORDER BY launch.updated_at DESC, launch.id DESC LIMIT 50"
    ))
    .bind(project_id)
    .bind(caller.actor_id)
    .fetch_all(&state.pool)
    .await?;
    // The launched task's key and title only for someone whose role (and an
    // app's limit) lets them see tasks here; the launch itself stays listed.
    let sees_tasks = sees_tasks(state, caller, project_id).await?;
    let view = |row| {
        let mut view = launch_view(state, row, caller.actor_id, manage);
        if !sees_tasks {
            view.work_item = None;
        }
        view
    };
    Ok(LaunchList {
        queued: queued.into_iter().map(view).collect(),
        recent: recent.into_iter().map(view).collect(),
    })
}

async fn list_launches(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<LaunchList>, AppError> {
    let caller = LaunchCaller::browser(&principal, &HeaderMap::new())?;
    Ok(Json(list_for(&state, &caller, project_id).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

pub(crate) async fn cancel(
    state: &AppState,
    caller: &LaunchCaller,
    project_id: Uuid,
    launch_id: Uuid,
) -> Result<LaunchView, AppError> {
    environments::authorize_project(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let launch = sqlx::query_as::<_, LaunchRow>(&format!(
        "SELECT {LAUNCH_ROW} FROM launches WHERE project_id = $1 AND id = $2"
    ))
    .bind(project_id)
    .bind(launch_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("launch"))?;
    if launch.requested_by_actor_id != caller.actor_id
        && !can_manage(state, caller, project_id).await
    {
        return Err(AppError::Forbidden);
    }
    let mut transaction = state.pool.begin().await?;
    if !finish_launch(
        &mut *transaction,
        launch.id,
        "cancelled",
        "Cancelled before it started.",
        Some(caller.actor_id),
    )
    .await?
    {
        return Err(AppError::Conflict("launch", "it already started or ended"));
    }
    audit(
        &mut *transaction,
        caller,
        project_id,
        "launch.cancelled",
        launch.id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    views_by_id(state, caller, project_id, &[launch.id])
        .await?
        .pop()
        .ok_or(AppError::NotFound("launch"))
}

async fn cancel_launch(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, launch_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LaunchView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = LaunchCaller::browser(&principal, &headers)?;
    Ok(Json(cancel(&state, &caller, project_id, launch_id).await?))
}

// ---------------------------------------------------------------------------
// Tag to launch (A8)

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagPayload {
    pub message_id: Uuid,
    pub worker_actor_id: Uuid,
    /// The message was sent by a person's app or script, whose current agent
    /// limit the launch keeps (checked in the job and again at drain).
    #[serde(default)]
    pub made_through_app: bool,
}

/// A person tagged an agent backed by a harness: launch a run with the
/// project's defaults, in the thread the message is in, or a new thread
/// under it when it was posted in a channel. Called in the message's own
/// transaction; the work happens in a job.
pub(crate) async fn queue_tag_launches(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    message_id: Uuid,
    mentioned_actor_ids: &[Uuid],
) -> Result<(), AppError> {
    let workers = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor.id
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        JOIN execution_workers worker
          ON worker.project_id = membership.project_id AND worker.actor_id = actor.id
         AND worker.enabled
        WHERE membership.project_id = $1 AND membership.enabled
          AND actor.kind = 'execution_worker' AND actor.status = 'active'
          AND actor.id = ANY($2)
          AND conversation_visible_to($3, actor.id)
        ORDER BY actor.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(mentioned_actor_ids)
    .bind(conversation_id)
    .fetch_all(&mut **transaction)
    .await?;
    let Some(worker_actor_id) = workers.first() else {
        return Ok(());
    };
    // This runs inside the call that sent the message: an author's app whose
    // agent limit leaves out "start runs" doesn't launch one by tagging, as
    // the launch job later runs without that limit.
    let author =
        sqlx::query_scalar::<_, Uuid>("SELECT author_actor_id FROM messages WHERE id = $1")
            .bind(message_id)
            .fetch_one(&mut **transaction)
            .await?;
    if crate::policy::agent_limit_hides(author, project_id, crate::policy::Permission::RunsLaunch) {
        return Ok(());
    }
    // An app acting for the author is in this call when its limit is set.
    let made_through_app = crate::policy::current_agent_limit(
        author,
        crate::policy::MembershipScope::Project(project_id),
    )
    .is_some();
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    jobs::enqueue(
        &mut **transaction,
        NewJob::new(TAG_KIND)
            .project(organization_id, project_id)
            .payload(TagPayload {
                message_id,
                worker_actor_id: *worker_actor_id,
                made_through_app,
            })
            .unique(format!("launch.tag:{message_id}"))
            .max_attempts(3),
    )
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct TagMessage {
    project_id: Uuid,
    organization_id: Uuid,
    conversation_id: Uuid,
    conversation_kind: String,
    author_actor_id: Uuid,
    author_kind: String,
    plain_text: String,
    removed: bool,
}

async fn launch_from_tag(state: &AppState, payload: TagPayload) -> Result<(), AppError> {
    let Some(message) = sqlx::query_as::<_, TagMessage>(
        r#"
        SELECT message.project_id, project.organization_id, message.conversation_id,
               conversation.kind AS conversation_kind, message.author_actor_id,
               actor.kind AS author_kind, message.plain_text,
               message.removed_at IS NOT NULL AS removed
        FROM messages message
        JOIN conversations conversation ON conversation.id = message.conversation_id
        JOIN projects project ON project.id = message.project_id
        JOIN actors actor ON actor.id = message.author_actor_id
        WHERE message.id = $1
        "#,
    )
    .bind(payload.message_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(());
    };
    if message.removed || message.author_kind != "human" {
        return Ok(());
    }
    let caller = LaunchCaller {
        actor_id: message.author_actor_id,
        actor_kind: ActorKind::Human,
        via_grant_id: None,
        mcp: false,
        session_id: None,
        request_id: None,
    };
    // The author must still be able to start runs here and see the message.
    let visible = sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
        .bind(message.conversation_id)
        .bind(caller.actor_id)
        .fetch_one(&state.pool)
        .await?;
    // A message an app sent launches only within that app's current limit.
    let limits = if payload.made_through_app {
        let mut connection = state.pool.acquire().await?;
        Some(
            crate::roles::app_access(&mut connection, caller.actor_id)
                .await?
                .limits,
        )
    } else {
        None
    };
    let access = memory::project_access(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        message.project_id,
        Action::WorkerInvoke,
    );
    let access = match limits {
        Some(limits) => crate::policy::with_agent_limits(limits, access).await,
        None => access.await,
    };
    if !visible || access.is_err() {
        tracing::info!(
            message_id = %payload.message_id,
            "tag launch skipped: its author, or the app it came through, can no longer start runs here"
        );
        return Ok(());
    }
    let thread_id = if message.conversation_kind == "thread" {
        message.conversation_id
    } else if message.conversation_kind == "channel" {
        thread_under(state, &caller, &message, payload.message_id).await?
    } else {
        return Ok(());
    };
    let prompt = strip_mentions(&message.plain_text);
    // Tagging an agent profile (A9) launches with the profile's settings.
    let profile =
        crate::agent_profiles::profile_for_actor(&state.pool, payload.worker_actor_id).await?;
    let defaults = {
        let mut connection = state.pool.acquire().await?;
        run_defaults(&mut connection, message.project_id, caller.actor_id).await?
    };
    let launch_id = Uuid::now_v7();
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("launches:{}", message.project_id))
        .execute(&mut *transaction)
        .await?;
    let inserted = sqlx::query(
        r#"
        INSERT INTO launches (
            id, organization_id, project_id, conversation_id, source_message_id,
            requested_by_actor_id, source, environment_id, prompt, start_when, position,
            work_item_id, agent_profile_id, made_through_app
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'tag', $7, $8, 'now',
            (SELECT COALESCE(max(position), 0) + 1 FROM launches WHERE project_id = $3),
            (SELECT link.work_item_id FROM work_item_conversation_links link
             WHERE link.project_id = $3 AND link.conversation_id = $4
             ORDER BY link.created_at LIMIT 1),
            $9, $10
        )
        ON CONFLICT (source_message_id) WHERE source = 'tag' DO NOTHING
        "#,
    )
    .bind(launch_id)
    .bind(message.organization_id)
    .bind(message.project_id)
    .bind(thread_id)
    .bind(payload.message_id)
    .bind(caller.actor_id)
    .bind(match &profile {
        // A profile with its own harness but no environment runs on that
        // harness, not the project's default environment's.
        Some(profile) if profile.environment_id.is_some() || profile.harness.is_some() => {
            profile.environment_id
        }
        _ => defaults
            .environment
            .as_ref()
            .map(|environment| environment.id),
    })
    .bind(if prompt.is_empty() {
        message.plain_text.trim().to_owned()
    } else {
        prompt
    })
    .bind(profile.as_ref().map(|profile| profile.id))
    .bind(payload.made_through_app)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if inserted == 0 {
        transaction.commit().await?;
        return Ok(());
    }
    audit(
        &mut *transaction,
        &caller,
        message.project_id,
        "launch.created",
        launch_id,
        json!({ "messageId": payload.message_id, "source": "tag" }),
    )
    .await?;
    transaction.commit().await?;
    let launch = load_launch(&state.pool, launch_id).await?;
    if thread_busy(&state.pool, thread_id).await? {
        // A run is already working in the thread: this one waits its turn.
        set_reason(
            &state.pool,
            &launch,
            "Waiting for the run already working in its thread.",
        )
        .await?;
        kick(&state.pool).await?;
        return Ok(());
    }
    start_or_record(state, &caller, &launch).await?;
    Ok(())
}

/// The thread under a channel message the answer goes in, as W5's tags do.
async fn thread_under(
    state: &AppState,
    caller: &LaunchCaller,
    message: &TagMessage,
    message_id: Uuid,
) -> Result<Uuid, AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT id FROM conversations WHERE id = $1 FOR UPDATE")
        .bind(message.conversation_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(thread_id) = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM conversations
        WHERE origin_message_id = $1 AND parent_channel_id = $2 AND kind = 'thread'
          AND archived_at IS NULL
        ORDER BY created_at
        LIMIT 1
        "#,
    )
    .bind(message_id)
    .bind(message.conversation_id)
    .fetch_optional(&mut *transaction)
    .await?
    {
        transaction.commit().await?;
        return Ok(thread_id);
    }
    memory::project_access(
        &state.pool,
        caller.actor_id,
        caller.actor_kind,
        message.project_id,
        Action::ThreadCreate,
    )
    .await?;
    let base = crate::conversations::normalize_conversation_name(
        &crate::conversations::suggested_thread_name(&message.plain_text),
    )
    .unwrap_or_else(|_| "Agent run".to_owned());
    let name = unique_thread_name(&mut transaction, message.conversation_id, &base).await?;
    let thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, origin_message_id, name)
        VALUES ($1, $2, 'thread', $3, $4, $5)
        "#,
    )
    .bind(thread_id)
    .bind(message.project_id)
    .bind(message.conversation_id)
    .bind(message_id)
    .bind(&name)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (project_id, conversation_id, actor_id, highest_seen_sequence)
        VALUES ($1, $2, $3, 0)
        "#,
    )
    .bind(message.project_id)
    .bind(thread_id)
    .bind(caller.actor_id)
    .execute(&mut *transaction)
    .await?;
    enqueue_typed(
        &mut transaction,
        message.project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: thread_id,
            kind: OutboxConversationKind::Thread,
            version: 1,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(thread_id)
}

async fn unique_thread_name(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
    base: &str,
) -> Result<String, AppError> {
    let taken = sqlx::query_scalar::<_, String>(
        r#"
        SELECT lower(name) FROM conversations
        WHERE parent_channel_id = $1 AND kind = 'thread' AND archived_at IS NULL
        "#,
    )
    .bind(parent_id)
    .fetch_all(&mut **transaction)
    .await?;
    if !taken.contains(&base.to_lowercase()) {
        return Ok(base.to_owned());
    }
    for number in 2..1000 {
        let suffix = format!(" ({number})");
        let stem = base
            .chars()
            .take(120 - suffix.chars().count())
            .collect::<String>();
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict(
        "conversation_name",
        "an active Conversation already uses that name",
    ))
}

/// The message without the leading @mentions, as the run's prompt.
fn strip_mentions(text: &str) -> String {
    let mut rest = text.trim();
    while let Some(after) = rest.strip_prefix('@') {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        rest = after[end..].trim_start_matches([',', ':']).trim_start();
    }
    rest.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_skills_stay_within_their_budget() {
        let skills = (0..200)
            .map(|index| {
                (
                    Uuid::now_v7(),
                    format!("Skill {index}"),
                    "d".repeat(500),
                    "b".repeat(300),
                )
            })
            .collect::<Vec<_>>();
        let field = skills_field(&skills, "https://example.com/pages");
        assert!(
            field.chars().count() <= MAX_SKILLS_CHARS + 100,
            "{}",
            field.len()
        );
        assert!(
            field.ends_with("more skills: https://example.com/pages"),
            "{field}"
        );
        assert!(field.starts_with("### skill-0\n\n"));
    }

    fn fields() -> BTreeMap<&'static str, String> {
        let mut fields = BTreeMap::new();
        fields.insert("task.key", "AWD-12".to_owned());
        fields.insert("task.title", "Fix the flaky test".to_owned());
        fields.insert("task.description", String::new());
        fields.insert("task.criteria", "- [ ] CI is green".to_owned());
        fields.insert("project.name", "Agent Workspace".to_owned());
        fields
    }

    #[test]
    fn renders_fields_conditionals_and_harness_parts() {
        let template = "{{task.key}}: {{task.title}}\n{{#if task.description}}\n\nWhat: {{task.description}}\n{{/if}}\n{{#if task.criteria}}\n\nDone when:\n{{task.criteria}}\n{{/if}}\n{{#harness codex_cloud}}\n\nOpen a pull request.\n{{/harness}}\n\nThanks {{unknown.field}}";
        assert_eq!(
            render(template, &fields(), "codex_cloud"),
            "AWD-12: Fix the flaky test\n\nDone when:\n- [ ] CI is green\n\nOpen a pull request.\n\nThanks {{unknown.field}}"
        );
        assert_eq!(
            render(template, &fields(), "codex"),
            "AWD-12: Fix the flaky test\n\nDone when:\n- [ ] CI is green\n\nThanks {{unknown.field}}"
        );
    }

    #[test]
    fn the_default_template_reads_well() {
        let template = settings::definition("launch.template")
            .expect("launch.template is registered")
            .default_value();
        let rendered = render(
            template.as_str().expect("the default is a string"),
            &fields(),
            "codex",
        );
        assert!(rendered.starts_with("You're working on AWD-12 in Agent Workspace: Fix the flaky test\n\n## Done when\n\n- [ ] CI is green\n\nWhen you finish"));
        assert!(!rendered.contains("{{"));
        assert!(!rendered.contains("\n\n\n"));
    }

    #[test]
    fn unbalanced_and_unclosed_tags_do_not_panic() {
        assert_eq!(
            render("a {{/if}} b {{task.key", &fields(), ""),
            "a  b {{task.key"
        );
        assert_eq!(render("{{#if task.key}}x", &fields(), ""), "x");
    }

    #[test]
    fn tag_prompts_drop_leading_mentions() {
        assert_eq!(
            strip_mentions("@codex fix the flaky test"),
            "fix the flaky test"
        );
        assert_eq!(strip_mentions("@codex, @claude: look"), "look");
        assert_eq!(strip_mentions("please @codex look"), "please @codex look");
    }
}
