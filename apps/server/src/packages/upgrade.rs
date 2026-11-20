//! Package upgrades (M1.10): a newer version of an installed package,
//! planned three ways for each part and applied in one go.
//!
//! For each part: the base is what the install put there
//! (`package_objects.installed_hash`), the local side is the object as it
//! is now, compared with a fingerprint taken when the package last put it
//! there (`local_hash`), and the incoming side is the new version.
//!
//! - unchanged upstream: kept as it is here;
//! - changed upstream, untouched here: the new version is applied;
//! - changed both: a conflict, which keeps yours unless you take theirs;
//! - added upstream: made (or an object of the same name reused), as an
//!   install does;
//! - removed upstream: detached: it stays where it is, as it is, and
//!   removing the package leaves it alone;
//! - collections only grow: the new version's new fields and views are
//!   added, a field whose type changed comes in beside the old one, and
//!   nothing here is removed or changed.
//!
//! Nothing is ever deleted. A rule, a form or a theme acts as (or belongs
//! to) the person who installed it, so only they can take its new version.

use std::collections::{BTreeMap, HashMap};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    apply::{self, InstallError, Installer},
    format::{ContentKind, Package, PackageTarget, canonical},
    install::{self, Place, Plan},
};
use crate::{error::AppError, modules::built::Reader, ops::OpError};

const NAME: &str = "package_upgrade";

/// A fingerprint of the object a `package_objects` row (aliased `object`)
/// points at, as it is now: NULL when it is gone or archived. Only ever
/// compared with the same object's earlier fingerprint.
const LOCAL_HASH_SQL: &str = r#"
    CASE object.object_kind
        WHEN 'page' THEN (SELECT md5(jsonb_build_array(p.title, p.body, p.skill_description,
                                                       p.is_guide)::text)
                          FROM project_pages p WHERE p.id = object.object_id
                            AND p.archived_at IS NULL)
        WHEN 'skill' THEN (SELECT md5(jsonb_build_array(p.title, p.body, p.skill_description,
                                                        p.is_guide)::text)
                           FROM project_pages p WHERE p.id = object.object_id
                             AND p.archived_at IS NULL)
        WHEN 'automation' THEN (SELECT md5(jsonb_build_array(r.name, r.trigger, r.conditions,
                                                             r.actions, r.enabled)::text)
                                FROM automation_rules r WHERE r.id = object.object_id)
        WHEN 'form' THEN (SELECT md5(jsonb_build_array(l.name, l.collection_id, l.form)::text)
                          FROM bug_report_links l WHERE l.id = object.object_id
                            AND l.state <> 'disabled')
        WHEN 'role' THEN (SELECT md5(jsonb_build_array(w.name, w.description, w.base_role,
                                                       w.permissions, w.modules_off,
                                                       w.defaults)::text)
                          FROM workspace_roles w WHERE w.id = object.object_id)
        WHEN 'theme' THEN (SELECT md5(jsonb_build_array(t.name, t.document)::text)
                           FROM themes t WHERE t.id = object.object_id)
        WHEN 'agent' THEN (SELECT md5(jsonb_build_array(a.name, a.purpose, a.definition)::text)
                           FROM agent_templates a WHERE a.id = object.object_id
                             AND a.archived_at IS NULL)
        WHEN 'module' THEN (
            SELECT md5(jsonb_build_array(
                m.name, m.description, m.icon,
                (SELECT coalesce(jsonb_agg(jsonb_build_array(part.part_kind, part.object_id)
                                           ORDER BY part.position), '[]'::jsonb)
                 FROM module_parts part WHERE part.module_id = m.id),
                (SELECT coalesce(jsonb_agg(jsonb_build_array(nav.label, nav.icon, nav.target)
                                           ORDER BY nav.position), '[]'::jsonb)
                 FROM module_nav nav WHERE nav.module_id = m.id))::text)
            FROM modules m WHERE m.id = object.object_id AND m.archived_at IS NULL)
        WHEN 'collection' THEN (SELECT 'live' FROM collections c
                                WHERE c.id = object.object_id AND c.archived_at IS NULL)
        WHEN 'space' THEN (SELECT 'live' FROM projects s
                           WHERE s.id = object.object_id AND s.archived_at IS NULL)
        ELSE 'live'
    END
"#;

