//! Outgoing webhooks as operations (K2.18): list your webhooks, make,
//! change or remove one, rotate its secret, send it a ping, read its
//! deliveries and send one again. Each calls the functions the web routes
//! call (`webhooks::create` and the rest), as the caller the dispatcher
//! checked; those keep the webhook's own rules (only its owner finds it,
//! people only) and write their own audit rows. A webhook acts where it
//! lives: the dispatcher checks that Automations is on in its space, or its
//! workspace when it hears the whole workspace. The old `webhook_write` and
//! `webhook_deliveries` tools are aliases.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{CreateWebhook, UpdateWebhook};
use crate::{
    AppState,
    error::AppError,
    events::Caller,
    mcp::task_commands::principal_for,
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Registry, Target,
        Targeted, refs::parse_id,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<WebhookList>()
        .add::<WebhookCreate>()
        .add::<WebhookUpdate>()
        .add::<WebhookDelete>()
        .add::<WebhookSecretRotate>()
        .add::<WebhookPingSend>()
        .add::<WebhookDeliveryList>()
        .add::<WebhookDeliveryRetry>();
}

fn caller(call: &OpCall<'_>) -> Caller {
    Caller::mcp(&principal_for(call.ctx, None))
}

fn webhook(id: &str) -> Target {
    Target::thing("webhook", parse_id(id).unwrap_or_else(Uuid::nil))
}

fn webhook_id(value: &str) -> Result<Uuid, OpError> {
    parse_id(value).ok_or(OpError::Invalid {
        field: "webhookId".into(),
        message: "Use the webhook's ID.".into(),
    })
}

fn to_value(value: impl Serialize) -> Result<Value, OpError> {
    serde_json::to_value(value).map_err(|_| OpError::Unavailable)
}

/// The webhook functions' refusals, keeping the sentence an `Invalid`
/// carries and saying what a missing keyring needs.
fn refused(error: AppError) -> OpError {
    match error {
        AppError::Invalid(message) => OpError::Invalid {
            field: "request".into(),
            message: message.into(),
        },
        AppError::SecretsNotConfigured => OpError::Conflict {
            reason: ConflictReason::State {
                code: "secrets_not_configured",
            },
            message: "Webhooks need the server's secrets keyring; ask whoever runs this \
                workspace to set it up."
                .into(),
            current: None,
        },
        other => other.into(),
    }
}

/// AWD-75: the spaces and workspaces where Automations is off for the
/// caller, among their own webhooks' spaces and the spaces they're in.
async fn automations_off(
    state: &AppState,
    actor_id: Uuid,
) -> Result<(Vec<Uuid>, Vec<Uuid>), AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let workspaces = crate::modules::state::by_workspace(&mut connection, actor_id)
        .await?
        .into_iter()
        .filter(|(_, modules)| !modules.contains(WebhookList::MODULE))
        .map(|(workspace_id, _)| workspace_id)
        .collect();
    let spaces = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project_id FROM webhook_subscriptions
        WHERE owner_actor_id = $1 AND project_id IS NOT NULL
        UNION
        SELECT project_id FROM project_memberships WHERE actor_id = $1 AND enabled
        "#,
    )
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let on = crate::modules::state::ProjectModules::load(&mut connection, actor_id, &spaces)
        .await?
        .keep(&spaces, |modules, _| modules.contains(WebhookList::MODULE));
    Ok((
        spaces
            .into_iter()
            .filter(|space| !on.contains(space))
            .collect(),
        workspaces,
    ))
}

// ---------------------------------------------------------------------------
// webhook_list

pub(crate) struct WebhookList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListInput {}

impl Targeted for ListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for WebhookList {
    const NAME: &'static str = "webhook_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const SPANS_WORKSPACES: bool = true;
    const TITLE: &'static str = "List webhooks";
    const DESCRIPTION: &'static str = "Your outgoing webhooks: each one's URL, the events it \
        hears, whether it's on, and its last delivery; plus every event type and how receivers \
        check a delivery's signature. Secrets are never shown again after they're made.";

    type Input = ListInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, _input: ListInput) -> Result<Value, OpError> {
        let caller = caller(call);
        // AWD-75: only webhooks (and spaces to limit one to) where
        // Automations is on for the caller.
        let (off_spaces, off_workspaces) = automations_off(call.state, caller.actor_id)
            .await
            .map_err(refused)?;
        to_value(
            super::list_in(call.state, &caller, &off_spaces, &off_workspaces)
                .await
                .map_err(refused)?,
        )
    }
}

// ---------------------------------------------------------------------------
// webhook_create

pub(crate) struct WebhookCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateInput {
    pub name: String,
    /// An https URL that receives each delivery.
    pub url: String,
    /// Event types from webhook_list, such as run.finished or
    /// message.created.
    pub event_types: Vec<String>,
    /// Hear only this space (ID, link or key); left out, every space in the
    /// workspace you can see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl Targeted for CreateInput {
    fn target(&self) -> Target {
        match &self.project_id {
            Some(space) => Target::Space(parse_id(space).unwrap_or_else(Uuid::nil)),
            None => Target::Workspace(None),
        }
    }
}

impl Operation for WebhookCreate {
    const NAME: &'static str = "webhook_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Create webhook";
    const DESCRIPTION: &'static str = "Make an outgoing webhook that POSTs signed JSON to a \
        URL when tasks, runs, pull requests or messages change. It hears what you can see, in \
        one space or the whole workspace, and stops if you lose access. Returns the signing \
        secret once.";
    const EMITS: &'static [&'static str] = &["webhook.created"];

    type Input = CreateInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: CreateInput) -> Result<Value, OpError> {
        let caller = caller(call);
        let (project_id, organization_id) = match input.project_id {
            Some(_) => (call.ctx.space_id, None),
            None => (None, Some(call.ctx.workspace_id)),
        };
        let made = super::create(
            call.state,
            &caller,
            CreateWebhook {
                name: input.name,
                url: input.url,
                event_types: input.event_types,
                project_id,
                organization_id,
            },
        )
        .await
        .map_err(refused)?;
        call.audited_by_command();
        to_value(made)
    }
}

