//! Delivery operations (A2.2, A2.3): reading events, and managing the
//! connections an account is told through.

use std::time::Duration;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    catalogue,
    connections::{self, Connection, Subscription},
};
use crate::{
    access,
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<EventList>()
        .add::<EventKindList>()
        .add::<ConnectionList>()
        .add::<ConnectionCreate>()
        .add::<ConnectionUpdate>()
        .add::<ConnectionSecretRotate>()
        .add::<ConnectionRemove>();
}

// --- event_list --------------------------------------------------------------

/// Read what happened, for you.
pub struct EventList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventListInput {
    /// Where the last read stopped (its nextCursor). Left out: where this
    /// connection last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// How many events at most (default and limit 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    /// Seconds to wait for the first event when none is waiting (at most 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_seconds: Option<u8>,
    /// Read through this connection of yours instead of your default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<IdRef>,
}

impl Targeted for EventListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for EventList {
    const NAME: &'static str = "event_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List events";
    const DESCRIPTION: &'static str = "What happened that concerns you, oldest first, across \
        every workspace you are in: reply requests, mentions, tasks assigned to you, requests you \
        may answer or raised, follow-ups due, and what you follow. Each event says why you hear \
        it (reason) and carries ids only: open what it names to read it. Pass nextCursor back as \
        cursor to read on; waitSeconds waits for the next event. event_kind_list lists the kinds.";

    type Input = EventListInput;
    type Output = super::Page;

    async fn run(call: &mut OpCall<'_>, input: EventListInput) -> Result<super::Page, OpError> {
        let page = super::poll(
            call.state,
            &call.ctx.caller,
            input.connection_id.map(IdRef::id),
            input.cursor.as_deref(),
            i64::from(input.limit.unwrap_or(100)),
            Duration::from_secs(u64::from(input.wait_seconds.unwrap_or(0))),
        )
        .await
        .map_err(|error| match error {
            AppError::Validation("cursor") => OpError::invalid(
                "cursor",
                "That cursor isn't this connection's. Pass the nextCursor of your last read, or none.",
            ),
            other => other.into(),
        })?;
        Ok(page)
    }
}

// --- event_kind_list ---------------------------------------------------------

/// The kinds of events you can hear.
pub struct EventKindList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventKindListInput {}

impl Targeted for EventKindListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventKindListView {
    pub events: Vec<catalogue::Entry>,
}

impl Operation for EventKindList {
    const NAME: &'static str = "event_kind_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Event kinds";
    const DESCRIPTION: &'static str = "Every kind of event event_list and connections can \
        deliver: its family, whether it is addressed to one account (always heard), and what it \
        means. Subscriptions name kinds, families (task.*) or *.";

    type Input = EventKindListInput;
    type Output = EventKindListView;

    async fn run(
        _call: &mut OpCall<'_>,
        _input: EventKindListInput,
    ) -> Result<EventKindListView, OpError> {
        Ok(EventKindListView {
            events: catalogue::ENTRIES.to_vec(),
        })
    }
}

// --- connections -------------------------------------------------------------

/// A connection, as its account and its managers see it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub name: String,
    /// Reaches every workspace the account is in; else only its home.
    pub reaches_all: bool,
    #[schemars(with = "String")]
    pub home_workspace_id: Uuid,
    /// The default the account reads through when it names none.
    pub is_default: bool,
    /// Made for an older project credential.
    pub for_credential: bool,
    pub webhook_url: Option<String>,
    /// standard, typed_v1 or inbox_v1.
    pub webhook_format: String,
    /// active, push_disabled (pushes stopped after failing; saving the
    /// webhook again turns them back on) or removed.
    pub state: String,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub last_delivered_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub last_error: Option<String>,
    pub subscriptions: Vec<Subscription>,
    pub version: i64,
}

async fn view(
    connection: &mut sqlx::PgConnection,
    delivery: Connection,
) -> Result<ConnectionView, AppError> {
    let subscriptions = sqlx::query_as::<_, Subscription>(
        "SELECT id, kinds, workspace_ids, space_ids, conversation_ids, includes_dms, echoes, \
         min_reason FROM event_subscriptions WHERE connection_id = $1 ORDER BY created_at, id",
    )
    .bind(delivery.id)
    .fetch_all(&mut *connection)
    .await?;
    let time = |value: Option<chrono::DateTime<chrono::Utc>>| value.map(|value| value.to_rfc3339());
    Ok(ConnectionView {
        id: delivery.id,
        account_id: delivery.account_id,
        name: delivery.name,
        reaches_all: delivery.reaches_all,
        home_workspace_id: delivery.home_workspace_id,
        is_default: delivery.is_default,
        for_credential: delivery.source_connection_id.is_some(),
        webhook_url: delivery.webhook_url,
        webhook_format: delivery.webhook_format,
        state: delivery.state,
        disabled_reason: delivery.disabled_reason,
        consecutive_failures: delivery.consecutive_failures,
        last_delivered_at: time(delivery.last_delivered_at),
        last_seen_at: time(delivery.last_seen_at),
        last_error: delivery.last_error,
        subscriptions,
        version: delivery.version,
    })
}