/// Who an object acts as, for the kinds that act as (or belong to) the
/// person who installed them.
const OWNER_SQL: &str = r#"
    CASE object.object_kind
        WHEN 'automation' THEN (SELECT r.owner_actor_id FROM automation_rules r
                                WHERE r.id = object.object_id)
        WHEN 'form' THEN (SELECT l.owner_actor_id FROM bug_report_links l
                          WHERE l.id = object.object_id)
        WHEN 'theme' THEN (SELECT t.owner_account_id FROM themes t WHERE t.id = object.object_id)
    END
"#;

/// Fingerprints what an install made or reused that has none yet.
pub(crate) async fn record_local_hashes(
    connection: &mut PgConnection,
    install_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(&format!(
        "UPDATE package_objects object SET local_hash = ({LOCAL_HASH_SQL}) \
         WHERE object.install_id = $1 AND object.local_hash IS NULL"
    ))
    .bind(install_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// One line of an upgrade's plan: an install plan's line.
pub type UpgradeItem = install::PlanItem;

/// An upgrade's plan.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePlan {
    pub from_version: String,
    pub to_version: String,
    pub major: bool,
    pub items: Vec<UpgradeItem>,
    pub turns_off: Vec<String>,
    pub turns_on: Vec<String>,
    #[schemars(with = "Value")]
    pub roles: Vec<Value>,
    pub leaves: Vec<String>,
    pub plan_hash: String,
}

/// What an upgrade did. Lists name parts as "kind:key"; kept ones say why.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Upgraded {
    /// The new install; the old one is superseded.
    #[schemars(with = "String")]
    pub install_id: Uuid,
    #[schemars(with = "String")]
    pub upgraded_from: Uuid,
    pub version: String,
    pub changes: Vec<String>,
    pub updated: Vec<String>,
    pub added: Vec<String>,
    pub kept: Vec<String>,
    pub detached: Vec<String>,
    pub skipped: Vec<String>,
}

/// A conflict's resolution.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    KeepMine,
    TakeTheirs,
}

