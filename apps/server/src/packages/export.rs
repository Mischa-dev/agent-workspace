//! `package_export`'s work: a package from a live setup, through each
//! part's portable form (M1 section 3.6). A space gives its roles, channels,
//! collections, forms, automations, outbound webhooks and skill pages (and
//! its pages when asked); the workspace gives its switches, layout and
//! roles. Ids never leave: a collection a form or rule names becomes
//! `@<key>`, a channel `#name`, and anything else the installer must choose
//! again (an environment, an agent profile, a webhook's address) is left
//! out and named in the part's `needsSetup`.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::format::{Content, ContentKind, Keys, Package, PackageTarget, from_template};
use crate::{
    collection_forms::FormConfig, collections, error::AppError, roles::Standing,
    workspace_templates,
};

/// Where a package comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Space(Uuid),
    Workspace,
}

/// What else goes in.
#[derive(Clone, Copy, Debug)]
pub struct Include {
    pub pages: bool,
    pub skills: bool,
}

/// The package, sealed with its digest. The caller runs the no-secrets
/// check on it.
pub(crate) async fn export(
    connection: &mut PgConnection,
    caller: Standing,
    workspace_id: Uuid,
    source: Source,
    include: Include,
) -> Result<Package, AppError> {
    let space_id = match source {
        Source::Space(space) => Some(space),
        Source::Workspace => None,
    };
    let definition =
        workspace_templates::snapshot(connection, caller, workspace_id, space_id).await?;
    let name = match &definition.project {
        Some(project) => project.name.clone(),
        None => {
            sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
                .bind(workspace_id)
                .fetch_one(&mut *connection)
                .await?
        }
    };
    let mut package = from_template(&name, &name, "", &definition);
    let Some(space_id) = space_id else {
        // A workspace's setup without a space: switches, layout, roles.
        package
            .contents
            .retain(|part| !matches!(part.kind, ContentKind::Space | ContentKind::Channels));
        return Ok(package.sealed());
    };
    package.target = PackageTarget::Space;
    package
        .contents
        .retain(|part| !matches!(part.kind, ContentKind::Switches | ContentKind::Layout));
    let mut keys = Keys::default();
    for part in &package.contents {
        keys.seed(part.kind, &part.key);
    }
    let names = Names::read(connection, space_id, &package).await?;
    let mut parts = Vec::new();
    for (key, body) in forms(connection, space_id, &names, &mut keys).await? {
        package.needs_admin.push(format!("form:{key}"));
        parts.push((ContentKind::Form, key, body));
    }
    for (key, body) in automations(connection, space_id, &names, &mut keys).await? {
        if body["outbound"] == Value::Bool(true) {
            package.needs_admin.push(format!("automation:{key}"));
        }
        parts.push((ContentKind::Automation, key, body));
    }
    for (kind, key, body) in pages(connection, space_id, include, &mut keys).await? {
        parts.push((kind, key, body));
    }
    package
        .contents
        .extend(parts.into_iter().map(|(kind, key, body)| Content {
            kind,
            key,
            path: None,
            body: Some(body),
        }));
    Ok(package.sealed())
}

/// The space's collections and channels by id, as the package names them.
struct Names {
    collections: HashMap<Uuid, String>,
    channels: HashMap<Uuid, String>,
}

impl Names {
    async fn read(
        connection: &mut PgConnection,
        space_id: Uuid,
        package: &Package,
    ) -> Result<Self, AppError> {
        let by_name = package
            .contents
            .iter()
            .filter(|part| part.kind == ContentKind::Collection)
            .filter_map(|part| {
                let name = part.body.as_ref()?["name"].as_str()?.to_owned();
                Some((name, part.key.clone()))
            })
            .collect::<HashMap<_, _>>();
        let collections = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM collections WHERE project_id = $1",
        )
        .bind(space_id)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .filter_map(|(id, name)| Some((id, by_name.get(&name)?.clone())))
        .collect();
        // Only channels the package carries (everyone in the space sees them).
        let carried = package
            .contents
            .iter()
            .filter(|part| part.kind == ContentKind::Channels)
            .flat_map(|part| {
                part.body
                    .as_ref()
                    .and_then(|body| body["channels"].as_array().cloned())
                    .unwrap_or_default()
            })
            .filter_map(|channel| channel["name"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let channels = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM conversations \
             WHERE project_id = $1 AND kind = 'channel' AND name = ANY($2)",
        )
        .bind(space_id)
        .bind(&carried)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .collect();
        Ok(Self {
            collections,
            channels,
        })
    }

    /// A value with its ids made portable: a collection's id becomes
    /// `@<key>`, a channel's `#name`; any other id is left out and its
    /// field named in `setup`.
    fn portable(&self, value: Value, at: &str, setup: &mut Vec<String>) -> Option<Value> {
        match value {
            Value::String(text) => match Uuid::parse_str(&text) {
                Ok(id) => {
                    if let Some(key) = self.collections.get(&id) {
                        Some(Value::String(format!("@{key}")))
                    } else if let Some(name) = self.channels.get(&id) {
                        Some(Value::String(format!("#{name}")))
                    } else {
                        setup.push(at.to_owned());
                        None
                    }
                }
                Err(_) => Some(Value::String(text)),
            },
            Value::Array(items) => Some(Value::Array(
                items
                    .into_iter()
                    .filter_map(|item| self.portable(item, at, setup))
                    .collect(),
            )),
            Value::Object(map) => Some(Value::Object(
                map.into_iter()
                    .filter_map(|(key, item)| {
                        let path = if at.is_empty() {
                            key.clone()
                        } else {
                            format!("{at}.{key}")
                        };
                        self.portable(item, &path, setup).map(|item| (key, item))
                    })
                    .collect::<Map<_, _>>(),
            )),
            other => Some(other),
        }
    }
}