/// The account a call acts on: the caller's own, or one it manages.
async fn account_for(call: &mut OpCall<'_>, account: Option<Uuid>) -> Result<Uuid, OpError> {
    let caller = call.ctx.caller.clone();
    let Some(account) = account.filter(|id| *id != caller.account_id) else {
        return Ok(caller.account_id);
    };
    let tx = call.tx().await?;
    match access::may_manage_account(tx, &caller, account).await? {
        Ok(_) => Ok(account),
        Err(access::Denied::NotFound) => Err(OpError::not_found("That account")),
        Err(_) => Err(OpError::invalid(
            "account",
            "You can't manage that account. Ask its owner or one of its managers.",
        )),
    }
}

/// A connection the call may manage.
async fn managed_connection(call: &mut OpCall<'_>, id: Uuid) -> Result<Connection, OpError> {
    let tx = call.tx().await?;
    let found = sqlx::query_as::<_, Connection>(&format!(
        "SELECT {} FROM agent_connections WHERE id = $1 AND state <> 'removed' FOR UPDATE",
        connections::COLUMNS
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That connection"))?;
    let account = account_for(call, Some(found.account_id)).await;
    match account {
        Ok(_) => Ok(found),
        // Someone who can't manage it doesn't learn it exists.
        Err(_) => Err(OpError::not_found("That connection")),
    }
}

/// List the connections of an account.
pub struct ConnectionList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionListInput {
    /// Whose: an agent or app you manage. Left out: yours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for ConnectionListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionListView {
    pub connections: Vec<ConnectionView>,
}

impl Operation for ConnectionList {
    const NAME: &'static str = "connection_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List connections";
    const DESCRIPTION: &'static str = "The connections an account is told through: each one's \
        webhook, format, whether pushes are working, and what it subscribes to. Yours, or an \
        agent's you manage (account).";

    type Input = ConnectionListInput;
    type Output = ConnectionListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectionListInput,
    ) -> Result<ConnectionListView, OpError> {
        let account = account_for(call, input.account.map(IdRef::id)).await?;
        let tx = call.tx().await?;
        let found = sqlx::query_as::<_, Connection>(&format!(
            "SELECT {} FROM agent_connections WHERE account_id = $1 AND state <> 'removed' \
             ORDER BY created_at, id",
            connections::COLUMNS
        ))
        .bind(account)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let mut views = Vec::with_capacity(found.len());
        for delivery in found {
            views.push(view(tx, delivery).await?);
        }
        Ok(ConnectionListView { connections: views })
    }
}

/// What a subscription asks for.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionInput {
    /// Kinds (task.assigned), families (task.*) or * (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// Only these workspaces (ids). Left out: every one the connection reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ids: Option<Vec<IdRef>>,
    /// Only events in these spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_ids: Option<Vec<IdRef>>,
    /// Only events in these conversations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ids: Option<Vec<IdRef>>,
    /// Direct messages and groups too (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub includes_dms: Option<bool>,
    /// What you did yourself too (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echoes: Option<bool>,
    /// The least a reason must be: addressed (only what names you),
    /// following (default: also what you follow) or visible (everything you
    /// can see).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_reason: Option<String>,
}

/// Create or change a connection.
pub struct ConnectionCreate;
pub struct ConnectionUpdate;

/// What the two operations ask of `write`.
struct ConnectionWriteInput {
    connection_id: Option<IdRef>,
    account: Option<IdRef>,
    name: Option<String>,
    reaches_all: Option<bool>,
    webhook_url: Option<String>,
    webhook_format: Option<String>,
    subscriptions: Option<Vec<SubscriptionInput>>,
    expected_version: Option<i64>,
}

/// What creating a connection takes.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionCreateInput {
    /// An agent or app you manage; left out, your own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Every workspace the account is in (default), or only this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaches_all: Option<bool>,
    /// Where events are pushed (a public https address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    /// standard (default), or typed_v1 / inbox_v1 for older receivers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_format: Option<String>,
    /// What to hear; left out, what concerns the account (following and
    /// above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<SubscriptionInput>>,
}

