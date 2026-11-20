//! Connected modules as operations (M1.12): `connected_module_install`
//! (with `preview: true` to see it first), `connected_module_list`,
//! `connected_module_refresh`,
//! `connected_module_approve` and `connected_module_revoke`. All need
//! `modules.manage`; approving also needs holding what is approved. A
//! manifest is fetched before any transaction opens (rule 14).

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{AppRow, ConnectedModuleView, Refreshed};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ConnectedModuleInstall>()
        .add::<ConnectedModuleList>()
        .add::<ConnectedModuleRefresh>()
        .add::<ConnectedModuleApprove>()
        .add::<ConnectedModuleRevoke>()
        .add::<ConnectedModuleRun>()
        .add::<ThingUpsert>();
}

const THE_MODULE: &str = "That connected module";

/// The install the call acts on, in the located workspace.
async fn row(call: &mut OpCall<'_>, module_id: Uuid, lock: bool) -> Result<AppRow, OpError> {
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    super::get(tx, workspace, module_id, lock)
        .await?
        .ok_or(OpError::not_found(THE_MODULE))
}

async fn view(call: &mut OpCall<'_>, module_id: Uuid) -> Result<ConnectedModuleView, OpError> {
    Ok(row(call, module_id, false).await?.view()?)
}

// ------------------------------------------------------------ install

pub struct ConnectedModuleInstall;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectedModuleInstallInput {
    /// The address of the app's manifest.
    pub url: String,
    /// The scopes to approve. Leave out to approve every one it asks for
    /// that this server has; required ones must be included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// The digest a preview showed: the install is refused if the manifest
    /// has changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl Targeted for ConnectedModuleInstallInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ConnectedModuleInstall {
    const NAME: &'static str = "connected_module_install";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    /// The manifest is read before the request id is kept: nothing waits
    /// on the network holding a connection (rule 14).
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Install a connected module";
    const DESCRIPTION: &'static str = "Install an outside app as a module from its manifest's \
        address, approving the scopes given (or all it asks for that this server has). The app \
        gets its own account here with a role holding exactly what was approved, and is sent a \
        one-time code at its install address to collect its credentials. You can only approve \
        what you hold yourself. Pass preview: true first to see who publishes it, each scope it \
        asks for and why, and the kinds, operations, events, panels and frame origins it adds, \
        with nothing installed; then install with the digest it showed.";
    const EMITS: &'static [&'static str] = &["module.connected"];
    const PREVIEW: bool = true;

    type Input = ConnectedModuleInstallInput;
    type Output = ConnectedModuleView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectedModuleInstallInput,
    ) -> Result<ConnectedModuleView, OpError> {
        let fetched = super::fetch(call.state, &input.url).await?;
        if input
            .digest
            .as_deref()
            .is_some_and(|digest| digest != fetched.digest)
        {
            return Err(OpError::Conflict {
                reason: crate::ops::error::ConflictReason::State {
                    code: "manifest_changed",
                },
                message: "The manifest changed since the preview. Preview it again.".into(),
                current: None,
            });
        }
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let key = call
            .ctx
            .request_id
            .as_deref()
            .and_then(|key| crate::idempotency::IdempotencyKey::parse(key).ok());
        let mut reservation = None;
        if let Some(key) = key {
            call.key_honoured();
            let scope = format!("op.connected_module_install.{}", workspace.simple());
            let hash = crate::idempotency::RequestHash::for_value(&input)
                .map_err(|_| OpError::Unavailable)?;
            let tx = call.tx().await?;
            match crate::idempotency::reserve(tx, actor, &scope, &key, hash).await? {
                crate::idempotency::IdempotencyOutcome::Replay(replay) => {
                    call.mark_replayed(true);
                    return Ok(replay.decode::<ConnectedModuleView>()?);
                }
                crate::idempotency::IdempotencyOutcome::Reserved(reserved) => {
                    reservation = Some(reserved);
                }
            }
        }
        let state = call.state;
        let tx = call.tx().await?;
        let module_id = super::install(
            state,
            tx,
            workspace,
            actor,
            &fetched,
            input.scopes.as_deref(),
        )
        .await?;
        let view = super::get(tx, workspace, module_id, false)
            .await?
            .ok_or(OpError::not_found(THE_MODULE))?
            .view()?;
        if let Some(reservation) = reservation {
            crate::idempotency::complete(tx, &reservation, axum::http::StatusCode::OK, &view)
                .await?;
        }
        call.audit_target(
            "module",
            module_id,
            json!({ "key": view.manifest.key, "scopes": view.manifest.scopes.iter()
                .filter(|scope| scope.approved).map(|scope| &scope.scope).collect::<Vec<_>>() }),
        );
        Ok(view)
    }
}

// ------------------------------------------------------------ list and get

pub struct ConnectedModuleList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectedModuleListInput {
    /// Just this one (its module id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<IdRef>,
    /// Include revoked ones.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_revoked: bool,
}

