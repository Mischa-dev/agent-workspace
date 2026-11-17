//! `package_export`: a package from a space or the workspace, checked for
//! anything private before it is returned (M1.8). `package_preview`,
//! `package_install`, `package_list`, `package_remove` and
//! `agent_template_list` (M1.9): what installing would do, doing it, what
//! was installed, and removing it.

use std::{borrow::Cow, collections::HashMap};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    export::{self, Include, Source},
    format::{ContentKind, Package},
    install::{self, AgentTemplateView, InstallView, NewSpace, Place, Plan},
    secrets_check,
};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, access_shim,
    error::{ConflictReason, DenyReason},
    refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<PackageExport>();
    registry.add::<PackagePreview>();
    registry.add::<PackageInstall>();
    registry.add::<PackageList>();
    registry.add::<PackageRemove>();
    registry.add::<AgentTemplateList>();
}

/// Export a setup as a package.
pub struct PackageExport;

fn yes() -> bool {
    true
}

/// Name one source: `spaceId`, `module` or `wholeWorkspace`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageExportInput {
    /// A space (its id, link or key): its roles, channels, collections,
    /// forms, automations and skills.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// A built module, by key (built modules arrive later; refused today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// The workspace's setup: switches, layout and roles. (`workspace`
    /// itself names which workspace, as on every operation.)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole_workspace: bool,
    /// Also the space's pages besides skills (default false).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_pages: bool,
    /// The space's skill pages, as skills (default true).
    #[serde(default = "yes")]
    pub include_skills: bool,
}

impl Targeted for PackageExportInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageExportOutput {
    /// The package in its inline form (agent-workspace-package/1).
    pub package: Package,
    /// The package's digest, "sha256:<hex>".
    pub digest: String,
}

impl Operation for PackageExport {
    const NAME: &'static str = "package_export";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Export package";
    const DESCRIPTION: &'static str = "Export a space's setup (roles, channels, collections, \
        forms, automations, skills) or the workspace's (switches, layout, roles) as one package \
        document. It never holds a secret, token, member or form link: ids become part keys, and \
        what the installer must choose again is named in needsSetup. The same setup exported \
        twice has the same digest.";

    type Input = PackageExportInput;
    type Output = PackageExportOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: PackageExportInput,
    ) -> Result<PackageExportOutput, OpError> {
        let chosen = usize::from(input.space_id.is_some())
            + usize::from(input.module.is_some())
            + usize::from(input.whole_workspace);
        if chosen != 1 {
            return Err(OpError::invalid(
                "spaceId",
                "Name one of spaceId, module or wholeWorkspace.",
            ));
        }
        if input.module.is_some() {
            return Err(OpError::invalid(
                "module",
                "Built modules can't be exported yet. Export the space they are in.",
            ));
        }
        let source = match (input.space_id, call.ctx.space_id) {
            (Some(_), Some(space)) => Source::Space(space),
            (Some(_), None) => return Err(OpError::not_found("That space")),
            (None, _) => Source::Workspace,
        };
        let include = Include {
            pages: input.include_pages,
            skills: input.include_skills,
        };
        let mut connection = call
            .state
            .pool
            .acquire()
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        let caller =
            crate::roles::standing(&mut connection, call.ctx.workspace_id, call.account_id())
                .await?;
        let package = export::export(
            &mut connection,
            caller,
            call.ctx.workspace_id,
            source,
            include,
        )
        .await?;
        let document = serde_json::to_value(&package).map_err(|_| OpError::Unavailable)?;
        if let Err(found) = secrets_check::check(&document) {
            return Err(OpError::Invalid {
                field: Cow::Borrowed("spaceId"),
                message: Cow::Owned(format!(
                    "The package would carry something that looks private, at {}. Take it out \
                     of the setup (or leave pages out) and export again.",
                    found.join(", ")
                )),
            });
        }
        let digest = package.digest.clone().unwrap_or_default();
        Ok(PackageExportOutput { package, digest })
    }
}

// ------------------------------------------------- package_preview (M1.9)

/// Where a package goes: an existing space, a new one, or (both left out)
/// the package's own starter space or the workspace.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallTarget {
    /// An existing space (its id, link or key).
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// A new space: {name, key}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_space: Option<NewSpace>,
}

