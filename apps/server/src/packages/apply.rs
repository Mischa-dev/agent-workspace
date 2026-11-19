//! Installing a package (M1.9, M1.11): the one function the
//! `package_install` operation, the workspace-template routes and tools
//! (which install a template read as a package) and shapes
//! (`workspace_shape_set`, M1.17) all run. The caller has read the
//! package, planned it in this transaction and checked who may install
//! what; this makes it, never deleting anything.

use std::collections::HashMap;

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    format::{ContentKind, Package},
    install::{self, Place, Plan},
};
use crate::{
    error::AppError,
    ops::OpError,
    roles::{Auditor, Standing},
    workspace_templates::{ApplyError, Problem},
};

const NAME: &str = "package_install";

/// An object an install made or reused.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstalledObject {
    pub kind: String,
    pub key: String,
    #[schemars(with = "String")]
    pub id: Uuid,
    /// created, reused or updated.
    pub action: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    #[schemars(with = "String")]
    pub install_id: Uuid,
    pub package_id: String,
    pub version: String,
    /// The space it went into.
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    /// What changed, in plain words.
    pub changes: Vec<String>,
    pub objects: Vec<InstalledObject>,
    /// Parts left out, each "kind:key: why".
    pub skipped: Vec<String>,
}

/// Who installs, and where the package came from.
pub(crate) struct Installer<'a> {
    pub workspace: Uuid,
    /// The account installing (a person through an app: the person).
    pub actor: Uuid,
    pub standing: Standing,
    pub auditor: &'a Auditor<'a>,
    pub maker: crate::events::Caller,
    pub writer: crate::memory::Writer,
    /// The template being used (its id and name), when a template route
    /// installs: the setup path's record names it.
    pub template: Option<(String, String)>,
    /// `packages.source`: builtin, directory, upload or legacy_template.
    pub source: &'static str,
    pub source_ref: Option<String>,
    /// The one space the connection is held to, if it is: module parts
    /// come only from there.
    pub confined: Option<Uuid>,
}

/// Why an install stopped. Nothing it did is kept.
#[derive(Debug)]
pub(crate) enum InstallError {
    /// A part the template path refused, with the field the person can fix.
    Problem(Problem),
    Op(OpError),
}

impl From<OpError> for InstallError {
    fn from(error: OpError) -> Self {
        Self::Op(error)
    }
}

impl From<AppError> for InstallError {
    fn from(error: AppError) -> Self {
        Self::Op(error.into())
    }
}

impl From<sqlx::Error> for InstallError {
    fn from(error: sqlx::Error) -> Self {
        Self::Op(OpError::logged(NAME, &error.into()))
    }
}

impl InstallError {
    /// As an operation's refusal: a problem names its field in the package.
    pub(crate) fn into_op(self) -> OpError {
        match self {
            Self::Problem(problem) => OpError::Invalid {
                field: format!("package.{}", problem.field).into(),
                message: problem.message.into(),
            },
            Self::Op(error) => error,
        }
    }
}

fn apply_error(error: ApplyError) -> InstallError {
    match error {
        ApplyError::Problem(problem) => InstallError::Problem(problem),
        ApplyError::App(error) => error.into(),
    }
}

fn part_kind_static(kind: &str) -> Option<&'static str> {
    crate::modules::built::LINKABLE
        .iter()
        .copied()
        .find(|known| *known == kind)
}