impl Targeted for ConnectedModuleListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedModuleListView {
    pub modules: Vec<ConnectedModuleView>,
}

impl Operation for ConnectedModuleList {
    const NAME: &'static str = "connected_module_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List connected modules";
    const DESCRIPTION: &'static str = "List the outside apps installed here as modules (or open \
        one by moduleId): their state, account, manifest as approved, the scopes they hold, and \
        any re-fetched manifest waiting for approval.";

    type Input = ConnectedModuleListInput;
    type Output = ConnectedModuleListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectedModuleListInput,
    ) -> Result<ConnectedModuleListView, OpError> {
        if let Some(module) = input.module_id {
            return Ok(ConnectedModuleListView {
                modules: vec![view(call, module.id()).await?],
            });
        }
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let modules = super::list(tx, workspace, input.include_revoked)
            .await?
            .iter()
            .map(AppRow::view)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ConnectedModuleListView { modules })
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectedModuleRef {
    /// The connected module (its module id).
    pub module_id: IdRef,
}

impl Targeted for ConnectedModuleRef {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

// ------------------------------------------------------------ refresh

pub struct ConnectedModuleRefresh;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefreshView {
    /// unchanged, applied, or pending (it asks for more; approve it with
    /// connected_module_approve).
    pub refreshed: Refreshed,
    pub module: ConnectedModuleView,
}

impl Operation for ConnectedModuleRefresh {
    const NAME: &'static str = "connected_module_refresh";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    /// Fetching again is safe to repeat, and is done before any
    /// transaction opens.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Re-read a connected module's manifest";
    const DESCRIPTION: &'static str = "Fetch a connected module's manifest again. A manifest \
        that asks for nothing new applies at once; one with new scopes or frame origins waits for \
        connected_module_approve, and scopes it no longer asks for are dropped at once.";
    const EMITS: &'static [&'static str] = &["module.connected"];

    type Input = ConnectedModuleRef;
    type Output = RefreshView;

    async fn run(call: &mut OpCall<'_>, input: ConnectedModuleRef) -> Result<RefreshView, OpError> {
        let workspace = call.ctx.workspace_id;
        let module_id = input.module_id.id();
        let address = crate::db::scoped_one(&call.state.pool, async |connection| {
            super::get(connection, workspace, module_id, false).await
        })
        .await?
        .ok_or(OpError::not_found(THE_MODULE))?
        .manifest_url;
        let fetched = super::fetch(call.state, &address).await?;
        call.key_honoured();
        let actor = call.account_id();
        let current = row(call, module_id, true).await?;
        let tx = call.tx().await?;
        let outcome = super::refresh(tx, &current, &fetched, actor).await?;
        call.audit_target(
            "module",
            module_id,
            json!({ "refreshed": outcome, "digest": fetched.digest }),
        );
        Ok(RefreshView {
            refreshed: outcome,
            module: view(call, module_id).await?,
        })
    }
}

// ------------------------------------------------------------ approve

pub struct ConnectedModuleApprove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectedModuleApproveInput {
    /// The connected module (its module id).
    pub module_id: IdRef,
    /// Every scope the app should hold from now on. Leave out to approve
    /// all it asks for that this server has; required ones must be
    /// included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// The digest of the manifest you looked at (the pending one, if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl Targeted for ConnectedModuleApproveInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ConnectedModuleApprove {
    const NAME: &'static str = "connected_module_approve";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Approve a connected module's scopes";
    const DESCRIPTION: &'static str = "Set what a connected module's app may do: approve a \
        re-fetched manifest that asks for more, or approve more or fewer of its optional scopes. \
        Its account's role then holds exactly the scopes given. You can only approve what you \
        hold yourself.";
    const EMITS: &'static [&'static str] = &["module.connected"];

    type Input = ConnectedModuleApproveInput;
    type Output = ConnectedModuleView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectedModuleApproveInput,
    ) -> Result<ConnectedModuleView, OpError> {
        let module_id = input.module_id.id();
        let actor = call.account_id();
        let current = row(call, module_id, true).await?;
        let tx = call.tx().await?;
        super::approve(
            tx,
            &current,
            input.scopes.as_deref(),
            input.digest.as_deref(),
            actor,
        )
        .await?;
        let view = view(call, module_id).await?;
        call.audit_target(
            "module",
            module_id,
            json!({ "scopes": view.manifest.scopes.iter().filter(|scope| scope.approved)
                .map(|scope| &scope.scope).collect::<Vec<_>>() }),
        );
        Ok(view)
    }
}

// ------------------------------------------------------------ revoke

pub struct ConnectedModuleRevoke;

