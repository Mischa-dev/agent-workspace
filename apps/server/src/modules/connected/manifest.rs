//! The connected-module manifest (`agent-workspace-module/1`, M1 plan 3.7):
//! what an outside app publishes at its own address to become a module.
//! Read here, checked whole, and every problem reported with where it is.
//!
//! Unknown fields are kept out of nothing: a newer manifest may carry more,
//! and this server reads what it knows. A scope it doesn't know can't be
//! approved; a required one it doesn't know stops the install.

use std::collections::BTreeSet;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::access::catalogue::Permission;

pub const FORMAT: &str = "agent-workspace-module/1";
/// The most a manifest may weigh.
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_SCOPES: usize = 50;
const MAX_OPERATIONS: usize = 100;
const MAX_EVENTS: usize = 100;
const MAX_KINDS: usize = 20;
const MAX_PANELS: usize = 20;
const MAX_OFFERS: usize = 20;
pub const MAX_FRAME_ORIGINS: usize = 4;

/// The places a panel may be drawn.
pub const SLOTS: &[&str] = &[
    "space.tab",
    "home.card",
    "settings.module",
    "conversation.tab",
    "agents.create",
    "agents.detail",
];
/// A slot beside a thing of one kind: `thing.side:<kind>`.
const THING_SIDE: &str = "thing.side:";

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    /// `publisher.module`, the module's key here.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: Option<Publisher>,
    /// Where every relative address below starts; on the manifest's own
    /// origin.
    pub base_url: String,
    /// Where the one-time install code is sent, relative to baseUrl.
    pub install: String,
    pub account: AccountSpec,
    #[serde(default)]
    pub scopes: Vec<ScopeRequest>,
    #[serde(default)]
    pub kinds: Vec<KindSpec>,
    #[serde(default)]
    pub operations: Vec<OperationSpec>,
    #[serde(default)]
    pub events: EventsSpec,
    #[serde(default)]
    pub panels: Vec<PanelSpec>,
    #[serde(default)]
    #[schemars(with = "Vec<Value>")]
    pub offers: Vec<Value>,
    #[serde(default)]
    pub frame_origins: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Value>")]
    pub file_source: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Publisher {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountSpec {
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRequest {
    /// A permission key (`tasks.edit`) or `events:<type>`.
    pub scope: String,
    pub why: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KindSpec {
    pub kind: String,
    pub one: String,
    pub many: String,
    #[serde(default)]
    #[schemars(with = "Vec<Value>")]
    pub fields: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSpec {
    pub name: String,
    /// `query`, `describe`, `create` or `change`.
    pub verb: String,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[schemars(with = "Value")]
    pub input: Value,
    pub endpoint: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventsSpec {
    #[serde(default)]
    pub emits: Vec<EmittedEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmittedEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelSpec {
    pub id: String,
    pub slot: String,
    pub title: String,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub frame_only: bool,
}

/// A scope as this server reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scope {
    Permission(Permission),
    /// Hearing events of one type (M1.13 sends them).
    Events(String),
    /// A permission this server doesn't have: never approvable.
    Unknown,
}

impl Scope {
    pub fn read(text: &str) -> Self {
        if let Some(event) = text.strip_prefix("events:") {
            return if event_type(event) {
                Self::Events(event.to_owned())
            } else {
                Self::Unknown
            };
        }
        Permission::from_key(text).map_or(Self::Unknown, Self::Permission)
    }
}

/// One thing wrong with a manifest, and where.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

fn problem(path: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem {
        path: path.into(),
        message: message.into(),
    }
}

fn one_line(text: &str, max: usize) -> bool {
    let count = text.chars().count();
    (1..=max).contains(&count) && text == text.trim() && !text.chars().any(char::is_control)
}

fn segment(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && text.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `publisher.module`: two segments of lowercase letters, digits and
/// underscores, as `modules.module_key` takes them.
pub fn module_key(text: &str) -> bool {
    text.split_once('.')
        .is_some_and(|(left, right)| segment(left) && segment(right))
}

/// `acme.build.finished`: dotted lowercase segments.
fn event_type(text: &str) -> bool {
    text.len() <= 100 && text.split('.').count() >= 2 && text.split('.').all(segment)
}

/// A path under baseUrl: no scheme, no leading slash, no `..`.
fn relative_path(text: &str) -> bool {
    (1..=200).contains(&text.len())
        && !text.starts_with('/')
        && !text.split('/').any(|part| part == ".." || part == ".")
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
}

/// An https origin with nothing after it: `https://host[:port]`.
fn https_origin(text: &str, allow_http: bool) -> bool {
    let Ok(url) = Url::parse(text) else {
        return false;
    };
    (url.scheme() == "https" || (allow_http && url.scheme() == "http"))
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.origin().ascii_serialization() == text.trim_end_matches('/')
}

/// The manifest's address, as fetched: http only where the operator
/// trusts that origin.
pub fn manifest_address(text: &str, trusted_origins: &[String]) -> Option<Url> {
    let url = Url::parse(text.trim()).ok()?;
    let trusted = trusted_origins.contains(&url.origin().ascii_serialization());
    let fine = (url.scheme() == "https" || (trusted && url.scheme() == "http"))
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && text.len() <= 2000;
    fine.then_some(url)
}

/// A manifest checked against the address it came from.
#[derive(Clone, Debug)]
pub struct Checked {
    pub manifest: Manifest,
    pub base_url: Url,
    pub install_url: Url,
}

impl Checked {
    /// The prefix its operations, kinds and events carry: the id's first
    /// segment, so no app can name anything this server or another
    /// publisher owns.
    pub fn prefix(&self) -> &str {
        self.manifest
            .id
            .split_once('.')
            .map_or("", |(publisher, _)| publisher)
    }

    /// The scopes it asks for, read.
    pub fn scopes(&self) -> Vec<(ScopeRequest, Scope)> {
        self.manifest
            .scopes
            .iter()
            .map(|request| (request.clone(), Scope::read(&request.scope)))
            .collect()
    }

    pub fn scope_keys(&self) -> Vec<String> {
        self.manifest
            .scopes
            .iter()
            .map(|request| request.scope.clone())
            .collect()
    }

    pub fn frame_origins(&self) -> Vec<String> {
        self.manifest
            .frame_origins
            .iter()
            .map(|origin| origin.trim_end_matches('/').to_owned())
            .collect()
    }
}

/// Reads and checks the bytes fetched from `address`.
pub fn check(bytes: &[u8], address: &Url, allow_http: bool) -> Result<Checked, Vec<Problem>> {
    if bytes.len() > MAX_BYTES {
        return Err(vec![problem("", "The manifest is larger than 64 KB.")]);
    }
    let value = serde_json::from_slice::<Value>(bytes)
        .map_err(|_| vec![problem("", "The address doesn't serve JSON.")])?;
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(vec![problem(
            "format",
            format!("A connected module's manifest says \"format\": \"{FORMAT}\"."),
        )]);
    }
    let manifest = serde_json::from_value::<Manifest>(value)
        .map_err(|error| vec![problem("", format!("The manifest can't be read: {error}."))])?;
    let mut problems = Vec::new();
    if !module_key(&manifest.id) || manifest.id.starts_with("local.") {
        problems.push(problem(
            "id",
            "Name the module publisher.module, in lowercase letters, digits and underscores \
             (local. is for modules built here).",
        ));
    }
    if !one_line(&manifest.name, 60) {
        problems.push(problem(
            "name",
            "Name it in 1 to 60 characters, on one line.",
        ));
    }
    if !one_line(&manifest.version, 40) {
        problems.push(problem("version", "Give a version of 1 to 40 characters."));
    }
    if manifest.description.chars().count() > 500 {
        problems.push(problem(
            "description",
            "Describe it in 500 characters or fewer.",
        ));
    }
    if let Some(publisher) = &manifest.publisher
        && (!one_line(&publisher.name, 80)
            || publisher
                .url
                .as_deref()
                .is_some_and(|url| Url::parse(url).map_or(true, |url| url.scheme() != "https")))
    {
        problems.push(problem(
            "publisher",
            "Name the publisher in 1 to 80 characters, with an https address if any.",
        ));
    }
    let base_url = match Url::parse(&manifest.base_url) {
        Ok(base)
            if base.origin() == address.origin()
                && base.path().ends_with('/')
                && base.query().is_none()
                && base.fragment().is_none()
                && base.username().is_empty()
                && base.password().is_none()
                && (base.scheme() == "https" || allow_http) =>
        {
            Some(base)
        }
        _ => {
            problems.push(problem(
                "baseUrl",
                "baseUrl is on the manifest's own origin, ends with /, and has no query.",
            ));
            None
        }
    };
    let joined = |path: &str| -> Option<Url> {
        let base = base_url.as_ref()?;
        let url = base.join(path).ok()?;
        (relative_path(path) && url.as_str().starts_with(base.as_str())).then_some(url)
    };
    let install_url = joined(&manifest.install);
    if base_url.is_some() && install_url.is_none() {
        problems.push(problem(
            "install",
            "install is a path under baseUrl, such as \"install\".",
        ));
    }
    if !one_line(&manifest.account.name, 80) {
        problems.push(problem(
            "account.name",
            "Name the app's account in 1 to 80 characters.",
        ));
    }
    if manifest
        .account
        .avatar
        .as_deref()
        .is_some_and(|avatar| Url::parse(avatar).map_or(true, |url| url.scheme() != "https"))
    {
        problems.push(problem("account.avatar", "An avatar is an https address."));
    }

    let prefix = manifest.id.split_once('.').map_or("", |(left, _)| left);
    let owned = |name: &str, separator: char| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(separator) && rest.len() > 1)
    };

    if manifest.scopes.len() > MAX_SCOPES {
        problems.push(problem("scopes", "Ask for 50 scopes at most."));
    }
    let mut seen = BTreeSet::new();
    for (index, request) in manifest.scopes.iter().enumerate() {
        let path = format!("scopes[{index}]");
        if !seen.insert(request.scope.as_str()) {
            problems.push(problem(&path, "Each scope is asked for once."));
        }
        if request.scope.is_empty() || request.scope.len() > 120 {
            problems.push(problem(&path, "Name the scope."));
        }
        if !one_line(&request.why, 200) {
            problems.push(problem(
                format!("{path}.why"),
                "Say why in 1 to 200 characters.",
            ));
        }
    }
    let permissions: BTreeSet<&str> = manifest
        .scopes
        .iter()
        .filter(|request| !request.scope.starts_with("events:"))
        .map(|request| request.scope.as_str())
        .collect();

    if manifest.kinds.len() > MAX_KINDS {
        problems.push(problem("kinds", "Declare 20 kinds at most."));
    }
    let mut kinds = BTreeSet::new();
    for (index, kind) in manifest.kinds.iter().enumerate() {
        let path = format!("kinds[{index}]");
        if !owned(&kind.kind, '.')
            || !crate::things::kind::valid_name(&kind.kind)
            || !kinds.insert(kind.kind.as_str())
        {
            problems.push(problem(
                format!("{path}.kind"),
                format!("A kind is named once, starting \"{prefix}.\"."),
            ));
        }
        if !one_line(&kind.one, 40) || !one_line(&kind.many, 40) {
            problems.push(problem(
                &path,
                "Give the kind's one and many names, 1 to 40 characters each.",
            ));
        }
        if kind.fields.len() > 50 {
            problems.push(problem(format!("{path}.fields"), "50 fields at most."));
        }
    }

    if manifest.operations.len() > MAX_OPERATIONS {
        problems.push(problem("operations", "Declare 100 operations at most."));
    }
    let mut names = BTreeSet::new();
    for (index, operation) in manifest.operations.iter().enumerate() {
        let path = format!("operations[{index}]");
        let name_ok = owned(&operation.name, '_')
            && operation.name.len() <= 64
            && operation
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !name_ok || !names.insert(operation.name.as_str()) {
            problems.push(problem(
                format!("{path}.name"),
                format!("An operation is named once, starting \"{prefix}_\"."),
            ));
        }
        if !matches!(
            operation.verb.as_str(),
            "query" | "describe" | "create" | "change"
        ) {
            problems.push(problem(
                format!("{path}.verb"),
                "verb is query, describe, create or change.",
            ));
        }
        if let Some(permission) = operation.permission.as_deref()
            && !permissions.contains(permission)
        {
            problems.push(problem(
                format!("{path}.permission"),
                "An operation needs a permission the manifest asks for in scopes.",
            ));
        }
        if operation
            .description
            .as_deref()
            .is_some_and(|text| text.chars().count() > 1000)
        {
            problems.push(problem(
                format!("{path}.description"),
                "Describe it in 1000 characters or fewer.",
            ));
        }
        if operation.input.get("type").and_then(Value::as_str) != Some("object")
            || serde_json::to_vec(&operation.input).map_or(true, |bytes| bytes.len() > 16 * 1024)
        {
            problems.push(problem(
                format!("{path}.input"),
                "input is a JSON Schema of an object, 16 KB at most.",
            ));
        }
        if base_url.is_some() && joined(&operation.endpoint).is_none() {
            problems.push(problem(
                format!("{path}.endpoint"),
                "endpoint is a path under baseUrl.",
            ));
        }
    }

    if manifest.events.emits.len() > MAX_EVENTS {
        problems.push(problem("events.emits", "Declare 100 events at most."));
    }
    let mut types = BTreeSet::new();
    for (index, event) in manifest.events.emits.iter().enumerate() {
        let path = format!("events.emits[{index}]");
        if !owned(&event.event_type, '.')
            || !event_type(&event.event_type)
            || !types.insert(event.event_type.as_str())
        {
            problems.push(problem(
                format!("{path}.type"),
                format!("An event type is named once, starting \"{prefix}.\"."),
            ));
        }
        if event
            .kind
            .as_deref()
            .is_some_and(|kind| !kinds.contains(kind))
        {
            problems.push(problem(
                format!("{path}.kind"),
                "An event is about a kind the manifest declares.",
            ));
        }
    }

    if manifest.panels.len() > MAX_PANELS {
        problems.push(problem("panels", "Declare 20 panels at most."));
    }
    let mut panels = BTreeSet::new();
    for (index, panel) in manifest.panels.iter().enumerate() {
        let path = format!("panels[{index}]");
        if !segment(&panel.id) || !panels.insert(panel.id.as_str()) {
            problems.push(problem(
                format!("{path}.id"),
                "A panel's id is lowercase letters, digits and underscores, once each.",
            ));
        }
        let slot_ok = SLOTS.contains(&panel.slot.as_str())
            || panel
                .slot
                .strip_prefix(THING_SIDE)
                .is_some_and(|kind| !kind.is_empty() && kind.len() <= 100);
        if !slot_ok {
            problems.push(problem(
                format!("{path}.slot"),
                format!("slot is one of {}, or thing.side:<kind>.", SLOTS.join(", ")),
            ));
        }
        if !one_line(&panel.title, 60) {
            problems.push(problem(
                format!("{path}.title"),
                "Title it in 1 to 60 characters.",
            ));
        }
        match (&panel.entry, panel.frame_only) {
            (Some(entry), false) if base_url.is_none() || joined(entry).is_some() => {}
            (None, true) => {}
            _ => problems.push(problem(
                format!("{path}.entry"),
                "A panel has an entry path under baseUrl, or is frameOnly with none.",
            )),
        }
    }

    if manifest.offers.len() > MAX_OFFERS {
        problems.push(problem("offers", "Declare 20 offers at most."));
    }

    if manifest.frame_origins.len() > MAX_FRAME_ORIGINS {
        problems.push(problem("frameOrigins", "Name 4 frame origins at most."));
    }
    for (index, origin) in manifest.frame_origins.iter().enumerate() {
        if !https_origin(origin, allow_http) {
            problems.push(problem(
                format!("frameOrigins[{index}]"),
                "A frame origin is https://host, with nothing after it.",
            ));
        }
    }

    match (problems.is_empty(), base_url, install_url) {
        (true, Some(base_url), Some(install_url)) => Ok(Checked {
            manifest,
            base_url,
            install_url,
        }),
        _ => Err(problems),
    }
}

/// The manifest's digest: SHA-256 of the bytes served, in hex.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    pub(crate) fn sample(origin: &str) -> Value {
        json!({
            "format": FORMAT,
            "id": "acme.builds",
            "name": "Builds",
            "version": "1.4.0",
            "description": "Shows each build and links builds to tasks.",
            "publisher": {"name": "Acme", "url": "https://builds.example.com"},
            "baseUrl": format!("{origin}/workspace/"),
            "install": "install",
            "account": {"name": "Builds"},
            "scopes": [
                {"scope": "members.view", "why": "To show who started a build.", "required": true},
                {"scope": "tasks.edit", "why": "To mark tasks fixed."},
                {"scope": "events:task.updated", "why": "To notice tasks moved to Done."}
            ],
            "kinds": [{"kind": "acme.build", "one": "build", "many": "builds"}],
            "operations": [{"name": "acme_build_rerun", "verb": "change",
                            "permission": "tasks.edit",
                            "input": {"type": "object", "properties": {"build": {"type": "string"}}},
                            "endpoint": "ops/rerun"}],
            "events": {"emits": [{"type": "acme.build.finished", "kind": "acme.build"}]},
            "panels": [{"id": "builds", "slot": "space.tab", "title": "Builds", "entry": "panels/builds"}],
            "frameOrigins": [origin]
        })
    }

    fn read(value: &Value) -> Result<Checked, Vec<Problem>> {
        let address = Url::parse("https://builds.example.com/manifest.json").expect("address");
        check(&serde_json::to_vec(value).expect("bytes"), &address, false)
    }

    fn paths(value: &Value) -> Vec<String> {
        read(value)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|problem| problem.path)
            .collect()
    }

    #[test]
    fn a_whole_manifest_reads() {
        let checked = read(&sample("https://builds.example.com")).expect("reads");
        assert_eq!(checked.prefix(), "acme");
        assert_eq!(
            checked.install_url.as_str(),
            "https://builds.example.com/workspace/install"
        );
        let scopes = checked.scopes();
        assert_eq!(scopes[0].1, Scope::Permission(Permission::MembersView));
        assert_eq!(scopes[2].1, Scope::Events("task.updated".to_owned()));
        assert_eq!(Scope::read("tasks.fly"), Scope::Unknown);
    }

    #[test]
    fn an_app_names_only_what_it_owns_on_its_own_address() {
        let mut manifest = sample("https://builds.example.com");
        manifest["baseUrl"] = json!("https://elsewhere.example.com/workspace/");
        assert_eq!(paths(&manifest), ["baseUrl"]);

        let mut manifest = sample("https://builds.example.com");
        manifest["install"] = json!("../../admin");
        manifest["operations"][0]["name"] = json!("task_delete");
        manifest["operations"][0]["permission"] = json!("roles.manage");
        manifest["events"]["emits"][0]["type"] = json!("task.updated");
        manifest["kinds"][0]["kind"] = json!("task");
        manifest["id"] = json!("local.builds");
        let found = paths(&manifest);
        for path in [
            "id",
            "install",
            "operations[0].name",
            "operations[0].permission",
            "events.emits[0].type",
            "kinds[0].kind",
        ] {
            assert!(found.iter().any(|found| found == path), "{path}: {found:?}");
        }
    }

    #[test]
    fn frames_panels_and_sizes_are_held() {
        let mut manifest = sample("https://builds.example.com");
        manifest["frameOrigins"] = json!(["http://builds.example.com", "https://a.example/x"]);
        manifest["panels"][0]["slot"] = json!("sidebar");
        manifest["panels"].as_array_mut().expect("panels").push(
            json!({"id": "keys", "slot": "agents.create", "title": "Keys", "frameOnly": true}),
        );
        let found = paths(&manifest);
        assert!(found.contains(&"frameOrigins[0]".to_owned()), "{found:?}");
        assert!(found.contains(&"frameOrigins[1]".to_owned()), "{found:?}");
        assert!(found.contains(&"panels[0].slot".to_owned()), "{found:?}");
        assert!(!found.contains(&"panels[1].entry".to_owned()), "{found:?}");

        let big = vec![b' '; MAX_BYTES + 1];
        let address = Url::parse("https://builds.example.com/m.json").expect("address");
        assert!(check(&big, &address, false).is_err());
        assert!(manifest_address("http://builds.example.com/m.json", &[]).is_none());
        assert!(
            manifest_address(
                "http://127.0.0.1:9/m.json",
                &["http://127.0.0.1:9".to_owned()]
            )
            .is_some()
        );
    }
}