/// The install being upgraded.
#[derive(Clone, Debug)]
pub(crate) struct OldInstall {
    pub id: Uuid,
    pub space_id: Option<Uuid>,
    pub version: String,
    pub package: Package,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct OldObject {
    key: String,
    kind: String,
    id: Uuid,
    action: String,
    installed_hash: String,
    local_hash: Option<String>,
    now: Option<String>,
    owner: Option<Uuid>,
}

/// A plan, with what applying it needs.
pub(crate) struct Planned {
    pub plan: UpgradePlan,
    pub old: OldInstall,
    place: Place,
    install_plan: Plan,
    objects: Vec<OldObject>,
    incoming: HashMap<(String, String), String>,
}

/// Part hashes by (kind, key), with a line per channel as installs keep
/// them.
fn part_hashes(package: &Package) -> HashMap<(String, String), String> {
    let mut hashes = HashMap::new();
    for part in &package.contents {
        let hash = install::part_hash(part);
        if part.kind == ContentKind::Channels {
            for channel in part
                .body
                .as_ref()
                .and_then(|body| body["channels"].as_array().cloned())
                .unwrap_or_default()
            {
                if let Some(name) = channel["name"].as_str() {
                    hashes.insert(("channel".to_owned(), name.trim().to_owned()), hash.clone());
                }
            }
        }
        hashes.insert((part.kind.key().to_owned(), part.key.clone()), hash);
    }
    hashes
}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

fn line(action: &str, from: &install::PlanItem, reason: Option<String>) -> UpgradeItem {
    UpgradeItem {
        action: action.to_owned(),
        kind: from.kind.clone(),
        key: from.key.clone(),
        name: from.name.clone(),
        needs_admin: from.needs_admin,
        reason,
        existing_id: from.existing_id,
    }
}

fn because(text: &str) -> Option<String> {
    Some(text.to_owned())
}

/// The install, if the reader reaches it and it is installed.
pub(crate) async fn old_install(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    install_id: Uuid,
    reader: Reader,
) -> Result<OldInstall, OpError> {
    let row = sqlx::query_as::<_, (Option<Uuid>, String, String, Value)>(&format!(
        "SELECT install.space_id, install.state, package.version, package.document \
         FROM package_installs install JOIN packages package ON package.id = install.package_id \
         WHERE install.workspace_id = $1 AND install.id = $2 \
           AND (install.space_id IS NULL OR {})",
        Reader::reaches_sql("install.space_id", 3)
    ))
    .bind(workspace_id)
    .bind(install_id)
    .bind(reader.account)
    .bind(reader.connection)
    .bind(reader.held)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?
    .ok_or(OpError::not_found("That install"))?;
    let (space_id, state, version, document) = row;
    if state != "installed" {
        return Err(OpError::invalid(
            "installId",
            match state.as_str() {
                "superseded" => "A newer version replaced that install: upgrade that one.",
                "removed" => "That install was removed: install the new version instead.",
                _ => "That install didn't finish: install the package again.",
            },
        ));
    }
    let package = install::read(&document)?;
    Ok(OldInstall {
        id: install_id,
        space_id,
        version,
        package,
    })
}

/// The plan for upgrading `old` to `package`, read on the transaction that
/// will apply it.
pub(crate) async fn plan(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    reader: Reader,
    old: OldInstall,
    package: &Package,
) -> Result<Planned, OpError> {
    if package.id != old.package.id {
        return Err(apply::invalid_at(
            "package".to_owned(),
            format!(
                "That install is of {}, not {}: install this package instead.",
                old.package.id, package.id
            ),
        ));
    }
    let place = match (old.space_id, package.target) {
        (Some(space), _) => Place::Space(space),
        (None, PackageTarget::Workspace) => Place::Workspace,
        (None, PackageTarget::Space) if old.package.target == PackageTarget::Workspace => {
            return Err(OpError::invalid(
                "package",
                "The new version goes in a space and the old one didn't: install it into one.",
            ));
        }
        (None, PackageTarget::Space) => {
            return Err(OpError::invalid(
                "installId",
                "The space it was installed in is gone: install the new version instead.",
            ));
        }
    };
    let install_plan = install::plan(connection, workspace_id, reader, package, &place).await?;
    let objects = sqlx::query_as::<_, OldObject>(&format!(
        "SELECT object.package_key AS key, object.object_kind AS kind, object.object_id AS id, \
                object.action, object.installed_hash, object.local_hash, \
                ({LOCAL_HASH_SQL}) AS now, ({OWNER_SQL}) AS owner \
         FROM package_objects object WHERE object.install_id = $1 \
         ORDER BY object.object_kind, object.package_key"
    ))
    .bind(old.id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?;
    let by_part = objects
        .iter()
        .filter(|object| object.action != "detached")
        .map(|object| ((object.kind.clone(), object.key.clone()), object))
        .collect::<HashMap<_, _>>();
    let base = part_hashes(&old.package);
    let incoming = part_hashes(package);
    let parts = package
        .contents
        .iter()
        .map(|part| ((part.kind.key().to_owned(), part.key.clone()), part))
        .collect::<HashMap<_, _>>();
    let mut items = Vec::new();
    for planned in &install_plan.items {
        let at = (planned.kind.clone(), planned.key.clone());
        let new_hash = incoming.get(&at);
        let item = match (planned.kind.as_str(), by_part.get(&at)) {
            // Switches and the layout are the workspace's: nothing tells
            // whether they were set here since, so a change is a conflict.
            ("switches" | "layout", _) => {
                if base.get(&at) == new_hash {
                    line("keep", planned, because("Unchanged in the new version."))
                } else if planned.action == "skip" {
                    line("keep", planned, because("The workspace already has it."))
                } else {
                    line(
                        "conflict",
                        planned,
                        because(
                            "The new version changes these workspace settings, which may have \
                             been set here since. Yours stay unless you take theirs.",
                        ),
                    )
                }
            }
            (_, None) => planned.clone(),
            (kind, Some(object)) => {
                let mut item = line("keep", planned, None);
                item.existing_id = Some(object.id);
                if object.now.is_none() {
                    item.reason =
                        because("It was archived or turned off here, so it stays that way.");
                } else if planned.action == "skip" {
                    item.reason = planned.reason.clone();
                } else if new_hash == Some(&object.installed_hash) {
                    item.reason = because("Unchanged in the new version.");
                } else if matches!(kind, "space" | "channel") {
                    item.reason = because("Nothing to change.");
                } else if kind == "collection" {
                    let body = parts
                        .get(&at)
                        .and_then(|part| part.body.clone())
                        .unwrap_or_default();
                    let space = old.space_id.ok_or(OpError::Unavailable)?;
                    let added = collection_additions(connection, space, object.id, &body).await?;
                    if added.is_empty() {
                        item.reason = because("Nothing new to add.");
                    } else {
                        item.action = "update".to_owned();
                        item.reason = Some(format!(
                            "Adds {}. Nothing here is removed or changed.",
                            added.describe()
                        ));
                    }
                } else if matches!(kind, "automation" | "form" | "theme")
                    && object.owner != Some(reader.account)
                {
                    item.reason = because(
                        "It acts as the person who installed it, so only they can take the \
                         new version.",
                    );
                } else if object.local_hash.is_some() && object.local_hash == object.now {
                    item.action = "update".to_owned();
                    item.reason = because("Unchanged here, so it takes the new version.");
                } else {
                    item.action = "conflict".to_owned();
                    item.reason = because(
                        "Changed here and in the new version. Yours stays unless you take \
                         theirs.",
                    );
                }
                item
            }
        };
        items.push(item);
    }
    for object in &objects {
        let at = (object.kind.clone(), object.key.clone());
        if object.action == "detached" || incoming.contains_key(&at) {
            continue;
        }
        items.push(UpgradeItem {
            action: "detach".to_owned(),
            kind: object.kind.clone(),
            key: object.key.clone(),
            name: object.key.clone(),
            needs_admin: false,
            reason: because("No longer in the package: it stays here as it is."),
            existing_id: Some(object.id),
        });
    }
    let plan_hash = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(
            canonical(&json!({
                "install": old.id,
                "digest": install_plan.digest,
                "items": items,
                "turnsOff": install_plan.turns_off,
                "turnsOn": install_plan.turns_on,
            }))
            .as_bytes()
        ))
    );
    let plan = UpgradePlan {
        from_version: old.version.clone(),
        to_version: package.version.clone(),
        major: major(&old.version) != major(&package.version),
        items,
        turns_off: install_plan.turns_off.clone(),
        turns_on: install_plan.turns_on.clone(),
        roles: install_plan.roles.clone(),
        leaves: install_plan.leaves.clone(),
        plan_hash,
    };
    Ok(Planned {
        plan,
        old,
        place,
        install_plan,
        objects,
        incoming,
    })
}