fn invalid_at(field: String, message: String) -> OpError {
    OpError::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

/// Installs `package` (its stored form `document`) at `place` as `plan`
/// says, in the caller's transaction.
#[allow(clippy::too_many_lines)]
pub(crate) async fn install(
    tx: &mut Transaction<'static, Postgres>,
    who: &Installer<'_>,
    package: &Package,
    document: &Value,
    place: &Place,
    plan: &Plan,
) -> Result<Installed, InstallError> {
    use crate::workspace_templates::{self as templates, TemplateDefinition, TemplateProject};

    let workspace = who.workspace;
    let actor = who.actor;
    let maker = &who.maker;
    let writer = who.writer;
    let place = place.clone();
    let action_of = |kind: &str, key: &str| {
        plan.items
            .iter()
            .find(|line| line.kind == kind && line.key == key)
            .map(|line| line.action.as_str())
            .unwrap_or("skip")
    };

    // The package and the install, first: what it makes points at it.
    let package_row = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO packages (
            id, workspace_id, package_key, version, name, description, document, digest,
            source, created_by_actor_id, source_ref
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $10, $9, $11)
        ON CONFLICT (workspace_id, package_key, version) DO UPDATE SET
            name = EXCLUDED.name, description = EXCLUDED.description,
            document = EXCLUDED.document, digest = EXCLUDED.digest,
            source = EXCLUDED.source, source_ref = EXCLUDED.source_ref
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(&package.id)
    .bind(&package.version)
    .bind(package.name.trim())
    .bind(&package.description)
    .bind(document)
    .bind(&plan.digest)
    .bind(actor)
    .bind(who.source)
    .bind(who.source_ref.as_deref())
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?;
    let install_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO package_installs (
            id, workspace_id, space_id, package_id, state, plan, plan_hash,
            installed_by_actor_id
        ) VALUES ($1, $2, $3, $4, 'installing', $5, $6, $7)
        "#,
    )
    .bind(install_id)
    .bind(workspace)
    .bind(match place {
        Place::Space(id) => Some(id),
        _ => None,
    })
    .bind(package_row)
    .bind(serde_json::to_value(&plan.items).unwrap_or_default())
    .bind(&plan.plan_hash)
    .bind(actor)
    .execute(&mut **tx)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?;

    // Switches, layout, roles and the starter space: the template path.
    let mut definition = TemplateDefinition::default();
    let mut starter = TemplateProject::default();
    for part in &package.contents {
        let body = part.body.clone().unwrap_or_default();
        if action_of(part.kind.key(), &part.key) == "skip"
            && !matches!(part.kind, ContentKind::Channels | ContentKind::Space)
        {
            continue;
        }
        match part.kind {
            ContentKind::Switches => {
                let off = body["off"].as_array().cloned().unwrap_or_default();
                definition.modules = Some(
                    crate::modules::manifest::core()
                        .iter()
                        .filter(|module| module.switchable)
                        .filter(|module| !off.iter().any(|id| *id == module.id.as_str()))
                        .map(|module| module.id.clone())
                        .collect(),
                );
            }
            ContentKind::Layout => {
                definition.layout = body["preset"].as_str().map(str::to_owned);
                definition.layout_changes =
                    serde_json::from_value(body["changes"].clone()).unwrap_or_default();
                definition.layout_words =
                    serde_json::from_value(body["words"].clone()).unwrap_or_default();
            }
            ContentKind::Role if super::format::is_default_reference(&body) => {
                definition.default_role = body["name"].as_str().map(str::to_owned);
            }
            ContentKind::Role => {
                let (role, default) = install::template_role(&body)
                    .map_err(|message| invalid_at(format!("package.role:{}", part.key), message))?;
                if default {
                    definition.default_role = Some(role.name.clone());
                }
                definition.roles.push(role);
            }
            ContentKind::Channels => {
                for channel in body["channels"].as_array().cloned().unwrap_or_default() {
                    if let Some(name) = channel["name"].as_str()
                        && action_of("channel", name) != "skip"
                    {
                        starter.channels.push(name.to_owned());
                    }
                }
            }
            // A collection the preview showed as reused is left as it is:
            // applying its definition would change a live schema the
            // preview never showed.
            ContentKind::Collection if action_of("collection", &part.key) != "create" => {}
            ContentKind::Collection => {
                let collection = serde_json::from_value(body).map_err(|error| {
                    invalid_at(
                        format!("package.collection:{}", part.key),
                        format!("This collection can't be read: {error}"),
                    )
                })?;
                starter.collections.push(collection);
            }
            _ => {}
        }
    }
    if let Place::New(space) = &place {
        definition.project = Some(TemplateProject {
            name: space.name.clone(),
            key: space.key.clone(),
            ..starter.clone()
        });
    }
    let (template_id, template_name) = who
        .template
        .clone()
        .unwrap_or_else(|| (package.id.clone(), package.name.clone()));
    let view = templates::TemplateView {
        id: template_id,
        name: template_name,
        description: package.description.clone(),
        built_in: false,
        what_it_does: Vec::new(),
        definition,
        saved_by: None,
        saved_at: None,
    };
    let applied = templates::apply(tx, who.auditor, who.standing, workspace, &view)
        .await
        .map_err(apply_error)?;
    let mut changes = applied.changes;
    let space = match &place {
        Place::Space(id) => {
            if !starter.channels.is_empty() || !starter.collections.is_empty() {
                let project_key = sqlx::query_scalar::<_, String>(
                    "SELECT project_key FROM projects WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&mut **tx)
                .await
                .map_err(|error| OpError::logged(NAME, &error.into()))?;
                changes.extend(
                    templates::starter_contents(
                        tx,
                        actor,
                        templates::StarterProject {
                            organization_id: workspace,
                            id: *id,
                            new: false,
                        },
                        &project_key,
                        &starter,
                    )
                    .await
                    .map_err(apply_error)?,
                );
            }
            Some(*id)
        }
        Place::New(_) => applied.project_id,
        Place::Workspace => None,
    };

    // What the template path made, by key.
    let mut objects: Vec<InstalledObject> = Vec::new();
    let mut hashes: HashMap<(String, String), String> = HashMap::new();
    let mut skipped = Vec::new();
    let record = |objects: &mut Vec<InstalledObject>, kind: &str, key: &str, id: Uuid| {
        let action = match action_of(kind, key) {
            "create" => "created",
            "update" => "updated",
            _ => "reused",
        };
        objects.push(InstalledObject {
            kind: kind.to_owned(),
            key: key.to_owned(),
            id,
            action: action.to_owned(),
        });
    };
    let mut collections: HashMap<String, Uuid> = HashMap::new();
    let mut channels: HashMap<String, Uuid> = HashMap::new();
    if let Some(space) = space {
        let rows = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, lower(name) FROM conversations WHERE project_id = $1 \
             AND kind = 'channel' AND archived_at IS NULL \
             AND conversation_visible_to(id, $2)",
        )
        .bind(space)
        .bind(actor)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| OpError::logged(NAME, &error.into()))?;
        channels.extend(rows.into_iter().map(|(id, name)| (name, id)));
    }
    for part in &package.contents {
        let body = part.body.clone().unwrap_or_default();
        hashes.insert(
            (part.kind.key().to_owned(), part.key.clone()),
            install::part_hash(part),
        );
        match part.kind {
            ContentKind::Role if action_of("role", &part.key) != "skip" => {
                let name = body["name"].as_str().unwrap_or("").trim();
                if let Some(id) = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM workspace_roles WHERE organization_id = $1 \
                     AND lower(name) = lower($2)",
                )
                .bind(workspace)
                .bind(name)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|error| OpError::logged(NAME, &error.into()))?
                {
                    record(&mut objects, "role", &part.key, id);
                }
            }
            ContentKind::Space => {
                if let Some(space) = space {
                    record(&mut objects, "space", &part.key, space);
                }
            }
            ContentKind::Channels => {
                for channel in body["channels"].as_array().cloned().unwrap_or_default() {
                    let name = channel["name"].as_str().unwrap_or("").trim().to_owned();
                    if let Some(id) = channels.get(&name.to_lowercase()) {
                        hashes.insert(
                            ("channel".to_owned(), name.clone()),
                            install::part_hash(part),
                        );
                        record(&mut objects, "channel", &name, *id);
                    }
                }
            }
            ContentKind::Collection => {
                let name = body["name"].as_str().unwrap_or("").trim();
                if let Some(space) = space
                    && let Some(id) = sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM collections WHERE project_id = $1 \
                         AND archived_at IS NULL AND lower(name) = lower($2)",
                    )
                    .bind(space)
                    .bind(name)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|error| OpError::logged(NAME, &error.into()))?
                {
                    collections.insert(part.key.clone(), id);
                    record(&mut objects, "collection", &part.key, id);
                }
            }
            _ => {}
        }
    }
    let fresh = objects
        .iter()
        .filter(|object| object.kind == "collection" && object.action == "created")
        .map(|object| object.id)
        .collect::<Vec<_>>();

    // Forms, automations, pages, skills, agent templates and modules.
    for part in &package.contents {
        let kind = part.kind.key();
        let key = part.key.as_str();
        let line = plan
            .items
            .iter()
            .find(|line| line.kind == kind && line.key == key);
        let Some(line) = line else { continue };
        if line.action == "skip" {
            if !matches!(part.kind, ContentKind::Channels) {
                skipped.push(format!(
                    "{kind}:{key}: {}",
                    line.reason.as_deref().unwrap_or("left out")
                ));
            }
            continue;
        }
        if let Some(id) = line.existing_id
            && matches!(
                part.kind,
                ContentKind::Form
                    | ContentKind::Automation
                    | ContentKind::Page
                    | ContentKind::Skill
                    | ContentKind::Agent
                    | ContentKind::Module
                    | ContentKind::Theme
            )
        {
            record(&mut objects, kind, key, id);
            continue;
        }
        let field = |what: &str| format!("package.{kind}:{key}{what}");
        // Only forms, automations and modules point at other parts; a
        // page's or skill's text stays exactly as it was written.
        let body = part.body.clone().unwrap_or_default();
        let body = if matches!(
            part.kind,
            ContentKind::Form | ContentKind::Automation | ContentKind::Module
        ) {
            install::resolve_refs(body, &collections, &channels)
                .map_err(|message| invalid_at(field(""), message))?
        } else {
            body
        };
        let made = match part.kind {
            // Offered to the workspace at once, owned by whoever installs it.
            ContentKind::Theme => {
                let (name, document) = super::format::check_theme(&body)
                    .map_err(|message| invalid_at(field(""), message))?;
                crate::themes::insert_offered(tx, workspace, actor, &name, &document).await?
            }
            ContentKind::Form => {
                let space = space.ok_or(OpError::Unavailable)?;
                let name = crate::bug_reports::normalize_name(body["name"].as_str().unwrap_or(""))
                    .map_err(|_| invalid_at(field(".name"), "Name the form.".to_owned()))?;
                let collection_id = body["collection"]
                    .as_str()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or_else(|| {
                        invalid_at(
                            field(".collection"),
                            "A form names its collection as @<key>.".to_owned(),
                        )
                    })?;
                let definitions =
                    crate::collections::field_definitions(tx, space, collection_id, false).await?;
                let mut questions = Vec::new();
                for asked in body["fields"].as_array().cloned().unwrap_or_default() {
                    let wanted = asked["field"].as_str().unwrap_or("").trim();
                    let found = definitions
                        .iter()
                        .find(|definition| definition.name.eq_ignore_ascii_case(wanted))
                        .ok_or_else(|| {
                            invalid_at(
                                field(".fields"),
                                format!("The collection has no field \"{wanted}\"."),
                            )
                        })?;
                    questions.push(crate::collection_forms::FormField {
                        field: found.id,
                        required: asked["required"].as_bool().unwrap_or(false),
                    });
                }
                let form = crate::collection_forms::FormConfig {
                    intro: body["intro"].as_str().unwrap_or("").to_owned(),
                    fields: questions,
                    thanks: body["thanks"].as_str().unwrap_or("").to_owned(),
                    daily_limit: None,
                };
                let form = crate::bug_reports::checked_form_in(tx, space, collection_id, &form)
                    .await?
                    .map_err(|message| invalid_at(field(".fields"), message))?;
                // The link's token is never returned: the form's
                // settings make one to share.
                let (id, _token) = crate::bug_reports::insert_link(
                    tx,
                    maker,
                    space,
                    &name,
                    Some((collection_id, form)),
                )
                .await?;
                id
            }
            ContentKind::Automation => {
                let space = space.ok_or(OpError::Unavailable)?;
                let rule = json!({
                    "name": body["name"],
                    "trigger": body["trigger"],
                    "conditions": body.get("conditions").cloned().unwrap_or_else(|| json!({})),
                    "actions": body.get("actions").cloned().unwrap_or_else(|| json!([])),
                    "enabled": body.get("enabled").cloned().unwrap_or(Value::Bool(true)),
                });
                let rule = serde_json::from_value::<crate::automations::CreateRule>(rule).map_err(
                    |error| {
                        invalid_at(
                            field(""),
                            error
                                .to_string()
                                .split(" at line ")
                                .next()
                                .unwrap_or("This rule can't be read.")
                                .to_owned(),
                        )
                    },
                )?;
                let prepared =
                    crate::automations::prepare_in(tx, maker, space, rule, &fresh).await?;
                crate::automations::insert(tx, maker, workspace, space, &prepared).await?
            }
            ContentKind::Page | ContentKind::Skill => {
                let space = space.ok_or(OpError::Unavailable)?;
                let page = if part.kind == ContentKind::Skill {
                    crate::memory::PageInput {
                        title: body["name"].as_str().map(str::to_owned),
                        body: body["instructions"].as_str().map(str::to_owned),
                        skill_description: body["description"].as_str().map(str::to_owned),
                        ..crate::memory::PageInput::default()
                    }
                } else {
                    crate::memory::PageInput {
                        title: body["title"].as_str().map(str::to_owned),
                        body: body["markdown"].as_str().map(str::to_owned),
                        is_guide: body["guide"].as_bool(),
                        ..crate::memory::PageInput::default()
                    }
                };
                crate::memory::create_page(tx, writer, space, &page).await?
            }
            ContentKind::Agent => {
                let name = body["name"].as_str().unwrap_or("").trim().to_owned();
                if name.is_empty() || name.chars().count() > 80 {
                    return Err(invalid_at(
                        field(".name"),
                        "Name the agent template in 1 to 80 characters.".to_owned(),
                    )
                    .into());
                }
                let purpose = body["purpose"].as_str().unwrap_or("").trim().to_owned();
                let id = Uuid::now_v7();
                sqlx::query(
                    r#"
                    INSERT INTO agent_templates (
                        id, workspace_id, space_id, install_id, template_key, name, purpose,
                        definition, created_by_actor_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                )
                .bind(id)
                .bind(workspace)
                .bind(space)
                .bind(install_id)
                .bind(key)
                .bind(&name)
                .bind(purpose.chars().take(500).collect::<String>())
                .bind(&body)
                .bind(actor)
                .execute(&mut **tx)
                .await
                .map_err(|_| {
                    invalid_at(
                        field(""),
                        "This agent template can't be stored: keep it under 64 KB.".to_owned(),
                    )
                })?;
                id
            }
            ContentKind::Module => {
                let module_key = crate::modules::built::full_key(key).ok_or_else(|| {
                    invalid_at(field(""), "A module's key is lower snake case.".to_owned())
                })?;
                let name = body["name"].as_str().unwrap_or("").trim().to_owned();
                let id = crate::modules::built::insert(
                    tx,
                    workspace,
                    space,
                    &module_key,
                    &name,
                    body["description"].as_str().unwrap_or(""),
                    body["icon"].as_str(),
                    actor,
                )
                .await
                .map_err(|_| {
                    invalid_at(
                        field(""),
                        "This module can't be made: check its name (1 to 60 characters) \
                         and icon."
                            .to_owned(),
                    )
                })?;
                sqlx::query("UPDATE modules SET package_install_id = $2 WHERE id = $1")
                    .bind(id)
                    .bind(install_id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|error| OpError::logged(NAME, &error.into()))?;
                let module = crate::modules::built::get(tx, workspace, id)
                    .await?
                    .ok_or(OpError::Unavailable)?;
                for wanted in body["parts"].as_array().cloned().unwrap_or_default() {
                    let part_kind = wanted["kind"].as_str().and_then(part_kind_static);
                    let part_key = wanted["key"].as_str().unwrap_or("");
                    let object = objects.iter().find(|object| {
                        Some(object.kind.as_str()) == part_kind && object.key == part_key
                    });
                    let (Some(part_kind), Some(object)) = (part_kind, object) else {
                        return Err(invalid_at(
                            field(".parts"),
                            format!(
                                "A module's parts are collections, forms, automations or \
                                 pages listed before it in the package; {part_key} isn't."
                            ),
                        )
                        .into());
                    };
                    crate::modules::built::link_part(
                        tx,
                        &module,
                        part_kind,
                        object.id,
                        actor,
                        who.confined,
                    )
                    .await?;
                }
                for entry in body["nav"].as_array().cloned().unwrap_or_default() {
                    let label = entry["label"].as_str().unwrap_or("").trim();
                    crate::modules::built::add_nav(
                        tx,
                        &module,
                        label,
                        entry["icon"].as_str(),
                        &entry["target"],
                    )
                    .await?;
                }
                id
            }
            _ => continue,
        };
        record(&mut objects, kind, key, made);
    }

    // What it made, for removal and upgrades.
    for object in &objects {
        sqlx::query(
            r#"
            INSERT INTO package_objects (
                install_id, workspace_id, package_key, object_kind, object_id, action,
                installed_hash
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(install_id)
        .bind(workspace)
        .bind(&object.key)
        .bind(&object.kind)
        .bind(object.id)
        .bind(&object.action)
        .bind(
            hashes
                .get(&(object.kind.clone(), object.key.clone()))
                .cloned()
                .unwrap_or_default(),
        )
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(NAME, &error.into()))?;
    }
    for object in objects.iter().filter(|object| object.action == "created") {
        if object.kind == "collection"
            && let Some(space) = space
        {
            crate::outbox::enqueue_collection_updated(tx, space, object.id, &[]).await?;
        }
    }
    let made = objects
        .iter()
        .filter(|object| object.action == "created")
        .count();
    if made > 0 {
        changes.push(format!(
            "Installed {} ({made} new parts).",
            package.name.trim()
        ));
    }
    sqlx::query(
        "UPDATE package_installs SET state = 'installed', space_id = $2, summary = $3, \
         finished_at = now() WHERE id = $1",
    )
    .bind(install_id)
    .bind(space)
    .bind(&changes)
    .execute(&mut **tx)
    .await
    .map_err(|error| OpError::logged(NAME, &error.into()))?;
    Ok(Installed {
        install_id,
        package_id: package.id.clone(),
        version: package.version.clone(),
        space_id: space,
        changes,
        objects,
        skipped,
    })
}

/// Uses a workspace template (M1.11): the template read as a package (a
/// built-in one is its own package) and installed through [`install`],
/// with the plan as it is now. The workspace-template routes and tools
/// call this after their own checks, so a template does what it did, and
/// each use is a package install too.
pub(crate) async fn use_template(
    tx: &mut Transaction<'static, Postgres>,
    who: Installer<'_>,
    template: &crate::workspace_templates::TemplateView,
) -> Result<crate::workspace_templates::Applied, ApplyError> {
    let (package, document, source, source_ref) = match template
        .built_in
        .then(|| super::builtin::find(&template.id))
        .flatten()
    {
        Some(builtin) => (
            builtin.package.clone(),
            builtin.document.clone(),
            "builtin",
            format!("builtin:{}", builtin.folder),
        ),
        None => {
            let package = super::format::from_template(
                &template.id,
                &template.name,
                &template.description,
                &template.definition,
            );
            let document = serde_json::to_value(&package).unwrap_or(Value::Null);
            (
                package,
                document,
                "legacy_template",
                format!("template:{}", template.id),
            )
        }
    };
    crate::workspace_templates::check(&template.definition).map_err(ApplyError::Problem)?;
    let place = Place::of(&package, None, None).map_err(op_to_apply)?;
    // Template routes refuse space-held connections before they get here, so
    // the reader is the account itself.
    let reader = crate::modules::built::Reader {
        account: who.actor,
        connection: None,
        held: who.confined,
    };
    let plan = install::plan(tx, who.workspace, reader, &package, &place).await?;
    let who = Installer {
        template: Some((template.id.clone(), template.name.clone())),
        source,
        source_ref: Some(source_ref),
        ..who
    };
    let installed = install(tx, &who, &package, &document, &place, &plan)
        .await
        .map_err(|error| match error {
            InstallError::Problem(problem) => ApplyError::Problem(problem),
            InstallError::Op(error) => op_to_apply(error),
        })?;
    Ok(crate::workspace_templates::Applied {
        template: template.id.clone(),
        template_name: template.name.clone(),
        changes: installed.changes,
        project_id: installed.space_id,
    })
}

/// An operation's refusal as the template routes report it.
fn op_to_apply(error: OpError) -> ApplyError {
    match error {
        OpError::Invalid { field, message } => ApplyError::Problem(Problem {
            field: field.strip_prefix("package.").unwrap_or(&field).to_owned(),
            message: message.into_owned(),
        }),
        OpError::Denied { .. } | OpError::Unauthenticated => ApplyError::App(AppError::Forbidden),
        OpError::NotFound { what } => ApplyError::App(AppError::NotFound(what)),
        _ => ApplyError::App(AppError::Internal("the template couldn't be used")),
    }
}
