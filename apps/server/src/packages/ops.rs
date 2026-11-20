//! `package_export`: a package from a space or the workspace, checked for
//! anything private before it is returned (M1.8). `package_preview`,
//! `package_install`, `package_list`, `package_remove` and
//! `agent_template_list` (M1.9): what installing would do, doing it, what
//! was installed, and removing it. `package_upgrade_preview` and
//! `package_upgrade` (M1.10): a newer version, planned three ways.

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    apply::{self, Installed, Installer},
    export::{self, Include, Source},
    format::Package,
    install::{self, AgentTemplateView, InstallView, NewSpace, Place, Plan},
    secrets_check,
    source::{self, Origin},
    upgrade,
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
    registry.add::<PackageUpgradePreview>();
    registry.add::<PackageUpgrade>();
    registry.add::<PackageList>();
    registry.add::<PackageRemove>();
}

/// The Agents module's share: the agent templates packages brought, which
/// show only where Agents is on.
pub fn register_agent_ops(registry: &mut Registry) {
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
    /// The workspace's setup: switches, layout, roles and the themes it
    /// offers. (`workspace`
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
    const LONG: bool = true;
    const TITLE: &'static str = "Export package";
    const DESCRIPTION: &'static str = "Export a space's setup (roles, channels, collections, \
        forms, automations, skills) or the workspace's (switches, layout, roles) as one package \
        document. It never holds a secret, token, member or form link: ids become part keys, and \
        what the installer must choose again is named in needsSetup. The same setup exported \
        twice has the same digest.";

    type Input = PackageExportInput;
    type Output = PackageExportOutput;

    /// K2.21: a space's pages make it big enough to run as a job.
    fn runs_long(input: &PackageExportInput) -> bool {
        input.include_pages
    }

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
    /// package_export returns it or an agent drafts it. Or give entry or
    /// fileId instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub package: Option<serde_json::Value>,
    /// A package from the directory, as directory_list names it (such as
    /// "software_team", "studio.game_studio@1.2.0" or "template:<id>").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// An uploaded file (file_upload): a zipped package, or its JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
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
        let origin = source::resolve(
            call,
            input.package.as_ref(),
            input.entry.as_deref(),
            input.file_id.as_deref(),
        )
        .await?;
        let place = input.target.place(call, &origin.package)?;
        let workspace = call.ctx.workspace_id;
        let reader = crate::modules::built::Reader::of(&call.ctx.caller);
        let tx = call.tx().await?;
        Ok(install::plan(tx, workspace, reader, &origin.package, &place).await?)
    }
}

// ------------------------------------------------- package_install (M1.9)

/// Install a package.
pub struct PackageInstall;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageInstallInput {
    /// The package in its inline form (agent-workspace-package/1), as
    /// package_export returns it or an agent drafts it. Or give entry or
    /// fileId instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub package: Option<serde_json::Value>,
    /// A package from the directory, as directory_list names it (such as
    /// "software_team", "studio.game_studio@1.2.0" or "template:<id>").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// An uploaded file (file_upload): a zipped package, or its JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
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

impl Operation for PackageInstall {
    const NAME: &'static str = "package_install";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    /// The package is read (an uploaded file may come over the network)
    /// before the request id is kept, so nothing waits holding a
    /// connection (rule 14).
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
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

