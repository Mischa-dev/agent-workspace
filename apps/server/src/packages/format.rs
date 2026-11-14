//! The package format, `agent-workspace-package/1` (M1 section 3.6), in its
//! inline form: one JSON document with every part's body in it (what
//! agents write and the API takes). The zip form names a `path` for each
//! part instead of a `body`; both read into [`Package`].
//!
//! Cross-references between parts use `@<key>` (a form's collection). The digest is a SHA-256
//! over the canonical document (keys sorted, no whitespace, `digest` left
//! out), so the same setup exported twice has the same digest.

use std::collections::BTreeSet;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::workspace_templates::TemplateDefinition;

pub const FORMAT: &str = "agent-workspace-package/1";
/// The most parts one package holds (M1 section 3.6: 200 in total).
pub const MAX_PARTS: usize = 200;
/// The most collections one package holds.
pub const MAX_COLLECTIONS: usize = 50;

/// A package: a setup in one file. It never holds secrets, credentials,
/// tokens, member lists or content beyond what its author put in.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Package {
    /// Always "agent-workspace-package/1".
    pub format: String,
    /// "publisher.key" or "key": lower case letters, digits and underscores.
    pub id: String,
    /// A semantic version, such as "1.2.0".
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<Publisher>,
    /// What it installs into.
    pub target: PackageTarget,
    #[serde(default, skip_serializing_if = "Requires::is_empty")]
    pub requires: Requires,
    /// The parts, each by kind and key.
    pub contents: Vec<Content>,
    /// Parts that need an admin to install, as "kind:key".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_admin: Vec<String>,
    /// "sha256:<hex>" over the canonical document without this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publisher {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageTarget {
    /// Installs into one space, creating it if asked.
    Space,
    /// Also sets switches, roles, the default role and the default layout.
    Workspace,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requires {
    /// The module contract it was written for, such as ">=2".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    /// Core modules it needs on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

impl Requires {
    fn is_empty(&self) -> bool {
        self.contract.is_none() && self.modules.is_empty()
    }
}

/// One part of a package.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Content {
    pub kind: ContentKind,
    /// Unique among the package's parts of this kind; `@<key>` refers to it.
    pub key: String,
    /// In the zip form: the file that holds the part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// In the inline form: the part itself, in its portable form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub body: Option<Value>,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// Core modules to turn off: `{"off": [...]}`.
    Switches,
    /// The default layout: `{"preset", "changes", "words"}`.
    Layout,
    /// A role: name, base role, permissions, modules off, defaults.
    Role,
    /// A starter space: `{"name", "key"}`.
    Space,
    /// Channel names: `{"channels": [{"name"}]}`.
    Channels,
    /// A built module's definition.
    Module,
    /// `collection_describe`'s definition.
    Collection,
    /// A form on a collection, without its link.
    Form,
    /// An automation rule, or an outbound webhook (flagged `outbound`).
    Automation,
    /// A page or page template.
    Page,
    /// An Agent Skill, installed as a skill page.
    Skill,
    /// An agent template (never an account).
    Agent,
}

impl ContentKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Switches => "switches",
            Self::Layout => "layout",
            Self::Role => "role",
            Self::Space => "space",
            Self::Channels => "channels",
            Self::Module => "module",
            Self::Collection => "collection",
            Self::Form => "form",
            Self::Automation => "automation",
            Self::Page => "page",
            Self::Skill => "skill",
            Self::Agent => "agent",
        }
    }
}

/// A part's key from a name: lower case, words joined by `_`, starting
/// with a letter, at most 64 characters.
pub fn key_of(name: &str) -> String {
    let mut key = String::new();
    for character in name.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            key.push(character);
        } else if !key.is_empty() && !key.ends_with('_') {
            key.push('_');
        }
    }
    let key = key.trim_end_matches('_');
    let mut key = if key.starts_with(|c: char| c.is_ascii_lowercase()) {
        key.to_owned()
    } else {
        format!("part_{key}").trim_end_matches('_').to_owned()
    };
    key.truncate(64);
    key
}

/// Keys handed out so far, so two parts named alike get `x` and `x_2`.
#[derive(Default)]
pub struct Keys(BTreeSet<(ContentKind, String)>);

impl Keys {
    /// Marks a key as taken.
    pub fn seed(&mut self, kind: ContentKind, key: &str) {
        self.0.insert((kind, key.to_owned()));
    }

    pub fn take(&mut self, kind: ContentKind, name: &str) -> String {
        let base = key_of(name);
        let mut key = base.clone();
        let mut next = 2;
        while !self.0.insert((kind, key.clone())) {
            let suffix = format!("_{next}");
            let mut stem = base.clone();
            stem.truncate(64 - suffix.len());
            key = format!("{stem}{suffix}");
            next += 1;
        }
        key
    }
}