/// The space's forms, each without its link: its name, its collection as
/// `@<key>`, its fields by name, its words.
async fn forms(
    connection: &mut PgConnection,
    space_id: Uuid,
    names: &Names,
    keys: &mut Keys,
) -> Result<Vec<(String, Value)>, AppError> {
    let rows = sqlx::query_as::<_, (String, Uuid, Value)>(
        r#"
        SELECT name, collection_id, form FROM bug_report_links
        WHERE project_id = $1 AND collection_id IS NOT NULL AND form IS NOT NULL
          AND state <> 'disabled'
        ORDER BY created_at, id
        "#,
    )
    .bind(space_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut forms = Vec::new();
    for (name, collection_id, form) in rows {
        // A form on a collection the package doesn't carry stays behind.
        let Some(collection) = names.collections.get(&collection_id) else {
            continue;
        };
        let Ok(form) = serde_json::from_value::<FormConfig>(form) else {
            continue;
        };
        let fields =
            collections::field_definitions(&mut *connection, space_id, collection_id, false)
                .await?
                .into_iter()
                .map(|field| (field.id, field.name))
                .collect::<HashMap<_, _>>();
        let asked = form
            .fields
            .iter()
            .filter_map(|asked| {
                let name = fields.get(&asked.field)?;
                Some(if asked.required {
                    json!({ "field": name, "required": true })
                } else {
                    json!({ "field": name })
                })
            })
            .collect::<Vec<_>>();
        let mut body = json!({
            "name": name,
            "collection": format!("@{collection}"),
            "fields": asked,
        });
        if !form.intro.is_empty() {
            body["intro"] = Value::String(form.intro);
        }
        if !form.thanks.is_empty() {
            body["thanks"] = Value::String(form.thanks);
        }
        forms.push((keys.take(ContentKind::Form, &name), body));
    }
    Ok(forms)
}

/// The space's automation rules, and its outbound webhooks as automations
/// flagged `outbound`. A webhook's address often carries its secret, so
/// only its host travels; the installer gives the address again.
async fn automations(
    connection: &mut PgConnection,
    space_id: Uuid,
    names: &Names,
    keys: &mut Keys,
) -> Result<Vec<(String, Value)>, AppError> {
    let rules = sqlx::query_as::<_, (String, bool, String, Value, Option<Uuid>, Value, Value)>(
        r#"
        SELECT name, enabled, trigger_kind, trigger, view_id, conditions, actions
        FROM automation_rules
        WHERE project_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(space_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut parts = Vec::new();
    for (name, enabled, trigger_kind, trigger, view_id, conditions, actions) in rules {
        let mut setup = Vec::new();
        let mut trigger = names
            .portable(trigger, "trigger", &mut setup)
            .unwrap_or_else(|| json!({}));
        if let Some(map) = trigger.as_object_mut() {
            map.insert("kind".to_owned(), Value::String(trigger_kind));
        }
        if view_id.is_some() {
            setup.push("view".to_owned());
        }
        let mut body = json!({
            "name": name,
            "enabled": enabled,
            "trigger": trigger,
            "conditions": names.portable(conditions, "conditions", &mut setup).unwrap_or_else(|| json!({})),
            "actions": names.portable(actions, "actions", &mut setup).unwrap_or_else(|| json!([])),
        });
        if !setup.is_empty() {
            body["needsSetup"] = json!(setup);
        }
        parts.push((keys.take(ContentKind::Automation, &name), body));
    }
    let hooks = sqlx::query_as::<_, (String, String, Vec<String>)>(
        r#"
        SELECT name, url, event_types FROM webhook_subscriptions
        WHERE project_id = $1 AND state <> 'disabled'
        ORDER BY created_at, id
        "#,
    )
    .bind(space_id)
    .fetch_all(&mut *connection)
    .await?;
    for (name, url, events) in hooks {
        let host = url
            .split_once("://")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .map(|authority| authority.rsplit('@').next().unwrap_or(authority).to_owned())
            .unwrap_or_default();
        let body = json!({
            "name": name,
            "outbound": true,
            "webhook": { "host": host, "events": events },
            "needsSetup": ["webhook.url"],
        });
        parts.push((keys.take(ContentKind::Automation, &name), body));
    }
    Ok(parts)
}

/// The space's skill pages (as skills) and, when asked, its other pages.
async fn pages(
    connection: &mut PgConnection,
    space_id: Uuid,
    include: Include,
    keys: &mut Keys,
) -> Result<Vec<(ContentKind, String, Value)>, AppError> {
    if !include.pages && !include.skills {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (String, String, bool, Option<String>)>(
        r#"
        SELECT title, body, is_guide, skill_description FROM project_pages
        WHERE project_id = $1 AND archived_at IS NULL
        ORDER BY created_at, id
        "#,
    )
    .bind(space_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut parts = Vec::new();
    for (title, body, is_guide, skill) in rows {
        match skill {
            Some(description) if include.skills => parts.push((
                ContentKind::Skill,
                keys.take(ContentKind::Skill, &title),
                json!({ "name": title, "description": description, "instructions": body }),
            )),
            None if include.pages => parts.push((
                ContentKind::Page,
                keys.take(ContentKind::Page, &title),
                json!({ "title": title, "guide": is_guide, "markdown": body }),
            )),
            _ => {}
        }
    }
    Ok(parts)
}
