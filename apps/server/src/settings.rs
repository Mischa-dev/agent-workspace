//! S2: settings that inherit.
//!
//! Every setting is declared once in [`REGISTRY`] with its type, the scopes
//! it may be set at, and its built-in default. A stored value belongs to a
//! workspace, a project or a person (within a workspace). Reading resolves
//! each key from, most specific first:
//!
//! 1. a launch override (only for keys that allow one),
//! 2. the project,
//! 3. the person,
//! 4. the workspace,
//! 5. the built-in default,
//!
//! and always says which one the value came from and which it overrides.
//! The project wins over the person because a project's choice (say, its
//! cloud environment) is the team's; a person's choice wins over the
//! workspace's default.
//!
//! Changes are people-only: owners and admins for the workspace and its
//! projects, and each person for their own settings.
//!
//! To add a setting, add a [`SettingDef`] to [`REGISTRY`] (add lines only;
//! keep keys grouped by their first segment). Screens are drawn from the
//! registry, so a new key needs no web code to be editable.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{FromRow, PgConnection, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
};

// ---------------------------------------------------------------------------
// The registry

/// Where a value is stored.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Workspace,
    Project,
    User,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Project => "project",
            Self::User => "user",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "workspace" => Some(Self::Workspace),
            "project" => Some(Self::Project),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// Where an effective value came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Launch,
    Project,
    User,
    Workspace,
    Default,
}

impl From<Scope> for Source {
    fn from(scope: Scope) -> Self {
        match scope {
            Scope::Workspace => Self::Workspace,
            Scope::Project => Self::Project,
            Scope::User => Self::User,
        }
    }
}

/// A setting's type. Values are JSON; `null` means "not chosen" for the
/// optional types.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingType {
    Bool,
    Integer {
        min: i64,
        max: i64,
    },
    /// Free text up to `max_chars`.
    Text {
        max_chars: usize,
        optional: bool,
    },
    /// Multi-line Markdown up to `max_chars`, such as a prompt template.
    Markdown {
        max_chars: usize,
    },
    /// An identifier such as a harness, model or environment id: letters,
    /// digits and `._:/-`. Adapters say which ones exist; the setting only
    /// checks the shape.
    Identifier {
        optional: bool,
    },
    /// One of `options`.
    Choice {
        options: &'static [&'static str],
    },
}

/// One setting.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingDef {
    /// Dotted, lower case, such as `run.model`.
    pub key: &'static str,
    /// A short name for a settings screen.
    pub label: &'static str,
    /// One sentence on what it changes.
    pub description: &'static str,
    #[serde(rename = "valueType")]
    pub kind: SettingType,
    /// Where it may be set, broadest first.
    pub scopes: &'static [Scope],
    /// Whether a single launch may override it.
    pub launch_override: bool,
    /// The built-in default, as JSON.
    #[serde(skip)]
    default: &'static str,
}

impl SettingDef {
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).unwrap_or(Value::Null)
    }

    /// Check a value against the type.
    pub fn validate(&self, value: &Value) -> Result<(), AppError> {
        let valid = match (self.kind, value) {
            (SettingType::Bool, Value::Bool(_)) => true,
            (SettingType::Integer { min, max }, Value::Number(number)) => number
                .as_i64()
                .is_some_and(|number| (min..=max).contains(&number)),
            (SettingType::Text { optional, .. }, Value::Null)
            | (SettingType::Identifier { optional }, Value::Null) => optional,
            (SettingType::Text { max_chars, .. }, Value::String(text)) => {
                text.chars().count() <= max_chars && !text.chars().any(char::is_control)
            }
            (SettingType::Markdown { max_chars }, Value::String(text)) => {
                text.chars().count() <= max_chars
                    && !text
                        .chars()
                        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                    // Stored values are capped at 16 KiB of JSON.
                    && serde_json::to_string(value).is_ok_and(|json| json.len() <= 16_000)
            }
            (SettingType::Identifier { .. }, Value::String(text)) => {
                (1..=200).contains(&text.len())
                    && text.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-')
                    })
            }
            (SettingType::Choice { options }, Value::String(text)) => {
                options.contains(&text.as_str())
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(AppError::Validation("setting value"))
        }
    }
}