impl Targeted for ConnectionCreateInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

/// What changing a connection takes.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionUpdateInput {
    pub connection_id: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaches_all: Option<bool>,
    /// A new address, or "" to stop pushes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_format: Option<String>,
    /// Replaces the connection's subscriptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<SubscriptionInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for ConnectionUpdateInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionWritten {
    pub connection: ConnectionView,
    /// The webhook's new signing secret: shown once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
}

const CREATE_DESCRIPTION: &str = "Create a connection to be told through: its name, the \
    webhook events are pushed to and its format (standard by default), whether it reaches every \
    workspace, and what it subscribes to. For your own account, or an agent or app you manage \
    (account). A webhook gets a signing secret, shown once: a retry of the same call doesn't \
    show it again (rotate it if it was lost).";

impl Operation for ConnectionCreate {
    const NAME: &'static str = "connection_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const SHOWN_ONCE: &'static [&'static str] = &["webhookSecret"];
    const TITLE: &'static str = "Create connection";
    const DESCRIPTION: &'static str = CREATE_DESCRIPTION;
    const EMITS: &'static [&'static str] = &["connection.changed"];

    type Input = ConnectionCreateInput;
    type Output = ConnectionWritten;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectionCreateInput,
    ) -> Result<ConnectionWritten, OpError> {
        write(
            call,
            ConnectionWriteInput {
                connection_id: None,
                account: input.account,
                name: input.name,
                reaches_all: input.reaches_all,
                webhook_url: input.webhook_url,
                webhook_format: input.webhook_format.or_else(|| Some("standard".to_owned())),
                subscriptions: input.subscriptions,
                expected_version: None,
            },
        )
        .await
    }
}

impl Operation for ConnectionUpdate {
    const NAME: &'static str = "connection_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const SHOWN_ONCE: &'static [&'static str] = &["webhookSecret"];
    const TITLE: &'static str = "Update connection";
    const DESCRIPTION: &'static str = "Change a connection: its name, the webhook events are \
        pushed to (\"\" stops pushes) and its format, whether it reaches every workspace, and \
        what it subscribes to (the list replaces the old one). Yours, or one of an agent or app \
        you manage. A new webhook address gets a signing secret, shown once; saving a webhook \
        whose pushes stopped turns them back on.";
    const EMITS: &'static [&'static str] = &["connection.changed"];

    type Input = ConnectionUpdateInput;
    type Output = ConnectionWritten;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectionUpdateInput,
    ) -> Result<ConnectionWritten, OpError> {
        write(
            call,
            ConnectionWriteInput {
                connection_id: Some(input.connection_id),
                account: None,
                name: input.name,
                reaches_all: input.reaches_all,
                webhook_url: input.webhook_url,
                webhook_format: input.webhook_format,
                subscriptions: input.subscriptions,
                expected_version: input.expected_version,
            },
        )
        .await
    }
}