impl InstallTarget {
    fn dispatch_target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }

    /// The place, with the space dispatch located.
    fn place(&self, call: &OpCall<'_>, package: &Package) -> Result<Place, OpError> {
        let space = match (self.space_id, call.ctx.space_id) {
            (Some(_), Some(space)) => Some(space),
            (Some(_), None) => return Err(OpError::not_found("That space")),
            (None, _) => None,
        };
        if space.is_some() && self.new_space.is_some() {
            return Err(OpError::invalid(
                "target",
                "Name an existing space or a new one, not both.",
            ));
        }
        Place::of(package, space, self.new_space.clone())
    }
}

/// Preview installing a package.
pub struct PackagePreview;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagePreviewInput {
    /// The package in its inline form (agent-workspace-package/1), as
    /// package_export returns it or an agent drafts it.
    #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
    pub package: serde_json::Value,
    #[serde(default)]
    pub target: InstallTarget,
}

impl Targeted for PackagePreviewInput {
    fn target(&self) -> Target {
        self.target.dispatch_target()
    }
}

impl Operation for PackagePreview {
    const NAME: &'static str = "package_preview";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const MAX_BODY_BYTES: usize = install::MAX_DOCUMENT_BYTES;
    const TITLE: &'static str = "Preview a package";
    const DESCRIPTION: &'static str = "Say what installing a package would do, part by part, \
        without changing anything: create, reuse an object of the same name, update (roles, \
        module switches, the layout) or skip and why. It lists the modules it switches, the \
        permissions its roles would hold, the data that would leave the workspace, and which \
        parts need an admin. Pass its planHash to package_install.";

    type Input = PackagePreviewInput;
    type Output = Plan;

    async fn run(call: &mut OpCall<'_>, input: PackagePreviewInput) -> Result<Plan, OpError> {
        let package = install::read(&input.package)?;
        let place = input.target.place(call, &package)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let tx = call.tx().await?;
        Ok(install::plan(tx, workspace, actor, &package, &place).await?)
    }
}

// ------------------------------------------------- package_install (M1.9)

/// Install a package.
pub struct PackageInstall;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageInstallInput {
    /// The package, as package_preview took it.
    #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
    pub package: serde_json::Value,
    #[serde(default)]
    pub target: InstallTarget,
    /// package_preview's planHash: the install is refused if the plan
    /// changed since.
    pub plan_hash: String,
}

impl Targeted for PackageInstallInput {
    fn target(&self) -> Target {
        self.target.dispatch_target()
    }
}

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

