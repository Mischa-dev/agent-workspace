//! AWD-76: workspace templates.
//!
//! A template is a ready-made start for a workspace. It says which modules
//! are on (AWD-75), the layout everyone starts with (AWD-66), roles to add or
//! change and the one new members get (AWD-74), and can bring a starter
//! project with its channels and collections. A collection comes as the
//! definition `collection_describe` returns (AWD-90), made when the template
//! is used.
//!
//! The built-in templates are files in the repository
//! (`modules/templates/*.json`), Everything first. A workspace can save
//! itself as a template too; those are rows in `workspace_templates`, in the
//! same shape. Using one changes the workspace in one go and keeps a record
//! in `workspace_template_uses`, which the setup path's first step reads.
//!
//! Using a template never deletes anything: modules it leaves off are hidden,
//! roles it doesn't name stay, and a project, channel or collection that's
//! already there (by key or name) is reused, so using the same template
//! twice changes nothing the second time.
//!
//! Anyone in the workspace, agents included, can read the templates; an
//! agent can suggest one. Only people who manage the workspace's settings
//! use or save them, in the browser or through an app acting for them.

use std::{collections::BTreeMap, sync::OnceLock};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    collections::{
        self, CreateOutcome, NewCollection,
        definition::{self, CollectionDefinition},
    },
    error::{ApiJson, ApiPath, AppError},
    layouts,
    modules::{manifest, state},
    policy::{ActorKind, Permission},
    roles::{self, Auditor, RoleChange, RoleDefaults, RoleInput, Standing},
    work_item_fields::Writer,
};

pub const MAX_SAVED: i64 = 50;
const MAX_ROLES: usize = 20;
const MAX_CHANNELS: usize = 20;
const MAX_COLLECTIONS: usize = 10;
const MAX_NAME_CHARS: usize = 60;
const MAX_DESCRIPTION_CHARS: usize = 300;

// ---------------------------------------------------------------------------
// The shape

/// A template as the files, the database and agents hold it. Every part is
/// optional: a part left out leaves that part of the workspace as it is.
#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateDefinition {
    /// Module ids (module_list) to have on; every other module that can be
    /// switched goes off. Off hides, never deletes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<Vec<String>>,
    /// The workspace's default layout: a built-in layout preset's id, such
    /// as "software_team", or "everything" for every part shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// With layout: parts shown or hidden where they differ from the
    /// preset (part id to shown), as Settings > Layout saves them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(with = "Value")]
    pub layout_changes: BTreeMap<String, bool>,
    /// With layout: nouns renamed from the preset's words.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(with = "Value")]
    pub layout_words: BTreeMap<String, layouts::Word>,
    /// Roles to add, or to change when the workspace has one of that name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<TemplateRole>,
    /// The role new members and invitations start with, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role: Option<String>,
    /// A starter project with its channels and collections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<TemplateProject>,
}

/// A role a template adds, or changes when one of that name exists. A
/// built-in role (Admin, Member, Viewer) keeps its permissions; the rest
/// can change.
#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateRole {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// For a new role: admin, member or viewer. It keeps only permissions
    /// its base has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_role: Option<String>,
    /// Permission keys (role_list), for a role that isn't built in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    /// What apps and agents acting for holders may do. Left out, the same as
    /// the holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_limit: Option<Vec<String>>,
    /// Modules holders don't get, on top of what the workspace has off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules_off: Vec<String>,
    /// Where holders start: {"layoutPreset", "channels", "pinnedViews",
    /// "homeHidden"}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Value>")]
    pub defaults: Option<Value>,
}

/// A starter project. One with the same key that the person using the
/// template already owns, with the same name, is reused.
#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateProject {
    pub name: String,
    /// 2 to 10 letters or digits, starting with a letter. Taken by another
    /// project, a number is added.
    pub key: String,
    /// Channel names, such as "general".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    /// Collections, each as collection_describe returns it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<CollectionDefinition>,
}

/// A built-in template's file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuiltinFile {
    id: String,
    name: String,
    description: String,
    #[serde(flatten)]
    definition: TemplateDefinition,
}

/// The built-in templates, Everything first.
const BUILTIN_SOURCES: &[&str] = &[
    include_str!("../../../modules/templates/everything.json"),
    include_str!("../../../modules/templates/chat-with-agents.json"),
    include_str!("../../../modules/templates/personal.json"),
    include_str!("../../../modules/templates/small-team.json"),
    include_str!("../../../modules/templates/software-team.json"),
    include_str!("../../../modules/templates/company.json"),
];

fn builtins() -> &'static [BuiltinFile] {
    static FILES: OnceLock<Vec<BuiltinFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        BUILTIN_SOURCES
            .iter()
            .filter_map(|source| serde_json::from_str::<BuiltinFile>(source).ok())
            .collect()
    })
}

/// A template, built in or saved, as people and agents read it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    /// A built-in template's id (such as "software_team"), or a saved one's
    /// ID. Pass it to workspace_template_apply.
    pub id: String,
    pub name: String,
    pub description: String,
    pub built_in: bool,
    /// What using it does, in plain words, one line each.
    pub what_it_does: Vec<String>,
    /// What it sets: modules (ids on), layout (a preset id), roles (name,
    /// description, baseRole, permissions, agentLimit, modulesOff,
    /// defaults), defaultRole, and project (name, key, channels, and
    /// collections as collection_describe returns them).
    #[schemars(with = "Value")]
    pub definition: TemplateDefinition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub saved_at: Option<DateTime<Utc>>,
}

fn builtin_view(file: &BuiltinFile) -> TemplateView {
    TemplateView {
        id: file.id.clone(),
        name: file.name.clone(),
        description: file.description.clone(),
        built_in: true,
        what_it_does: describe(&file.definition),
        definition: file.definition.clone(),
        saved_by: None,
        saved_at: None,
    }
}

// ---------------------------------------------------------------------------
// In plain words

/// "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn module_name(id: &str) -> String {
    manifest::get(id).map_or_else(|| id.to_owned(), |module| module.name.clone())
}

