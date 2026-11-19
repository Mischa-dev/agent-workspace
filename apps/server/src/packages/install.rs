//! M1.9: a package's preview and install (M1 section 3.6).
//!
//! The preview reads a package (inline form) against the workspace and
//! says, part by part, what installing it would do: create, reuse an
//! object of the same name, update (roles, switches, layout) or skip (with
//! the reason). Its `planHash` covers the package, the destination and
//! every line, so an install whose plan changed since the preview (someone
//! made a collection of the same name meanwhile) is refused.
//!
//! Install never deletes: it reuses what exists, runs in the caller's one
//! transaction, and records what it made in `package_objects`. The
//! workspace-level parts (switches, layout, roles, the starter space with
//! its channels and collections) go through the template path
//! (`workspace_templates::apply`), which saved templates already use; the
//! rest (forms, automations, pages, skills, agent templates, built
//! modules) are made here, `@<key>` resolved to the ids just made.

use std::collections::{BTreeMap, HashMap};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use super::format::{Content, ContentKind, Package, canonical};
use crate::{error::AppError, modules::built::Reader, ops::OpError};

/// The most bytes of one inline package an operation takes.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Reads an inline package and checks its format.
pub fn read(document: &Value) -> Result<Package, OpError> {
    let package =
        serde_json::from_value::<Package>(document.clone()).map_err(|error| OpError::Invalid {
            field: "package".into(),
            message: format!(
                "This isn't a package (agent-workspace-package/1): {}",
                error.to_string().split(" at line ").next().unwrap_or("")
            )
            .into(),
        })?;
    if let Err(problems) = package.validate() {
        return Err(OpError::Invalid {
            field: "package".into(),
            message: format!("The package isn't valid: {}.", problems.join("; ")).into(),
        });
    }
    if package.contents.iter().any(|part| part.body.is_none()) {
        return Err(OpError::invalid(
            "package",
            "Give every part inline, with its body. Zipped packages are read from an upload, \
             which isn't supported yet.",
        ));
    }
    Ok(package)
}

/// A new space to install into.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewSpace {
    pub name: String,
    /// 2 to 10 letters or digits, starting with a letter. Taken, a number
    /// is added.
    pub key: String,
}

/// Where the package goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// An existing space.
    Space(Uuid),
    /// A new space (one the installer owns with this key and name is
    /// reused).
    New(NewSpace),
    /// The workspace only.
    Workspace,
}

impl Place {
    /// Where a package goes: the space named, a new one, the package's own
    /// starter space, or the workspace.
    pub fn of(
        package: &Package,
        space: Option<Uuid>,
        new_space: Option<NewSpace>,
    ) -> Result<Self, OpError> {
        if let Some(space) = space {
            return Ok(Self::Space(space));
        }
        if let Some(new_space) = new_space {
            return Ok(Self::New(new_space));
        }
        let starter = package
            .contents
            .iter()
            .find(|part| part.kind == ContentKind::Space)
            .and_then(|part| serde_json::from_value::<NewSpace>(part.body.clone()?).ok());
        match (starter, package.target) {
            (Some(starter), _) => Ok(Self::New(starter)),
            (None, super::format::PackageTarget::Workspace) => Ok(Self::Workspace),
            (None, super::format::PackageTarget::Space) => Err(OpError::invalid(
                "target",
                "This package installs into a space: name one (target.spaceId) or a new one \
                 (target.newSpace).",
            )),
        }
    }

    fn describe(&self) -> Value {
        match self {
            Self::Space(id) => json!({ "spaceId": id }),
            Self::New(space) => json!({ "newSpace": space }),
            Self::Workspace => json!({ "workspace": true }),
        }
    }
}

/// One line of the plan.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    /// create, reuse, update or skip.
    pub action: String,
    /// The part's kind: switches, layout, role, space, channel, collection,
    /// form, automation, page, skill, agent or module.
    pub kind: String,
    /// The part's key in the package (a channel's name for a channel).
    pub key: String,
    pub name: String,
    /// Installing it needs roles.manage (a role) or settings.manage.
    pub needs_admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What it reuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub existing_id: Option<Uuid>,
}