impl Operation for PackageInstall {
    const NAME: &'static str = "package_install";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const MAX_BODY_BYTES: usize = install::MAX_DOCUMENT_BYTES;
    const TITLE: &'static str = "Install a package";
    const DESCRIPTION: &'static str = "Install a package as package_preview planned it: its \
        roles, module switches and layout, its space with channels and collections, and its \
        forms, automations, pages, skills, agent templates and built modules, all in one go \
        or nothing. It never deletes: objects of the same name are reused. Refused if the \
        plan changed since the preview (planHash). Roles need roles.manage; switches, the \
        layout and parts marked needsAdmin need settings.manage. A retry with the same \
        request id changes nothing.";
    const EMITS: &'static [&'static str] = &["package.installed"];

    type Input = PackageInstallInput;
    type Output = Installed;

    #[allow(clippy::too_many_lines)]
    async fn run(call: &mut OpCall<'_>, input: PackageInstallInput) -> Result<Installed, OpError> {
        use crate::workspace_templates::{self as templates, TemplateDefinition, TemplateProject};

        let package = install::read(&input.package)?;
        let place = input.target.place(call, &package)?;
        let ctx = call.ctx;
        let workspace = ctx.workspace_id;
        let actor = call.account_id();
        let principal = crate::mcp::messaging::principal_of(ctx);
        let maker = crate::events::Caller::from(ctx);
        let writer = crate::memory::Writer {
            actor_id: actor,
            actor_kind: ctx.caller.kind,
            via_grant_id: ctx.caller.via_grant_id,
        };
        let tx = call.tx().await?;

        // The plan the preview showed, read again in this transaction.
        let plan = install::plan(tx, workspace, actor, &package, &place).await?;
        if plan.plan_hash != input.plan_hash.trim() {
            return Err(OpError::Conflict {
                reason: ConflictReason::State {
                    code: "plan_changed",
                },
                message: "The workspace changed since the preview, so installing would do \
                          something else. Preview again and install that plan."
                    .into(),
                current: None,
            });
        }
        let live = |line: &&install::PlanItem| line.action != "skip";
        // Roles, module switches, the default layout and the other admin
        // lines change the whole workspace, so they are asked of the
        // workspace even when the package lands in a space.
        let located = crate::ops::Located {
            workspace_id: workspace,
            space_id: None,
        };
        let workspace_target = Target::Workspace(Some(workspace));
        // A connection held to one space installs only into that space, and
        // only what stays there: its workspace role came with the account.
        if ctx.caller.confined_space.is_some()
            && (ctx.space_id.is_none()
                || plan
                    .items
                    .iter()
                    .filter(live)
                    .any(|line| line.kind == "role" || line.needs_admin))
        {
            return Err(OpError::Denied {
                reason: DenyReason::Delegation,
                message: "This connection is held to one space, so it can only install what \
                          stays in that space. Reconnect it for your whole account to change \
                          the workspace."
                    .into(),
            });
        }
        if plan
            .items
            .iter()
            .filter(live)
            .any(|line| line.kind == "role")
        {
            access_shim::check(
                tx,
                &ctx.caller,
                &workspace_target,
                &located,
                Some(Perm::RolesManage),
                true,
                "Installing a package that adds or changes a role",
            )
            .await?;
        }
        if plan
            .items
            .iter()
            .filter(live)
            .any(|line| line.needs_admin && line.kind != "role")
        {
            access_shim::check(
                tx,
                &ctx.caller,
                &workspace_target,
                &located,
                Some(Perm::SettingsManage),
                true,
                "Installing a package that changes the workspace's settings",
            )
            .await?;
        }
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
                source, created_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'upload', $9)
            ON CONFLICT (workspace_id, package_key, version) DO UPDATE SET
                name = EXCLUDED.name, description = EXCLUDED.description,
                document = EXCLUDED.document, digest = EXCLUDED.digest
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace)
        .bind(&package.id)
        .bind(&package.version)
        .bind(package.name.trim())
        .bind(&package.description)
        .bind(&input.package)
        .bind(&plan.digest)
        .bind(actor)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
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
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;

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
                ContentKind::Role => {
                    let (role, default) = install::template_role(&body).map_err(|message| {
                        invalid_at(format!("package.role:{}", part.key), message)
                    })?;
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
                // A collection the preview showed as reused is left as it
                // is: applying its definition would change a live schema
                // the preview never showed.
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
        let standing = crate::roles::standing(tx, workspace, actor).await?;
        let view = templates::TemplateView {
            id: package.id.clone(),
            name: package.name.clone(),
            description: package.description.clone(),
            built_in: false,
            what_it_does: Vec::new(),
            definition,
            saved_by: None,
            saved_at: None,
        };
        let applied = templates::apply(
            tx,
            &crate::roles::Auditor::App(&principal),
            standing,
            workspace,
            &view,
        )
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
                    .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
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
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
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
                    .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
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
                        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
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
                )
            {
                record(&mut objects, kind, key, id);
                continue;
            }
            let field = |what: &str| format!("package.{kind}:{key}{what}");
            // Only forms, automations and modules point at other parts;
            // a page's or skill's text stays exactly as it was written.
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
                ContentKind::Form => {
                    let space = space.ok_or(OpError::Unavailable)?;
                    let name =
                        crate::bug_reports::normalize_name(body["name"].as_str().unwrap_or(""))
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
                        crate::collections::field_definitions(tx, space, collection_id, false)
                            .await?;
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
                    };
                    let form = crate::bug_reports::checked_form_in(tx, space, collection_id, &form)
                        .await?
                        .map_err(|message| invalid_at(field(".fields"), message))?;
                    // The link's token is never returned: the form's
                    // settings make one to share.
                    let (id, _token) = crate::bug_reports::insert_link(
                        tx,
                        &maker,
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
                    let rule = serde_json::from_value::<crate::automations::CreateRule>(rule)
                        .map_err(|error| {
                            invalid_at(
                                field(""),
                                error
                                    .to_string()
                                    .split(" at line ")
                                    .next()
                                    .unwrap_or("This rule can't be read.")
                                    .to_owned(),
                            )
                        })?;
                    let prepared =
                        crate::automations::prepare_in(tx, &maker, space, rule, &fresh).await?;
                    crate::automations::insert(tx, &maker, workspace, space, &prepared).await?
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
                        ));
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
                        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
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
                            ));
                        };
                        crate::modules::built::link_part(
                            tx,
                            &module,
                            part_kind,
                            object.id,
                            actor,
                            ctx.caller.confined_space,
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
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
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
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        call.audit_target(
            "package_install",
            install_id,
            json!({
                "package": package.id,
                "version": package.version,
                "created": made,
                "objects": objects.len(),
            }),
        );
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
}