/// What a template does, one line each, for the picker and for agents.
pub fn describe(definition: &TemplateDefinition) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(on) = &definition.modules {
        let switchable = manifest::core()
            .iter()
            .filter(|module| module.switchable)
            .collect::<Vec<_>>();
        let off = switchable
            .iter()
            .filter(|module| !on.contains(&module.id))
            .map(|module| module.name.clone())
            .collect::<Vec<_>>();
        if off.is_empty() {
            lines.push("Turns every module on.".to_owned());
        } else {
            let names = switchable
                .iter()
                .filter(|module| on.contains(&module.id))
                .map(|module| module.name.clone())
                .collect::<Vec<_>>();
            lines.push(if names.is_empty() {
                "Turns every module off but the workspace itself.".to_owned()
            } else {
                format!(
                    "Turns on {}, and turns off {}.",
                    and_list(&names),
                    and_list(&off)
                )
            });
        }
    }
    match definition.layout.as_deref() {
        None => {}
        Some("everything") => lines.push("Shows every part of the app.".to_owned()),
        Some(preset) => lines.push(format!(
            "Starts everyone with the {} layout.",
            layouts::builtin_preset_name(preset).unwrap_or("saved")
        )),
    }
    if !definition.roles.is_empty() {
        let names = definition
            .roles
            .iter()
            .map(|role| role.name.clone())
            .collect::<Vec<_>>();
        lines.push(format!(
            "Adds or updates the {} {}.",
            and_list(&names),
            if names.len() == 1 { "role" } else { "roles" }
        ));
        let limited = definition
            .roles
            .iter()
            .filter(|role| role.agent_limit.is_some())
            .map(|role| role.name.clone())
            .collect::<Vec<_>>();
        if !limited.is_empty() {
            lines.push(format!(
                "Agents acting for {} can do less than they can.",
                and_list(&limited)
            ));
        }
    }
    if let Some(role) = &definition.default_role {
        lines.push(format!("New members start as {role}."));
    }
    if let Some(project) = &definition.project {
        let mut parts = Vec::new();
        if !project.channels.is_empty() {
            parts.push(and_list(
                &project
                    .channels
                    .iter()
                    .map(|channel| format!("#{channel}"))
                    .collect::<Vec<_>>(),
            ));
        }
        if !project.collections.is_empty() {
            let names = project
                .collections
                .iter()
                .map(|collection| collection.name.clone())
                .collect::<Vec<_>>();
            parts.push(format!(
                "the {} {}",
                and_list(&names),
                if names.len() == 1 {
                    "collection"
                } else {
                    "collections"
                }
            ));
        }
        lines.push(if parts.is_empty() {
            format!("Makes a project called {}.", project.name)
        } else {
            format!(
                "Makes a project called {} with {}.",
                project.name,
                and_list(&parts)
            )
        });
    }
    lines
}

// ---------------------------------------------------------------------------
// Checking a template

/// Why a template can't be used or saved: which part, and what to do.
#[derive(Clone, Debug)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

fn problem(field: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem {
        field: field.into(),
        message: message.into(),
    }
}

fn one_line(value: &str, max: usize) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty() && value.chars().count() <= max).then_some(value)
}

/// A saved template's name: 1 to 60 characters on one line.
pub fn normalize_name(value: &str) -> Option<String> {
    one_line(value, MAX_NAME_CHARS)
}

pub fn normalize_description(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (value.chars().count() <= MAX_DESCRIPTION_CHARS).then_some(value)
}