/// What installing a package would do.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub package_id: String,
    pub version: String,
    pub name: String,
    pub digest: String,
    /// Where it goes: {spaceId}, {newSpace} or {workspace: true}.
    #[schemars(with = "Value")]
    pub target: Value,
    pub items: Vec<PlanItem>,
    /// Core modules it turns off, and on.
    pub turns_off: Vec<String>,
    pub turns_on: Vec<String>,
    /// The permissions each role it adds or changes would hold.
    #[schemars(with = "Value")]
    pub roles: Vec<Value>,
    /// Data that would leave the workspace (outbound webhooks), as
    /// "kind:key". They are not installed: they need their address again.
    pub leaves: Vec<String>,
    /// Pass it to package_install: the install is refused if the plan
    /// changed since.
    pub plan_hash: String,
}

fn item(action: &str, kind: &str, key: &str, name: &str) -> PlanItem {
    PlanItem {
        action: action.to_owned(),
        kind: kind.to_owned(),
        key: key.to_owned(),
        name: name.to_owned(),
        needs_admin: false,
        reason: None,
        existing_id: None,
    }
}

fn found(kind: &str, key: &str, name: &str, existing: Option<Uuid>) -> PlanItem {
    let mut line = item(
        if existing.is_some() {
            "reuse"
        } else {
            "create"
        },
        kind,
        key,
        name,
    );
    line.existing_id = existing;
    line
}

fn skipped(kind: &str, key: &str, name: &str, reason: &str) -> PlanItem {
    let mut line = item("skip", kind, key, name);
    line.reason = Some(reason.to_owned());
    line
}

fn text<'a>(body: &'a Value, field: &str) -> &'a str {
    body.get(field).and_then(Value::as_str).unwrap_or("").trim()
}

/// The space a plan's space parts land in, when it exists already.
async fn landing(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    viewer: Reader,
    place: &Place,
) -> Result<Option<Uuid>, AppError> {
    Ok(match place {
        Place::Space(id) => Some(*id),
        // The installer's own starter space, found as the install will find
        // it (another person's space with that key is never planned
        // against: the install makes a new one).
        Place::New(space) => {
            let (Ok(name), Ok(key)) = (
                crate::projects::normalize_project_name(&space.name),
                crate::projects::normalize_project_key(&space.key),
            ) else {
                return Ok(None);
            };
            crate::workspace_templates::reusable_starter(
                connection,
                workspace_id,
                viewer.account,
                &name,
                &key,
            )
            .await?
        }
        Place::Workspace => None,
    })
}

async fn existing(
    connection: &mut PgConnection,
    sql: &str,
    scope: Uuid,
    name: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(sql)
        .bind(scope)
        .bind(name)
        .fetch_optional(&mut *connection)
        .await?)
}

const ROLE_BY_NAME: &str =
    "SELECT id FROM workspace_roles WHERE organization_id = $1 AND lower(name) = lower($2)";
/// A channel of this name in the space, and whether the viewer may see it:
/// a members-only channel they aren't on is reused without its id shown.
const CHANNEL_BY_NAME: &str = "SELECT id, conversation_visible_to(id, $3) FROM conversations \
     WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL \
     AND lower(name) = lower($2) LIMIT 1";
const COLLECTION_BY_NAME: &str = "SELECT id FROM collections WHERE project_id = $1 \
     AND archived_at IS NULL AND lower(name) = lower($2) LIMIT 1";
const FORM_BY_NAME: &str = "SELECT id FROM bug_report_links WHERE project_id = $1 \
     AND state <> 'disabled' AND lower(name) = lower($2) LIMIT 1";
const RULE_BY_NAME: &str = "SELECT id FROM automation_rules WHERE project_id = $1 \
     AND lower(name) = lower($2) LIMIT 1";
const PAGE_BY_TITLE: &str = "SELECT id FROM project_pages WHERE project_id = $1 \
     AND archived_at IS NULL AND lower(title) = lower($2) LIMIT 1";