const RUN_SCOPES: &[Scope] = &[Scope::Workspace, Scope::User, Scope::Project];
const TEAM_SCOPES: &[Scope] = &[Scope::Workspace, Scope::Project];
const PERSONAL_SCOPES: &[Scope] = &[Scope::Workspace, Scope::User];

/// Every setting. Add lines; don't reorder.
pub const REGISTRY: &[SettingDef] = &[
    SettingDef {
        key: "notifications.quiet_hours",
        label: "Quiet hours",
        description: "Hold push notifications overnight; they arrive when quiet hours end.",
        kind: SettingType::Bool,
        scopes: PERSONAL_SCOPES,
        launch_override: false,
        default: "false",
    },
    SettingDef {
        key: "notifications.quiet_hours_start",
        label: "Quiet hours start",
        description: "When quiet hours begin, in minutes after midnight.",
        kind: SettingType::Integer { min: 0, max: 1439 },
        scopes: PERSONAL_SCOPES,
        launch_override: false,
        default: "1320",
    },
    SettingDef {
        key: "notifications.quiet_hours_end",
        label: "Quiet hours end",
        description: "When quiet hours end, in minutes after midnight.",
        kind: SettingType::Integer { min: 0, max: 1439 },
        scopes: PERSONAL_SCOPES,
        launch_override: false,
        default: "420",
    },
    SettingDef {
        key: "notifications.time_zone",
        label: "Time zone",
        description: "The time zone quiet hours follow, such as Europe/Berlin. Unset, UTC.",
        kind: SettingType::Identifier { optional: true },
        scopes: PERSONAL_SCOPES,
        launch_override: false,
        default: "null",
    },
    SettingDef {
        key: "notifications.digest",
        label: "Email digest",
        description: "How often email collects what you missed.",
        kind: SettingType::Choice {
            options: &["off", "daily", "weekly"],
        },
        scopes: PERSONAL_SCOPES,
        launch_override: false,
        default: "\"off\"",
    },
    SettingDef {
        key: "run.harness",
        label: "Harness",
        description: "The harness new runs use, such as Codex or Claude Code. Unset, the first one connected.",
        kind: SettingType::Identifier { optional: true },
        scopes: RUN_SCOPES,
        launch_override: true,
        default: "null",
    },
    SettingDef {
        key: "run.model",
        label: "Model",
        description: "The model new runs ask for. Unset, the harness's own default.",
        kind: SettingType::Identifier { optional: true },
        scopes: RUN_SCOPES,
        launch_override: true,
        default: "null",
    },
    SettingDef {
        key: "run.reasoning",
        label: "Reasoning",
        description: "How hard the model thinks before answering, where the harness supports it.",
        kind: SettingType::Choice {
            options: &["default", "minimal", "low", "medium", "high", "max"],
        },
        scopes: RUN_SCOPES,
        launch_override: true,
        default: "\"default\"",
    },
    SettingDef {
        key: "run.speed",
        label: "Speed",
        description: "Whether runs trade cost for speed, where the harness supports it.",
        kind: SettingType::Choice {
            options: &["default", "standard", "fast"],
        },
        scopes: RUN_SCOPES,
        launch_override: true,
        default: "\"default\"",
    },
    SettingDef {
        key: "run.attempts",
        label: "Attempts",
        description: "How many attempts a run makes, where the harness can try several and keep the best (such as Codex Cloud).",
        kind: SettingType::Integer { min: 1, max: 8 },
        scopes: RUN_SCOPES,
        launch_override: true,
        default: "1",
    },
    SettingDef {
        key: "run.environment",
        label: "Environment",
        description: "Where new runs execute, such as a provider's cloud environment. Unset, this host.",
        kind: SettingType::Identifier { optional: true },
        scopes: TEAM_SCOPES,
        launch_override: true,
        default: "null",
    },
    SettingDef {
        key: "run.repository",
        label: "Repository",
        description: "The repository new runs work in when a project has more than one.",
        kind: SettingType::Identifier { optional: true },
        scopes: &[Scope::Project],
        launch_override: true,
        default: "null",
    },
    SettingDef {
        key: "launch.template",
        label: "Launch prompt",
        description: "The prompt Launch builds from a task, in Markdown. Fields such as {{task.title}}, {{task.criteria}} and {{project.guide}} are filled in; {{#if task.pages}}…{{/if}} keeps a part only when the field has a value, and {{#harness codex_cloud}}…{{/harness}} only for that harness.",
        kind: SettingType::Markdown { max_chars: 12000 },
        scopes: TEAM_SCOPES,
        launch_override: false,
        default: r#""You're working on {{task.key}} in {{project.name}}: {{task.title}}\n{{#if task.description}}\n\n## What to do\n\n{{task.description}}\n{{/if}}\n{{#if task.criteria}}\n\n## Done when\n\n{{task.criteria}}\n{{/if}}\n{{#if task.pages}}\n\n## Linked pages\n\n{{task.pages}}\n{{/if}}\n{{#if project.guide}}\n\n## Project guide\n\n{{project.guide}}\n{{/if}}\n\nWhen you finish, say what you changed and how you checked it.\n""#,
    },
    SettingDef {
        key: "ai.auto_apply_suggestions",
        label: "Apply AI suggestions automatically",
        description: "Accept the AI's suggested dependencies and fields without review.",
        kind: SettingType::Bool,
        scopes: TEAM_SCOPES,
        launch_override: false,
        default: "false",
    },
    SettingDef {
        key: "ai.provider",
        label: "AI provider",
        description: "Which provider the server's short AI jobs (like dependency detection) use.",
        kind: SettingType::Identifier { optional: true },
        scopes: TEAM_SCOPES,
        launch_override: false,
        default: "null",
    },
    SettingDef {
        key: "ai.model",
        label: "AI model",
        description: "The model the server's short AI jobs use. Unset, the provider's default.",
        kind: SettingType::Identifier { optional: true },
        scopes: TEAM_SCOPES,
        launch_override: false,
        default: "null",
    },
];