/// Checks what can be checked without the workspace: known modules and
/// layouts, sizes, names. Roles' permissions and collections' fields are
/// checked by the code that makes them, when the template is used.
pub fn check(definition: &TemplateDefinition) -> Result<(), Problem> {
    if let Some(on) = &definition.modules {
        for (index, id) in on.iter().enumerate() {
            match manifest::get(id) {
                Some(module) if module.switchable => {}
                Some(_) => {
                    return Err(problem(
                        format!("modules[{index}]"),
                        format!("{id} is always on; leave it out."),
                    ));
                }
                None => {
                    let known = manifest::core()
                        .iter()
                        .filter(|module| module.switchable)
                        .map(|module| module.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(problem(
                        format!("modules[{index}]"),
                        format!("There's no module {id}. Use some of {known}."),
                    ));
                }
            }
        }
    }
    if let Some(layout) = definition.layout.as_deref()
        && layout != "everything"
        && layouts::builtin_preset_name(layout).is_none()
        && Uuid::parse_str(layout).is_err()
    {
        return Err(problem(
            "layout",
            "Use a built-in layout preset's id, such as \"software_team\", or \"everything\".",
        ));
    }
    if definition.roles.len() > MAX_ROLES {
        return Err(problem(
            "roles",
            format!("A template brings at most {MAX_ROLES} roles."),
        ));
    }
    for (index, role) in definition.roles.iter().enumerate() {
        if one_line(&role.name, MAX_NAME_CHARS).is_none() {
            return Err(problem(
                format!("roles[{index}].name"),
                "Give each role a name of 1 to 60 characters.",
            ));
        }
        if definition.roles[..index]
            .iter()
            .any(|other| other.name.trim().eq_ignore_ascii_case(role.name.trim()))
        {
            return Err(problem(
                format!("roles[{index}].name"),
                format!("{} is named twice.", role.name.trim()),
            ));
        }
        if let Some(defaults) = &role.defaults
            && serde_json::from_value::<RoleDefaults>(defaults.clone()).is_err()
        {
            return Err(problem(
                format!("roles[{index}].defaults"),
                "Defaults take layoutPreset, channels, pinnedViews and homeHidden.",
            ));
        }
    }
    if let Some(role) = &definition.default_role
        && one_line(role, MAX_NAME_CHARS).is_none()
    {
        return Err(problem("defaultRole", "Name the default role."));
    }
    if let Some(project) = &definition.project {
        if crate::projects::normalize_project_name(&project.name).is_err() {
            return Err(problem(
                "project.name",
                "Give the project a name of 1 to 120 bytes, on one line.",
            ));
        }
        if crate::projects::normalize_project_key(&project.key).is_err() {
            return Err(problem(
                "project.key",
                "Use 2 to 10 letters or digits, starting with a letter, such as \"ENG\".",
            ));
        }
        if project.channels.len() > MAX_CHANNELS {
            return Err(problem(
                "project.channels",
                format!("A template brings at most {MAX_CHANNELS} channels."),
            ));
        }
        for (index, channel) in project.channels.iter().enumerate() {
            if crate::conversations::normalize_conversation_name(channel_name(channel)).is_err() {
                return Err(problem(
                    format!("project.channels[{index}]"),
                    "Use a channel name up to 120 characters, on one line.",
                ));
            }
        }
        if project.collections.len() > MAX_COLLECTIONS {
            return Err(problem(
                "project.collections",
                format!("A template brings at most {MAX_COLLECTIONS} collections."),
            ));
        }
        for (index, collection) in project.collections.iter().enumerate() {
            let at = format!("project.collections[{index}]");
            if collections::normalize_name(&collection.name).is_none()
                || collections::normalize_title_name(&collection.title_name).is_none()
                || collections::normalize_description(&collection.description).is_none()
            {
                return Err(problem(
                    at,
                    "Each collection needs a name, a title field name and a one-line description.",
                ));
            }
            if project.collections[..index].iter().any(|other| {
                other
                    .name
                    .trim()
                    .eq_ignore_ascii_case(collection.name.trim())
            }) {
                return Err(problem(at, format!("{} is named twice.", collection.name)));
            }
            if collection.fields.len() + collection.views.len() > definition::MAX_WRITES {
                return Err(problem(
                    at,
                    format!(
                        "A collection brings at most {} fields and views.",
                        definition::MAX_WRITES
                    ),
                ));
            }
            if let Some(name) = definition::duplicate_name(&collection.fields, &collection.views) {
                return Err(problem(at, format!("{name} is named twice.")));
            }
        }
    }
    Ok(())
}

fn channel_name(value: &str) -> &str {
    value.trim().trim_start_matches('#').trim()
}

// ---------------------------------------------------------------------------
// Reading

#[derive(FromRow)]
struct SavedRow {
    id: Uuid,
    name: String,
    description: String,
    definition: Value,
    saved_by: Option<String>,
    created_at: DateTime<Utc>,
}

const SAVED_SELECT: &str = r#"
    SELECT template.id, template.name, template.description, template.definition,
           author.display_name AS saved_by, template.created_at
    FROM workspace_templates template
    LEFT JOIN users author ON author.actor_id = template.created_by_actor_id
"#;

fn saved_view(row: SavedRow) -> Option<TemplateView> {
    let definition = serde_json::from_value::<TemplateDefinition>(row.definition).ok()?;
    Some(TemplateView {
        id: row.id.to_string(),
        name: row.name,
        description: row.description,
        built_in: false,
        what_it_does: describe(&definition),
        definition,
        saved_by: row.saved_by,
        saved_at: Some(row.created_at),
    })
}

/// The built-in templates, Everything first, then the workspace's saved
/// ones, oldest first.
pub async fn list(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Vec<TemplateView>, AppError> {
    let mut found = builtins().iter().map(builtin_view).collect::<Vec<_>>();
    let saved = sqlx::query_as::<_, SavedRow>(&format!(
        "{SAVED_SELECT} WHERE template.organization_id = $1 AND template.archived_at IS NULL \
         ORDER BY template.created_at, template.id"
    ))
    .bind(organization_id)
    .fetch_all(&mut *connection)
    .await?;
    found.extend(saved.into_iter().filter_map(saved_view));
    Ok(found)
}

/// A template by a built-in id or name, or a saved one's ID or name (any
/// case) in this workspace.
pub async fn find(
    connection: &mut PgConnection,
    organization_id: Uuid,
    reference: &str,
) -> Result<Option<TemplateView>, AppError> {
    let reference = reference.trim();
    if let Some(file) = builtins().iter().find(|file| {
        file.id == reference.to_lowercase() || file.name.eq_ignore_ascii_case(reference)
    }) {
        return Ok(Some(builtin_view(file)));
    }
    let rows = sqlx::query_as::<_, SavedRow>(&format!(
        "{SAVED_SELECT} WHERE template.organization_id = $1 AND template.archived_at IS NULL \
         AND (template.id::text = $2 OR lower(template.name) = lower($2)) LIMIT 2"
    ))
    .bind(organization_id)
    .bind(reference)
    .fetch_all(&mut *connection)
    .await?;
    Ok(match <[SavedRow; 1]>::try_from(rows) {
        Ok([row]) => saved_view(row),
        Err(_) => None,
    })
}

/// The last time a template was used in a workspace.
#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateUse {
    pub template: String,
    pub template_name: String,
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Uuid>,
    pub summary: Vec<String>,
    pub used_by: String,
    #[schemars(with = "String")]
    pub used_at: DateTime<Utc>,
}

pub async fn last_use(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Option<TemplateUse>, AppError> {
    Ok(sqlx::query_as::<_, TemplateUse>(
        r#"
        SELECT template_use.template, template_use.template_name, template_use.project_id,
               template_use.summary, COALESCE(user_account.display_name, 'Someone') AS used_by,
               template_use.used_at
        FROM workspace_template_uses template_use
        LEFT JOIN users user_account ON user_account.actor_id = template_use.used_by_actor_id
        WHERE template_use.organization_id = $1
        ORDER BY template_use.used_at DESC, template_use.id DESC
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Whether a workspace has used a template yet: the setup path's first step.
pub async fn used_any(pool: &sqlx::PgPool, organization_id: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM workspace_template_uses WHERE organization_id = $1)",
    )
    .bind(organization_id)
    .fetch_one(pool)
    .await?)
}

/// Whether `caller` may use and save templates: a person who manages the
/// workspace's settings and its roles (an app acting for them is held to
/// their agent limit).
pub(crate) fn may_use(caller: Standing) -> bool {
    caller.kind == ActorKind::Human
        && caller.may(Permission::SettingsManage)
        && caller.may(Permission::RolesManage)
}

// ---------------------------------------------------------------------------
// Using a template

/// What using a template did.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub template: String,
    pub template_name: String,
    /// What changed, in plain words. Empty when the workspace already
    /// matched.
    pub changes: Vec<String>,
    /// The starter project it made or reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Uuid>,
}

/// Why using a template stopped. Nothing it did is kept.
pub enum ApplyError {
    Problem(Problem),
    App(AppError),
}

impl From<AppError> for ApplyError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for ApplyError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

fn refusal_text(error: &AppError) -> Option<(String, String)> {
    match error {
        AppError::Refused { field, message } => Some(((*field).to_owned(), (*message).to_owned())),
        AppError::Validation(what) => Some((
            (*what).to_owned(),
            format!("The template's {what} isn't valid here."),
        )),
        AppError::Conflict("role", "name_taken") => Some((
            "roles".to_owned(),
            "Another role already has that name.".to_owned(),
        )),
        AppError::Conflict("role", "too_many_roles") => Some((
            "roles".to_owned(),
            "The workspace has as many custom roles as it can hold. Remove one first.".to_owned(),
        )),
        _ => None,
    }
}

/// A role error, as a problem with the role named, where the person can act
/// on it.
fn role_problem(index: usize, error: AppError) -> ApplyError {
    match refusal_text(&error) {
        Some((field, message)) => {
            ApplyError::Problem(problem(format!("roles[{index}].{field}"), message))
        }
        None => ApplyError::App(error),
    }
}

/// Uses a template in a workspace: modules, layout, roles, the default role
/// and the starter project, in the caller's transaction, then records the
/// use. The caller has checked [`may_use`].
pub(crate) async fn apply(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    template: &TemplateView,
) -> Result<Applied, ApplyError> {
    let definition = &template.definition;
    check(definition).map_err(ApplyError::Problem)?;
    let mut changes = Vec::new();

    // Modules.
    if let Some(on) = &definition.modules {
        let wanted = manifest::core()
            .iter()
            .filter(|module| module.switchable)
            .map(|module| (module.id.clone(), on.contains(&module.id)))
            .collect::<BTreeMap<_, _>>();
        let changed = crate::modules::http::write_switches(
            transaction,
            organization_id,
            caller.actor_id,
            &wanted,
        )
        .await?;
        let named = |on: bool| {
            changed
                .iter()
                .filter(|(_, value)| **value == on)
                .map(|(id, _)| module_name(id))
                .collect::<Vec<_>>()
        };
        let (turned_on, turned_off) = (named(true), named(false));
        if !turned_on.is_empty() {
            changes.push(format!("Turned on {}.", and_list(&turned_on)));
        }
        if !turned_off.is_empty() {
            changes.push(format!("Turned off {}.", and_list(&turned_off)));
        }
    }

    // The workspace's default layout.
    if let Some(layout) = definition.layout.as_deref() {
        let current = sqlx::query_scalar::<_, Value>(
            "SELECT value FROM settings \
             WHERE organization_id = $1 AND scope = 'workspace' AND key = 'layout.default'",
        )
        .bind(organization_id)
        .fetch_optional(&mut **transaction)
        .await?
        .and_then(|value| serde_json::from_value::<layouts::Layout>(value).ok());
        let wanted = layouts::Layout {
            preset: layout.to_owned(),
            changes: definition.layout_changes.clone(),
            words: definition.layout_words.clone(),
        };
        let target = crate::settings::Target::Workspace { organization_id };
        if layout == "everything" {
            if current.is_some() {
                layouts::write_layout(transaction, target, None, None, caller.actor_id).await?;
                changes.push("Everyone sees every part of the app.".to_owned());
            }
        } else if current.as_ref() != Some(&wanted) {
            layouts::write_layout(transaction, target, Some(wanted), None, caller.actor_id)
                .await
                .map_err(|error| match error {
                    AppError::Validation(_) => ApplyError::Problem(problem(
                        "layout",
                        "That layout isn't one this workspace has.",
                    )),
                    error => ApplyError::App(error),
                })?;
            changes.push(format!(
                "Everyone starts with the {} layout.",
                layouts::builtin_preset_name(layout).unwrap_or("saved")
            ));
        }
    }

    // Roles, matched by name.
    for (index, role) in definition.roles.iter().enumerate() {
        let name = role.name.trim();
        let existing = sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT id, built_in FROM workspace_roles WHERE organization_id = $1 AND lower(name) = lower($2)",
        )
        .bind(organization_id)
        .bind(name)
        .fetch_optional(&mut **transaction)
        .await?;
        let defaults = match &role.defaults {
            Some(value) => Some(
                serde_json::from_value::<RoleDefaults>(value.clone()).map_err(|_| {
                    ApplyError::Problem(problem(
                        format!("roles[{index}].defaults"),
                        "Defaults take layoutPreset, channels, pinnedViews and homeHidden.",
                    ))
                })?,
            ),
            None => None,
        };
        match existing {
            Some((role_id, built_in)) => {
                let before = roles::read(transaction, caller, organization_id, role_id)
                    .await
                    .map_err(|error| role_problem(index, error))?;
                let permissions = if built_in {
                    None
                } else {
                    role.permissions.clone()
                };
                let same = (role.description.is_empty() || role.description == before.description)
                    && permissions.as_ref().is_none_or(|keys| {
                        let mut wanted = keys.clone();
                        wanted.sort();
                        wanted.dedup();
                        let mut held = before
                            .permissions
                            .iter()
                            .map(|key| (*key).to_owned())
                            .collect::<Vec<_>>();
                        held.sort();
                        wanted == held
                    })
                    && limit_matches(role.agent_limit.as_deref(), before.agent_limit.as_deref())
                    && sorted(&role.modules_off) == sorted(&before.modules_off)
                    && defaults
                        .as_ref()
                        .is_none_or(|defaults| *defaults == before.defaults);
                if same {
                    continue;
                }
                roles::change(
                    transaction,
                    auditor,
                    caller,
                    organization_id,
                    role_id,
                    RoleChange {
                        description: (!role.description.is_empty())
                            .then(|| role.description.clone()),
                        permissions,
                        agent_limit: Some(role.agent_limit.clone()),
                        defaults,
                        modules_off: Some(role.modules_off.clone()),
                        ..RoleChange::default()
                    },
                )
                .await
                .map_err(|error| role_problem(index, error))?;
                changes.push(format!("Updated the {} role.", before.name));
            }
            None => {
                let Some(base_role) = role.base_role.clone() else {
                    return Err(ApplyError::Problem(problem(
                        format!("roles[{index}].baseRole"),
                        format!(
                            "The workspace has no {name} role, so say what it's based on: admin, member or viewer."
                        ),
                    )));
                };
                roles::create(
                    transaction,
                    auditor,
                    caller,
                    organization_id,
                    RoleInput {
                        name: name.to_owned(),
                        description: role.description.clone(),
                        base_role,
                        permissions: role.permissions.clone().unwrap_or_default(),
                        agent_limit: role.agent_limit.clone(),
                        defaults: defaults.unwrap_or_default(),
                        modules_off: role.modules_off.clone(),
                        is_default: false,
                    },
                )
                .await
                .map_err(|error| role_problem(index, error))?;
                changes.push(format!("Added the {name} role."));
            }
        }
    }

    // The role new members start with.
    if let Some(reference) = definition.default_role.as_deref() {
        let Some(role_id) = roles::find_role(transaction, organization_id, reference).await? else {
            return Err(ApplyError::Problem(problem(
                "defaultRole",
                format!("The workspace has no {} role.", reference.trim()),
            )));
        };
        let role = roles::read(transaction, caller, organization_id, role_id).await?;
        if !role.is_default {
            roles::change(
                transaction,
                auditor,
                caller,
                organization_id,
                role_id,
                RoleChange {
                    is_default: Some(true),
                    ..RoleChange::default()
                },
            )
            .await
            .map_err(|error| match refusal_text(&error) {
                Some((_, message)) => ApplyError::Problem(problem("defaultRole", message)),
                None => ApplyError::App(error),
            })?;
            changes.push(format!("New members now start as {}.", role.name));
        }
    }

    // The starter project, where the workspace has projects on.
    let mut project_id = None;
    if let Some(project) = &definition.project {
        let projects_on = state::workspace(transaction, organization_id)
            .await?
            .contains("projects");
        if projects_on {
            let (id, made) = starter_project(transaction, caller, organization_id, project).await?;
            project_id = Some(id);
            let project_key =
                sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                    .bind(id)
                    .fetch_one(&mut **transaction)
                    .await?;
            if made {
                changes.push(format!(
                    "Made the project {} ({project_key}).",
                    project.name.trim()
                ));
            }
            changes.extend(
                starter_contents(transaction, caller.actor_id, id, &project_key, project).await?,
            );
            // The setup path carries on in the project it made.
            sqlx::query(
                r#"
                INSERT INTO onboarding_progress (organization_id, actor_id, project_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (organization_id, actor_id) DO UPDATE SET
                    project_id = EXCLUDED.project_id, updated_at = now()
                "#,
            )
            .bind(organization_id)
            .bind(caller.actor_id)
            .bind(id)
            .execute(&mut **transaction)
            .await?;
        }
    }

    sqlx::query(
        r#"
        INSERT INTO workspace_template_uses (
            id, organization_id, template, template_name, project_id, summary, used_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(&template.id)
    .bind(&template.name)
    .bind(project_id)
    .bind(&changes)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    auditor
        .record(
            transaction,
            organization_id,
            "workspace_template.used",
            "organization",
            organization_id,
            json!({
                "template": template.id,
                "templateName": template.name,
                "projectId": project_id,
                "changes": changes,
            }),
        )
        .await?;
    Ok(Applied {
        template: template.id.clone(),
        template_name: template.name.clone(),
        changes,
        project_id,
    })
}

fn sorted(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn limit_matches(wanted: Option<&[String]>, held: Option<&[&'static str]>) -> bool {
    match (wanted, held) {
        (None, None) => true,
        (Some(wanted), Some(held)) => {
            sorted(wanted) == sorted(&held.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>())
        }
        _ => false,
    }
}

/// The starter project: one the caller owns with the template's key and
/// name, or a new one (with a number added to the key when it's taken).
/// Returns it and whether it's new.
async fn starter_project(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Standing,
    organization_id: Uuid,
    project: &TemplateProject,
) -> Result<(Uuid, bool), ApplyError> {
    let name = crate::projects::normalize_project_name(&project.name)?;
    let key = crate::projects::normalize_project_key(&project.key)?;
    let candidates = key_candidates(&key);
    // A project made by an earlier use may carry a numbered key (ENG2 when
    // ENG was taken), so any of the candidates counts.
    let existing = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $3
         AND membership.enabled AND membership.role = 'owner'
        WHERE project.organization_id = $1 AND project.project_key = ANY($2)
          AND project.name = $4 AND project.archived_at IS NULL AND project.kind = 'standard'
        ORDER BY array_position($2, project.project_key)
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .bind(&candidates)
    .bind(caller.actor_id)
    .bind(&name)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(id) = existing {
        return Ok((id, false));
    }
    let (role, role_permissions, display_name, login_name) =
        sqlx::query_as::<_, (String, Option<Vec<String>>, String, String)>(
            r#"
            SELECT membership.role, workspace_role_permissions(membership.role_id),
                   user_account.display_name, user_account.login_name
            FROM organization_memberships membership
            JOIN users user_account ON user_account.actor_id = membership.actor_id
             AND user_account.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
            FOR UPDATE OF membership
            "#,
        )
        .bind(organization_id)
        .bind(caller.actor_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    crate::projects::authorize_project_create(
        caller.actor_id,
        caller.kind,
        organization_id,
        &role,
        role_permissions.as_deref(),
    )?;
    if !caller.may(Permission::ProjectsCreate) {
        return Err(AppError::Forbidden.into());
    }
    let taken = sqlx::query_scalar::<_, String>(
        "SELECT project_key FROM projects WHERE organization_id = $1 AND project_key = ANY($2)",
    )
    .bind(organization_id)
    .bind(&candidates)
    .fetch_all(&mut **transaction)
    .await?;
    let free = candidates
        .into_iter()
        .find(|candidate| !taken.contains(candidate))
        .ok_or_else(|| {
            ApplyError::Problem(problem(
                "project.key",
                "Every key like this one is taken. Use the template from settings with another key.",
            ))
        })?;
    let id = crate::projects::insert_new_project(
        transaction,
        organization_id,
        &free,
        &name,
        crate::projects::ProjectOwner {
            actor_id: caller.actor_id,
            display_name: &display_name,
            login_name: &login_name,
        },
    )
    .await?;
    Ok((id, true))
}

/// The keys a starter project may have: the template's, then numbered
/// ones (ENG2, ENG3 ...), cut to fit the ten-character limit.
fn key_candidates(key: &str) -> Vec<String> {
    std::iter::once(key.to_owned())
        .chain((2..100).map(|n| {
            let suffix = n.to_string();
            let stem = key.chars().take(10 - suffix.len()).collect::<String>();
            format!("{stem}{suffix}")
        }))
        .collect()
}

/// The starter project's channels and collections, where they aren't there
/// yet. A collection that is there gets the template's fields and views
/// added, as collection_write does with a definition.
async fn starter_contents(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    project_key: &str,
    project: &TemplateProject,
) -> Result<Vec<String>, ApplyError> {
    let mut changes = Vec::new();
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    // What the caller may do in the project, as channel and collection
    // writes there check it: an app's agent limit for the project included.
    let holding = roles::project_holding(transaction, actor_id, project_id)
        .await?
        .unwrap_or(crate::policy::PermissionSet::EMPTY);
    if !project.channels.is_empty() && !holding.contains(Permission::ChannelsCreate) {
        return Err(ApplyError::Problem(problem(
            "project.channels",
            "You can't make channels in the starter project, so this template can't be used here.",
        )));
    }
    if !project.collections.is_empty() && !holding.contains(Permission::CollectionsBuild) {
        return Err(ApplyError::Problem(problem(
            "project.collections",
            "You can't build collections in the starter project, so this template can't be used here.",
        )));
    }
    let mut made = Vec::new();
    for channel in &project.channels {
        let name = crate::conversations::normalize_conversation_name(channel_name(channel))?;
        // Any channel with the name counts, even one the caller can't see:
        // channel names are unique in a project, so making another would
        // fail. Only whether it exists is read, never what's in it.
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations
                WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL
                  AND lower(name) = lower($2)
            )
            "#,
        )
        .bind(project_id)
        .bind(&name)
        .fetch_one(&mut **transaction)
        .await?;
        if !exists {
            crate::conversations::insert_channel(transaction, project_id, actor_id, &name).await?;
            made.push(format!("#{name}"));
        }
    }
    if !made.is_empty() {
        changes.push(format!("Made {} in {project_key}.", and_list(&made)));
    }
    for (index, wanted) in project.collections.iter().enumerate() {
        let at = format!("project.collections[{index}]");
        let name = collections::normalize_name(&wanted.name)
            .ok_or_else(|| ApplyError::Problem(problem(&at, "Name the collection.")))?;
        let (collection, created) = match collections::create(
            transaction,
            project_id,
            actor_id,
            NewCollection {
                name: name.clone(),
                description: collections::normalize_description(&wanted.description)
                    .unwrap_or_default(),
                title_name: collections::normalize_title_name(&wanted.title_name)
                    .unwrap_or_else(|| collections::DEFAULT_TITLE_NAME.to_owned()),
                first_view: wanted.views.is_empty(),
            },
        )
        .await?
        {
            CreateOutcome::Created(collection) => (collection, true),
            CreateOutcome::NameTaken(collection) => {
                if collection.archived {
                    return Err(ApplyError::Problem(problem(
                        at,
                        format!(
                            "{project_key} has an archived collection called {name}. Restore or rename it first."
                        ),
                    )));
                }
                (collection, false)
            }
            CreateOutcome::ProjectFull => {
                return Err(ApplyError::Problem(problem(
                    at,
                    format!("{project_key} has as many collections as it can hold."),
                )));
            }
        };
        let applied = definition::apply(
            transaction,
            project_id,
            &collection,
            Writer {
                actor_id,
                project_key,
            },
            &wanted.fields,
            &wanted.views,
        )
        .await?
        .map_err(|problem_found| {
            ApplyError::Problem(problem(
                format!("{at}.{}", problem_found.field),
                problem_found.message,
            ))
        })?;
        if created {
            changes.push(format!("Made the {name} collection in {project_key}."));
        } else if !applied.is_empty() {
            changes.push(format!("Updated the {name} collection in {project_key}."));
        }
        if created || !applied.is_empty() {
            crate::outbox::enqueue_collection_updated(transaction, project_id, collection.id, &[])
                .await?;
        }
    }
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Saving the workspace as a template

/// The workspace as a template: its modules, default layout, the roles it
/// changed or added and its default role, and, when `project_id` names one
/// the caller can open, that project's channels and collections.
pub(crate) async fn snapshot(
    connection: &mut PgConnection,
    caller: Standing,
    organization_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<TemplateDefinition, AppError> {
    let switches = state::switches(connection, organization_id).await?;
    let modules = manifest::core()
        .iter()
        .filter(|module| module.switchable && !switches.is_off(&module.id))
        .map(|module| module.id.clone())
        .collect::<Vec<_>>();
    let layout = sqlx::query_scalar::<_, Value>(
        "SELECT value FROM settings \
         WHERE organization_id = $1 AND scope = 'workspace' AND key = 'layout.default'",
    )
    .bind(organization_id)
    .fetch_optional(&mut *connection)
    .await?
    .and_then(|value| serde_json::from_value::<layouts::Layout>(value).ok());
    // The whole layout, with its shown parts and renamed nouns.
    let (layout, layout_changes, layout_words) = match layout {
        Some(layout) => (layout.preset, layout.changes, layout.words),
        None => ("everything".to_owned(), BTreeMap::new(), BTreeMap::new()),
    };
    let mut template_roles = Vec::new();
    let mut default_role = None;
    for role in roles::list(connection, caller, organization_id).await? {
        if role.is_default {
            default_role = Some(role.name.clone());
        }
        if role.key == "owner" {
            continue;
        }
        let changed_built_in = role.agent_limit.is_some()
            || !role.modules_off.is_empty()
            || role.defaults != RoleDefaults::default();
        if role.built_in && !changed_built_in {
            continue;
        }
        template_roles.push(TemplateRole {
            name: role.name.clone(),
            description: if role.built_in {
                String::new()
            } else {
                role.description.clone()
            },
            base_role: (!role.built_in).then(|| role.base_role.clone()),
            permissions: (!role.built_in).then(|| {
                role.permissions
                    .iter()
                    .map(|key| (*key).to_owned())
                    .collect()
            }),
            agent_limit: role
                .agent_limit
                .map(|keys| keys.iter().map(|key| (*key).to_owned()).collect()),
            modules_off: role.modules_off.clone(),
            // Always written, empty or not, so using the template puts the
            // role's defaults back as they were saved.
            defaults: serde_json::to_value(&role.defaults).ok(),
        });
    }
    let project = match project_id {
        Some(project_id) => {
            Some(project_snapshot(connection, caller.actor_id, organization_id, project_id).await?)
        }
        None => None,
    };
    Ok(TemplateDefinition {
        modules: Some(modules),
        layout: Some(layout),
        layout_changes,
        layout_words,
        roles: template_roles,
        default_role,
        project,
    })
}

async fn project_snapshot(
    connection: &mut PgConnection,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
) -> Result<TemplateProject, AppError> {
    let (name, key) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT name, project_key FROM projects
        WHERE id = $1 AND organization_id = $2 AND kind = 'standard' AND archived_at IS NULL
          AND actor_can_access_project($3, NULL, id)
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    // Channels everyone in the project sees; private ones stay out.
    let channels = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM conversations
        WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL
          AND visibility = 'project' AND name IS NOT NULL
          AND conversation_visible_to(id, $2)
        ORDER BY created_at, id
        LIMIT 20
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut definitions = Vec::new();
    for summary in collections::list(&mut *connection, project_id, false).await? {
        if definitions.len() >= MAX_COLLECTIONS {
            break;
        }
        let Ok(collection_id) = Uuid::parse_str(&summary.id) else {
            continue;
        };
        let Some(collection) =
            collections::get(&mut *connection, project_id, collection_id).await?
        else {
            continue;
        };
        let fields =
            collections::field_definitions(&mut *connection, project_id, collection.id, true)
                .await?;
        let views = collections::views::list(&mut *connection, project_id, collection.id).await?;
        definitions.push(
            definition::describe(&mut *connection, project_id, &collection, &fields, &views)
                .await?,
        );
    }
    Ok(TemplateProject {
        name,
        key,
        channels,
        collections: definitions,
    })
}

/// A saved template.
pub enum SaveOutcome {
    Saved(Box<TemplateView>),
    NameTaken,
    Full,
}

pub(crate) async fn save(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    name: &str,
    description: &str,
    project_id: Option<Uuid>,
) -> Result<SaveOutcome, AppError> {
    let definition = snapshot(transaction, caller, organization_id, project_id).await?;
    if check(&definition).is_err() {
        return Err(AppError::Validation("template"));
    }
    // One save at a time per workspace, so the count holds.
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(organization_id)
        .execute(&mut **transaction)
        .await?;
    let (count, taken) = sqlx::query_as::<_, (i64, bool)>(
        r#"
        SELECT count(*), COALESCE(bool_or(lower(name) = lower($2)), false)
        FROM workspace_templates WHERE organization_id = $1 AND archived_at IS NULL
        "#,
    )
    .bind(organization_id)
    .bind(name)
    .fetch_one(&mut **transaction)
    .await?;
    if taken
        || builtins()
            .iter()
            .any(|file| file.name.eq_ignore_ascii_case(name))
    {
        return Ok(SaveOutcome::NameTaken);
    }
    if count >= MAX_SAVED {
        return Ok(SaveOutcome::Full);
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO workspace_templates (id, organization_id, name, description, definition,
                                         created_by_actor_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(name)
    .bind(description)
    .bind(serde_json::to_value(&definition).map_err(|_| AppError::Internal("template"))?)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    auditor
        .record(
            transaction,
            organization_id,
            "workspace_template.saved",
            "workspace_template",
            id,
            json!({ "name": name, "projectId": project_id }),
        )
        .await?;
    let view = find(transaction, organization_id, &id.to_string())
        .await?
        .ok_or(AppError::Internal("saved template is missing"))?;
    Ok(SaveOutcome::Saved(Box::new(view)))
}

/// Removes a saved template (archives it). Built-in ones stay.
pub(crate) async fn remove(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    organization_id: Uuid,
    template_id: Uuid,
) -> Result<bool, AppError> {
    let name = sqlx::query_scalar::<_, String>(
        r#"
        UPDATE workspace_templates SET archived_at = now()
        WHERE organization_id = $1 AND id = $2 AND archived_at IS NULL
        RETURNING name
        "#,
    )
    .bind(organization_id)
    .bind(template_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(name) = name else {
        return Ok(false);
    };
    auditor
        .record(
            transaction,
            organization_id,
            "workspace_template.removed",
            "workspace_template",
            template_id,
            json!({ "name": name }),
        )
        .await?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// The routes

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/workspace-templates",
            get(list_route).post(save_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/workspace-templates/apply",
            post(apply_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/workspace-templates/{template_id}",
            delete(remove_route),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    organization_id: Uuid,
    templates: Vec<TemplateView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used: Option<TemplateUse>,
    /// Whether the reader may use and save templates.
    can_use: bool,
}

async fn list_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<ListResponse>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let caller = roles::standing(&mut connection, organization_id, principal.actor_id).await?;
    Ok(Json(ListResponse {
        organization_id,
        templates: list(&mut connection, organization_id).await?,
        last_used: last_use(&mut connection, organization_id).await?,
        can_use: may_use(caller),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRequest {
    /// A built-in template's id or a saved one's ID.
    template: String,
}

/// A refusal the person can act on, as the browser shows it.
fn refused(problem: Problem) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error": {
                "code": "template_refused",
                "message": problem.message,
                "field": problem.field,
            }
        })),
    )
}

async fn apply_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ApplyRequest>,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
    if !may_use(caller) {
        return Err(AppError::Forbidden);
    }
    let template = find(&mut transaction, organization_id, &request.template)
        .await?
        .ok_or(AppError::NotFound("template"))?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    match apply(
        &mut transaction,
        &auditor,
        caller,
        organization_id,
        &template,
    )
    .await
    {
        Ok(applied) => {
            transaction.commit().await?;
            Ok(Json(applied).into_response())
        }
        Err(ApplyError::Problem(problem)) => Ok(refused(problem).into_response()),
        Err(ApplyError::App(error)) => Err(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRequest {
    name: String,
    #[serde(default)]
    description: String,
    /// A project to carry its channels and collections.
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn save_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SaveRequest>,
) -> Result<(StatusCode, Json<TemplateView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = normalize_name(&request.name).ok_or(AppError::Validation("name"))?;
    let description =
        normalize_description(&request.description).ok_or(AppError::Validation("description"))?;
    let mut transaction = state.pool.begin().await?;
    let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
    if !may_use(caller) {
        return Err(AppError::Forbidden);
    }
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    match save(
        &mut transaction,
        &auditor,
        caller,
        organization_id,
        &name,
        &description,
        request.project_id,
    )
    .await?
    {
        SaveOutcome::Saved(view) => {
            transaction.commit().await?;
            Ok((StatusCode::CREATED, Json(*view)))
        }
        SaveOutcome::NameTaken => Err(AppError::Conflict("template", "name_taken")),
        SaveOutcome::Full => Err(AppError::Conflict("template", "too_many_templates")),
    }
}

async fn remove_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, template_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
    if !may_use(caller) {
        return Err(AppError::Forbidden);
    }
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    if !remove(&mut transaction, &auditor, organization_id, template_id).await? {
        return Err(AppError::NotFound("template"));
    }
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_keys_fit_the_key_limit() {
        let short = key_candidates("ENG");
        assert_eq!(&short[..3], ["ENG", "ENG2", "ENG3"]);
        let long = key_candidates("ABCDEFGHIJ");
        assert_eq!(&long[..3], ["ABCDEFGHIJ", "ABCDEFGHI2", "ABCDEFGHI3"]);
        assert_eq!(long[9], "ABCDEFGH10");
        assert!(long.iter().all(|key| key.chars().count() <= 10));
    }

    #[test]
    fn every_builtin_template_parses_and_checks() {
        assert_eq!(
            builtins().len(),
            BUILTIN_SOURCES.len(),
            "a file in modules/templates doesn't parse"
        );
        assert_eq!(builtins()[0].id, "everything", "Everything comes first");
        let folder =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/templates");
        let files = std::fs::read_dir(&folder)
            .expect("templates folder")
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            })
            .count();
        assert_eq!(
            files,
            BUILTIN_SOURCES.len(),
            "every file is in BUILTIN_SOURCES"
        );
        let mut ids = std::collections::BTreeSet::new();
        for file in builtins() {
            assert!(ids.insert(file.id.clone()), "{} twice", file.id);
            assert!(file.description.ends_with('.'), "{}", file.id);
            check(&file.definition).unwrap_or_else(|problem| {
                panic!("{}: {} {}", file.id, problem.field, problem.message)
            });
            assert!(!describe(&file.definition).is_empty(), "{}", file.id);
            for role in &file.definition.roles {
                for key in role
                    .permissions
                    .iter()
                    .flatten()
                    .chain(role.agent_limit.iter().flatten())
                {
                    assert!(
                        crate::policy::PERMISSIONS
                            .iter()
                            .any(|info| info.key == key),
                        "{}: {} has no permission {key}",
                        file.id,
                        role.name
                    );
                }
            }
        }
        // The examples the plan names.
        for id in [
            "everything",
            "chat_with_agents",
            "personal",
            "small_team",
            "software_team",
            "company",
        ] {
            assert!(ids.contains(id), "{id}");
        }
    }

    #[test]
    fn plain_words_name_what_changes() {
        let everything = &builtins()[0].definition;
        assert_eq!(describe(everything), ["Turns every module on."]);
        let chat = builtins()
            .iter()
            .find(|file| file.id == "chat_with_agents")
            .expect("chat");
        let words = describe(&chat.definition).join(" ");
        assert!(words.contains("turns off Projects"), "{words}");
        assert_eq!(
            and_list(&["a".into(), "b".into(), "c".into()]),
            "a, b and c"
        );
    }

    #[test]
    fn checks_refuse_unknown_modules_and_layouts() {
        let bad = TemplateDefinition {
            modules: Some(vec!["workspace".to_owned()]),
            ..TemplateDefinition::default()
        };
        assert_eq!(check(&bad).expect_err("unknown module").field, "modules[0]");
        let bad = TemplateDefinition {
            layout: Some("no-such-layout".to_owned()),
            ..TemplateDefinition::default()
        };
        assert_eq!(check(&bad).expect_err("unknown layout").field, "layout");
    }
}