impl Operation for ConnectedModuleRevoke {
    const NAME: &'static str = "connected_module_revoke";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Revoke a connected module";
    const DESCRIPTION: &'static str = "Disconnect an outside app: its account is disabled and \
        its credentials revoked at once, its signing secrets deleted, and its module switched off \
        and archived. What it made stays. Install it again to reconnect.";
    const EMITS: &'static [&'static str] = &["module.connected"];

    type Input = ConnectedModuleRef;
    type Output = ConnectedModuleView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectedModuleRef,
    ) -> Result<ConnectedModuleView, OpError> {
        let module_id = input.module_id.id();
        let actor = call.account_id();
        let current = row(call, module_id, true).await?;
        let tx = call.tx().await?;
        super::revoke(tx, &current, actor).await?;
        call.audit_target("module", module_id, json!({ "revoked": true }));
        view(call, module_id).await
    }
}

// ------------------------------------------------------------ call

/// Runs an operation a connected module declares (M1.13). The edges give
/// each declared operation its own tool and call this with its arguments.
pub struct ConnectedModuleRun;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectedModuleRunInput {
    /// The declared operation's name.
    pub operation: String,
    /// Its arguments, as its input schema says.
    #[serde(default = "empty_object")]
    #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
    pub input: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    json!({})
}

impl Targeted for ConnectedModuleRunInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ConnectedModuleRun {
    const NAME: &'static str = super::remote::CALL_OP;
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    // The declared operation's own permission is checked when it runs.
    const PERMISSION: Option<Perm> = None;
    // The module receives the request id and keeps it; this server holds
    // nothing open while it answers, so a retry calls it again.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const AUTOMATION: bool = false;
    const MAX_BODY_BYTES: usize = super::remote::MAX_INPUT_BYTES + 1024;
    const EMITS: &'static [&'static str] = &["module.operation_ran"];
    const TITLE: &'static str = "Run a connected module's operation";
    const DESCRIPTION: &'static str = "Run an operation an outside app connected to this \
        workspace declares, by its name and input. The app runs it and its answer is the value. \
        Each declared operation is also a tool of its own name.";

    type Input = ConnectedModuleRunInput;
    type Output = super::remote::RemoteAnswer;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectedModuleRunInput,
    ) -> Result<super::remote::RemoteAnswer, OpError> {
        super::remote::call(call, input.operation.trim(), input.input).await
    }
}

// ------------------------------------------------------------ things

/// A connected module writes one of its things (M1.13, ruling R54): only
/// the install that owns the kind, as its own account.
pub struct ThingUpsert;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingUpsertInput {
    /// The thing's kind, one your manifest declares (`acme.build`).
    pub kind: String,
    /// The thing to change. Leave out to make a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<IdRef>,
    /// What it's called, on one line.
    pub title: String,
    /// A key such as `running` or `passed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The space it belongs in; left out, it is the workspace's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// Its address in your app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Its field values, by your manifest's field ids (16 KB at most).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
    /// Put away (true) or bring back (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

impl Targeted for ThingUpsertInput {
    /// A change is checked at the thing where it is, so an app taken out of
    /// a space can't reach what it left there.
    fn target(&self) -> Target {
        match (&self.id, &self.space_id) {
            (Some(id), _) => Target::thing("thing", id.id()),
            (None, Some(space)) => Target::Space(space.id()),
            (None, None) => Target::Workspace(None),
        }
    }
}

impl Operation for ThingUpsert {
    const NAME: &'static str = "thing_upsert";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    // Owning the kind is what lets it write; checked when it runs.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Add or change a connected thing";
    const DESCRIPTION: &'static str = "For a connected module, as its own account: add one of \
        the things its manifest declares a kind of (a build, say), or change one it added, so \
        people can find, open and link it here. Only the module that owns the kind may.";
    const EMITS: &'static [&'static str] = &["connected_thing.changed"];

    type Input = ThingUpsertInput;
    type Output = super::things::ConnectedThingView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThingUpsertInput,
    ) -> Result<super::things::ConnectedThingView, OpError> {
        // A thing stays in the space it was made in.
        if input.id.is_some()
            && let Some(space) = input.space_id
            && Some(space.id()) != call.ctx.space_id
        {
            return Err(OpError::invalid(
                "spaceId",
                "A thing stays in the space it was made in. Leave spaceId out to change it.",
            ));
        }
        let upsert = super::things::Upsert {
            id: input.id.map(|id| id.id()),
            kind: input.kind.trim(),
            title: &input.title,
            status: input.status.as_deref(),
            space_id: call.ctx.space_id,
            url: input.url.as_deref(),
            fields: input.fields.as_ref(),
            archived: input.archived,
        };
        super::things::check(&upsert)?;
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let tx = call.tx().await?;
        let module_id = super::things::owner_for(tx, workspace, upsert.kind, account)
            .await?
            .ok_or(OpError::Denied {
                reason: crate::ops::DenyReason::Rule,
                message: "Only the connected module that owns this kind writes its things.".into(),
            })?;
        let view = super::things::upsert(tx, workspace, module_id, account, upsert).await?;
        call.audit_target("thing", view.id, json!({ "kind": view.kind }));
        Ok(view)
    }
}
