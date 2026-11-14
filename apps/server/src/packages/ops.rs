//! `package_export`: a package from a space or the workspace, checked for
//! anything private before it is returned. It is a read until M1.9 adds
//! the `packages` table it is stored in (`source = 'export'`).

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::{
    export::{self, Include, Source},
    format::Package,
    secrets_check,
};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<PackageExport>();
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