// ---------------------------------------------------------------------------
// webhook_update

pub(crate) struct WebhookUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateInput {
    pub webhook_id: String,
    /// The version you last read.
    pub expected_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_types: Option<Vec<String>>,
    /// active or paused. Turning a disabled one back on is "active".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Targeted for UpdateInput {
    fn target(&self) -> Target {
        webhook(&self.webhook_id)
    }
}

impl Operation for WebhookUpdate {
    const NAME: &'static str = "webhook_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update webhook";
    const DESCRIPTION: &'static str = "Change one of your webhooks: its name, URL, the events \
        it hears, or pause it (state: paused) and turn it back on (state: active). Pass the \
        version you last read.";
    const EMITS: &'static [&'static str] = &["webhook.updated"];

    type Input = UpdateInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: UpdateInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        let changed = super::update(
            call.state,
            &caller(call),
            id,
            UpdateWebhook {
                expected_version: input.expected_version,
                name: input.name,
                url: input.url,
                event_types: input.event_types,
                state: input.state,
            },
        )
        .await
        .map_err(refused)?;
        call.audited_by_command();
        to_value(changed)
    }
}

// ---------------------------------------------------------------------------
// webhook_delete, webhook_secret_rotate, webhook_ping_send

pub(crate) struct WebhookDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WebhookInput {
    pub webhook_id: String,
}

impl Targeted for WebhookInput {
    fn target(&self) -> Target {
        webhook(&self.webhook_id)
    }
}

impl Operation for WebhookDelete {
    const NAME: &'static str = "webhook_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove webhook";
    const DESCRIPTION: &'static str = "Remove one of your webhooks. Nothing more is sent to it.";
    const EMITS: &'static [&'static str] = &["webhook.removed"];

    type Input = WebhookInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: WebhookInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        super::delete(call.state, &caller(call), id)
            .await
            .map_err(refused)?;
        call.audited_by_command();
        Ok(json!({ "deleted": true }))
    }
}

pub(crate) struct WebhookSecretRotate;

impl Operation for WebhookSecretRotate {
    const NAME: &'static str = "webhook_secret_rotate";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Rotate webhook secret";
    const DESCRIPTION: &'static str = "Give one of your webhooks a new signing secret, returned \
        once. Deliveries are signed with it from now on; the old one stops working.";
    const EMITS: &'static [&'static str] = &["webhook.updated"];

    type Input = WebhookInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: WebhookInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        let rotated = super::rotate_secret(call.state, &caller(call), id)
            .await
            .map_err(refused)?;
        call.audited_by_command();
        to_value(rotated)
    }
}

pub(crate) struct WebhookPingSend;

impl Operation for WebhookPingSend {
    const NAME: &'static str = "webhook_ping_send";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Test webhook";
    const DESCRIPTION: &'static str = "Send one of your webhooks a ping, to check it's \
        reachable. webhook_delivery_list shows how it went.";
    const EMITS: &'static [&'static str] = &["webhook.delivery_queued"];

    type Input = WebhookInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: WebhookInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        to_value(
            super::send_test(call.state, &caller(call), id)
                .await
                .map_err(refused)?,
        )
    }
}

// ---------------------------------------------------------------------------
// webhook_delivery_list, webhook_delivery_retry

pub(crate) struct WebhookDeliveryList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeliveriesInput {
    pub webhook_id: String,
    /// nextCursor from the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 1 to 100, default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl Targeted for DeliveriesInput {
    fn target(&self) -> Target {
        webhook(&self.webhook_id)
    }
}

impl Operation for WebhookDeliveryList {
    const NAME: &'static str = "webhook_delivery_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Webhook deliveries";
    const DESCRIPTION: &'static str = "A webhook's delivery log, newest first: each delivery's \
        id (the same on every retry), event, state, attempts, last status code and error.";

    type Input = DeliveriesInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: DeliveriesInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        to_value(
            super::deliveries(
                call.state,
                &caller(call),
                id,
                input.cursor.as_deref(),
                input.limit,
            )
            .await
            .map_err(refused)?,
        )
    }
}

pub(crate) struct WebhookDeliveryRetry;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RetryInput {
    pub webhook_id: String,
    /// The delivery to send again, from webhook_delivery_list.
    pub delivery_id: String,
}

impl Targeted for RetryInput {
    fn target(&self) -> Target {
        webhook(&self.webhook_id)
    }
}

impl Operation for WebhookDeliveryRetry {
    const NAME: &'static str = "webhook_delivery_retry";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "automations";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Send delivery again";
    const DESCRIPTION: &'static str = "Send one of a webhook's deliveries again, with the same \
        delivery id, so the receiver can tell it's a repeat.";
    const EMITS: &'static [&'static str] = &["webhook.delivery_queued"];

    type Input = RetryInput;
    type Output = Value;

    async fn run(call: &mut OpCall<'_>, input: RetryInput) -> Result<Value, OpError> {
        let id = webhook_id(&input.webhook_id)?;
        let delivery = parse_id(&input.delivery_id).ok_or(OpError::Invalid {
            field: "deliveryId".into(),
            message: "Use the delivery's ID.".into(),
        })?;
        to_value(
            super::redeliver(call.state, &caller(call), id, delivery)
                .await
                .map_err(refused)?,
        )
    }
}
