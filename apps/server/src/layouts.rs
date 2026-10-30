//! AWD-66: layouts, which parts of the app each person sees.
//!
//! Visual only: hiding a part never changes what anyone may do; roles and
//! the policy engine decide that. The parts a layout switches, the nouns it
//! may rename and the built-in presets are declared once, in
//! `apps/web/src/experience/registry.json`, which the web app and this
//! module both read.
//!
//! A layout is a preset (built in, or one the workspace saved) with parts
//! turned on or off and nouns renamed. Which one applies to a person in a
//! project, most specific first:
//!
//! 1. a rule for them by name,
//! 2. a rule for their role in the project (owners count as admins),
//! 3. the project's own layout,
//! 4. the workspace's default,
//! 5. everything on.
//!
//! A rule picks only a preset; nouns its preset doesn't rename keep the
//! project's words. On top, each person can hide more for themselves ("My
//! view"), and owners and admins can show what a layout hides ("Show hidden
//! parts"). The project's and the workspace's layouts and each person's
//! choices are settings (`layout.*`); saved presets and rules have their own
//! tables (migration 20261028080000).
//!
//! Reading fails open: a part, noun or preset this server doesn't know is
//! ignored, so a layout never hides more than it meant to.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    policy::{Action, Membership, Permission, ProjectScope, Resource, Subject, authorize},
    settings::{self, Target},
};

const MAX_LAYOUT_BYTES: usize = 8_000;
const MAX_CHANGES: usize = 100;
const MAX_WORDS: usize = 10;
const MAX_WORD_CHARS: usize = 30;
const MAX_HIDDEN: usize = 100;
const MAX_RULES: usize = 200;
const MAX_SAVED_PRESETS: i64 = 100;
const MAX_PRESET_NAME_CHARS: usize = 60;
const MAX_PRESET_DESCRIPTION_CHARS: usize = 200;

// ---------------------------------------------------------------------------
// The registry

#[derive(Debug, Default, Deserialize)]
struct Registry {
    // Groups, part groups and descriptions are for screens; the tests check them.
    #[cfg_attr(not(test), allow(dead_code))]
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    parts: Vec<Part>,
    #[serde(default)]
    nouns: Vec<Noun>,
    #[serde(default)]
    presets: Vec<BuiltinPreset>,
}

#[derive(Debug, Deserialize)]
struct Group {
    #[cfg_attr(not(test), allow(dead_code))]
    id: String,
}

#[derive(Debug, Deserialize)]
struct Part {
    id: String,
    #[cfg_attr(not(test), allow(dead_code))]
    group: String,
    /// Shown when no preset decides.
    default: bool,
}

#[derive(Debug, Deserialize)]
struct Noun {
    id: String,
    one: String,
    many: String,
}

#[derive(Debug, Deserialize)]
struct BuiltinPreset {
    id: String,
    name: String,
    #[cfg_attr(not(test), allow(dead_code))]
    #[serde(default)]
    description: String,
    /// How agents should write in a project that uses it.
    #[serde(default)]
    tone: String,
    /// Parts it hides; the rest show as they do by default.
    #[serde(default)]
    hide: Option<Vec<String>>,
    /// The only parts it shows.
    #[serde(default)]
    only: Option<Vec<String>>,
    /// Parts to put first.
    #[serde(default)]
    lead: Vec<String>,
    #[serde(default)]
    words: BTreeMap<String, Word>,
}

const REGISTRY_JSON: &str = include_str!("../../web/src/experience/registry.json");

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| serde_json::from_str(REGISTRY_JSON).unwrap_or_default())
}

fn builtin(id: &str) -> Option<&'static BuiltinPreset> {
    registry().presets.iter().find(|preset| preset.id == id)
}

fn part_known(id: &str) -> bool {
    registry().parts.iter().any(|part| part.id == id)
}

/// A noun's own words, before any layout renames it.
fn noun_default(id: &str) -> Option<Word> {
    registry()
        .nouns
        .iter()
        .find(|noun| noun.id == id)
        .map(|noun| Word {
            one: noun.one.clone(),
            many: noun.many.clone(),
        })
}

// ---------------------------------------------------------------------------
// Layouts and their shape

/// A noun's singular and plural, such as "to-do" and "to-dos".
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Word {
    pub one: String,
    pub many: String,
}

/// A stored layout: a preset, with parts turned on or off and nouns renamed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layout {
    /// A built-in preset's id, such as `personal_life`, or a saved preset's ID.
    pub preset: String,
    /// Part id to shown, where it differs from the preset.
    #[serde(default)]
    pub changes: BTreeMap<String, bool>,
    /// Noun id to its words, where they differ from the preset's.
    #[serde(default)]
    pub words: BTreeMap<String, Word>,
}

impl Layout {
    pub fn preset(preset: impl Into<String>) -> Self {
        Self {
            preset: preset.into(),
            changes: BTreeMap::new(),
            words: BTreeMap::new(),
        }
    }
}

/// `personal_life`: a built-in preset's or a noun's id.
fn is_name(text: &str) -> bool {
    (1..=40).contains(&text.len())
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `area.runs`: a part's id.
fn is_part_id(text: &str) -> bool {
    text.len() <= 60
        && text
            .split_once('.')
            .is_some_and(|(group, name)| is_name(group) && is_name(name))
}

fn is_word(text: &str) -> bool {
    (1..=MAX_WORD_CHARS).contains(&text.chars().count())
        && text.trim() == text
        && !text.chars().any(char::is_control)
}

fn is_preset_reference(text: &str) -> bool {
    is_name(text) || Uuid::parse_str(text).is_ok()
}

fn well_formed(layout: &Layout) -> bool {
    is_preset_reference(&layout.preset)
        && layout.changes.len() <= MAX_CHANGES
        && layout.changes.keys().all(|part| is_part_id(part))
        && layout.words.len() <= MAX_WORDS
        && layout
            .words
            .iter()
            .all(|(noun, word)| is_name(noun) && is_word(&word.one) && is_word(&word.many))
}

/// The shape of a `layout.default` setting. Writes also check the value
/// against the registry and the workspace's presets ([`checked_layout`]).
pub fn valid_layout_value(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    serde_json::to_string(value).is_ok_and(|json| json.len() <= MAX_LAYOUT_BYTES)
        && serde_json::from_value::<Layout>(value.clone()).is_ok_and(|layout| well_formed(&layout))
}

/// The shape of a `layout.my_view` setting: distinct part ids.
pub fn valid_part_list(value: &Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    let mut seen = BTreeSet::new();
    items.len() <= MAX_HIDDEN
        && items.iter().all(|item| {
            item.as_str()
                .is_some_and(|part| is_part_id(part) && seen.insert(part))
        })
}

// ---------------------------------------------------------------------------
// Saved presets

/// One of a workspace's own presets: a built-in one with parts turned on or
/// off and nouns renamed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPreset {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// The built-in preset it starts from.
    pub base: String,
    pub changes: BTreeMap<String, bool>,
    pub words: BTreeMap<String, Word>,
    pub version: i64,
    /// Gone from pickers; layouts and rules that use it keep working.
    pub removed: bool,
}

#[derive(FromRow)]
struct SavedRow {
    id: Uuid,
    name: String,
    description: String,
    base: String,
    changes: Value,
    words: Value,
    version: i64,
    removed: bool,
}