/// Whether the viewer may see a workspace-keyed thing that lives in a
/// space (`found.space_id`): one in a space they don't reach is reused
/// without its id shown.
fn space_visible() -> String {
    format!(
        "(found.space_id IS NULL OR {})",
        Reader::reaches_sql("found.space_id", 3)
    )
}
const AGENT_BY_KEY: &str = "SELECT found.id, {visible} FROM agent_templates found \
     WHERE found.workspace_id = $1 AND found.archived_at IS NULL AND found.template_key = $2";
const MODULE_BY_KEY: &str = "SELECT found.id, {visible} FROM modules found \
     WHERE found.workspace_id = $1 AND found.archived_at IS NULL AND found.module_key = $2";

const IN_ANOTHER_SPACE: &str = "One with this key is in a space you aren't in, so it's left out.";

/// A workspace-keyed thing by key, and its id only when the viewer may see it.
async fn existing_seen(
    connection: &mut PgConnection,
    sql: &str,
    workspace_id: Uuid,
    key: &str,
    viewer: Reader,
) -> Result<(Option<Uuid>, bool), AppError> {
    let held = sqlx::query_as::<_, (Uuid, bool)>(&sql.replace("{visible}", &space_visible()))
        .bind(workspace_id)
        .bind(key)
        .bind(viewer.account)
        .bind(viewer.connection)
        .bind(viewer.held)
        .fetch_optional(&mut *connection)
        .await?;
    Ok((held.map(|(id, _)| id), held.is_some_and(|(_, seen)| !seen)))
}