pub fn definition(key: &str) -> Option<&'static SettingDef> {
    REGISTRY.iter().find(|definition| definition.key == key)
}

// ---------------------------------------------------------------------------
// Resolving

/// Whose settings to resolve.
#[derive(Clone, Copy, Debug)]
pub struct SettingsContext {
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    /// The person whose own settings apply, if any.
    pub actor_id: Option<Uuid>,
}

/// One stored value.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub scope: Scope,
    pub value: Value,
    pub version: i64,
    pub updated_at: DateTime<Utc>,
    pub updated_by_actor_id: Option<Uuid>,
}

/// A key's effective value and how it got there.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub key: &'static str,
    pub value: Value,
    pub source: Source,
    /// The next source down that also has a value: what this one overrides,
    /// and what Reset at `source` falls back to.
    pub overrides: Option<Source>,
    /// The value Reset at `source` falls back to.
    pub fallback_value: Option<Value>,
    /// Every stored value for this key in the context, most specific first.
    pub layers: Vec<Layer>,
    pub default: Value,
}

/// Every key, resolved.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSettings {
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    pub settings: Vec<Resolved>,
}

impl EffectiveSettings {
    pub fn get(&self, key: &str) -> Option<&Resolved> {
        self.settings.iter().find(|setting| setting.key == key)
    }

    /// The value as `T`; `None` when the key is unknown, unset (null) or
    /// the wrong type.
    pub fn value<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let resolved = self.get(key)?;
        if resolved.value.is_null() {
            return None;
        }
        serde_json::from_value(resolved.value.clone()).ok()
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.value(key)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.value(key)
    }
}

#[derive(FromRow)]
struct StoredRow {
    scope: String,
    key: String,
    value: Value,
    version: i64,
    updated_at: DateTime<Utc>,
    updated_by_actor_id: Option<Uuid>,
}