/// Creates or changes a connection, as the two operations ask.
async fn write(
    call: &mut OpCall<'_>,
    input: ConnectionWriteInput,
) -> Result<ConnectionWritten, OpError> {
    if let Some(name) = &input.name
        && !(1..=80).contains(&name.trim().chars().count())
    {
        return Err(OpError::invalid(
            "name",
            "Give it a name of 1 to 80 characters.",
        ));
    }
    let subscriptions = match &input.subscriptions {
        Some(subscriptions) => Some(check_subscriptions(subscriptions)?),
        None => None,
    };
    let workspace_id = call.ctx.workspace_id;
    let setter = call.account_id();
    let request_id = call.ctx.request_id.clone();
    let delivery = match input.connection_id {
        Some(id) => {
            let found = managed_connection(call, id.id()).await?;
            if input
                .expected_version
                .is_some_and(|expected| expected != found.version)
            {
                return Err(AppError::VersionConflict {
                    resource: "connection",
                    current_version: found.version,
                }
                .into());
            }
            if found.source_connection_id.is_some() && input.reaches_all == Some(true) {
                return Err(OpError::invalid(
                    "reachesAll",
                    "This connection belongs to an older project credential and reaches only its workspace.",
                ));
            }
            found
        }
        None => {
            let account = account_for(call, input.account.map(IdRef::id)).await?;
            let tx = call.tx().await?;
            let member = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM organization_memberships \
                 WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
            )
            .bind(workspace_id)
            .bind(account)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if !member {
                return Err(OpError::not_found("That account"));
            }
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO agent_connections (
                    id, home_workspace_id, account_id, reaches_all, name, push_cursor,
                    read_cursor, client_request_id, created_by_account_id
                )
                SELECT $1, $2, $3, $4, $5, next_position - 1, next_position - 1, $6, $7
                FROM event_position_counter
                "#,
            )
            .bind(id)
            .bind(workspace_id)
            .bind(account)
            .bind(input.reaches_all.unwrap_or(true))
            .bind(input.name.as_deref().map(str::trim).unwrap_or("Connection"))
            .bind(
                request_id
                    .as_deref()
                    .map(|id| id.chars().take(200).collect::<String>()),
            )
            .bind(setter)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
            connections::load_any(tx, id)
                .await?
                .ok_or(OpError::Unavailable)?
        }
    };
    let config = std::sync::Arc::clone(&call.state.config);
    let tx = call.tx().await?;
    sqlx::query(
        r#"
        UPDATE agent_connections
        SET name = COALESCE($2, name), reaches_all = COALESCE($3, reaches_all),
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(delivery.id)
    .bind(input.name.as_deref().map(str::trim))
    .bind(input.reaches_all)
    .execute(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let webhook_secret = match input.webhook_url.as_deref().map(str::trim) {
        Some("") => {
            connections::set_webhook(tx, &config, &delivery, None, None).await?;
            None
        }
        Some(url) => connections::set_webhook(
            tx,
            &config,
            &delivery,
            Some(url),
            input.webhook_format.as_deref(),
        )
        .await
        .map_err(|error| match error {
            AppError::Validation(_) => OpError::invalid(
                "webhookUrl",
                "Use a public https address (webhookFormat: standard, typed_v1 or inbox_v1).",
            ),
            other => other.into(),
        })?,
        None => {
            if let Some(format) = input.webhook_format.as_deref() {
                if !matches!(format, "standard" | "typed_v1" | "inbox_v1") {
                    return Err(OpError::invalid(
                        "webhookFormat",
                        "Use standard, typed_v1 or inbox_v1.",
                    ));
                }
                sqlx::query("UPDATE agent_connections SET webhook_format = $2 WHERE id = $1")
                    .bind(delivery.id)
                    .bind(format)
                    .execute(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
            }
            None
        }
    };
    if let Some(subscriptions) = subscriptions {
        sqlx::query("DELETE FROM event_subscriptions WHERE connection_id = $1")
            .bind(delivery.id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        for subscription in subscriptions {
            sqlx::query(
                r#"
                INSERT INTO event_subscriptions (
                    id, connection_id, kinds, workspace_ids, space_ids,
                    conversation_ids, includes_dms, echoes, min_reason, set_by_account_id
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(delivery.id)
            .bind(&subscription.kinds)
            .bind(&subscription.workspace_ids)
            .bind(&subscription.space_ids)
            .bind(&subscription.conversation_ids)
            .bind(subscription.includes_dms)
            .bind(subscription.echoes)
            .bind(&subscription.min_reason)
            .bind(setter)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        }
    }
    let saved = connections::load_any(tx, delivery.id)
        .await?
        .ok_or(OpError::Unavailable)?;
    let view = view(tx, saved).await?;
    changed(tx, &delivery).await?;
    call.audit_target(
        "connection",
        delivery.id,
        json!({ "account": delivery.account_id, "webhook": view.webhook_url.is_some() }),
    );
    Ok(ConnectionWritten {
        connection: view,
        webhook_secret,
    })
}

/// Tells the account's other tabs and sessions a connection of its changed.
async fn changed(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    delivery: &Connection,
) -> Result<(), OpError> {
    crate::events::emit(
        tx,
        crate::events::NewEvent {
            id: Uuid::now_v7(),
            ty: &crate::events::catalog::CONNECTION_CHANGED,
            workspace_id: Some(delivery.home_workspace_id),
            space_id: None,
            thing: Some((delivery.id, "connection")),
            access_thing: None,
            version: None,
            changes: &[],
            audience: crate::events::Audience::Account(delivery.account_id),
            payload: json!({ "connectionId": delivery.id }),
            actor: None,
            legacy_type: None,
        },
    )
    .await?;
    Ok(())
}

/// Subscriptions as stored, checked.
fn check_subscriptions(inputs: &[SubscriptionInput]) -> Result<Vec<Subscription>, OpError> {
    if inputs.len() > 20 {
        return Err(OpError::invalid(
            "subscriptions",
            "At most 20 subscriptions.",
        ));
    }
    inputs
        .iter()
        .map(|input| {
            let kinds = input.kinds.clone().unwrap_or_else(|| vec!["*".to_owned()]);
            if kinds.is_empty() || kinds.len() > 64 {
                return Err(OpError::invalid("kinds", "Name 1 to 64 kinds."));
            }
            if let Some(unknown) = kinds.iter().find(|kind| !catalogue::valid_pattern(kind)) {
                return Err(OpError::Invalid {
                    field: "kinds".into(),
                    message: format!(
                        "{unknown} isn't a kind of event; event_kind_list lists them."
                    )
                    .into(),
                });
            }
            let min_reason = input
                .min_reason
                .clone()
                .unwrap_or_else(|| "following".to_owned());
            if super::reason::Level::parse(&min_reason).is_none() {
                return Err(OpError::invalid(
                    "minReason",
                    "Use addressed, following or visible.",
                ));
            }
            let ids = |list: &Option<Vec<IdRef>>| {
                list.as_ref()
                    .filter(|list| !list.is_empty())
                    .map(|list| list.iter().map(|id| id.id()).take(100).collect::<Vec<_>>())
            };
            Ok(Subscription {
                id: Uuid::nil(),
                kinds,
                workspace_ids: ids(&input.workspace_ids),
                space_ids: ids(&input.space_ids),
                conversation_ids: ids(&input.conversation_ids),
                includes_dms: input.includes_dms.unwrap_or(true),
                echoes: input.echoes.unwrap_or(false),
                min_reason,
            })
        })
        .collect()
}

/// Give a connection's webhook a new signing secret.
pub struct ConnectionSecretRotate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionRef {
    pub connection_id: IdRef,
}

impl Targeted for ConnectionRef {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for ConnectionSecretRotate {
    const NAME: &'static str = "connection_secret_rotate";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const SHOWN_ONCE: &'static [&'static str] = &["webhookSecret"];
    const TITLE: &'static str = "Rotate connection secret";
    const EMITS: &'static [&'static str] = &["connection.changed"];
    const DESCRIPTION: &'static str = "Give a connection's webhook a new signing secret, shown \
        once. For a day both secrets sign every push (two signatures in webhook-signature), so \
        the receiver can switch without missing one.";

    type Input = ConnectionRef;
    type Output = ConnectionWritten;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConnectionRef,
    ) -> Result<ConnectionWritten, OpError> {
        let delivery = managed_connection(call, input.connection_id.id()).await?;
        if delivery.webhook_url.is_none() {
            return Err(OpError::invalid(
                "connectionId",
                "This connection has no webhook to sign for. Set webhookUrl first.",
            ));
        }
        let config = std::sync::Arc::clone(&call.state.config);
        let tx = call.tx().await?;
        let secret = connections::new_signing_secret(tx, &config, &delivery, true).await?;
        sqlx::query(
            "UPDATE agent_connections SET version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(delivery.id)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let saved = connections::load_any(tx, delivery.id)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(tx, &saved).await?;
        let view = view(tx, saved).await?;
        call.audit_target("connection", delivery.id, json!({ "rotated": true }));
        Ok(ConnectionWritten {
            connection: view,
            webhook_secret: Some(secret),
        })
    }
}

/// Remove a connection.
pub struct ConnectionRemove;

impl Operation for ConnectionRemove {
    const NAME: &'static str = "connection_remove";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Remove connection";
    const EMITS: &'static [&'static str] = &["connection.changed"];
    const DESCRIPTION: &'static str = "Remove a connection: nothing more is pushed to it and its \
        cursor stops working. A default connection is made again the next time the account reads \
        without naming one.";

    type Input = ConnectionRef;
    type Output = ConnectionView;

    async fn run(call: &mut OpCall<'_>, input: ConnectionRef) -> Result<ConnectionView, OpError> {
        let delivery = managed_connection(call, input.connection_id.id()).await?;
        if delivery.source_connection_id.is_some() {
            return Err(OpError::invalid(
                "connectionId",
                "This connection belongs to an older project credential; remove the agent's connection in the project instead, or clear its webhook.",
            ));
        }
        let tx = call.tx().await?;
        connections::clear_signing_secrets(tx, delivery.id, None).await?;
        sqlx::query(
            "UPDATE agent_connections SET state = 'removed', webhook_url = NULL, \
             version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(delivery.id)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let saved = connections::load_any(tx, delivery.id)
            .await?
            .ok_or(OpError::Unavailable)?;
        changed(tx, &saved).await?;
        let shown = view(tx, saved).await?;
        call.audit_target("connection", delivery.id, json!({ "removed": true }));
        Ok(shown)
    }
}