/// The plan for installing `package` at `place`, read on `connection` (the
/// install's own transaction, so it is the plan the install applies).
pub async fn plan(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    viewer: Reader,
    package: &Package,
    place: &Place,
) -> Result<Plan, AppError> {
    let space = landing(connection, workspace_id, viewer, place).await?;
    let needs_space = "It goes in a space: install the package into one.";
    let mut items = Vec::new();
    let (mut turns_off, mut turns_on, mut roles, mut leaves) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let marked = |kind: ContentKind, key: &str| {
        package
            .needs_admin
            .iter()
            .any(|entry| *entry == format!("{}:{key}", kind.key()))
    };
    for part in &package.contents {
        let body = part.body.clone().unwrap_or(Value::Null);
        let key = part.key.as_str();
        let mut line = match part.kind {
            ContentKind::Switches => {
                let on = crate::modules::state::workspace(connection, workspace_id).await?;
                let off = body["off"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for module in crate::modules::manifest::core()
                    .iter()
                    .filter(|module| module.switchable)
                {
                    let wanted = !off.contains(&module.id);
                    if wanted && !on.contains(&module.id) {
                        turns_on.push(module.id.clone());
                    } else if !wanted && on.contains(&module.id) {
                        turns_off.push(module.id.clone());
                    }
                }
                let changes = !turns_on.is_empty() || !turns_off.is_empty();
                let mut line = item(
                    if changes { "update" } else { "skip" },
                    "switches",
                    key,
                    "Modules",
                );
                line.needs_admin = true;
                line
            }
            ContentKind::Layout => {
                let mut line = item("update", "layout", key, text(&body, "preset"));
                line.needs_admin = true;
                line
            }
            ContentKind::Role => {
                let name = text(&body, "name");
                let held = existing(connection, ROLE_BY_NAME, workspace_id, name).await?;
                roles.push(json!({
                    "name": name,
                    "baseRole": body.get("baseRole"),
                    "permissions": body.get("permissions"),
                }));
                let mut line = found("role", key, name, held);
                if held.is_some() {
                    line.action = "update".to_owned();
                }
                line.needs_admin = true;
                line
            }
            ContentKind::Space => {
                let name = text(&body, "name");
                match place {
                    Place::Space(id) => {
                        let mut line = found("space", key, name, Some(*id));
                        line.reason = Some("Installing into the space you named.".to_owned());
                        line
                    }
                    _ => found("space", key, name, space),
                }
            }
            ContentKind::Channels => {
                for channel in body["channels"].as_array().cloned().unwrap_or_default() {
                    let name = text(&channel, "name");
                    items.push(match space {
                        Some(space) => {
                            let held = sqlx::query_as::<_, (Uuid, bool)>(CHANNEL_BY_NAME)
                                .bind(space)
                                .bind(name)
                                .bind(viewer.account)
                                .fetch_optional(&mut *connection)
                                .await?;
                            let mut line = found("channel", name, name, held.map(|(id, _)| id));
                            if held.is_some_and(|(_, visible)| !visible) {
                                line.existing_id = None;
                            }
                            line
                        }
                        None if matches!(place, Place::New(_)) => {
                            found("channel", name, name, None)
                        }
                        None => skipped("channel", name, name, needs_space),
                    });
                }
                continue;
            }
            kind => {
                let (kind_key, name, sql) = match kind {
                    ContentKind::Collection => {
                        ("collection", text(&body, "name"), COLLECTION_BY_NAME)
                    }
                    ContentKind::Form => ("form", text(&body, "name"), FORM_BY_NAME),
                    ContentKind::Automation => ("automation", text(&body, "name"), RULE_BY_NAME),
                    ContentKind::Page => ("page", text(&body, "title"), PAGE_BY_TITLE),
                    ContentKind::Skill => ("skill", text(&body, "name"), PAGE_BY_TITLE),
                    ContentKind::Agent => ("agent", text(&body, "name"), AGENT_BY_KEY),
                    _ => ("module", text(&body, "name"), MODULE_BY_KEY),
                };
                if kind == ContentKind::Automation
                    && (body["outbound"] == Value::Bool(true)
                        || body["needsSetup"]
                            .as_array()
                            .is_some_and(|setup| !setup.is_empty()))
                {
                    if body["outbound"] == Value::Bool(true) {
                        leaves.push(format!("automation:{key}"));
                    }
                    items.push(skipped(
                        kind_key,
                        key,
                        name,
                        "It needs setting up after install (an address, a view or an \
                         environment it names): make it in the space's automations.",
                    ));
                    continue;
                }
                match kind {
                    ContentKind::Agent => {
                        let (held, hidden) =
                            existing_seen(connection, sql, workspace_id, key, viewer).await?;
                        if hidden {
                            skipped(kind_key, key, name, IN_ANOTHER_SPACE)
                        } else {
                            found(kind_key, key, name, held)
                        }
                    }
                    ContentKind::Module => {
                        let module_key = crate::modules::built::full_key(key).unwrap_or_default();
                        let (held, hidden) =
                            existing_seen(connection, sql, workspace_id, &module_key, viewer)
                                .await?;
                        let mut line = if hidden {
                            skipped(kind_key, key, name, IN_ANOTHER_SPACE)
                        } else {
                            found(kind_key, key, name, held)
                        };
                        if space.is_none() && !matches!(place, Place::New(_)) {
                            line = skipped(kind_key, key, name, needs_space);
                        }
                        line
                    }
                    _ => match space {
                        Some(space) => found(
                            kind_key,
                            key,
                            name,
                            existing(connection, sql, space, name).await?,
                        ),
                        None if matches!(place, Place::New(_)) => found(kind_key, key, name, None),
                        None => skipped(kind_key, key, name, needs_space),
                    },
                }
            }
        };
        line.needs_admin |= marked(part.kind, key);
        items.push(line);
    }
    let digest = package.compute_digest();
    let target = place.describe();
    let plan_hash = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(
            canonical(&json!({
                "digest": digest,
                "target": target,
                "items": items,
                // What the switches item would turn off and on: another
                // admin's switch change since the preview makes it stale.
                "turnsOff": turns_off,
                "turnsOn": turns_on,
            }))
            .as_bytes()
        ))
    );
    Ok(Plan {
        package_id: package.id.clone(),
        version: package.version.clone(),
        name: package.name.clone(),
        digest,
        target,
        items,
        turns_off,
        turns_on,
        roles,
        leaves,
        plan_hash,
    })
}