/// Resolve every setting for `context`, with `launch` overrides (keys that
/// don't allow one, or values of the wrong type, are refused).
///
/// The caller checks that whoever asks may see `context`.
pub async fn resolve(
    connection: &mut PgConnection,
    context: SettingsContext,
    launch: &BTreeMap<String, Value>,
) -> Result<EffectiveSettings, AppError> {
    for (key, value) in launch {
        let definition = definition(key).ok_or(AppError::Validation("setting key"))?;
        if !definition.launch_override {
            return Err(AppError::Validation("setting launch override"));
        }
        definition.validate(value)?;
    }
    if let Some(project_id) = context.project_id {
        let belongs = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1 AND organization_id = $2)",
        )
        .bind(project_id)
        .bind(context.organization_id)
        .fetch_one(&mut *connection)
        .await?;
        if !belongs {
            return Err(AppError::NotFound("project"));
        }
    }
    let rows = sqlx::query_as::<_, StoredRow>(
        r#"
        SELECT scope, key, value, version, updated_at, updated_by_actor_id
        FROM settings
        WHERE organization_id = $1
          AND (scope = 'workspace'
               OR (scope = 'project' AND project_id = $2)
               OR (scope = 'user' AND actor_id = $3))
        "#,
    )
    .bind(context.organization_id)
    .bind(context.project_id)
    .bind(context.actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut stored: BTreeMap<&str, Vec<Layer>> = BTreeMap::new();
    for row in &rows {
        let Some(definition) = definition(&row.key) else {
            // A key a newer server added, or one since removed.
            continue;
        };
        let Some(scope) = Scope::parse(&row.scope) else {
            continue;
        };
        if !definition.scopes.contains(&scope) || definition.validate(&row.value).is_err() {
            continue;
        }
        stored.entry(definition.key).or_default().push(Layer {
            scope,
            value: row.value.clone(),
            version: row.version,
            updated_at: row.updated_at,
            updated_by_actor_id: row.updated_by_actor_id,
        });
    }
    let settings = REGISTRY
        .iter()
        .map(|definition| {
            let mut layers = stored.remove(definition.key).unwrap_or_default();
            layers.sort_by_key(|layer| precedence(layer.scope));
            resolve_one(definition, launch.get(definition.key), layers)
        })
        .collect();
    Ok(EffectiveSettings {
        organization_id: context.organization_id,
        project_id: context.project_id,
        settings,
    })
}

/// Most specific first.
fn precedence(scope: Scope) -> u8 {
    match scope {
        Scope::Project => 0,
        Scope::User => 1,
        Scope::Workspace => 2,
    }
}