fn apply_error(error: crate::workspace_templates::ApplyError) -> OpError {
    match error {
        crate::workspace_templates::ApplyError::Problem(problem) => OpError::Invalid {
            field: Cow::Owned(format!("package.{}", problem.field)),
            message: Cow::Owned(problem.message),
        },
        crate::workspace_templates::ApplyError::App(error) => error.into(),
    }
}

// ---------------------------------------------------- package_list (M1.9)

/// The packages installed here.
pub struct PackageList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageListInput {
    /// Only the installs in this space (its id, link or key).
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for PackageListInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageListOutput {
    pub installs: Vec<InstallView>,
}

impl Operation for PackageList {
    const NAME: &'static str = "package_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List installed packages";
    const DESCRIPTION: &'static str = "List the packages installed in the workspace (or one \
        space), newest first: each install's package id, version, name, space, state, what it \
        did in plain words and how many objects it made or reused. package_remove takes the \
        install's id.";

    type Input = PackageListInput;
    type Output = PackageListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: PackageListInput,
    ) -> Result<PackageListOutput, OpError> {
        let space = match (input.space_id, call.ctx.space_id) {
            (Some(_), Some(space)) => Some(space),
            (Some(_), None) => return Err(OpError::not_found("That space")),
            (None, _) => None,
        };
        let workspace = call.ctx.workspace_id;
        let reader = call.account_id();
        let tx = call.tx().await?;
        Ok(PackageListOutput {
            installs: install::list(tx, workspace, space, reader).await?,
        })
    }
}

// -------------------------------------------------- package_remove (M1.9)

/// Remove an install.
pub struct PackageRemove;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoveParts {
    /// Archive what the install made (collections, pages and skills,
    /// modules and agent templates archived, forms turned off, automations
    /// switched off). Nothing is deleted.
    Archive,
    /// Keep what it made, as plain objects.
    Keep,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageRemoveInput {
    /// The install (package_list's id).
    pub install_id: IdRef,
    /// archive or keep what it made.
    pub parts: RemoveParts,
}

impl Targeted for PackageRemoveInput {
    fn target(&self) -> Target {
        Target::Thing {
            kind: "package_install",
            id: self.install_id.id(),
            expected_version: None,
        }
    }
}

impl Operation for PackageRemove {
    const NAME: &'static str = "package_remove";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove a package";
    const DESCRIPTION: &'static str = "Remove an installed package. parts \"archive\" archives \
        what the install made (collections, pages, skills, modules and agent templates \
        archived, forms turned off, automations switched off); \"keep\" leaves it all as plain \
        objects. Nothing is deleted, and what it reused stays as it was. Roles, switches and \
        the layout stay.";
    const EMITS: &'static [&'static str] = &["package.removed"];

    type Input = PackageRemoveInput;
    type Output = InstallView;

