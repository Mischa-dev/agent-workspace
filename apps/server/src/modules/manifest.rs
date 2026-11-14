//! A module's manifest: what it adds and which slots it uses to add it.
//!
//! Core modules' manifests are the JSON files in `modules/` at the root of
//! the repository, which the server and the web app both read. Extensions
//! will fill in the same manifest; a core module's only privilege is that
//! its code runs in this process.

use std::{collections::BTreeMap, sync::OnceLock};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::contract::{EmbedDecl, EventDecl, LinkDecl, ThingDecl, ToolVerb};

/// Who makes a module and where its code runs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    /// Written in this repository; runs in the server.
    Core,
    /// Assembled from fields, collections, views and rules; no code (later).
    Built,
    /// An outside app with its own identity and approved scopes (later).
    Connected,
}

/// What a module's automations offer: the triggers and actions it brings.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDecl {
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
}

/// Where a module shows up. Every entry point names its module by being
/// listed here, and the guard tests fail on one that isn't. A trailing `*`
/// matches anything after it; the longest match wins.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Slots {
    /// Web app routes, as `apps/web/src/app/App.tsx` writes their paths.
    #[serde(default)]
    pub pages: Vec<String>,
    /// Layout parts (`apps/web/src/experience/registry.json`): sidebar rows,
    /// tabs, task fields and composer extras.
    #[serde(default)]
    pub parts: Vec<String>,
    /// Rail and sidebar rows that aren't layout parts.
    #[serde(default)]
    pub sidebar: Vec<String>,
    /// Home sections.
    #[serde(default)]
    pub home: Vec<String>,
    /// Settings pages, as `<area>/<id>`.
    #[serde(default)]
    pub settings: Vec<String>,
    /// Command palette entries, by id.
    #[serde(default)]
    pub palette: Vec<String>,
    /// Keyboard shortcuts, by action.
    #[serde(default)]
    pub shortcuts: Vec<String>,
    /// Slash commands in the message box, by name.
    #[serde(default)]
    pub slash: Vec<String>,
    /// Setup steps (`onboarding::STEPS`).
    #[serde(default)]
    pub setup: Vec<String>,
}

/// One module.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Stable, lower case: `tasks`.
    pub id: String,
    pub name: String,
    /// One or two sentences on what it's for.
    pub description: String,
    pub version: u32,
    /// The manifest format: 2 adds `order`; absent means 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<u32>,
    /// Where the module comes in lists, before the id breaks ties. A module
    /// always comes after the ones it needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    pub kind: ModuleKind,
    /// Whether a workspace can turn it off. Only the workspace itself can't.
    pub switchable: bool,
    /// Modules it can't work without; it's off whenever one of them is.
    #[serde(default)]
    pub needs: Vec<String>,
    /// Modules it does more with, when they're on.
    #[serde(default)]
    pub works_with: Vec<String>,
    /// Permission keys it owns, which the Roles page groups under it.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Its agent tools (MCP and the public API) and what each does.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolVerb>,
    /// Its server routes, as the router writes their paths.
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub events: EventDecl,
    /// Notification kinds it sends.
    #[serde(default)]
    pub notifications: Vec<String>,
    /// Search kinds it answers.
    #[serde(default)]
    pub search: Vec<String>,
    #[serde(default)]
    pub automations: AutomationDecl,
    #[serde(default)]
    pub things: Vec<ThingDecl>,
    #[serde(default)]
    pub links: Vec<LinkDecl>,
    #[serde(default)]
    pub embeds: Vec<EmbedDecl>,
    #[serde(default)]
    pub slots: Slots,
    /// Setting keys it owns.
    #[serde(default)]
    pub settings: Vec<String>,
    /// Nouns a layout may rename.
    #[serde(default)]
    pub nouns: Vec<String>,
}

/// Every core module, in the registry's order (the order screens list
/// them). Add a module by adding its folder, `modules/core/<id>/mod.rs`.
pub fn core() -> &'static [Manifest] {
    super::registry::Registry::global().descriptions()
}

pub fn get(id: &str) -> Option<&'static Manifest> {
    super::registry::Registry::global().describe(id)
}

pub(super) fn index_of(id: &str) -> Option<usize> {
    super::registry::Registry::global().index_of(id)
}

/// Whether `pattern` (exact, or ending in `*` for a prefix) matches `value`,
/// and how specific the match is.
pub fn pattern_match(pattern: &str, value: &str) -> Option<usize> {
    match pattern.strip_suffix('*') {
        Some(prefix) if value.starts_with(prefix) => Some(prefix.len()),
        Some(_) => None,
        None if pattern == value => Some(pattern.len() + 1),
        None => None,
    }
}

/// The module whose patterns in `list` match `value` most specifically.
pub fn owner_by(list: fn(&Manifest) -> &[String], value: &str) -> Option<&'static Manifest> {
    core()
        .iter()
        .filter_map(|manifest| {
            list(manifest)
                .iter()
                .filter_map(|pattern| pattern_match(pattern, value))
                .max()
                .map(|score| (score, manifest))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, manifest)| manifest)
}

/// The module a tool belongs to.
pub fn of_tool(tool: &str) -> Option<&'static Manifest> {
    super::registry::Registry::global().module_of_tool(tool)
}

/// The module a server route belongs to.
pub fn of_route(path: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.routes, path)
}

pub fn of_part(part: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.slots.parts, part)
}

pub fn of_setting(key: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.settings, key)
}

pub fn of_setup_step(step: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.slots.setup, step)
}

pub fn of_notification(kind: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.notifications, kind)
}

pub fn of_search(kind: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.search, kind)
}

pub fn of_trigger(kind: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.automations.triggers, kind)
}

pub fn of_action(kind: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.automations.actions, kind)
}

pub fn of_event(event_type: &str) -> Option<&'static Manifest> {
    owner_by(|manifest| &manifest.events.emits, event_type)
}

pub fn of_thing(kind: &str) -> Option<&'static Manifest> {
    core()
        .iter()
        .find(|manifest| manifest.things.iter().any(|thing| thing.kind == kind))
}

/// The declaration of a kind of thing.
pub fn thing_decl(kind: &str) -> Option<&'static ThingDecl> {
    core()
        .iter()
        .flat_map(|manifest| manifest.things.iter())
        .find(|thing| thing.kind == kind)
}

/// Presets: which switchable modules are on in each. The rest are off.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModulePreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub on: Vec<String>,
}

pub fn presets() -> &'static [ModulePreset] {
    static PRESETS: OnceLock<Vec<ModulePreset>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../modules/presets.json")).unwrap_or_default()
    })
}

pub fn preset(id: &str) -> Option<&'static ModulePreset> {
    presets().iter().find(|preset| preset.id == id)
}