fn resolve_one(
    definition: &'static SettingDef,
    launch: Option<&Value>,
    layers: Vec<Layer>,
) -> Resolved {
    let default = definition.default_value();
    let mut chain: Vec<(Source, Value)> = Vec::new();
    if let Some(value) = launch {
        chain.push((Source::Launch, value.clone()));
    }
    chain.extend(
        layers
            .iter()
            .map(|layer| (Source::from(layer.scope), layer.value.clone())),
    );
    let (source, value) = chain
        .first()
        .cloned()
        .unwrap_or((Source::Default, default.clone()));
    let (overrides, fallback_value) = match chain.get(1) {
        Some((source, value)) => (Some(*source), Some(value.clone())),
        None if source == Source::Default => (None, None),
        None => (Some(Source::Default), Some(default.clone())),
    };
    Resolved {
        key: definition.key,
        value,
        source,
        overrides,
        fallback_value,
        layers,
        default,
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Where a write goes.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    Workspace {
        organization_id: Uuid,
    },
    Project {
        organization_id: Uuid,
        project_id: Uuid,
    },
    User {
        organization_id: Uuid,
        actor_id: Uuid,
    },
}

impl Target {
    pub fn scope(self) -> Scope {
        match self {
            Self::Workspace { .. } => Scope::Workspace,
            Self::Project { .. } => Scope::Project,
            Self::User { .. } => Scope::User,
        }
    }

    fn organization_id(self) -> Uuid {
        match self {
            Self::Workspace { organization_id }
            | Self::Project {
                organization_id, ..
            }
            | Self::User {
                organization_id, ..
            } => organization_id,
        }
    }

    fn project_id(self) -> Option<Uuid> {
        match self {
            Self::Project { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    fn actor_id(self) -> Option<Uuid> {
        match self {
            Self::User { actor_id, .. } => Some(actor_id),
            _ => None,
        }
    }
}

/// Set `key` at `target`. With `expected_version`, refuse if someone else
/// changed it first (0 means "it isn't set yet"). Returns the new version.
/// The caller has checked who may write.
pub async fn set(
    transaction: &mut Transaction<'_, Postgres>,
    target: Target,
    key: &str,
    value: Value,
    expected_version: Option<i64>,
    actor_id: Uuid,
) -> Result<i64, AppError> {
    let definition = definition(key).ok_or(AppError::NotFound("setting"))?;
    if !definition.scopes.contains(&target.scope()) {
        return Err(AppError::Validation("setting scope"));
    }
    definition.validate(&value)?;
    let current = current_version(transaction, target, key).await?;
    if let Some(expected) = expected_version
        && expected != current.unwrap_or(0)
    {
        return Err(AppError::VersionConflict {
            resource: "setting",
            current_version: current.unwrap_or(0),
        });
    }
    let version = if current.is_some() {
        sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE settings
            SET value = $5, version = version + 1, updated_by_actor_id = $6, updated_at = now()
            WHERE organization_id = $1 AND scope = $2
              AND project_id IS NOT DISTINCT FROM $3 AND actor_id IS NOT DISTINCT FROM $4
              AND key = $7
            RETURNING version
            "#,
        )
        .bind(target.organization_id())
        .bind(target.scope().as_str())
        .bind(target.project_id())
        .bind(target.actor_id())
        .bind(&value)
        .bind(actor_id)
        .bind(definition.key)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO settings (
                id, organization_id, scope, project_id, actor_id, key, value,
                updated_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING version
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(target.organization_id())
        .bind(target.scope().as_str())
        .bind(target.project_id())
        .bind(target.actor_id())
        .bind(definition.key)
        .bind(&value)
        .bind(actor_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|error| match &error {
            sqlx::Error::Database(database) if database.is_unique_violation() => {
                AppError::Conflict("setting", "changed_concurrently")
            }
            _ => AppError::Database(error),
        })?
    };
    Ok(version)
}

/// Remove the value at `target`, so the key inherits again. Returns whether
/// there was one.
pub async fn reset(
    transaction: &mut Transaction<'_, Postgres>,
    target: Target,
    key: &str,
    expected_version: Option<i64>,
) -> Result<bool, AppError> {
    let definition = definition(key).ok_or(AppError::NotFound("setting"))?;
    let current = current_version(transaction, target, definition.key).await?;
    if let Some(expected) = expected_version
        && expected != current.unwrap_or(0)
    {
        return Err(AppError::VersionConflict {
            resource: "setting",
            current_version: current.unwrap_or(0),
        });
    }
    Ok(sqlx::query(
        r#"
        DELETE FROM settings
        WHERE organization_id = $1 AND scope = $2
          AND project_id IS NOT DISTINCT FROM $3 AND actor_id IS NOT DISTINCT FROM $4
          AND key = $5
        "#,
    )
    .bind(target.organization_id())
    .bind(target.scope().as_str())
    .bind(target.project_id())
    .bind(target.actor_id())
    .bind(definition.key)
    .execute(&mut **transaction)
    .await?
    .rows_affected()
        > 0)
}

async fn current_version(
    transaction: &mut Transaction<'_, Postgres>,
    target: Target,
    key: &str,
) -> Result<Option<i64>, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT version FROM settings
        WHERE organization_id = $1 AND scope = $2
          AND project_id IS NOT DISTINCT FROM $3 AND actor_id IS NOT DISTINCT FROM $4
          AND key = $5
        FOR UPDATE
        "#,
    )
    .bind(target.organization_id())
    .bind(target.scope().as_str())
    .bind(target.project_id())
    .bind(target.actor_id())
    .bind(key)
    .fetch_optional(&mut **transaction)
    .await?)
}

// ---------------------------------------------------------------------------
// Who may read and write (shared with secrets)

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

/// The caller's role in the workspace, if they're an active member. The
/// rows are locked `FOR SHARE`, so inside a transaction a revocation waits
/// for the write it would have stopped.
async fn workspace_role<'e, E: PgExecutor<'e>>(
    executor: E,
    actor_id: Uuid,
    organization_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT membership.role
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled AND actor.status = 'active'
        FOR SHARE OF membership, actor
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(executor)
    .await?)
}