/// A part's own hash, for `package_objects.installed_hash` (M1.10 compares
/// it with the object's portable form now).
pub fn part_hash(part: &Content) -> String {
    let body = part.body.clone().unwrap_or(Value::Null);
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canonical(&body).as_bytes()))
    )
}

/// Whether `rest` (after `@` or `#`) is a reference's key: one word, so a
/// page that opens with a Markdown heading or text that starts with an
/// @mention stays text.
fn reference_key(rest: &str) -> Option<&str> {
    (!rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')))
    .then_some(rest)
}

/// `"@key"` strings resolved to the ids made (or reused) for those keys,
/// and `"#name"` to channels'. Only a whole string is a reference: a
/// one-word one, or one naming a channel exactly.
pub fn resolve_refs(
    value: Value,
    collections: &HashMap<String, Uuid>,
    channels: &HashMap<String, Uuid>,
) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => {
            if let Some(key) = text.strip_prefix('@').and_then(reference_key) {
                let id = collections
                    .get(key)
                    .ok_or_else(|| format!("@{key} names no collection in the package"))?;
                Value::String(id.to_string())
            } else if let Some(name) = text.strip_prefix('#') {
                // A channel's name may hold spaces ("#Release planning"), so
                // a whole string naming one resolves; a one-word name that
                // names none is refused; anything else is text.
                match channels.get(&name.to_lowercase()) {
                    Some(id) => Value::String(id.to_string()),
                    None if reference_key(name).is_some() => {
                        return Err(format!("#{name} names no channel in the space"));
                    }
                    None => Value::String(text),
                }
            } else {
                Value::String(text)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| resolve_refs(item, collections, channels))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| Ok((key, resolve_refs(item, collections, channels)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other,
    })
}

/// A role part as the template path takes it, and whether it is the
/// default role.
pub fn template_role(
    body: &Value,
) -> Result<(crate::workspace_templates::TemplateRole, bool), String> {
    let mut body = body.clone();
    let default = body
        .as_object_mut()
        .and_then(|map| map.remove("default"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    serde_json::from_value(body)
        .map(|role| (role, default))
        .map_err(|error| {
            error
                .to_string()
                .split(" at line ")
                .next()
                .unwrap_or("")
                .to_owned()
        })
}

/// An installed package, as package_list shows it.
#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InstallView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub package_id: String,
    pub version: String,
    pub name: String,
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    /// installed or removed.
    pub state: String,
    /// What it did, in plain words.
    pub summary: Vec<String>,
    /// How many objects it made or reused.
    pub objects: i64,
    #[schemars(with = "String")]
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

pub const INSTALL_SELECT: &str = r#"
    SELECT install.id, package.package_key AS package_id, package.version, package.name,
           install.space_id, install.state, install.summary,
           (SELECT count(*) FROM package_objects object WHERE object.install_id = install.id)
               AS objects,
           install.created_at AS installed_at
    FROM package_installs install
    JOIN packages package ON package.id = install.package_id
"#;

/// The installs in a workspace (in one space, when named) the reader can
/// see, newest first: one in a space only to a reader who reaches it.
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    reader: Reader,
) -> Result<Vec<InstallView>, AppError> {
    Ok(sqlx::query_as::<_, InstallView>(&format!(
        "{INSTALL_SELECT} WHERE install.workspace_id = $1 \
         AND ($2::uuid IS NULL OR install.space_id = $2) \
         AND (install.space_id IS NULL OR {}) \
         ORDER BY install.created_at DESC, install.id DESC LIMIT 200",
        Reader::reaches_sql("install.space_id", 3)
    ))
    .bind(workspace_id)
    .bind(space_id)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_all(&mut *connection)
    .await?)
}

/// One install.
pub async fn get(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    install_id: Uuid,
) -> Result<Option<InstallView>, AppError> {
    Ok(sqlx::query_as::<_, InstallView>(&format!(
        "{INSTALL_SELECT} WHERE install.workspace_id = $1 AND install.id = $2"
    ))
    .bind(workspace_id)
    .bind(install_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// What an install made, by kind, for removal.
pub async fn created_objects(
    connection: &mut PgConnection,
    install_id: Uuid,
) -> Result<BTreeMap<String, Vec<Uuid>>, AppError> {
    let rows = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT object_kind, object_id FROM package_objects \
         WHERE install_id = $1 AND action = 'created'",
    )
    .bind(install_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut by_kind = BTreeMap::<String, Vec<Uuid>>::new();
    for (kind, id) in rows {
        by_kind.entry(kind).or_default().push(id);
    }
    Ok(by_kind)
}

/// An agent template, as agent_template_list shows it.
#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplateView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub purpose: String,
    /// The template: instructions, what it may and must not do, when it
    /// asks a person, a suggested role and its skills. Never an account.
    #[schemars(with = "Value")]
    pub definition: Value,
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    #[schemars(with = "Option<String>")]
    pub install_id: Option<Uuid>,
}

/// The workspace's agent templates the reader can see: workspace-wide
/// ones, and those in a space only to a reader who reaches it.
pub async fn agent_templates(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    reader: Reader,
) -> Result<Vec<AgentTemplateView>, AppError> {
    Ok(sqlx::query_as::<_, AgentTemplateView>(&format!(
        r#"
        SELECT id, template_key AS key, name, purpose, definition, space_id, install_id
        FROM agent_templates template
        WHERE workspace_id = $1 AND archived_at IS NULL
          AND ($2::uuid IS NULL OR space_id = $2)
          AND (template.space_id IS NULL OR {reaches})
        ORDER BY name, id
        LIMIT 200
        "#,
        reaches = Reader::reaches_sql("template.space_id", 3)
    ))
    .bind(workspace_id)
    .bind(space_id)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_all(&mut *connection)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_starts_like_a_reference_stays_text() {
        let none = HashMap::new();
        let kept = json!({"markdown": "# Hello\n", "instructions": "@lena reads this"});
        assert_eq!(resolve_refs(kept.clone(), &none, &none), Ok(kept));
    }

    #[test]
    fn references_resolve_or_say_what_is_missing() {
        let feedback = Uuid::now_v7();
        let general = Uuid::now_v7();
        let collections = HashMap::from([("feedback".to_owned(), feedback)]);
        let planning = Uuid::now_v7();
        let channels = HashMap::from([
            ("general".to_owned(), general),
            ("release planning".to_owned(), planning),
        ]);
        let resolved = resolve_refs(
            json!({"trigger": {"collectionId": "@feedback"},
                   "actions": [{"conversationId": "#General", "text": "plain"},
                               {"conversationId": "#Release planning"}]}),
            &collections,
            &channels,
        )
        .expect("resolved");
        assert_eq!(resolved["trigger"]["collectionId"], feedback.to_string());
        assert_eq!(
            resolved["actions"][0]["conversationId"],
            general.to_string()
        );
        assert_eq!(resolved["actions"][0]["text"], "plain");
        // A channel's name may hold spaces, as an export writes it.
        assert_eq!(
            resolved["actions"][1]["conversationId"],
            planning.to_string()
        );
        assert!(resolve_refs(json!("@missing"), &collections, &channels).is_err());
        // Text that only starts with # or @ is not a reference.
        let prose = json!({"body": "# Getting started", "note": "@rook see this"});
        assert_eq!(
            resolve_refs(prose.clone(), &collections, &channels).expect("prose resolves"),
            prose
        );
    }

    #[test]
    fn a_role_part_reads_as_a_template_role() {
        let (role, default) = template_role(&json!({
            "name": "Tester", "baseRole": "member", "permissions": ["tasks.view"], "default": true
        }))
        .expect("role");
        assert_eq!(role.name, "Tester");
        assert!(default);
        assert!(template_role(&json!({"name": "X", "colour": "red"})).is_err());
    }
}