impl From<SavedRow> for SavedPreset {
    fn from(row: SavedRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            description: row.description,
            base: row.base,
            changes: serde_json::from_value(row.changes).unwrap_or_default(),
            words: serde_json::from_value(row.words).unwrap_or_default(),
            version: row.version,
            removed: row.removed,
        }
    }
}

const SAVED_COLUMNS: &str =
    "id, name, description, base, changes, words, version, removed_at IS NOT NULL AS removed";

/// The workspace's presets, plus any removed one in `also`, by name.
async fn saved_presets(
    connection: &mut PgConnection,
    organization_id: Uuid,
    also: &[Uuid],
) -> Result<Vec<SavedPreset>, AppError> {
    Ok(sqlx::query_as::<_, SavedRow>(&format!(
        r#"
        SELECT {SAVED_COLUMNS}
        FROM layout_presets
        WHERE organization_id = $1 AND (removed_at IS NULL OR id = ANY($2))
        ORDER BY lower(name), id
        "#
    ))
    .bind(organization_id)
    .bind(also)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(SavedPreset::from)
    .collect())
}

/// One of the workspace's presets, removed or not. Another workspace's is
/// never found.
async fn saved_preset(
    connection: &mut PgConnection,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<SavedPreset>, AppError> {
    Ok(sqlx::query_as::<_, SavedRow>(&format!(
        "SELECT {SAVED_COLUMNS} FROM layout_presets WHERE id = $1 AND organization_id = $2"
    ))
    .bind(id)
    .bind(organization_id)
    .fetch_optional(&mut *connection)
    .await?
    .map(SavedPreset::from))
}

// ---------------------------------------------------------------------------
// Working out what shows

/// Which preset a layout uses, as the screens name it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRef {
    /// A built-in preset's id or a saved preset's ID.
    pub id: String,
    pub name: String,
    pub built_in: bool,
    /// The built-in preset it starts from: itself, for a built-in one.
    pub base: String,
    pub removed: bool,
}

/// What a preset or a layout shows, and the words it uses.
#[derive(Clone, Debug)]
struct Shape {
    visible: BTreeSet<&'static str>,
    words: BTreeMap<String, Word>,
    lead: Vec<String>,
    preset: Option<PresetRef>,
}

impl Shape {
    /// Everything that shows by default, with no preset.
    fn everything() -> Self {
        Self {
            visible: registry()
                .parts
                .iter()
                .filter(|part| part.default)
                .map(|part| part.id.as_str())
                .collect(),
            words: BTreeMap::new(),
            lead: Vec::new(),
            preset: None,
        }
    }

    fn builtin(preset: &'static BuiltinPreset) -> Self {
        let parts = &registry().parts;
        let visible = match (&preset.only, &preset.hide) {
            (Some(only), _) => parts
                .iter()
                .filter(|part| only.contains(&part.id))
                .map(|part| part.id.as_str())
                .collect(),
            (None, hide) => parts
                .iter()
                .filter(|part| {
                    part.default && !hide.as_ref().is_some_and(|hide| hide.contains(&part.id))
                })
                .map(|part| part.id.as_str())
                .collect(),
        };
        Self {
            visible,
            words: preset.words.clone(),
            lead: preset.lead.clone(),
            preset: Some(PresetRef {
                id: preset.id.clone(),
                name: preset.name.clone(),
                built_in: true,
                base: preset.id.clone(),
                removed: false,
            }),
        }
    }

    /// What `reference` (a built-in preset's id or a saved preset's ID)
    /// shows; `None` when it names no preset among these.
    fn preset(reference: &str, saved: &[SavedPreset]) -> Option<Self> {
        if let Some(preset) = builtin(reference) {
            return Some(Self::builtin(preset));
        }
        let id = Uuid::parse_str(reference).ok()?;
        let preset = saved.iter().find(|preset| preset.id == id)?;
        let mut shape = builtin(&preset.base).map_or_else(Self::everything, Self::builtin);
        shape.apply(&preset.changes, &preset.words);
        shape.preset = Some(PresetRef {
            id: preset.id.to_string(),
            name: preset.name.clone(),
            built_in: false,
            base: preset.base.clone(),
            removed: preset.removed,
        });
        Some(shape)
    }

    fn layout(layout: &Layout, saved: &[SavedPreset]) -> Self {
        let mut shape = Self::preset(&layout.preset, saved).unwrap_or_else(Self::everything);
        shape.apply(&layout.changes, &layout.words);
        shape
    }

    fn apply(&mut self, changes: &BTreeMap<String, bool>, words: &BTreeMap<String, Word>) {
        for part in &registry().parts {
            match changes.get(&part.id) {
                Some(true) => {
                    self.visible.insert(part.id.as_str());
                }
                Some(false) => {
                    self.visible.remove(part.id.as_str());
                }
                None => {}
            }
        }
        for (noun, word) in words {
            if noun_default(noun).is_some() {
                self.words.insert(noun.clone(), word.clone());
            }
        }
    }

    /// The word this shape uses for `noun`.
    fn word(&self, noun: &str) -> Option<Word> {
        self.words.get(noun).cloned().or_else(|| noun_default(noun))
    }
}

/// Where a person's layout came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    /// A rule for them by name.
    Person,
    /// A rule for their role in the project.
    Role,
    Project,
    Workspace,
    /// Nothing chosen: everything on.
    Default,
}

/// What one person sees.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    pub source: LayoutSource,
    /// The preset in use; none when nothing was chosen.
    pub preset: Option<PresetRef>,
    /// Parts the layout hides, in registry order.
    pub layout_hidden: Vec<&'static str>,
    /// Parts this person doesn't see: the layout's (unless they show hidden
    /// parts) and the ones they hid for themselves.
    pub hidden: Vec<&'static str>,
    /// Nouns renamed from their own words.
    pub words: BTreeMap<String, Word>,
    /// Parts to put first, such as Runs in Agent lab.
    pub lead: Vec<String>,
    /// Owners and admins who asked to see what the layout hides.
    pub showing_hidden: bool,
}

/// Everything that decides one person's layout.
struct Inputs<'a> {
    /// The preset a rule gives them by name.
    person: Option<&'a str>,
    /// The preset a rule gives their role.
    role: Option<&'a str>,
    project: Option<&'a Layout>,
    workspace: Option<&'a Layout>,
    saved: &'a [SavedPreset],
    /// Parts their role starts them without (Home sections).
    role_hidden: &'a [String],
    my_view: &'a [String],
    /// They may and want to see what the layout hides.
    show_hidden: bool,
}

fn effective(inputs: &Inputs<'_>) -> Effective {
    let (base_source, base) = match (inputs.project, inputs.workspace) {
        (Some(layout), _) => (LayoutSource::Project, Shape::layout(layout, inputs.saved)),
        (None, Some(layout)) => (LayoutSource::Workspace, Shape::layout(layout, inputs.saved)),
        (None, None) => (LayoutSource::Default, Shape::everything()),
    };
    // A rule whose preset is gone falls through to the next one down.
    let rule = [
        (LayoutSource::Person, inputs.person),
        (LayoutSource::Role, inputs.role),
    ]
    .into_iter()
    .find_map(|(source, reference)| Some((source, Shape::preset(reference?, inputs.saved)?)));
    let (source, shape) = match rule {
        Some((source, mut shape)) => {
            let mut words = base.words.clone();
            words.append(&mut shape.words);
            shape.words = words;
            (source, shape)
        }
        None => (base_source, base),
    };
    let mut shape = shape;
    for part in inputs.role_hidden {
        shape.visible.remove(part.as_str());
    }
    let parts = || registry().parts.iter().map(|part| part.id.as_str());
    let mine = inputs
        .my_view
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    Effective {
        source,
        layout_hidden: parts().filter(|id| !shape.visible.contains(id)).collect(),
        hidden: parts()
            .filter(|id| mine.contains(id) || (!inputs.show_hidden && !shape.visible.contains(id)))
            .collect(),
        words: shape
            .words
            .iter()
            .filter(|(noun, word)| noun_default(noun).is_some_and(|own| own != **word))
            .map(|(noun, word)| (noun.clone(), word.clone()))
            .collect(),
        lead: shape
            .lead
            .iter()
            .filter(|part| part_known(part))
            .cloned()
            .collect(),
        preset: shape.preset,
        showing_hidden: inputs.show_hidden,
    }
}