/// The caller's role in the project and its workspace, if they're an
/// active member of both and the project isn't archived. Locked like
/// [`workspace_role`].
async fn project_role<'e, E: PgExecutor<'e>>(
    executor: E,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<(Uuid, String)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT project.organization_id, membership.role
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2 AND organization_membership.enabled
        JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
        WHERE project.id = $1 AND project.archived_at IS NULL
        FOR SHARE OF project, membership, organization_membership, actor
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(executor)
    .await?)
}

/// Owners and admins of the workspace, as people. Anyone else gets "not
/// found" when they aren't a member, and "forbidden" when they are.
pub async fn authorize_workspace_manage<'e, E: PgExecutor<'e>>(
    executor: E,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<(), AppError> {
    let role = workspace_role(executor, principal.actor_id, organization_id)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::organization(
            organization_id,
            membership_role(&role)?,
            true,
        )),
        None,
    );
    if authorize(
        &subject,
        Action::SettingsManage,
        Resource::Organization { organization_id },
    )
    .is_allowed()
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Owners and admins of the project, as people. Returns the project's
/// workspace.
pub async fn authorize_project_manage<'e, E: PgExecutor<'e>>(
    executor: E,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    let (organization_id, role) = project_role(executor, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            project_id,
            membership_role(&role)?,
            true,
        )),
        None,
    );
    if authorize(
        &subject,
        Action::SettingsManage,
        Resource::Project(ProjectScope::project(project_id)),
    )
    .is_allowed()
    {
        Ok(organization_id)
    } else {
        Err(AppError::Forbidden)
    }
}

/// Any person who is an active member of the workspace, for their own
/// settings and keys.
pub async fn authorize_personal<'e, E: PgExecutor<'e>>(
    executor: E,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<(), AppError> {
    workspace_role(executor, principal.actor_id, organization_id)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Record a settings or secrets change in the audit log. Never pass a
/// secret's value in `metadata`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Option<Uuid>,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, channel, metadata
        ) VALUES ($1, $2, $3, $4, 'password_session', $5, $6, $7, 'succeeded', $8, 'browser', $9)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/settings/registry", get(registry))
        .route("/api/v1/settings/effective", get(effective))
        .route("/api/v1/settings/values/{key}", put(set_value))
        .route("/api/v1/settings/values/{key}/reset", post(reset_value))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    #[serde(flatten)]
    definition: &'static SettingDef,
    default: Value,
}

#[derive(Serialize)]
struct RegistryResponse {
    settings: Vec<RegistryEntry>,
}