    async fn run(call: &mut OpCall<'_>, input: PackageRemoveInput) -> Result<InstallView, OpError> {
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let install_id = input.install_id.id();
        let tx = call.tx().await?;
        let state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM package_installs WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(workspace)
        .bind(install_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
        .ok_or(OpError::not_found("That install"))?;
        if state == "removed" {
            return Err(OpError::Conflict {
                reason: ConflictReason::State {
                    code: "already_removed",
                },
                message: "That install was removed already.".into(),
                current: None,
            });
        }
        let mut archived = 0usize;
        if input.parts == RemoveParts::Archive {
            let made = install::created_objects(tx, install_id).await?;
            for (kind, ids) in &made {
                let sql = match kind.as_str() {
                    "collection" => {
                        "UPDATE collections SET archived_at = now(), \
                         archived_by_actor_id = $2, version = version + 1 \
                         WHERE id = ANY($1) AND archived_at IS NULL"
                    }
                    "page" | "skill" => {
                        // Open pages hear of it, as from any page change.
                        let changed = sqlx::query_as::<_, (Uuid, Uuid, i64)>(
                            "UPDATE project_pages SET archived_at = now(), version = version + 1 \
                             WHERE id = ANY($1) AND archived_at IS NULL \
                             RETURNING id, project_id, version",
                        )
                        .bind(ids)
                        .fetch_all(&mut **tx)
                        .await
                        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
                        archived += changed.len();
                        for (page, space, version) in changed {
                            crate::page_docs::store::page_updated(tx, space, page, version).await?;
                        }
                        continue;
                    }
                    "form" => {
                        "UPDATE bug_report_links SET state = 'disabled' \
                         WHERE id = ANY($1) AND state <> 'disabled'"
                    }
                    "automation" => {
                        "UPDATE automation_rules SET enabled = false, next_run_at = NULL \
                         WHERE id = ANY($1) AND enabled"
                    }
                    "agent" => {
                        "UPDATE agent_templates SET archived_at = now() \
                         WHERE id = ANY($1) AND archived_at IS NULL"
                    }
                    _ => continue,
                };
                let mut query = sqlx::query(sql).bind(ids);
                if kind == "collection" {
                    query = query.bind(actor);
                }
                archived += query
                    .execute(&mut **tx)
                    .await
                    .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
                    .rows_affected() as usize;
                if kind == "collection" {
                    for id in ids {
                        let space = sqlx::query_scalar::<_, Uuid>(
                            "SELECT project_id FROM collections WHERE id = $1",
                        )
                        .bind(id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
                        crate::outbox::enqueue_collection_updated(tx, space, *id, &[]).await?;
                    }
                }
            }
            for id in made.get("module").cloned().unwrap_or_default() {
                if let Some(module) = crate::modules::built::get(tx, workspace, id).await?
                    && module.archived_at.is_none()
                {
                    crate::modules::built::archive(tx, &module, false).await?;
                    archived += 1;
                }
            }
        }
        sqlx::query(
            "UPDATE package_installs SET state = 'removed', finished_at = now() WHERE id = $1",
        )
        .bind(install_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        call.audit_target(
            "package_install",
            install_id,
            json!({ "parts": input.parts, "archived": archived }),
        );
        let tx = call.tx().await?;
        install::get(tx, workspace, install_id)
            .await?
            .ok_or(OpError::Unavailable)
    }
}

// --------------------------------------------- agent_template_list (M1.9)

/// The agent templates packages brought.
pub struct AgentTemplateList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateListInput {
    /// Only the templates installed into this space (its id, link or key).
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for AgentTemplateListInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplateListOutput {
    pub templates: Vec<AgentTemplateView>,
}

impl Operation for AgentTemplateList {
    const NAME: &'static str = "agent_template_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List agent templates";
    const DESCRIPTION: &'static str = "List the agent templates installed packages brought: \
        each one's key, name, purpose and definition (instructions, what it may and must not \
        do, when it asks a person, a suggested role and its skills). A template is a starting \
        point for adding an agent, never an account or a connection.";

    type Input = AgentTemplateListInput;
    type Output = AgentTemplateListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentTemplateListInput,
    ) -> Result<AgentTemplateListOutput, OpError> {
        let space = match (input.space_id, call.ctx.space_id) {
            (Some(_), Some(space)) => Some(space),
            (Some(_), None) => return Err(OpError::not_found("That space")),
            (None, _) => None,
        };
        let workspace = call.ctx.workspace_id;
        let reader = call.account_id();
        let tx = call.tx().await?;
        Ok(AgentTemplateListOutput {
            templates: install::agent_templates(tx, workspace, space, reader).await?,
        })
    }
}
