//! Incoming hooks as operations (K3.26): make one (its secret shown once),
//! list a space's hooks with their latest deliveries, delete one.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::HookView;
use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<IncomingHookCreate>()
        .add::<IncomingHookList>()
        .add::<IncomingHookDelete>();
}

/// Make an incoming hook.
pub struct IncomingHookCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncomingHookCreateInput {
    /// The space (a project) whose rules it starts: its id, link or key.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// What sends to it, in a few words ("Billing alerts").
    pub name: String,
}

impl Targeted for IncomingHookCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncomingHookCreated {
    pub hook: HookView,
    /// The signing secret, shown only in the answer that made the hook;
    /// absent when a retry found a hook already made. A lost secret can't be
    /// shown again: delete the hook and make another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl Operation for IncomingHookCreate {
    const NAME: &'static str = "incoming_hook_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::AutomationsManage);
    // The answer carries the secret, which must not be stored for a replay:
    // the hook keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["incoming_hook.created"];
    const TITLE: &'static str = "Make an incoming hook";
    const DESCRIPTION: &'static str = "Make an address in a space that another service can post \
        to, to start automation rules there (trigger kind incoming_hook with its hookId). \
        Returns the hook's url and its signing secret, shown only this once. The sender signs \
        each delivery: X-Hook-Delivery (its own id), X-Hook-Timestamp (Unix seconds) and \
        X-Hook-Signature (sha256= and the hex HMAC-SHA256 of \"<timestamp>.<delivery id>.<body>\" \
        keyed by the secret).";

    type Input = IncomingHookCreateInput;
    type Output = IncomingHookCreated;

    async fn run(
        call: &mut OpCall<'_>,
        input: IncomingHookCreateInput,
    ) -> Result<IncomingHookCreated, OpError> {
        let name = super::clean_name(&input.name).ok_or_else(|| {
            OpError::invalid("name", "Give it a one-line name, up to 120 characters.")
        })?;
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let request_id = call.ctx.request_id.clone();
        let state = call.state;
        let tx = call.tx().await?;
        if let Some(request_id) = request_id.as_deref()
            && let Some(made) = super::made_by_request(tx, space, account, request_id).await?
        {
            let hook = one(tx, state, space, made).await?;
            call.mark_replayed(true);
            call.audit_target("incoming_hook", made, json!({ "replayed": true }));
            return Ok(IncomingHookCreated { hook, secret: None });
        }
        let (id, secret) = super::create(
            tx,
            state,
            workspace,
            space,
            account,
            &name,
            request_id.as_deref(),
        )
        .await?;
        let hook = one(tx, state, space, id).await?;
        call.audit_target("incoming_hook", id, json!({}));
        Ok(IncomingHookCreated {
            hook,
            secret: Some(secret),
        })
    }
}

async fn one(
    connection: &mut sqlx::PgConnection,
    state: &crate::AppState,
    space: uuid::Uuid,
    id: uuid::Uuid,
) -> Result<HookView, OpError> {
    super::list(connection, state, space, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(OpError::Unavailable)
}

/// List a space's incoming hooks.
pub struct IncomingHookList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncomingHookListInput {
    /// The space (a project): its id, link or key.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
}

impl Targeted for IncomingHookListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncomingHookListOutput {
    /// Newest first, each with its latest deliveries. Never a secret.
    pub hooks: Vec<HookView>,
}

impl Operation for IncomingHookList {
    const NAME: &'static str = "incoming_hook_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List incoming hooks";
    const DESCRIPTION: &'static str = "List a space's incoming hooks: each one's url, when it \
        last received a delivery, and its latest deliveries, accepted or refused with the \
        reason. Use a hook's id in an automation rule's incoming_hook trigger.";

    type Input = IncomingHookListInput;
    type Output = IncomingHookListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: IncomingHookListInput,
    ) -> Result<IncomingHookListOutput, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let mut connection = crate::db::scoped_tx(&call.state.pool)
            .await
            .map_err(AppError::from)?;
        let hooks = super::list(&mut connection, call.state, space, None).await?;
        Ok(IncomingHookListOutput { hooks })
    }
}

/// Delete an incoming hook.
pub struct IncomingHookDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncomingHookDeleteInput {
    /// The space (a project) the hook is in: its id, link or key.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// The hook's id.
    pub hook_id: IdRef,
}

impl Targeted for IncomingHookDeleteInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncomingHookDeleted {
    pub deleted: bool,
}

impl Operation for IncomingHookDelete {
    const NAME: &'static str = "incoming_hook_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<Perm> = Some(Perm::AutomationsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["incoming_hook.deleted"];
    const TITLE: &'static str = "Delete an incoming hook";
    const DESCRIPTION: &'static str = "Delete an incoming hook, its secret and its deliveries: \
        its url stops taking deliveries at once, and rules triggered by it stop firing.";

    type Input = IncomingHookDeleteInput;
    type Output = IncomingHookDeleted;

    async fn run(
        call: &mut OpCall<'_>,
        input: IncomingHookDeleteInput,
    ) -> Result<IncomingHookDeleted, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let hook = input.hook_id.id();
        let tx = call.tx().await?;
        if !super::delete(tx, space, hook).await? {
            return Err(OpError::not_found("That incoming hook"));
        }
        call.audit_target("incoming_hook", hook, json!({}));
        Ok(IncomingHookDeleted { deleted: true })
    }
}