// ---------------------------------------------------------------------------
// What the screens read

/// A layout as one person sees it in a project, with what the Layout
/// settings page needs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLayoutView {
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub effective: Effective,
    /// The project's own layout; none while it uses the workspace's default.
    pub layout: Option<Layout>,
    /// Pass back as `expectedVersion`; 0 while the project has no layout.
    pub version: i64,
    /// The workspace's default, which applies while the project has none.
    pub workspace_layout: Option<Layout>,
    /// Who gets which preset here. Only owners and admins see these.
    pub rules: Vec<RuleView>,
    /// The workspace's presets, and any removed one still in use here.
    pub saved_presets: Vec<SavedPreset>,
    pub my_view: MyView,
    pub capabilities: LayoutCapabilities,
}

/// The workspace's default layout, as one person sees it on workspace pages
/// such as Home.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayoutView {
    pub organization_id: Uuid,
    pub effective: Effective,
    pub layout: Option<Layout>,
    /// Pass back as `expectedVersion`; 0 while there's no default.
    pub version: i64,
    /// The workspace's presets, and any removed one its default still uses.
    pub saved_presets: Vec<SavedPreset>,
    pub my_view: MyView,
    pub capabilities: LayoutCapabilities,
}

/// A rule: one role in the project, or one person, gets a preset.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub id: Uuid,
    /// admin (which covers owners), member or viewer.
    pub role: Option<String>,
    /// A custom role of the workspace's.
    pub role_id: Option<Uuid>,
    pub person: Option<RulePerson>,
    /// A built-in preset's id or a saved preset's ID.
    pub preset: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePerson {
    pub actor_id: Uuid,
    pub name: String,
    /// Whether they're still in the project.
    pub in_project: bool,
}

/// What a person changed for themselves, across the workspace.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyView {
    /// Parts they hid.
    pub hidden: Vec<String>,
    /// Whether they asked to see the parts layouts hide.
    pub show_hidden: bool,
    /// Whether that works for them here: owners and admins only.
    pub can_show_hidden: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutCapabilities {
    /// May change this layout (and, in a project, its rules).
    pub manage: bool,
    /// May change the workspace's default and its saved presets.
    pub manage_workspace: bool,
}

#[derive(FromRow)]
struct SettingRow {
    scope: String,
    key: String,
    value: Value,
    version: i64,
}

/// The layout settings that apply to one person in a workspace, or in one
/// of its projects.
#[derive(Default)]
struct Stored {
    workspace: Option<Layout>,
    workspace_version: i64,
    project: Option<Layout>,
    project_version: i64,
    my_view: Vec<String>,
    show_hidden: bool,
}

impl Stored {
    async fn load(
        connection: &mut PgConnection,
        organization_id: Uuid,
        project_id: Option<Uuid>,
        actor_id: Uuid,
    ) -> Result<Self, AppError> {
        let rows = sqlx::query_as::<_, SettingRow>(
            r#"
            SELECT scope, key, value, version
            FROM settings
            WHERE organization_id = $1 AND key LIKE 'layout.%'
              AND (scope = 'workspace'
                   OR (scope = 'project' AND project_id = $2)
                   OR (scope = 'user' AND actor_id = $3))
            "#,
        )
        .bind(organization_id)
        .bind(project_id)
        .bind(actor_id)
        .fetch_all(&mut *connection)
        .await?;
        let mut stored = Self::default();
        for row in rows {
            let valid = settings::definition(&row.key)
                .is_some_and(|definition| definition.validate(&row.value).is_ok());
            match (row.scope.as_str(), row.key.as_str()) {
                ("workspace", "layout.default") => {
                    stored.workspace_version = row.version;
                    stored.workspace = valid
                        .then(|| serde_json::from_value(row.value).ok())
                        .flatten();
                }
                ("project", "layout.default") => {
                    stored.project_version = row.version;
                    stored.project = valid
                        .then(|| serde_json::from_value(row.value).ok())
                        .flatten();
                }
                ("user", "layout.my_view") if valid => {
                    stored.my_view = serde_json::from_value(row.value).unwrap_or_default();
                }
                ("user", "layout.show_hidden") => {
                    stored.show_hidden = row.value.as_bool().unwrap_or(false);
                }
                _ => {}
            }
        }
        Ok(stored)
    }

    fn my_view(&self, can_show_hidden: bool) -> MyView {
        MyView {
            hidden: self.my_view.clone(),
            show_hidden: self.show_hidden,
            can_show_hidden,
        }
    }

    /// Saved presets the stored layouts use.
    fn saved_ids(&self) -> Vec<Uuid> {
        [&self.workspace, &self.project]
            .into_iter()
            .flatten()
            .filter_map(|layout| Uuid::parse_str(&layout.preset).ok())
            .collect()
    }
}

#[derive(FromRow)]
struct RuleRow {
    id: Uuid,
    role: Option<String>,
    role_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    builtin_preset: Option<String>,
    saved_preset_id: Option<Uuid>,
    version: i64,
    name: Option<String>,
    in_project: bool,
}

impl RuleRow {
    fn preset(&self) -> String {
        self.saved_preset_id
            .map(|id| id.to_string())
            .or_else(|| self.builtin_preset.clone())
            .unwrap_or_default()
    }

    fn view(&self) -> RuleView {
        RuleView {
            id: self.id,
            role: self.role.clone(),
            role_id: self.role_id,
            person: self.actor_id.map(|actor_id| RulePerson {
                actor_id,
                name: self.name.clone().unwrap_or_default(),
                in_project: self.in_project,
            }),
            preset: self.preset(),
            version: self.version,
        }
    }
}