fn is_key(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_version(text: &str) -> bool {
    let parts = text.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 9
                && part.chars().all(|c| c.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

/// The JSON with object keys sorted and no whitespace.
pub fn canonical(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                let mut keys = map.keys().collect::<Vec<_>>();
                keys.sort();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    write(&map[key], out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

impl Package {
    /// The digest of the document as it is, without its `digest` field.
    pub fn compute_digest(&self) -> String {
        let mut document = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(map) = document.as_object_mut() {
            map.remove("digest");
        }
        let hash = Sha256::digest(canonical(&document).as_bytes());
        format!("sha256:{}", hex::encode(hash))
    }

    /// Sets the digest from the contents.
    pub fn sealed(mut self) -> Self {
        self.digest = None;
        self.digest = Some(self.compute_digest());
        self
    }

    /// Every rule of the format the document keeps; `Err` lists the broken
    /// ones. Doesn't check that parts can be installed (the preview does).
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.format != FORMAT {
            problems.push(format!("format is \"{FORMAT}\""));
        }
        let mut id_parts = self.id.split('.');
        let id_ok = match (id_parts.next(), id_parts.next(), id_parts.next()) {
            (Some(key), None, None) => is_key(key),
            (Some(publisher), Some(key), None) => is_key(publisher) && is_key(key),
            _ => false,
        };
        if !id_ok {
            problems.push(format!(
                "id {:?} is \"key\" or \"publisher.key\" in lower snake case",
                self.id
            ));
        }
        if !is_version(&self.version) {
            problems.push(format!(
                "version {:?} is a semantic version (1.2.0)",
                self.version
            ));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            problems.push("name is 1 to 80 characters".to_owned());
        }
        if self.contents.len() > MAX_PARTS {
            problems.push(format!("at most {MAX_PARTS} parts"));
        }
        let collections = self
            .contents
            .iter()
            .filter(|part| part.kind == ContentKind::Collection)
            .count();
        if collections > MAX_COLLECTIONS {
            problems.push(format!("at most {MAX_COLLECTIONS} collections"));
        }
        let mut seen = BTreeSet::new();
        for part in &self.contents {
            let name = format!("{}:{}", part.kind.key(), part.key);
            if !is_key(&part.key) {
                problems.push(format!("{name}: keys are lower snake case"));
            }
            if !seen.insert(name.clone()) {
                problems.push(format!("{name} twice"));
            }
            match (&part.path, &part.body) {
                (None, Some(Value::Object(_))) | (Some(_), None) => {}
                _ => problems.push(format!("{name}: a path or an object body, not both")),
            }
            if let Some(path) = &part.path
                && (path.starts_with('/') || path.split('/').any(|step| step == ".."))
            {
                problems.push(format!("{name}: the path stays inside the package"));
            }
            if self.target == PackageTarget::Space
                && matches!(part.kind, ContentKind::Switches | ContentKind::Layout)
            {
                problems.push(format!("{name}: only a workspace package sets this"));
            }
        }
        for needed in &self.needs_admin {
            if !seen.contains(needed) {
                problems.push(format!("needsAdmin names {needed}, which isn't a part"));
            }
        }
        // A form names its collection as `@<key>`, a part of the package.
        let collections = self
            .contents
            .iter()
            .filter(|part| part.kind == ContentKind::Collection)
            .map(|part| format!("@{}", part.key))
            .collect::<BTreeSet<_>>();
        for part in &self.contents {
            if part.kind == ContentKind::Form
                && let Some(body) = &part.body
                && !body["collection"]
                    .as_str()
                    .is_some_and(|reference| collections.contains(reference))
            {
                problems.push(format!(
                    "form:{}: its collection is \"@<key>\" of a collection in the package",
                    part.key
                ));
            }
        }
        if let Some(digest) = &self.digest
            && *digest != self.compute_digest()
        {
            problems.push("the digest doesn't match the contents".to_owned());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

/// A workspace template (built in or saved) read as a package
/// (`packages::from_template`, M1 section 3.6): saved templates keep
/// working without a data migration.
pub fn from_template(
    id: &str,
    name: &str,
    description: &str,
    definition: &TemplateDefinition,
) -> Package {
    let mut keys = Keys::default();
    let mut contents = Vec::new();
    let mut needs_admin = Vec::new();
    let mut part = |kind: ContentKind, key: String, body: Value| {
        contents.push(Content {
            kind,
            key,
            path: None,
            body: Some(body),
        });
    };
    if let Some(on) = &definition.modules {
        let off = crate::modules::manifest::core()
            .iter()
            .filter(|module| module.switchable && !on.contains(&module.id))
            .map(|module| module.id.clone())
            .collect::<Vec<_>>();
        part(
            ContentKind::Switches,
            keys.take(ContentKind::Switches, "switches"),
            serde_json::json!({ "off": off }),
        );
    }
    if let Some(layout) = &definition.layout {
        part(
            ContentKind::Layout,
            keys.take(ContentKind::Layout, "layout"),
            serde_json::json!({
                "preset": layout,
                "changes": definition.layout_changes,
                "words": definition.layout_words,
            }),
        );
    }
    for role in &definition.roles {
        let key = keys.take(ContentKind::Role, &role.name);
        let mut body = serde_json::to_value(role).unwrap_or(Value::Null);
        if definition.default_role.as_deref() == Some(role.name.as_str())
            && let Some(map) = body.as_object_mut()
        {
            map.insert("default".to_owned(), Value::Bool(true));
        }
        needs_admin.push(format!("role:{key}"));
        part(ContentKind::Role, key, body);
    }
    if let Some(project) = &definition.project {
        part(
            ContentKind::Space,
            keys.take(ContentKind::Space, &project.name),
            serde_json::json!({ "name": project.name, "key": project.key }),
        );
        if !project.channels.is_empty() {
            let channels = project
                .channels
                .iter()
                .map(|name| serde_json::json!({ "name": name }))
                .collect::<Vec<_>>();
            part(
                ContentKind::Channels,
                keys.take(ContentKind::Channels, "channels"),
                serde_json::json!({ "channels": channels }),
            );
        }
        for collection in &project.collections {
            part(
                ContentKind::Collection,
                keys.take(ContentKind::Collection, &collection.name),
                serde_json::to_value(collection).unwrap_or(Value::Null),
            );
        }
    }
    let modules = definition.modules.clone().unwrap_or_default();
    Package {
        format: FORMAT.to_owned(),
        id: key_of(id),
        version: "1.0.0".to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        publisher: None,
        target: PackageTarget::Workspace,
        requires: Requires {
            contract: None,
            modules,
        },
        contents,
        needs_admin,
        digest: None,
    }
    .sealed()
}

/// The JSON Schema of the inline form, as `modules/packages/package.schema.json`
/// holds it for the web.
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Package)).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_templates_are_packages_that_validate() {
        let mut count = 0;
        for (id, name, description, definition) in crate::workspace_templates::builtin_definitions()
        {
            let package = from_template(id, name, description, definition);
            package
                .validate()
                .unwrap_or_else(|problems| panic!("{id}: {problems:?}"));
            assert_eq!(package.target, PackageTarget::Workspace);
            // Through JSON and back, the same package and digest.
            let text = serde_json::to_string(&package).expect("JSON");
            let read = serde_json::from_str::<Package>(&text).expect("reads");
            assert_eq!(read, package);
            assert_eq!(
                read.compute_digest(),
                package.digest.clone().expect("digest")
            );
            count += 1;
        }
        assert_eq!(count, 6, "the six built-in templates");
    }

    #[test]
    fn the_digest_ignores_key_order_and_follows_the_contents() {
        let package = from_template(
            "small_team",
            "Small team",
            "",
            &TemplateDefinition {
                modules: Some(vec!["tasks".to_owned()]),
                ..TemplateDefinition::default()
            },
        );
        let mut value = serde_json::to_value(&package).expect("JSON");
        // Reorder by reading a hand-written string with keys reversed.
        let reversed = {
            let map = value.as_object().expect("object");
            let mut text = String::from("{");
            for (index, (key, item)) in map.iter().rev().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                text.push_str(&format!("{}:{}", Value::String(key.clone()), item));
            }
            text.push('}');
            text
        };
        let read = serde_json::from_str::<Package>(&reversed).expect("reads");
        assert_eq!(read.compute_digest(), package.compute_digest());
        value["name"] = Value::String("Other".into());
        let changed = serde_json::from_value::<Package>(value).expect("reads");
        assert!(changed.validate().is_err(), "a stale digest is refused");
        assert_ne!(changed.compute_digest(), package.compute_digest());
    }

    #[test]
    fn a_broken_package_says_what_is_wrong() {
        let mut package = from_template("x", "X", "", &TemplateDefinition::default());
        package.format = "zip".into();
        package.id = "Bad Id".into();
        package.version = "1.0".into();
        package.target = PackageTarget::Space;
        package.contents.push(Content {
            kind: ContentKind::Layout,
            key: "layout".into(),
            path: None,
            body: Some(serde_json::json!({ "preset": "everything" })),
        });
        package.contents.push(Content {
            kind: ContentKind::Form,
            key: "feedback".into(),
            path: None,
            body: Some(serde_json::json!({ "collection": "@nowhere" })),
        });
        package.needs_admin.push("role:ghost".into());
        package.digest = None;
        let problems = package.validate().expect_err("refused");
        for expected in [
            "format",
            "id",
            "version",
            "only a workspace",
            "role:ghost",
            "form:feedback",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "{expected}: {problems:?}"
            );
        }
    }

    #[test]
    fn keys_come_from_names() {
        assert_eq!(key_of("Bug reports!"), "bug_reports");
        assert_eq!(key_of("2025 plans"), "part_2025_plans");
        let mut keys = Keys::default();
        assert_eq!(keys.take(ContentKind::Role, "Tester"), "tester");
        assert_eq!(keys.take(ContentKind::Role, "tester"), "tester_2");
        assert_eq!(keys.take(ContentKind::Form, "Tester"), "tester");
    }

    #[test]
    fn the_shared_schema_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../modules/packages/package.schema.json");
        let current = serde_json::to_string_pretty(&schema()).expect("JSON") + "\n";
        if std::env::var_os("OPS_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
            std::fs::write(&path, &current).expect("write");
        }
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            saved, current,
            "modules/packages/package.schema.json is stale: run with OPS_BLESS=1"
        );
    }
}