/// What a collection's new version adds here: fields by names it lacks
/// (a field whose type changed comes in beside the old one, with the type
/// in its name) and views by names it lacks.
#[derive(Debug, Default)]
struct Additions {
    fields: Vec<crate::collections::definition::FieldWrite>,
    views: Vec<crate::collections::definition::ViewWrite>,
}

impl Additions {
    fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.views.is_empty()
    }

    fn describe(&self) -> String {
        let mut named = self
            .fields
            .iter()
            .filter_map(|field| field.name.clone())
            .map(|name| format!("the field {name}"))
            .collect::<Vec<_>>();
        named.extend(
            self.views
                .iter()
                .filter_map(|view| view.name.clone())
                .map(|name| format!("the view {name}")),
        );
        named.join(", ")
    }
}

async fn collection_additions(
    connection: &mut PgConnection,
    space: Uuid,
    collection_id: Uuid,
    body: &Value,
) -> Result<Additions, OpError> {
    let Ok(wanted) = serde_json::from_value::<crate::collections::definition::CollectionDefinition>(
        body.clone(),
    ) else {
        return Ok(Additions::default());
    };
    let fields =
        crate::collections::field_definitions(&mut *connection, space, collection_id, false)
            .await?;
    let views = sqlx::query_scalar::<_, String>(
        "SELECT lower(name) FROM collection_views WHERE collection_id = $1",
    )
    .bind(collection_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?;
    let held = |name: &str| {
        fields
            .iter()
            .find(|field| field.name.eq_ignore_ascii_case(name.trim()))
    };
    let mut additions = Additions::default();
    for field in wanted.fields {
        let Some(name) = field.name.clone() else {
            continue;
        };
        match (held(&name), field.field_type.as_deref()) {
            (None, _) => additions.fields.push(field),
            (Some(here), Some(wanted_type)) if here.field_type.as_str() != wanted_type => {
                let beside = format!("{} ({})", name.trim(), wanted_type.replace('_', " "));
                if held(&beside).is_none() {
                    additions
                        .fields
                        .push(crate::collections::definition::FieldWrite {
                            name: Some(beside),
                            ..field
                        });
                }
            }
            _ => {}
        }
    }
    additions.views = wanted
        .views
        .into_iter()
        .filter(|view| {
            view.name
                .as_deref()
                .is_some_and(|name| !views.contains(&name.trim().to_lowercase()))
        })
        .collect();
    Ok(additions)
}

/// Applies a plan: the parts added upstream (and the roles and settings
/// taken) through an install of the new version, then each part that takes
/// the new version in place. The new install holds every object the old
/// one did; the old one is marked superseded.
pub(crate) async fn upgrade(
    tx: &mut Transaction<'static, Postgres>,
    who: &Installer<'_>,
    planned: Planned,
    package: &Package,
    document: &Value,
    choices: &BTreeMap<String, Choice>,
) -> Result<Upgraded, InstallError> {
    let Planned {
        plan,
        old,
        place,
        mut install_plan,
        objects,
        incoming,
    } = planned;
    for chosen in choices.keys() {
        if !plan.items.iter().any(|item| {
            item.action == "conflict" && format!("{}:{}", item.kind, item.key) == *chosen
        }) {
            return Err(apply::invalid_at(
                "choices".to_owned(),
                format!("{chosen} isn't a conflict in this plan: name conflicts as kind:key."),
            )
            .into());
        }
    }
    // What each line ends as.
    let taken = |item: &UpgradeItem| match item.action.as_str() {
        "update" => true,
        "conflict" => {
            choices.get(&format!("{}:{}", item.kind, item.key)) == Some(&Choice::TakeTheirs)
        }
        _ => false,
    };
    let finals = plan
        .items
        .iter()
        .map(|item| ((item.kind.clone(), item.key.clone()), (item, taken(item))))
        .collect::<HashMap<_, _>>();
    // The install of the new version makes what was added upstream, and
    // takes the roles and workspace settings that take theirs; everything
    // else it leaves to the steps below.
    for line in &mut install_plan.items {
        let Some((item, take)) = finals.get(&(line.kind.clone(), line.key.clone())) else {
            continue;
        };
        let keep_line = match item.action.as_str() {
            "create" | "reuse" | "skip" => true,
            _ => *take && matches!(line.kind.as_str(), "role" | "switches" | "layout"),
        };
        if !keep_line {
            line.action = "skip".to_owned();
            line.reason = None;
        }
    }
    // Collections taken grow first: a new or updated form, view or
    // automation may name a field the new version adds. (Growing is
    // additive, so the pass below finds nothing more to add.)
    let none = HashMap::new();
    let no_objects = HashMap::new();
    let growing = Applying {
        workspace: who.workspace,
        space: old.space_id,
        collections: &none,
        channels: &none,
        objects: &no_objects,
    };
    for object in objects
        .iter()
        .filter(|object| object.kind == "collection" && object.action != "detached")
    {
        let at = (object.kind.clone(), object.key.clone());
        let Some((_, true)) = finals.get(&at) else {
            continue;
        };
        let Some(body) = package
            .contents
            .iter()
            .find(|part| part.kind.key() == "collection" && part.key == object.key)
            .and_then(|part| part.body.as_ref())
        else {
            continue;
        };
        let field = format!("package.collection:{}", object.key);
        take_new(tx, who, &growing, "collection", object.id, body, &field).await?;
    }
    let installed = apply::install(tx, who, package, document, &place, &install_plan).await?;
    let new_install = installed.install_id;
    let space = installed.space_id.or(old.space_id);

    // References a part's new version makes, by key.
    let mut collections = HashMap::new();
    let mut channels = HashMap::new();
    let mut known = HashMap::new();
    for (kind, key, id) in objects
        .iter()
        .filter(|object| object.action != "detached")
        .map(|object| (object.kind.as_str(), object.key.as_str(), object.id))
        .chain(
            installed
                .objects
                .iter()
                .map(|object| (object.kind.as_str(), object.key.as_str(), object.id)),
        )
    {
        known.insert((kind.to_owned(), key.to_owned()), id);
        match kind {
            "collection" => {
                collections.insert(key.to_owned(), id);
            }
            "channel" => {
                channels.insert(key.to_lowercase(), id);
            }
            _ => {}
        }
    }
    let parts = package
        .contents
        .iter()
        .map(|part| ((part.kind.key().to_owned(), part.key.clone()), part))
        .collect::<HashMap<_, _>>();

    let mut updated = Vec::new();
    let mut kept = Vec::new();
    let mut detached = Vec::new();
    let mut changes = installed.changes.clone();
    for object in objects.iter().filter(|object| object.action != "detached") {
        let at = (object.kind.clone(), object.key.clone());
        let label = format!("{}:{}", object.kind, object.key);
        let Some((item, take)) = finals.get(&at) else {
            continue;
        };
        let (action, installed_hash, local_hash) = match item.action.as_str() {
            "detach" => {
                detached.push(label);
                (
                    "detached",
                    object.installed_hash.clone(),
                    object.local_hash.clone(),
                )
            }
            _ if *take => {
                let incoming_hash = incoming.get(&at).cloned().unwrap_or_default();
                let part = parts.get(&at).copied();
                let body = part.and_then(|part| part.body.clone()).unwrap_or_default();
                let field = format!("package.{label}");
                let applied = match object.kind.as_str() {
                    // Taken by the install above.
                    "role" => true,
                    kind => {
                        let place = Applying {
                            workspace: who.workspace,
                            space,
                            collections: &collections,
                            channels: &channels,
                            objects: &known,
                        };
                        take_new(tx, who, &place, kind, object.id, &body, &field).await?
                    }
                };
                if applied {
                    updated.push(label);
                    (object.action.as_str(), incoming_hash, None)
                } else {
                    kept.push(format!("{label}: it couldn't take the new version here"));
                    (
                        object.action.as_str(),
                        object.installed_hash.clone(),
                        object.local_hash.clone(),
                    )
                }
            }
            "conflict" => {
                kept.push(format!("{label}: yours, as you chose"));
                // The new version becomes the base, so a later version that
                // changes nothing more leaves it alone; the fingerprint
                // stays, so one that changes it again asks again.
                (
                    object.action.as_str(),
                    incoming.get(&at).cloned().unwrap_or_default(),
                    Some(
                        object
                            .local_hash
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned()),
                    ),
                )
            }
            _ => {
                if let Some(reason) = &item.reason {
                    kept.push(format!("{label}: {reason}"));
                }
                (
                    object.action.as_str(),
                    object.installed_hash.clone(),
                    Some(
                        object
                            .local_hash
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned()),
                    ),
                )
            }
        };
        sqlx::query(
            r#"
            INSERT INTO package_objects (
                install_id, workspace_id, package_key, object_kind, object_id, action,
                installed_hash, local_hash
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (install_id, package_key, object_kind) DO UPDATE SET
                object_id = EXCLUDED.object_id, action = EXCLUDED.action,
                installed_hash = EXCLUDED.installed_hash, local_hash = EXCLUDED.local_hash
            "#,
        )
        .bind(new_install)
        .bind(who.workspace)
        .bind(&object.key)
        .bind(&object.kind)
        .bind(object.id)
        .bind(action)
        .bind(installed_hash)
        .bind(local_hash)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(NAME, &error.into()))?;
    }
    record_local_hashes(tx, new_install)
        .await
        .map_err(|error| OpError::logged(NAME, &error))?;
    for sql in [
        "UPDATE package_installs SET upgraded_from = $2 WHERE id = $1",
        "UPDATE package_installs SET state = 'superseded' WHERE id = $2",
        "UPDATE modules SET package_install_id = $1 WHERE package_install_id = $2",
        "UPDATE agent_templates SET install_id = $1 WHERE install_id = $2",
    ] {
        sqlx::query(sql)
            .bind(new_install)
            .bind(old.id)
            .execute(&mut **tx)
            .await
            .map_err(|error| OpError::logged(NAME, &error.into()))?;
    }
    if !updated.is_empty() {
        changes.push(format!(
            "Brought {} part{} up to {}.",
            updated.len(),
            if updated.len() == 1 { "" } else { "s" },
            package.version
        ));
    }
    let added = installed
        .objects
        .iter()
        .filter(|object| {
            finals
                .get(&(object.kind.clone(), object.key.clone()))
                .is_some_and(|(item, _)| matches!(item.action.as_str(), "create" | "reuse"))
        })
        .map(|object| format!("{}:{}", object.kind, object.key))
        .collect();
    // The install's own skipped list names lines this upgrade left out on
    // purpose; only what the new version couldn't add is reported.
    let skipped = plan
        .items
        .iter()
        .filter(|item| item.action == "skip" && item.existing_id.is_none())
        .map(|item| {
            format!(
                "{}:{}: {}",
                item.kind,
                item.key,
                item.reason.as_deref().unwrap_or("left out")
            )
        })
        .collect();
    sqlx::query("UPDATE package_installs SET summary = $2 WHERE id = $1")
        .bind(new_install)
        .bind(&changes)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(NAME, &error.into()))?;
    Ok(Upgraded {
        install_id: new_install,
        upgraded_from: old.id,
        version: package.version.clone(),
        changes,
        updated,
        added,
        kept,
        detached,
        skipped,
    })
}