async fn rules(connection: &mut PgConnection, project_id: Uuid) -> Result<Vec<RuleRow>, AppError> {
    Ok(sqlx::query_as::<_, RuleRow>(
        r#"
        SELECT rule.id, rule.role, rule.role_id, rule.actor_id, rule.builtin_preset,
               rule.saved_preset_id,
               rule.version,
               COALESCE(membership.display_name, account.display_name) AS name,
               COALESCE(membership.enabled, false) AS in_project
        FROM project_layout_rules rule
        LEFT JOIN project_memberships membership
          ON membership.project_id = rule.project_id AND membership.actor_id = rule.actor_id
        LEFT JOIN users account ON account.actor_id = rule.actor_id
        WHERE rule.project_id = $1
        ORDER BY rule.role NULLS LAST, rule.role_id NULLS LAST,
                 lower(COALESCE(membership.display_name, account.display_name)), rule.id
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// The role a rule names for someone with this role in the project.
fn rule_role(project_role: &str) -> Option<&'static str> {
    match project_role {
        "owner" | "admin" => Some("admin"),
        "member" => Some("member"),
        "viewer" => Some("viewer"),
        _ => None,
    }
}

#[derive(FromRow)]
struct ProjectAccess {
    organization_id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
    /// The custom role they hold here, if any.
    role_id: Option<Uuid>,
    /// The preset their role (custom or built-in) starts people with.
    role_layout: Option<String>,
    organization_role: String,
    organization_role_permissions: Option<Vec<String>>,
    archived: bool,
}

/// The caller's standing in a project they can see, archived or not.
async fn project_access(
    connection: &mut PgConnection,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<ProjectAccess, AppError> {
    let access = sqlx::query_as::<_, ProjectAccess>(
        r#"
        SELECT project.organization_id, membership.role,
               workspace_role_permissions(membership.role_id) AS role_permissions,
               membership.role_id,
               held.defaults->>'layoutPreset' AS role_layout,
               organization_membership.role AS organization_role,
               workspace_role_permissions(organization_membership.role_id)
                 AS organization_role_permissions,
               project.archived_at IS NOT NULL AS archived
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2 AND organization_membership.enabled
        JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
        LEFT JOIN workspace_roles held
          ON held.organization_id = project.organization_id
         AND (held.id = membership.role_id
              OR (membership.role_id IS NULL AND held.key = membership.role))
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    let subject = Subject::new(
        principal.actor_id,
        settings::actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            project_id,
            settings::membership_grant(&access.role, access.role_permissions.as_deref())?,
            true,
        )),
        None,
    );
    let resource = Resource::Project(ProjectScope::project(project_id));
    if !authorize(&subject, Action::ProjectView, resource).is_allowed() {
        return Err(AppError::NotFound("project"));
    }
    Ok(access)
}

/// Owners and admins (or a role that kept "Manage settings"), as people:
/// who may change a layout, and who may show what one hides.
fn manages(principal: &AuthenticatedActor, role: &str, permissions: Option<&[String]>) -> bool {
    principal.actor_kind == "human"
        && settings::membership_grant(role, permissions)
            .is_ok_and(|grant| grant.permissions().contains(Permission::SettingsManage))
}

async fn project_view(
    connection: &mut PgConnection,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<ProjectLayoutView, AppError> {
    let access = project_access(connection, principal, project_id).await?;
    let manager = manages(principal, &access.role, access.role_permissions.as_deref());
    let stored = Stored::load(
        connection,
        access.organization_id,
        Some(project_id),
        principal.actor_id,
    )
    .await?;
    let rules = rules(connection, project_id).await?;
    let person = rules
        .iter()
        .find(|rule| rule.actor_id == Some(principal.actor_id))
        .map(RuleRow::preset);
    // The project's rule for their custom role, then for its base role, then
    // the preset their role starts people with.
    let role = access
        .role_id
        .and_then(|role_id| rules.iter().find(|rule| rule.role_id == Some(role_id)))
        .or_else(|| {
            rule_role(&access.role)
                .and_then(|role| rules.iter().find(|rule| rule.role.as_deref() == Some(role)))
        })
        .map(RuleRow::preset)
        .or_else(|| access.role_layout.clone());
    let mut also = stored.saved_ids();
    also.extend(rules.iter().filter_map(|rule| rule.saved_preset_id));
    also.extend(role.as_deref().and_then(|role| Uuid::parse_str(role).ok()));
    let saved = saved_presets(connection, access.organization_id, &also).await?;
    let effective = effective(&Inputs {
        person: person.as_deref(),
        role: role.as_deref(),
        project: stored.project.as_ref(),
        workspace: stored.workspace.as_ref(),
        saved: &saved,
        role_hidden: &[],
        my_view: &stored.my_view,
        show_hidden: manager && stored.show_hidden,
    });
    Ok(ProjectLayoutView {
        organization_id: access.organization_id,
        project_id,
        effective,
        layout: stored.project.clone(),
        version: stored.project_version,
        workspace_layout: stored.workspace.clone(),
        rules: if manager {
            rules.iter().map(RuleRow::view).collect()
        } else {
            Vec::new()
        },
        saved_presets: saved,
        my_view: stored.my_view(manager),
        capabilities: LayoutCapabilities {
            manage: manager && !access.archived,
            manage_workspace: manages(
                principal,
                &access.organization_role,
                access.organization_role_permissions.as_deref(),
            ),
        },
    })
}

#[derive(FromRow)]
struct WorkspaceAccess {
    role: String,
    role_permissions: Option<Vec<String>>,
    /// Where their workspace role starts people: a preset, and Home parts
    /// it hides.
    role_layout: Option<String>,
    role_home_hidden: Option<Value>,
}

/// The caller's role in the workspace, if they're an active member.
async fn workspace_role(
    connection: &mut PgConnection,
    actor_id: Uuid,
    organization_id: Uuid,
) -> Result<WorkspaceAccess, AppError> {
    sqlx::query_as::<_, WorkspaceAccess>(
        r#"
        SELECT membership.role,
               workspace_role_permissions(membership.role_id) AS role_permissions,
               held.defaults->>'layoutPreset' AS role_layout,
               held.defaults->'homeHidden' AS role_home_hidden
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        LEFT JOIN workspace_roles held
          ON held.organization_id = membership.organization_id
         AND (held.id = membership.role_id
              OR (membership.role_id IS NULL AND held.key = membership.role))
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled AND actor.status = 'active'
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace"))
}

async fn workspace_view(
    connection: &mut PgConnection,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<WorkspaceLayoutView, AppError> {
    let access = workspace_role(connection, principal.actor_id, organization_id).await?;
    let manager = manages(principal, &access.role, access.role_permissions.as_deref());
    let stored = Stored::load(connection, organization_id, None, principal.actor_id).await?;
    let mut also = stored.saved_ids();
    also.extend(
        access
            .role_layout
            .as_deref()
            .and_then(|role| Uuid::parse_str(role).ok()),
    );
    let saved = saved_presets(connection, organization_id, &also).await?;
    let role_hidden = access
        .role_home_hidden
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
        .unwrap_or_default();
    let effective = effective(&Inputs {
        person: None,
        role: access.role_layout.as_deref(),
        project: None,
        workspace: stored.workspace.as_ref(),
        saved: &saved,
        role_hidden: &role_hidden,
        my_view: &stored.my_view,
        show_hidden: manager && stored.show_hidden,
    });
    Ok(WorkspaceLayoutView {
        organization_id,
        effective,
        layout: stored.workspace.clone(),
        version: stored.workspace_version,
        saved_presets: saved,
        my_view: stored.my_view(manager),
        capabilities: LayoutCapabilities {
            manage: manager,
            manage_workspace: manager,
        },
    })
}

// ---------------------------------------------------------------------------
// Writing

/// Check a layout against the registry and the workspace's presets, and
/// make it minimal: changes and words its preset already has are dropped.
async fn checked_layout(
    connection: &mut PgConnection,
    organization_id: Uuid,
    mut layout: Layout,
) -> Result<Layout, AppError> {
    if !well_formed(&layout) {
        return Err(AppError::Validation("layout"));
    }
    if !layout.changes.keys().all(|part| part_known(part)) {
        return Err(AppError::Validation("layout part"));
    }
    if !layout.words.keys().all(|noun| noun_default(noun).is_some()) {
        return Err(AppError::Validation("layout word"));
    }
    let saved = if builtin(&layout.preset).is_some() {
        Vec::new()
    } else {
        let id =
            Uuid::parse_str(&layout.preset).map_err(|_| AppError::Validation("layout preset"))?;
        let preset = saved_preset(connection, organization_id, id)
            .await?
            .ok_or(AppError::Validation("layout preset"))?;
        layout.preset = id.to_string();
        vec![preset]
    };
    let shape =
        Shape::preset(&layout.preset, &saved).ok_or(AppError::Validation("layout preset"))?;
    layout
        .changes
        .retain(|part, shown| shape.visible.contains(part.as_str()) != *shown);
    layout
        .words
        .retain(|noun, word| shape.word(noun).as_ref() != Some(word));
    Ok(layout)
}

/// Store `layout` as the workspace's or a project's (or remove it, for
/// `None`), checked against the registry and the workspace's presets. The
/// caller has checked who may write. Returns the new version: 0 once removed.
pub async fn write_layout(
    transaction: &mut Transaction<'_, Postgres>,
    target: Target,
    layout: Option<Layout>,
    expected_version: Option<i64>,
    actor_id: Uuid,
) -> Result<i64, AppError> {
    let organization_id = match target {
        Target::Workspace { organization_id }
        | Target::Project {
            organization_id, ..
        } => organization_id,
        Target::User { .. } => return Err(AppError::Internal("layout target")),
    };
    match layout {
        Some(layout) => {
            let layout = checked_layout(transaction, organization_id, layout).await?;
            let value =
                serde_json::to_value(&layout).map_err(|_| AppError::Internal("layout value"))?;
            settings::set(
                transaction,
                target,
                "layout.default",
                value,
                expected_version,
                actor_id,
            )
            .await
        }
        None => {
            settings::reset(transaction, target, "layout.default", expected_version).await?;
            Ok(0)
        }
    }
}

/// A preset named by its built-in id or by ID, as a rule stores it: one of
/// `(builtin_preset, saved_preset_id)`. Saved presets must be this
/// workspace's; removed ones are fine, so a rule that uses one can be kept.
async fn rule_preset(
    connection: &mut PgConnection,
    organization_id: Uuid,
    reference: &str,
) -> Result<(Option<String>, Option<Uuid>), AppError> {
    if builtin(reference).is_some() {
        return Ok((Some(reference.to_owned()), None));
    }
    let id = Uuid::parse_str(reference).map_err(|_| AppError::Validation("rule preset"))?;
    saved_preset(connection, organization_id, id)
        .await?
        .ok_or(AppError::Validation("rule preset"))?;
    Ok((None, Some(id)))
}

/// A saved preset's name: trimmed, 1 to 60 characters on one line.
fn preset_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if (1..=MAX_PRESET_NAME_CHARS).contains(&name.chars().count())
        && !name.chars().any(char::is_control)
    {
        Ok(name.to_owned())
    } else {
        Err(AppError::Validation("preset name"))
    }
}

fn preset_description(description: &str) -> Result<String, AppError> {
    let description = description.trim();
    if description.chars().count() <= MAX_PRESET_DESCRIPTION_CHARS
        && !description.chars().any(char::is_control)
    {
        Ok(description.to_owned())
    } else {
        Err(AppError::Validation("preset description"))
    }
}

/// A layout as a saved preset stores it: built on a built-in preset, with
/// only what differs from it.
async fn flattened(
    connection: &mut PgConnection,
    organization_id: Uuid,
    layout: Layout,
) -> Result<(String, BTreeMap<String, bool>, BTreeMap<String, Word>), AppError> {
    let layout = checked_layout(connection, organization_id, layout).await?;
    if builtin(&layout.preset).is_some() {
        return Ok((layout.preset, layout.changes, layout.words));
    }
    let id = Uuid::parse_str(&layout.preset).map_err(|_| AppError::Validation("layout preset"))?;
    let saved = saved_preset(connection, organization_id, id)
        .await?
        .ok_or(AppError::Validation("layout preset"))?;
    let mut changes = saved.changes;
    changes.extend(layout.changes);
    let mut words = saved.words;
    words.extend(layout.words);
    let base = builtin(&saved.base).map_or_else(Shape::everything, Shape::builtin);
    changes
        .retain(|part, shown| part_known(part) && base.visible.contains(part.as_str()) != *shown);
    words.retain(|noun, word| {
        noun_default(noun).is_some() && base.word(noun).as_ref() != Some(word)
    });
    Ok((saved.base, changes, words))
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("layout json"))
}

// ---------------------------------------------------------------------------
// For agents (project_open and project_layout_set)

/// A project's own layout, or else the workspace's default, as agents see it.
#[derive(Clone, Debug)]
pub struct ProjectTone {
    /// The preset's name, such as "Personal life".
    pub preset: String,
    /// How to write for people in the project.
    pub tone: String,
    /// Nouns the project renames.
    pub words: BTreeMap<String, Word>,
    /// The project follows the workspace's default.
    pub from_workspace: bool,
}

/// The layout agents should match in a project; `None` when nothing was
/// chosen. The caller has checked the project is theirs to see.
pub async fn project_tone(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Option<ProjectTone>, AppError> {
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    let stored = Stored::load(connection, organization_id, Some(project_id), Uuid::nil()).await?;
    let (layout, from_workspace) = match (&stored.project, &stored.workspace) {
        (Some(layout), _) => (layout, false),
        (None, Some(layout)) => (layout, true),
        (None, None) => return Ok(None),
    };
    let saved = saved_presets(connection, organization_id, &stored.saved_ids()).await?;
    let shape = Shape::layout(layout, &saved);
    let Some(preset) = shape.preset.as_ref() else {
        return Ok(None);
    };
    Ok(Some(ProjectTone {
        preset: preset.name.clone(),
        tone: builtin(&preset.base)
            .map(|base| base.tone.clone())
            .unwrap_or_default(),
        words: shape
            .words
            .iter()
            .filter(|(noun, word)| noun_default(noun).is_some_and(|own| own != **word))
            .map(|(noun, word)| (noun.clone(), word.clone()))
            .collect(),
        from_workspace,
    }))
}

/// Find a preset by what someone typed: a built-in one's name or id, or one
/// of the workspace's saved presets by name or ID. Returns the reference a
/// layout stores.
pub async fn find_preset(
    connection: &mut PgConnection,
    organization_id: Uuid,
    text: &str,
) -> Result<Option<String>, AppError> {
    let text = text.trim();
    let wanted = text.to_lowercase();
    let as_id = wanted.replace([' ', '-'], "_");
    if let Some(preset) = registry()
        .presets
        .iter()
        .find(|preset| preset.id == as_id || preset.name.to_lowercase() == wanted)
    {
        return Ok(Some(preset.id.clone()));
    }
    let saved = saved_presets(connection, organization_id, &[]).await?;
    let id = Uuid::parse_str(text).ok();
    Ok(saved
        .iter()
        .find(|preset| Some(preset.id) == id || preset.name.to_lowercase() == wanted)
        .map(|preset| preset.id.to_string()))
}

/// Every preset a project in this workspace can pick, by name.
pub async fn preset_names(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let mut names = registry()
        .presets
        .iter()
        .map(|preset| preset.name.clone())
        .collect::<Vec<_>>();
    names.extend(
        saved_presets(connection, organization_id, &[])
            .await?
            .into_iter()
            .map(|preset| preset.name),
    );
    Ok(names)
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/layout",
            get(project_layout).put(change_project_layout),
        )
        .route(
            "/api/v1/projects/{project_id}/layout/rules",
            put(change_project_rules),
        )
        .route(
            "/api/v1/organizations/{organization_id}/layout",
            get(workspace_layout).put(change_workspace_layout),
        )
        .route(
            "/api/v1/organizations/{organization_id}/layout/mine",
            put(change_my_view),
        )
        .route(
            "/api/v1/organizations/{organization_id}/layout/presets",
            post(create_preset),
        )
        .route(
            "/api/v1/organizations/{organization_id}/layout/presets/{preset_id}",
            patch(change_preset).delete(remove_preset),
        )
}

/// `GET /api/v1/projects/{project_id}/layout`: what the caller sees in the
/// project, and the layout settings behind it.
async fn project_layout(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProjectLayoutView>, AppError> {
    let mut connection = state.pool.acquire().await?;
    Ok(Json(
        project_view(&mut connection, &principal, project_id).await?,
    ))
}

/// `GET /api/v1/organizations/{organization_id}/layout`: the workspace's
/// default as the caller sees it on workspace pages.
async fn workspace_layout(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<WorkspaceLayoutView>, AppError> {
    let mut connection = state.pool.acquire().await?;
    Ok(Json(
        workspace_view(&mut connection, &principal, organization_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutRequest {
    /// The new layout; null to fall back to the next one down.
    layout: Option<Layout>,
    #[serde(default)]
    expected_version: Option<i64>,
}

/// `PUT /api/v1/projects/{project_id}/layout`: owners and admins.
async fn change_project_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<LayoutRequest>,
) -> Result<Json<ProjectLayoutView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let organization_id =
        settings::authorize_project_manage(&mut *transaction, &principal, project_id).await?;
    let preset = request.layout.as_ref().map(|layout| layout.preset.clone());
    let version = write_layout(
        &mut transaction,
        Target::Project {
            organization_id,
            project_id,
        },
        request.layout,
        request.expected_version,
        principal.actor_id,
    )
    .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "layout.project_changed",
        "project",
        Some(project_id),
        json!({ "preset": preset, "version": version, "organizationId": organization_id }),
    )
    .await?;
    let view = project_view(&mut transaction, &principal, project_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuleInput {
    /// admin (owners too), member or viewer.
    #[serde(default)]
    role: Option<String>,
    /// One of the workspace's custom roles.
    #[serde(default)]
    role_id: Option<Uuid>,
    /// One person in the project.
    #[serde(default)]
    actor_id: Option<Uuid>,
    /// A built-in preset's id or a saved preset's ID.
    preset: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RulesRequest {
    /// Every rule the project should have; others are removed.
    rules: Vec<RuleInput>,
}

/// `PUT /api/v1/projects/{project_id}/layout/rules`: owners and admins set
/// which preset each role, or each named person, gets.
async fn change_project_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RulesRequest>,
) -> Result<Json<ProjectLayoutView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.rules.len() > MAX_RULES {
        return Err(AppError::Validation("rules"));
    }
    let mut transaction = state.pool.begin().await?;
    // Lock the project first, as membership changes do, so two saves of the
    // rules don't interleave.
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
    let organization_id =
        settings::authorize_project_manage(&mut *transaction, &principal, project_id).await?;
    let existing = rules(&mut transaction, project_id).await?;

    // People must be people in the project, or already have a rule here.
    let people = request
        .rules
        .iter()
        .filter_map(|rule| rule.actor_id)
        .collect::<Vec<_>>();
    let mut allowed = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.actor_id
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.kind = 'human'
        WHERE membership.project_id = $1 AND membership.enabled AND membership.actor_id = ANY($2)
        "#,
    )
    .bind(project_id)
    .bind(&people)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    allowed.extend(existing.iter().filter_map(|rule| rule.actor_id));

    // Custom roles must be this workspace's.
    let custom_roles = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workspace_roles WHERE organization_id = $1 AND NOT built_in",
    )
    .bind(organization_id)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();

    let mut seen_roles = BTreeSet::new();
    let mut seen_custom_roles = BTreeSet::new();
    let mut seen_people = BTreeSet::new();
    let mut kept = Vec::new();
    for rule in &request.rules {
        match (rule.role.as_deref(), rule.role_id, rule.actor_id) {
            (Some(role @ ("admin" | "member" | "viewer")), None, None) => {
                if !seen_roles.insert(role) {
                    return Err(AppError::Validation("rule role"));
                }
            }
            (None, Some(role_id), None) => {
                if !custom_roles.contains(&role_id) || !seen_custom_roles.insert(role_id) {
                    return Err(AppError::Validation("rule role"));
                }
            }
            (None, None, Some(actor_id)) => {
                if !allowed.contains(&actor_id) || !seen_people.insert(actor_id) {
                    return Err(AppError::Validation("rule person"));
                }
            }
            _ => return Err(AppError::Validation("rule")),
        }
        let (builtin_preset, saved_preset_id) =
            rule_preset(&mut transaction, organization_id, &rule.preset).await?;
        let current = existing.iter().find(|candidate| {
            candidate.role.as_deref() == rule.role.as_deref()
                && candidate.role_id == rule.role_id
                && candidate.actor_id == rule.actor_id
        });
        match current {
            Some(current)
                if current.builtin_preset == builtin_preset
                    && current.saved_preset_id == saved_preset_id =>
            {
                kept.push(current.id);
            }
            Some(current) => {
                sqlx::query(
                    r#"
                    UPDATE project_layout_rules
                    SET builtin_preset = $2, saved_preset_id = $3, version = version + 1,
                        updated_by_actor_id = $4, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(current.id)
                .bind(&builtin_preset)
                .bind(saved_preset_id)
                .bind(principal.actor_id)
                .execute(&mut *transaction)
                .await?;
                kept.push(current.id);
            }
            None => {
                let id = Uuid::now_v7();
                sqlx::query(
                    r#"
                    INSERT INTO project_layout_rules (
                        id, organization_id, project_id, role, actor_id, builtin_preset,
                        saved_preset_id, created_by_actor_id, updated_by_actor_id, role_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9)
                    "#,
                )
                .bind(id)
                .bind(organization_id)
                .bind(project_id)
                .bind(rule.role.as_deref())
                .bind(rule.actor_id)
                .bind(&builtin_preset)
                .bind(saved_preset_id)
                .bind(principal.actor_id)
                .bind(rule.role_id)
                .execute(&mut *transaction)
                .await?;
                kept.push(id);
            }
        }
    }
    sqlx::query("DELETE FROM project_layout_rules WHERE project_id = $1 AND NOT (id = ANY($2))")
        .bind(project_id)
        .bind(&kept)
        .execute(&mut *transaction)
        .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "layout.rules_changed",
        "project",
        Some(project_id),
        json!({
            "roles": seen_roles.into_iter().collect::<Vec<_>>(),
            "customRoles": seen_custom_roles.into_iter().collect::<Vec<_>>(),
            "people": seen_people.len(),
            "organizationId": organization_id,
        }),
    )
    .await?;
    let view = project_view(&mut transaction, &principal, project_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

/// `PUT /api/v1/organizations/{organization_id}/layout`: the workspace's
/// default, for owners and admins.
async fn change_workspace_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<LayoutRequest>,
) -> Result<Json<WorkspaceLayoutView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    settings::authorize_workspace_manage(&mut *transaction, &principal, organization_id).await?;
    let preset = request.layout.as_ref().map(|layout| layout.preset.clone());
    let version = write_layout(
        &mut transaction,
        Target::Workspace { organization_id },
        request.layout,
        request.expected_version,
        principal.actor_id,
    )
    .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "layout.workspace_changed",
        "organization",
        Some(organization_id),
        json!({ "preset": preset, "version": version, "organizationId": organization_id }),
    )
    .await?;
    let view = workspace_view(&mut transaction, &principal, organization_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MyViewRequest {
    /// Parts to hide for yourself, on top of any layout.
    hidden: Vec<String>,
    /// Show the parts layouts hide, where you're an owner or admin.
    show_hidden: bool,
}

/// `PUT /api/v1/organizations/{organization_id}/layout/mine`: the caller's
/// own view, across the workspace.
async fn change_my_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<MyViewRequest>,
) -> Result<Json<WorkspaceLayoutView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let wanted = request
        .hidden
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if wanted.len() != request.hidden.len() || !wanted.iter().all(|part| part_known(part)) {
        return Err(AppError::Validation("hidden parts"));
    }
    let hidden = registry()
        .parts
        .iter()
        .filter(|part| wanted.contains(part.id.as_str()))
        .map(|part| part.id.clone())
        .collect::<Vec<_>>();
    let mut transaction = state.pool.begin().await?;
    settings::authorize_personal(&mut *transaction, &principal, organization_id).await?;
    let target = Target::User {
        organization_id,
        actor_id: principal.actor_id,
    };
    settings::set(
        &mut transaction,
        target,
        "layout.my_view",
        json!(hidden),
        None,
        principal.actor_id,
    )
    .await?;
    settings::set(
        &mut transaction,
        target,
        "layout.show_hidden",
        json!(request.show_hidden),
        None,
        principal.actor_id,
    )
    .await?;
    let view = workspace_view(&mut transaction, &principal, organization_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetRequest {
    name: String,
    #[serde(default)]
    description: String,
    /// The layout to save: a preset with its changes and words.
    layout: Layout,
}

fn unique_name(error: sqlx::Error) -> AppError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            AppError::Conflict("layout preset", "name_taken")
        }
        _ => AppError::Database(error),
    }
}

/// `POST /api/v1/organizations/{organization_id}/layout/presets`: "Save as
/// preset", for the workspace's owners and admins.
async fn create_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<PresetRequest>,
) -> Result<(StatusCode, Json<SavedPreset>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = preset_name(&request.name)?;
    let description = preset_description(&request.description)?;
    let mut transaction = state.pool.begin().await?;
    settings::authorize_workspace_manage(&mut *transaction, &principal, organization_id).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM layout_presets WHERE organization_id = $1 AND removed_at IS NULL",
    )
    .bind(organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    if count >= MAX_SAVED_PRESETS {
        return Err(AppError::Conflict("layout preset", "too_many"));
    }
    let (base, changes, words) =
        flattened(&mut transaction, organization_id, request.layout).await?;
    let id = Uuid::now_v7();
    let preset = sqlx::query_as::<_, SavedRow>(&format!(
        r#"
        INSERT INTO layout_presets (
            id, organization_id, name, description, base, changes, words,
            created_by_actor_id, updated_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
        RETURNING {SAVED_COLUMNS}
        "#
    ))
    .bind(id)
    .bind(organization_id)
    .bind(&name)
    .bind(&description)
    .bind(&base)
    .bind(to_json(&changes)?)
    .bind(to_json(&words)?)
    .bind(principal.actor_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(unique_name)?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "layout.preset_created",
        "layout_preset",
        Some(id),
        json!({ "name": name, "base": base, "organizationId": organization_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(preset.into())))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetChange {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// Replace what it shows and its words with this layout's.
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    expected_version: Option<i64>,
}

/// The caller may change the workspace's presets, and this one is the
/// workspace's (another workspace's is "not found"). Returns its version.
async fn lock_preset(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
    preset_id: Uuid,
) -> Result<i64, AppError> {
    settings::authorize_workspace_manage(&mut **transaction, principal, organization_id).await?;
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT version FROM layout_presets
        WHERE id = $1 AND organization_id = $2 AND removed_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(preset_id)
    .bind(organization_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("layout preset"))
}

/// `PATCH /api/v1/organizations/{organization_id}/layout/presets/{preset_id}`.
async fn change_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, preset_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<PresetChange>,
) -> Result<Json<SavedPreset>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = request.name.as_deref().map(preset_name).transpose()?;
    let description = request
        .description
        .as_deref()
        .map(preset_description)
        .transpose()?;
    let mut transaction = state.pool.begin().await?;
    let version = lock_preset(&mut transaction, &principal, organization_id, preset_id).await?;
    if let Some(expected) = request.expected_version
        && expected != version
    {
        return Err(AppError::VersionConflict {
            resource: "layout preset",
            current_version: version,
        });
    }
    let shape = match request.layout {
        Some(layout) => Some(flattened(&mut transaction, organization_id, layout).await?),
        None => None,
    };
    let (base, changes, words) = match &shape {
        Some((base, changes, words)) => (
            Some(base.clone()),
            Some(to_json(changes)?),
            Some(to_json(words)?),
        ),
        None => (None, None, None),
    };
    let preset = sqlx::query_as::<_, SavedRow>(&format!(
        r#"
        UPDATE layout_presets
        SET name = COALESCE($3, name), description = COALESCE($4, description),
            base = COALESCE($5, base), changes = COALESCE($6, changes),
            words = COALESCE($7, words), version = version + 1,
            updated_by_actor_id = $8, updated_at = now()
        WHERE id = $1 AND organization_id = $2
        RETURNING {SAVED_COLUMNS}
        "#
    ))
    .bind(preset_id)
    .bind(organization_id)
    .bind(&name)
    .bind(&description)
    .bind(&base)
    .bind(changes)
    .bind(words)
    .bind(principal.actor_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(unique_name)?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "layout.preset_changed",
        "layout_preset",
        Some(preset_id),
        json!({ "name": preset.name, "version": preset.version, "organizationId": organization_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(preset.into()))
}

/// `DELETE /api/v1/organizations/{organization_id}/layout/presets/{preset_id}`:
/// takes it out of pickers. Layouts and rules that use it keep working.
async fn remove_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, preset_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    lock_preset(&mut transaction, &principal, organization_id, preset_id).await?;
    let name = sqlx::query_scalar::<_, String>(
        r#"
        UPDATE layout_presets
        SET removed_at = now(), version = version + 1, updated_by_actor_id = $3,
            updated_at = now()
        WHERE id = $1 AND organization_id = $2
        RETURNING name
        "#,
    )
    .bind(preset_id)
    .bind(organization_id)
    .bind(principal.actor_id)
    .fetch_one(&mut *transaction)
    .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "layout.preset_removed",
        "layout_preset",
        Some(preset_id),
        json!({ "name": name, "organizationId": organization_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed() -> Registry {
        serde_json::from_str(REGISTRY_JSON).expect("registry.json parses")
    }

    fn hidden(effective: &Effective) -> BTreeSet<&'static str> {
        effective.hidden.iter().copied().collect()
    }

    fn inputs<'a>(saved: &'a [SavedPreset], my_view: &'a [String]) -> Inputs<'a> {
        Inputs {
            person: None,
            role: None,
            project: None,
            workspace: None,
            saved,
            role_hidden: &[],
            my_view,
            show_hidden: false,
        }
    }

    #[test]
    fn the_registry_is_well_formed() {
        let registry = parsed();
        let groups = registry
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(groups.len(), registry.groups.len(), "a group id twice");
        let mut parts = BTreeSet::new();
        for part in &registry.parts {
            assert!(is_part_id(&part.id), "{} isn't a part id", part.id);
            assert!(parts.insert(part.id.as_str()), "{} twice", part.id);
            assert!(
                groups.contains(part.group.as_str()),
                "{}: no group {}",
                part.id,
                part.group
            );
        }
        let mut nouns = BTreeSet::new();
        for noun in &registry.nouns {
            assert!(
                is_name(&noun.id) && nouns.insert(noun.id.as_str()),
                "{}",
                noun.id
            );
            assert!(is_word(&noun.one) && is_word(&noun.many), "{}", noun.id);
        }
        assert!(registry.presets.len() >= 15, "at least 15 presets");
        let mut presets = BTreeSet::new();
        for preset in &registry.presets {
            let id = preset.id.as_str();
            assert!(is_name(id) && presets.insert(id), "{id}");
            assert!(
                !preset.name.is_empty() && !preset.description.is_empty(),
                "{id}"
            );
            assert!(!preset.tone.is_empty(), "{id} needs a tone for agents");
            assert!(
                preset.hide.is_some() != preset.only.is_some(),
                "{id}: give exactly one of hide and only"
            );
            for part in preset
                .hide
                .iter()
                .chain(preset.only.iter())
                .flatten()
                .chain(preset.lead.iter())
            {
                assert!(parts.contains(part.as_str()), "{id}: unknown part {part}");
            }
            for (noun, word) in &preset.words {
                assert!(nouns.contains(noun.as_str()), "{id}: unknown noun {noun}");
                assert!(is_word(&word.one) && is_word(&word.many), "{id}: {noun}");
            }
        }
    }

    #[test]
    fn software_team_shows_everything_and_nothing_chosen_does_too() {
        let everything = Shape::everything();
        assert_eq!(everything.visible.len(), registry().parts.len());
        let software = builtin("software_team")
            .map(Shape::builtin)
            .expect("software_team");
        assert_eq!(software.visible, everything.visible);
        let effective = effective(&inputs(&[], &[]));
        assert_eq!(effective.source, LayoutSource::Default);
        assert!(effective.hidden.is_empty() && effective.preset.is_none());
    }

    #[test]
    fn the_most_specific_layout_wins() {
        let saved = Vec::new();
        let workspace = Layout::preset("personal_life");
        let project = Layout::preset("kanban");
        let mut chosen = inputs(&saved, &[]);
        chosen.workspace = Some(&workspace);
        assert_eq!(effective(&chosen).source, LayoutSource::Workspace);
        chosen.project = Some(&project);
        let by_project = effective(&chosen);
        assert_eq!(by_project.source, LayoutSource::Project);
        assert_eq!(by_project.words["task"].many, "cards");
        chosen.role = Some("stakeholder");
        let by_role = effective(&chosen);
        assert_eq!(by_role.source, LayoutSource::Role);
        // Stakeholder renames nothing, so the project's words stay.
        assert_eq!(by_role.words["task"].many, "cards");
        assert!(by_role.hidden.contains(&"area.board"));
        chosen.person = Some("bug_reporting");
        let by_person = effective(&chosen);
        assert_eq!(by_person.source, LayoutSource::Person);
        assert_eq!(by_person.words["task"].one, "bug");
        assert!(!by_person.hidden.contains(&"area.tasks"));
        assert!(by_person.hidden.contains(&"area.chat"));
        // A rule naming a preset that's gone falls through.
        chosen.person = Some("no_such_preset");
        assert_eq!(effective(&chosen).source, LayoutSource::Role);
    }

    #[test]
    fn my_view_only_hides_and_show_hidden_only_undoes_the_layout() {
        let saved = Vec::new();
        let project = Layout::preset("minimal");
        let mine = vec!["area.tasks".to_owned()];
        let mut chosen = inputs(&saved, &mine);
        chosen.project = Some(&project);
        let plain = effective(&chosen);
        assert!(plain.hidden.contains(&"area.tasks") && plain.hidden.contains(&"area.runs"));
        assert!(!plain.layout_hidden.contains(&"area.tasks"));
        chosen.show_hidden = true;
        let showing = effective(&chosen);
        assert_eq!(hidden(&showing), BTreeSet::from(["area.tasks"]));
        assert!(showing.layout_hidden.contains(&"area.runs") && showing.showing_hidden);
    }

    #[test]
    fn saved_presets_and_changes_build_on_their_base() {
        let id = Uuid::now_v7();
        let saved = vec![SavedPreset {
            id,
            name: "Chores".to_owned(),
            description: String::new(),
            base: "household".to_owned(),
            changes: BTreeMap::from([("area.runs".to_owned(), true)]),
            words: BTreeMap::from([(
                "task".to_owned(),
                Word {
                    one: "chore".to_owned(),
                    many: "chores".to_owned(),
                },
            )]),
            version: 1,
            removed: true,
        }];
        let mut layout = Layout::preset(id.to_string());
        layout.changes.insert("area.chat".to_owned(), false);
        let project = layout;
        let mut chosen = inputs(&saved, &[]);
        chosen.project = Some(&project);
        let effective = effective(&chosen);
        let shown = |part| !effective.hidden.contains(&part);
        assert!(shown("area.runs") && shown("area.pages") && !shown("area.chat"));
        assert!(!shown("area.code"));
        assert_eq!(effective.words["task"].many, "chores");
        // Household's own word for pages stays.
        assert_eq!(effective.words["page"].one, "note");
        let preset = effective.preset.expect("preset");
        assert!(!preset.built_in && preset.removed && preset.name == "Chores");
        // Another workspace's preset isn't among `saved`: everything shows.
        let elsewhere = Layout::preset(Uuid::now_v7().to_string());
        let mut chosen = inputs(&[], &[]);
        chosen.project = Some(&elsewhere);
        assert!(super::effective(&chosen).hidden.is_empty());
    }

    #[test]
    fn stored_values_are_checked_for_shape() {
        assert!(valid_layout_value(&Value::Null));
        assert!(valid_layout_value(&json!({"preset": "personal_life"})));
        assert!(valid_layout_value(&json!({
            "preset": Uuid::now_v7().to_string(),
            "changes": {"area.runs": true},
            "words": {"task": {"one": "to-do", "many": "to-dos"}},
        })));
        assert!(!valid_layout_value(&json!({"preset": "Personal Life"})));
        assert!(!valid_layout_value(
            &json!({"preset": "minimal", "extra": 1})
        ));
        assert!(!valid_layout_value(
            &json!({"preset": "minimal", "changes": {"runs": true}})
        ));
        assert!(!valid_layout_value(&json!({
            "preset": "minimal",
            "words": {"task": {"one": " to-do", "many": "to-dos"}},
        })));
        assert!(!valid_layout_value(&json!({
            "preset": "minimal",
            "words": {"task": {"one": "x".repeat(31), "many": "to-dos"}},
        })));
        assert!(valid_part_list(&json!([])));
        assert!(valid_part_list(&json!(["area.runs", "task.type"])));
        assert!(!valid_part_list(&json!(["area.runs", "area.runs"])));
        assert!(!valid_part_list(&json!(["runs"])));
        assert!(!valid_part_list(&json!("area.runs")));
    }
}