async fn registry(_principal: AuthenticatedActor) -> Json<RegistryResponse> {
    Json(RegistryResponse {
        settings: REGISTRY
            .iter()
            .map(|definition| RegistryEntry {
                definition,
                default: definition.default_value(),
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectiveQuery {
    #[serde(default)]
    organization_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EffectiveResponse {
    #[serde(flatten)]
    effective: EffectiveSettings,
    capabilities: SettingsCapabilities,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsCapabilities {
    manage_workspace: bool,
    manage_project: bool,
    manage_own: bool,
}

/// `GET /api/v1/settings/effective?projectId=` (or `?organizationId=` for
/// the workspace alone): every setting as it applies to the caller there,
/// with where each value came from.
async fn effective(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<EffectiveQuery>,
) -> Result<Json<EffectiveResponse>, AppError> {
    let (organization_id, project_id) = match (query.organization_id, query.project_id) {
        (_, Some(project_id)) => {
            let (organization_id, _) = project_role(&state.pool, principal.actor_id, project_id)
                .await?
                .ok_or(AppError::NotFound("project"))?;
            if query
                .organization_id
                .is_some_and(|given| given != organization_id)
            {
                return Err(AppError::NotFound("project"));
            }
            (organization_id, Some(project_id))
        }
        (Some(organization_id), None) => {
            workspace_role(&state.pool, principal.actor_id, organization_id)
                .await?
                .ok_or(AppError::NotFound("workspace"))?;
            (organization_id, None)
        }
        (None, None) => return Err(AppError::Validation("projectId or organizationId")),
    };
    let mut connection = state.pool.acquire().await?;
    let effective = resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id,
            actor_id: Some(principal.actor_id),
        },
        &BTreeMap::new(),
    )
    .await?;
    drop(connection);
    let manage_workspace = authorize_workspace_manage(&state.pool, &principal, organization_id)
        .await
        .is_ok();
    let manage_project = match project_id {
        Some(project_id) => authorize_project_manage(&state.pool, &principal, project_id)
            .await
            .is_ok(),
        None => false,
    };
    Ok(Json(EffectiveResponse {
        effective,
        capabilities: SettingsCapabilities {
            manage_workspace,
            manage_project,
            manage_own: principal.actor_kind == "human",
        },
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetRequest {
    scope: Scope,
    #[serde(default)]
    organization_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
    value: Value,
    #[serde(default)]
    expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetRequest {
    scope: Scope,
    #[serde(default)]
    organization_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
    #[serde(default)]
    expected_version: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteResponse {
    /// The key as it now applies at the scope that was changed.
    setting: Resolved,
}

/// Check the caller may write at `scope`. Call it inside the write's
/// transaction: the membership rows stay locked until it commits.
async fn authorize_target(
    connection: &mut PgConnection,
    principal: &AuthenticatedActor,
    scope: Scope,
    organization_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> Result<Target, AppError> {
    match scope {
        Scope::Workspace => {
            let organization_id = organization_id.ok_or(AppError::Validation("organizationId"))?;
            authorize_workspace_manage(&mut *connection, principal, organization_id).await?;
            Ok(Target::Workspace { organization_id })
        }
        Scope::Project => {
            let project_id = project_id.ok_or(AppError::Validation("projectId"))?;
            let organization_id =
                authorize_project_manage(&mut *connection, principal, project_id).await?;
            Ok(Target::Project {
                organization_id,
                project_id,
            })
        }
        Scope::User => {
            let organization_id = organization_id.ok_or(AppError::Validation("organizationId"))?;
            authorize_personal(&mut *connection, principal, organization_id).await?;
            Ok(Target::User {
                organization_id,
                actor_id: principal.actor_id,
            })
        }
    }
}

async fn resolved_after(
    transaction: &mut Transaction<'_, Postgres>,
    target: Target,
    actor_id: Uuid,
    key: &str,
) -> Result<Resolved, AppError> {
    let effective = resolve(
        transaction,
        SettingsContext {
            organization_id: target.organization_id(),
            project_id: target.project_id(),
            actor_id: Some(actor_id),
        },
        &BTreeMap::new(),
    )
    .await?;
    effective
        .get(key)
        .cloned()
        .ok_or(AppError::Internal("resolved setting"))
}

async fn set_value(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(key): ApiPath<String>,
    ApiJson(request): ApiJson<SetRequest>,
) -> Result<Json<WriteResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let definition = definition(&key).ok_or(AppError::NotFound("setting"))?;
    let mut transaction = state.pool.begin().await?;
    let target = authorize_target(
        &mut transaction,
        &principal,
        request.scope,
        request.organization_id,
        request.project_id,
    )
    .await?;
    let version = set(
        &mut transaction,
        target,
        definition.key,
        request.value,
        request.expected_version,
        principal.actor_id,
    )
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        target.project_id(),
        "setting.changed",
        "setting",
        None,
        serde_json::json!({
            "key": definition.key,
            "scope": target.scope(),
            "organizationId": target.organization_id(),
            "version": version,
        }),
    )
    .await?;
    let setting =
        resolved_after(&mut transaction, target, principal.actor_id, definition.key).await?;
    transaction.commit().await?;
    Ok(Json(WriteResponse { setting }))
}

async fn reset_value(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(key): ApiPath<String>,
    ApiJson(request): ApiJson<ResetRequest>,
) -> Result<Json<WriteResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let definition = definition(&key).ok_or(AppError::NotFound("setting"))?;
    let mut transaction = state.pool.begin().await?;
    let target = authorize_target(
        &mut transaction,
        &principal,
        request.scope,
        request.organization_id,
        request.project_id,
    )
    .await?;
    let removed = reset(
        &mut transaction,
        target,
        definition.key,
        request.expected_version,
    )
    .await?;
    if removed {
        audit(
            &mut transaction,
            &headers,
            &principal,
            target.project_id(),
            "setting.reset",
            "setting",
            None,
            serde_json::json!({
                "key": definition.key,
                "scope": target.scope(),
                "organizationId": target.organization_id(),
            }),
        )
        .await?;
    }
    let setting =
        resolved_after(&mut transaction, target, principal.actor_id, definition.key).await?;
    transaction.commit().await?;
    Ok(Json(WriteResponse { setting }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(scope: Scope, value: Value) -> Layer {
        Layer {
            scope,
            value,
            version: 1,
            updated_at: Utc::now(),
            updated_by_actor_id: None,
        }
    }

    #[test]
    fn values_resolve_most_specific_first_and_say_where_from() {
        let model = definition("run.model").expect("run.model");
        let unset = resolve_one(model, None, vec![]);
        assert_eq!(unset.source, Source::Default);
        assert_eq!(unset.value, Value::Null);
        assert_eq!(unset.overrides, None);

        let workspace = resolve_one(model, None, vec![layer(Scope::Workspace, json!("big"))]);
        assert_eq!(workspace.source, Source::Workspace);
        assert_eq!(workspace.overrides, Some(Source::Default));

        let project = resolve_one(
            model,
            None,
            vec![
                layer(Scope::Project, json!("small")),
                layer(Scope::User, json!("medium")),
                layer(Scope::Workspace, json!("big")),
            ],
        );
        assert_eq!(project.value, json!("small"));
        assert_eq!(project.source, Source::Project);
        assert_eq!(project.overrides, Some(Source::User));
        assert_eq!(project.fallback_value, Some(json!("medium")));

        let launched = resolve_one(
            model,
            Some(&json!("tiny")),
            vec![layer(Scope::Workspace, json!("big"))],
        );
        assert_eq!(launched.source, Source::Launch);
        assert_eq!(launched.overrides, Some(Source::Workspace));
    }

    #[test]
    fn the_registry_is_well_formed() {
        let mut keys = std::collections::HashSet::new();
        for definition in REGISTRY {
            assert!(keys.insert(definition.key), "{} twice", definition.key);
            assert!(!definition.scopes.is_empty(), "{}", definition.key);
            assert!(
                definition.validate(&definition.default_value()).is_ok(),
                "{} default doesn't match its type",
                definition.key
            );
            let parts = definition.key.split('.').collect::<Vec<_>>();
            assert!(parts.len() >= 2, "{}", definition.key);
            assert!(
                parts.iter().all(|part| !part.is_empty()
                    && part.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')),
                "{}",
                definition.key
            );
        }
    }

    #[test]
    fn values_are_checked_against_their_type() {
        let reasoning = definition("run.reasoning").expect("run.reasoning");
        assert!(reasoning.validate(&json!("high")).is_ok());
        assert!(reasoning.validate(&json!("extreme")).is_err());
        assert!(reasoning.validate(&Value::Null).is_err());
        let model = definition("run.model").expect("run.model");
        assert!(model.validate(&json!("gpt-5.5-codex")).is_ok());
        assert!(model.validate(&json!("claude-opus-5-5[1m]")).is_err());
        assert!(model.validate(&Value::Null).is_ok());
        assert!(model.validate(&json!("a b")).is_err());
        let auto = definition("ai.auto_apply_suggestions").expect("auto apply");
        assert!(auto.validate(&json!(true)).is_ok());
        assert!(auto.validate(&json!("true")).is_err());
    }
}