    async fn run(call: &mut OpCall<'_>, input: PackageInstallInput) -> Result<Installed, OpError> {
        let origin = source::resolve(
            call,
            input.package.as_ref(),
            input.entry.as_deref(),
            input.file_id.as_deref(),
        )
        .await?;
        let place = input.target.place(call, &origin.package)?;
        let reservation = match keep_request(call, Self::NAME, &input).await? {
            Kept::Replay(installed) => return Ok(installed),
            Kept::Reserved(reservation) => reservation,
        };
        let installed = install_planned(call, origin, place, Some(&input.plan_hash)).await?;
        kept(call, reservation, &installed).await?;
        Ok(installed)
    }
}

/// Installs `origin` at `place` in the call's transaction: the plan read
/// again (refused when `plan_hash` is given and differs), the rights its
/// parts need checked, then made.
pub(super) async fn install_planned(
    call: &mut OpCall<'_>,
    origin: Origin,
    place: Place,
    plan_hash: Option<&str>,
) -> Result<Installed, OpError> {
    let ctx = call.ctx;
    let workspace = ctx.workspace_id;
    let actor = call.account_id();
    let principal = crate::mcp::messaging::principal_of(ctx);
    let maker = crate::events::Caller::from(ctx);
    let writer = crate::memory::Writer::from(ctx);
    let reader = crate::modules::built::Reader::of(&ctx.caller);
    let tx = call.tx().await?;

    // One install at a time per workspace, taken before the plan is read
    // again: two installs that both planned to create the same thing can't
    // both record it as theirs. (No key update, so writes that only point at
    // the workspace aren't held up.)
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR NO KEY UPDATE")
        .bind(workspace)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(PackageInstall::NAME, &error.into()))?;
    // The plan the preview showed, read again in this transaction.
    let package = &origin.package;
    let plan = install::plan(tx, workspace, reader, package, &place).await?;
    if plan_hash.is_some_and(|hash| plan.plan_hash != hash.trim()) {
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
    let lines = plan
        .items
        .iter()
        .filter(|line| line.action != "skip")
        .map(|line| (line.kind.as_str(), line.needs_admin))
        .collect::<Vec<_>>();
    check_rights(tx, ctx, &lines).await?;
    let standing = crate::roles::standing(tx, workspace, actor).await?;
    let auditor = crate::roles::Auditor::App(&principal);
    let who = Installer {
        workspace,
        actor,
        standing,
        auditor: &auditor,
        maker,
        writer,
        template: origin.template.clone(),
        source: origin.source,
        source_ref: origin.source_ref.clone(),
        confined: ctx.caller.confined_space,
    };
    let installed = apply::install(tx, &who, package, &origin.document, &place, &plan)
        .await
        .map_err(apply::InstallError::into_op)?;
    let made = installed
        .objects
        .iter()
        .filter(|object| object.action == "created")
        .count();
    call.audit_target(
        "package_install",
        installed.install_id,
        json!({
            "package": package.id,
            "version": package.version,
            "created": made,
            "objects": installed.objects.len(),
        }),
    );
    Ok(installed)
}

/// The rights the live lines of a plan need, as (kind, needsAdmin): a
/// connection held to one space installs only what stays there; a role
/// needs roles.manage; anything else marked for admins needs
/// settings.manage. Asked of the workspace, since these change all of it.
pub(super) async fn check_rights(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    ctx: &crate::ops::OpContext,
    lines: &[(&str, bool)],
) -> Result<(), OpError> {
    let located = crate::ops::Located {
        workspace_id: ctx.workspace_id,
        space_id: None,
        direct: None,
    };
    let workspace_target = Target::Workspace(Some(ctx.workspace_id));
    // A connection held to one space installs only into that space, and only
    // what stays there: its workspace role came with the account.
    if ctx.caller.confined_space.is_some()
        && (ctx.space_id.is_none()
            || lines
                .iter()
                .any(|(kind, needs_admin)| *kind == "role" || *needs_admin))
    {
        return Err(OpError::Denied {
            reason: DenyReason::Delegation,
            message: "This connection is held to one space, so it can only install what stays \
                      in that space. Reconnect it for your whole account to change the \
                      workspace."
                .into(),
        });
    }
    if lines.iter().any(|(kind, _)| *kind == "role") {
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
    if lines
        .iter()
        .any(|(kind, needs_admin)| *needs_admin && *kind != "role")
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
    Ok(())
}

// ------------------------------- package_upgrade_preview and _upgrade (M1.10)

/// Preview upgrading an install to a newer version.
pub struct PackageUpgradePreview;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageUpgradePreviewInput {
    /// The install (package_list's id).
    pub install_id: IdRef,
    /// The new version, given as package_preview takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub package: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

fn install_target(install: IdRef) -> Target {
    Target::Thing {
        kind: "package_install",
        id: install.id(),
        expected_version: None,
    }
}

impl Targeted for PackageUpgradePreviewInput {
    fn target(&self) -> Target {
        install_target(self.install_id)
    }
}

impl Operation for PackageUpgradePreview {
    const NAME: &'static str = "package_upgrade_preview";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const MAX_BODY_BYTES: usize = install::MAX_DOCUMENT_BYTES;
    const TITLE: &'static str = "Preview a package upgrade";
    const DESCRIPTION: &'static str = "Plan upgrading an install to a newer version, part by \
        part (see a line's action). Nothing is deleted. Pass planHash to package_upgrade.";

    type Input = PackageUpgradePreviewInput;
    type Output = upgrade::UpgradePlan;

    async fn run(
        call: &mut OpCall<'_>,
        input: PackageUpgradePreviewInput,
    ) -> Result<upgrade::UpgradePlan, OpError> {
        let origin = source::resolve(
            call,
            input.package.as_ref(),
            input.entry.as_deref(),
            input.file_id.as_deref(),
        )
        .await?;
        let workspace = call.ctx.workspace_id;
        let reader = crate::modules::built::Reader::of(&call.ctx.caller);
        let tx = call.tx().await?;
        let old = upgrade::old_install(tx, workspace, input.install_id.id(), reader).await?;
        Ok(upgrade::plan(tx, workspace, reader, old, &origin.package)
            .await?
            .plan)
    }
}

/// Upgrade an install to a newer version.
pub struct PackageUpgrade;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageUpgradeInput {
    /// The install (package_list's id).
    pub install_id: IdRef,
    /// The new version, given as package_preview takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub package: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    pub plan_hash: String,
    /// By conflict ("kind:key"): keep_mine (default) or take_theirs.
    #[serde(default)]
    pub choices: std::collections::BTreeMap<String, upgrade::Choice>,
}

impl Targeted for PackageUpgradeInput {
    fn target(&self) -> Target {
        install_target(self.install_id)
    }
}

impl Operation for PackageUpgrade {
    const NAME: &'static str = "package_upgrade";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    /// As package_install: the package is read before the request id is kept.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const MAX_BODY_BYTES: usize = install::MAX_DOCUMENT_BYTES;
    const TITLE: &'static str = "Upgrade a package";
    const DESCRIPTION: &'static str = "Upgrade an install as package_upgrade_preview planned it \
        (planHash), in one go. Conflicts keep yours unless choices says take_theirs. Nothing \
        is deleted.";
    const EMITS: &'static [&'static str] = &["package.installed"];

    type Input = PackageUpgradeInput;
    type Output = upgrade::Upgraded;

    async fn run(
        call: &mut OpCall<'_>,
        input: PackageUpgradeInput,
    ) -> Result<upgrade::Upgraded, OpError> {
        let origin = source::resolve(
            call,
            input.package.as_ref(),
            input.entry.as_deref(),
            input.file_id.as_deref(),
        )
        .await?;
        let reservation = match keep_request(call, Self::NAME, &input).await? {
            Kept::Replay(upgraded) => return Ok(upgraded),
            Kept::Reserved(reservation) => reservation,
        };
        let ctx = call.ctx;
        let workspace = ctx.workspace_id;
        let actor = call.account_id();
        let principal = crate::mcp::messaging::principal_of(ctx);
        let reader = crate::modules::built::Reader::of(&ctx.caller);
        let tx = call.tx().await?;
        // One install at a time per workspace, as package_install takes it.
        sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR NO KEY UPDATE")
            .bind(workspace)
            .execute(&mut **tx)
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        sqlx::query("SELECT id FROM package_installs WHERE id = $1 FOR UPDATE")
            .bind(input.install_id.id())
            .execute(&mut **tx)
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        let old = upgrade::old_install(tx, workspace, input.install_id.id(), reader).await?;
        let planned = upgrade::plan(tx, workspace, reader, old, &origin.package).await?;
        if planned.plan.plan_hash != input.plan_hash.trim() {
            return Err(OpError::Conflict {
                reason: ConflictReason::State {
                    code: "plan_changed",
                },
                message: "The workspace changed since the preview, so upgrading would do \
                          something else. Preview again and upgrade with that plan."
                    .into(),
                current: None,
            });
        }
        let taken = |item: &&upgrade::UpgradeItem| match item.action.as_str() {
            "create" | "reuse" | "update" => true,
            "conflict" => {
                input.choices.get(&format!("{}:{}", item.kind, item.key))
                    == Some(&upgrade::Choice::TakeTheirs)
            }
            _ => false,
        };
        let lines = planned
            .plan
            .items
            .iter()
            .filter(taken)
            .map(|item| (item.kind.as_str(), item.needs_admin))
            .collect::<Vec<_>>();
        check_rights(tx, ctx, &lines).await?;
        let standing = crate::roles::standing(tx, workspace, actor).await?;
        let auditor = crate::roles::Auditor::App(&principal);
        let who = Installer {
            workspace,
            actor,
            standing,
            auditor: &auditor,
            maker: crate::events::Caller::from(ctx),
            writer: crate::memory::Writer::from(ctx),
            template: origin.template.clone(),
            source: origin.source,
            source_ref: origin.source_ref.clone(),
            confined: ctx.caller.confined_space,
        };
        let upgraded = upgrade::upgrade(
            tx,
            &who,
            planned,
            &origin.package,
            &origin.document,
            &input.choices,
        )
        .await
        .map_err(apply::InstallError::into_op)?;
        call.audit_target(
            "package_install",
            upgraded.install_id,
            json!({
                "package": origin.package.id,
                "version": upgraded.version,
                "upgradedFrom": upgraded.upgraded_from,
                "updated": upgraded.updated.len(),
                "detached": upgraded.detached.len(),
            }),
        );
        kept(call, reservation, &upgraded).await?;
        Ok(upgraded)
    }
}

/// A request id kept by a package write once its package is in hand.
enum Kept<T> {
    /// The same request ran before: its answer.
    Replay(T),
    /// Kept for this run (none when the call gave no request id).
    Reserved(Option<crate::idempotency::IdempotencyReservation>),
}

/// Keeps the call's request id for `name`, in the call's transaction. Called
/// after the package is read, so no transaction is open across that read.
async fn keep_request<I: Serialize, T: serde::de::DeserializeOwned>(
    call: &mut OpCall<'_>,
    name: &str,
    input: &I,
) -> Result<Kept<T>, OpError> {
    let Some(key) = call
        .ctx
        .request_id
        .as_deref()
        .and_then(|key| crate::idempotency::IdempotencyKey::parse(key).ok())
    else {
        return Ok(Kept::Reserved(None));
    };
    call.key_honoured();
    let actor = call.account_id();
    let scope = format!("op.{name}.{}", call.ctx.workspace_id.simple());
    let hash =
        crate::idempotency::RequestHash::for_value(input).map_err(|_| OpError::Unavailable)?;
    let tx = call.tx().await?;
    match crate::idempotency::reserve(tx, actor, &scope, &key, hash).await? {
        crate::idempotency::IdempotencyOutcome::Replay(replay) => {
            call.mark_replayed(true);
            Ok(Kept::Replay(replay.decode::<T>()?))
        }
        crate::idempotency::IdempotencyOutcome::Reserved(reserved) => {
            Ok(Kept::Reserved(Some(reserved)))
        }
    }
}

/// Stores `output` as the kept request's answer.
async fn kept<T: Serialize>(
    call: &mut OpCall<'_>,
    reservation: Option<crate::idempotency::IdempotencyReservation>,
    output: &T,
) -> Result<(), OpError> {
    if let Some(reservation) = reservation {
        let tx = call.tx().await?;
        crate::idempotency::complete(tx, &reservation, axum::http::StatusCode::OK, output).await?;
    }
    Ok(())
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
        let reader = crate::modules::built::Reader::of(&call.ctx.caller);
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
        // An upgraded install's parts belong to the install that replaced it
        // (or were left as they are): only the current one is removed.
        if state == "superseded" {
            return Err(OpError::Conflict {
                reason: ConflictReason::State { code: "superseded" },
                message: "That install was upgraded: remove the install that replaced it \
                          (package_list shows it)."
                    .into(),
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
                            crate::collections::pages::page_record_changed(tx, space, page).await?;
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
            // What the install made, of any kind: only those stay in the
            // modules it archives; anything it reused is detached first, so
            // it stays as it was.
            let made_ids = made.values().flatten().copied().collect::<Vec<Uuid>>();
            for id in made.get("module").cloned().unwrap_or_default() {
                if let Some(module) = crate::modules::built::get(tx, workspace, id).await?
                    && module.archived_at.is_none()
                {
                    crate::modules::built::detach_all_but(tx, &module, &made_ids).await?;
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
    const MODULE: &'static str = "agents";
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
        let reader = crate::modules::built::Reader::of(&call.ctx.caller);
        let tx = call.tx().await?;
        Ok(AgentTemplateListOutput {
            templates: install::agent_templates(tx, workspace, space, reader).await?,
        })
    }
}