/// Where a part's new version lands, and what its references resolve to.
struct Applying<'a> {
    workspace: Uuid,
    space: Option<Uuid>,
    collections: &'a HashMap<String, Uuid>,
    channels: &'a HashMap<String, Uuid>,
    /// Every object the new install holds, by (kind, key).
    objects: &'a HashMap<(String, String), Uuid>,
}

/// Gives one object its part's new version. False when it couldn't (it is
/// gone, or someone else's).
async fn take_new(
    tx: &mut Transaction<'static, Postgres>,
    who: &Installer<'_>,
    place: &Applying<'_>,
    kind: &str,
    id: Uuid,
    body: &Value,
    field: &str,
) -> Result<bool, OpError> {
    let resolved = || {
        install::resolve_refs(body.clone(), place.collections, place.channels)
            .map_err(|message| apply::invalid_at(field.to_owned(), message))
    };
    let space = || place.space.ok_or(OpError::Unavailable);
    match kind {
        "page" | "skill" => {
            let kind = if kind == "skill" {
                ContentKind::Skill
            } else {
                ContentKind::Page
            };
            crate::memory::update_page(tx, who.writer, id, &apply::page_input(kind, body)).await?;
            Ok(true)
        }
        "agent" => {
            let name = body["name"].as_str().unwrap_or("").trim().to_owned();
            if name.is_empty() || name.chars().count() > 80 {
                return Err(apply::invalid_at(
                    format!("{field}.name"),
                    "Name the agent template in 1 to 80 characters.".to_owned(),
                ));
            }
            let purpose = body["purpose"].as_str().unwrap_or("").trim();
            let done = sqlx::query(
                "UPDATE agent_templates SET name = $3, purpose = $4, definition = $5 \
                 WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL",
            )
            .bind(id)
            .bind(place.workspace)
            .bind(&name)
            .bind(purpose.chars().take(500).collect::<String>())
            .bind(body)
            .execute(&mut **tx)
            .await
            .map_err(|_| {
                apply::invalid_at(
                    field.to_owned(),
                    "This agent template can't be stored: keep it under 64 KB.".to_owned(),
                )
            })?
            .rows_affected();
            Ok(done > 0)
        }
        "theme" => {
            let (name, document) = super::format::check_theme(body)
                .map_err(|message| apply::invalid_at(field.to_owned(), message))?;
            let document = serde_json::to_value(&document).map_err(|_| OpError::Unavailable)?;
            let done = sqlx::query(
                "UPDATE themes SET name = $4, document = $5, version = version + 1, \
                 updated_at = now() WHERE id = $1 AND workspace_id = $2 AND owner_account_id = $3",
            )
            .bind(id)
            .bind(place.workspace)
            .bind(who.actor)
            .bind(&name)
            .bind(&document)
            .execute(&mut **tx)
            .await
            .map_err(|error| OpError::logged(NAME, &error.into()))?
            .rows_affected();
            if done > 0 {
                crate::themes::announce(tx, place.workspace, who.actor, id, true).await?;
            }
            Ok(done > 0)
        }
        "automation" => {
            let space = space()?;
            let rule = apply::rule_from_body(&resolved()?, field)?;
            let prepared = crate::automations::prepare_in(tx, &who.maker, space, rule, &[]).await?;
            Ok(crate::automations::replace_in(tx, &who.maker, space, id, &prepared).await?)
        }
        "form" => {
            let space = space()?;
            let (name, collection_id, form) =
                apply::form_from_body(tx, space, &resolved()?, field).await?;
            Ok(crate::bug_reports::replace_form_in(
                tx,
                &who.maker,
                space,
                id,
                &name,
                collection_id,
                &form,
            )
            .await?)
        }
        "collection" => {
            let space = space()?;
            let current = sqlx::query_as::<_, (String, String)>(
                "SELECT collection.name, space.project_key FROM collections collection \
                 JOIN projects space ON space.id = collection.project_id \
                 WHERE collection.id = $1 AND collection.project_id = $2 \
                   AND collection.archived_at IS NULL",
            )
            .bind(id)
            .bind(space)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| OpError::logged(NAME, &error.into()))?;
            let Some((name, project_key)) = current else {
                return Ok(false);
            };
            let additions = collection_additions(tx, space, id, body).await?;
            if additions.is_empty() {
                return Ok(true);
            }
            // The collection as it is named here, so a renamed one grows
            // instead of a new one being made.
            let project = crate::workspace_templates::TemplateProject {
                name: String::new(),
                key: String::new(),
                channels: Vec::new(),
                collections: vec![crate::collections::definition::CollectionDefinition {
                    name,
                    description: String::new(),
                    title_name: String::new(),
                    fields: additions.fields,
                    views: additions.views,
                }],
            };
            crate::workspace_templates::starter_contents(
                tx,
                who.actor,
                crate::workspace_templates::StarterProject {
                    organization_id: place.workspace,
                    id: space,
                    new: false,
                },
                &project_key,
                &project,
            )
            .await
            .map_err(|error| match error {
                crate::workspace_templates::ApplyError::Problem(problem) => {
                    apply::invalid_at(format!("{field}.{}", problem.field), problem.message)
                }
                crate::workspace_templates::ApplyError::App(error) => OpError::logged(NAME, &error),
            })?;
            Ok(true)
        }
        "module" => {
            let Some(module) = crate::modules::built::get(tx, place.workspace, id).await? else {
                return Ok(false);
            };
            let name = body["name"].as_str().unwrap_or("").trim();
            sqlx::query(
                "UPDATE modules SET name = CASE WHEN $2 = '' THEN name ELSE $2 END, \
                 description = $3, icon = NULLIF($4, '') WHERE id = $1",
            )
            .bind(id)
            .bind(name)
            .bind(body["description"].as_str().unwrap_or(""))
            .bind(body["icon"].as_str().unwrap_or(""))
            .execute(&mut **tx)
            .await
            .map_err(|_| {
                apply::invalid_at(
                    field.to_owned(),
                    "This module can't be changed: check its name (1 to 60 characters) and \
                     icon."
                        .to_owned(),
                )
            })?;
            // New parts and nav entries are added; none is taken away.
            let linked = sqlx::query_scalar::<_, Uuid>(
                "SELECT object_id FROM module_parts WHERE module_id = $1",
            )
            .bind(id)
            .fetch_all(&mut **tx)
            .await
            .map_err(|error| OpError::logged(NAME, &error.into()))?;
            for wanted in body["parts"].as_array().cloned().unwrap_or_default() {
                let part_kind = wanted["kind"].as_str().and_then(apply::part_kind_static);
                let part_key = wanted["key"].as_str().unwrap_or("");
                let object = part_kind.and_then(|kind| {
                    place
                        .objects
                        .get(&(kind.to_owned(), part_key.to_owned()))
                        .copied()
                });
                let (Some(part_kind), Some(object)) = (part_kind, object) else {
                    continue;
                };
                if linked.contains(&object) {
                    continue;
                }
                crate::modules::built::link_part(
                    tx,
                    &module,
                    part_kind,
                    object,
                    who.actor,
                    who.confined,
                )
                .await?;
            }
            let labels = sqlx::query_scalar::<_, String>(
                "SELECT lower(label) FROM module_nav WHERE module_id = $1",
            )
            .bind(id)
            .fetch_all(&mut **tx)
            .await
            .map_err(|error| OpError::logged(NAME, &error.into()))?;
            for entry in body["nav"].as_array().cloned().unwrap_or_default() {
                let label = entry["label"].as_str().unwrap_or("").trim();
                if label.is_empty() || labels.contains(&label.to_lowercase()) {
                    continue;
                }
                crate::modules::built::add_nav(
                    tx,
                    &module,
                    label,
                    entry["icon"].as_str(),
                    &entry["target"],
                )
                .await?;
            }
            crate::modules::built::touch(tx, id).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_major_version_is_its_first_number() {
        assert_eq!(major("1.2.0"), "1");
        assert_ne!(major("1.9.0"), major("2.0.0"));
        assert_eq!(major("3"), "3");
    }
}
