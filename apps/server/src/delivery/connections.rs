//! Connections (A2 3.1): how one account is reached and told. An account
//! may have several; the one a call reads through is named, or its
//! default: for an older project credential, the connection made for that
//! credential (its home workspace only, as the credential reaches); for any
//! other credential, the account's own default, which reaches every
//! workspace the account is in (ruling R19). Either is made on first use,
//! starting at the log's head, so a new connection doesn't replay history.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{access::Caller, error::AppError};

/// One connection.
#[derive(Clone, Debug, FromRow)]
pub struct Connection {
    pub id: Uuid,
    pub home_workspace_id: Uuid,
    pub account_id: Uuid,
    pub reaches_all: bool,
    pub source_connection_id: Option<Uuid>,
    pub is_default: bool,
    pub name: String,
    pub webhook_url: Option<String>,
    pub webhook_format: String,
    pub cursor_key: Vec<u8>,
    pub push_cursor: i64,
    pub read_cursor: i64,
    pub state: String,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub first_failure_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// A person's app made or changed it: it is pushed to within that
    /// app's delegation limit, and stops once the app is disconnected.
    pub made_through_app: bool,
    pub via_grant_id: Option<Uuid>,
    /// The last test ping: when, what the receiver answered, and why it
    /// didn't work.
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test_status: Option<i32>,
    pub last_test_error: Option<String>,
}

pub(crate) const COLUMNS: &str = "id, home_workspace_id, account_id, reaches_all, \
    source_connection_id, is_default, name, webhook_url, webhook_format, cursor_key, \
    push_cursor, read_cursor, state, disabled_reason, consecutive_failures, first_failure_at, \
    last_seen_at, last_delivered_at, last_error, version, created_at, updated_at, \
    made_through_app, via_grant_id, last_test_at, last_test_status, last_test_error";

/// What a subscription asks for.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kinds: Vec<String>,
    #[schemars(with = "Option<Vec<String>>")]
    pub workspace_ids: Option<Vec<Uuid>>,
    #[schemars(with = "Option<Vec<String>>")]
    pub space_ids: Option<Vec<Uuid>>,
    #[schemars(with = "Option<Vec<String>>")]
    pub conversation_ids: Option<Vec<Uuid>>,
    pub includes_dms: bool,
    pub echoes: bool,
    pub min_reason: String,
}

impl Subscription {
    /// What a connection with no subscription hears: what is addressed to
    /// it and what it follows, DMs included, never its own doing.
    pub fn default_for(connection_id: Uuid) -> Self {
        Self {
            id: connection_id,
            kinds: vec!["*".to_owned()],
            workspace_ids: None,
            space_ids: None,
            conversation_ids: None,
            includes_dms: true,
            echoes: false,
            min_reason: "following".to_owned(),
        }
    }
}

/// The log's newest position.
pub async fn head(connection: &mut PgConnection) -> Result<i64, AppError> {
    crate::events::head(connection).await
}

/// A connection of `account_id` by id, not removed.
pub async fn load(
    connection: &mut PgConnection,
    account_id: Uuid,
    id: Uuid,
) -> Result<Option<Connection>, AppError> {
    Ok(sqlx::query_as::<_, Connection>(&format!(
        "SELECT {COLUMNS} FROM agent_connections \
         WHERE id = $1 AND account_id = $2 AND state <> 'removed'"
    ))
    .bind(id)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A connection by id alone, for the pusher.
pub async fn load_any(
    connection: &mut PgConnection,
    id: Uuid,
) -> Result<Option<Connection>, AppError> {
    Ok(sqlx::query_as::<_, Connection>(&format!(
        "SELECT {COLUMNS} FROM agent_connections WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The connection a call reads through: the one it names (the caller's
/// own), else its default, made now if it has none.
pub async fn for_caller(
    connection: &mut PgConnection,
    caller: &Caller,
    named: Option<Uuid>,
) -> Result<Connection, AppError> {
    if let Some(id) = named {
        let found = load(connection, caller.account_id, id)
            .await?
            .ok_or(AppError::NotFound("connection"))?;
        // An older credential reads only through its own connection.
        if caller.connection_id.is_some() && found.source_connection_id != caller.connection_id {
            return Err(AppError::NotFound("connection"));
        }
        return Ok(found);
    }
    match caller.connection_id {
        Some(source) => for_source(connection, caller.account_id, source).await,
        None => default_for(connection, caller.account_id).await,
    }
}

/// The connection made for an agent's token. Like the token, it reaches
/// each workspace the account is in (R19).
pub async fn for_source(
    connection: &mut PgConnection,
    account_id: Uuid,
    source: Uuid,
) -> Result<Connection, AppError> {
    sqlx::query(
        r#"
        INSERT INTO agent_connections (
            id, home_workspace_id, account_id, reaches_all, source_connection_id, is_default,
            name, push_cursor, read_cursor, created_by_account_id
        )
        SELECT $1, source.workspace_id, source.actor_id, true, source.id, true,
               source.connection_name, counter.next_position - 1, counter.next_position - 1,
               source.created_by_actor_id
        FROM external_agent_connections source, event_position_counter counter
        WHERE source.id = $2 AND source.actor_id = $3 AND source.status = 'active'
        ON CONFLICT (source_connection_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(source)
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query_as::<_, Connection>(&format!(
        "SELECT {COLUMNS} FROM agent_connections \
         WHERE source_connection_id = $1 AND account_id = $2 AND state <> 'removed'"
    ))
    .bind(source)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("connection"))
}

/// The account's own default connection, which reaches every workspace it
/// is in. Its home is the workspace it joined first.
pub async fn default_for(
    connection: &mut PgConnection,
    account_id: Uuid,
) -> Result<Connection, AppError> {
    sqlx::query(
        r#"
        INSERT INTO agent_connections (
            id, home_workspace_id, account_id, reaches_all, is_default, name,
            push_cursor, read_cursor, created_by_account_id
        )
        SELECT $1, membership.organization_id, $2, true, true, 'Default',
               counter.next_position - 1, counter.next_position - 1, $2
        FROM organization_memberships membership, event_position_counter counter
        WHERE membership.actor_id = $2 AND membership.enabled
        ORDER BY membership.created_at, membership.organization_id
        LIMIT 1
        ON CONFLICT (account_id)
            WHERE is_default AND source_connection_id IS NULL AND state <> 'removed'
            DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query_as::<_, Connection>(&format!(
        "SELECT {COLUMNS} FROM agent_connections \
         WHERE account_id = $1 AND is_default AND source_connection_id IS NULL \
           AND state <> 'removed'"
    ))
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("connection"))
}

/// The workspaces a connection reaches now: the account's enabled
/// memberships, or only its home. An older credential's connection also
/// stops when that credential's connection does.
pub async fn reach(
    connection: &mut PgConnection,
    delivery: &Connection,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.organization_id
        FROM organization_memberships membership
        JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
        WHERE membership.actor_id = $1 AND membership.enabled
          AND ($2 OR membership.organization_id = $3)
          AND ($4::uuid IS NULL OR EXISTS (
              SELECT 1 FROM external_agent_connections source
              WHERE source.id = $4 AND source.status = 'active'))
        ORDER BY membership.organization_id
        "#,
    )
    .bind(delivery.account_id)
    .bind(delivery.reaches_all)
    .bind(delivery.home_workspace_id)
    .bind(delivery.source_connection_id)
    .fetch_all(&mut *connection)
    .await
    .map(|mut workspaces| {
        // DMs and groups are in no workspace (K4.12): their events are in
        // the direct scope, which every connection that reaches a workspace
        // reads; each is still checked against who can see it.
        if !workspaces.is_empty() {
            workspaces.push(crate::spaces::DIRECT);
        }
        workspaces
    })?)
}

/// A connection's subscriptions, or the default when it has none.
pub async fn subscriptions(
    connection: &mut PgConnection,
    delivery: &Connection,
) -> Result<Vec<Subscription>, AppError> {
    let found = sqlx::query_as::<_, Subscription>(
        r#"
        SELECT id, kinds, workspace_ids, space_ids, conversation_ids, includes_dms, echoes,
               min_reason
        FROM event_subscriptions WHERE connection_id = $1 ORDER BY created_at, id
        "#,
    )
    .bind(delivery.id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(if found.is_empty() {
        vec![Subscription::default_for(delivery.id)]
    } else {
        found
    })
}

/// The client presented `position`: it has everything up to there.
pub async fn mark_read(
    connection: &mut PgConnection,
    id: Uuid,
    position: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE agent_connections \
         SET read_cursor = GREATEST(read_cursor, $2), last_seen_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(position)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Make a new signing secret the connection's active one and return it
/// (shown once). With `overlap`, the one it replaces keeps signing beside
/// it for a day (a rotation); otherwise it goes now.
pub async fn new_signing_secret(
    connection: &mut PgConnection,
    config: &crate::config::AppConfig,
    delivery: &Connection,
    overlap: bool,
) -> Result<String, AppError> {
    let secret = super::sign::new_secret();
    let keys = crate::secrets::keyring(config)?;
    let stored = crate::secrets::put(
        connection,
        keys.as_ref(),
        crate::secrets::NewSecret {
            owner: crate::secrets::SecretOwner::Actor {
                organization_id: delivery.home_workspace_id,
                actor_id: delivery.account_id,
            },
            name: &format!("webhook:{}:{}", delivery.id, Uuid::now_v7().simple()),
            description: Some("Signs what is pushed to this connection's webhook."),
            value: &crate::secrets::SecretValue::new(secret.clone()),
            actor_id: None,
            system_managed: true,
        },
    )
    .await?;
    if overlap {
        // A secret already retiring goes now: two sign at most.
        clear_signing_secrets(connection, delivery.id, Some("retiring")).await?;
        sqlx::query(
            "UPDATE connection_signing_secrets \
             SET state = 'retiring', retire_at = now() + interval '1 day' \
             WHERE connection_id = $1 AND state = 'active'",
        )
        .bind(delivery.id)
        .execute(&mut *connection)
        .await?;
    } else {
        clear_signing_secrets(connection, delivery.id, None).await?;
    }
    sqlx::query(
        "INSERT INTO connection_signing_secrets (id, connection_id, secret_id, state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(Uuid::now_v7())
    .bind(delivery.id)
    .bind(stored.id)
    .execute(&mut *connection)
    .await?;
    Ok(secret)
}

/// A workspace is being deleted: each connection at home there that reaches
/// other workspaces the account is still in moves its home to the one the
/// account joined first. Its signing secrets were kept in the old home, so
/// they go, and its webhook stops until it is saved again (which makes a new
/// secret). Every other connection at home there goes with the workspace.
pub async fn rehome_from(connection: &mut PgConnection, workspace: Uuid) -> Result<(), AppError> {
    let moved = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH next_home AS (
            SELECT DISTINCT ON (delivery.id) delivery.id, membership.organization_id
            FROM agent_connections delivery
            JOIN organization_memberships membership
              ON membership.actor_id = delivery.account_id
             AND membership.enabled
             AND membership.organization_id <> $1
            WHERE delivery.home_workspace_id = $1 AND delivery.reaches_all
              AND delivery.source_connection_id IS NULL AND delivery.state <> 'removed'
            ORDER BY delivery.id, membership.created_at, membership.organization_id
        )
        UPDATE agent_connections delivery
        SET home_workspace_id = next_home.organization_id,
            state = CASE WHEN delivery.webhook_url IS NOT NULL AND delivery.state = 'active'
                         THEN 'push_disabled' ELSE delivery.state END,
            disabled_reason = CASE WHEN delivery.webhook_url IS NOT NULL
                                   THEN 'Its home workspace was deleted. Save the webhook again for a new signing secret.'
                                   ELSE delivery.disabled_reason END,
            version = delivery.version + 1, updated_at = now()
        FROM next_home
        WHERE next_home.id = delivery.id
        RETURNING delivery.id
        "#,
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    for id in moved {
        clear_signing_secrets(connection, id, None).await?;
    }
    Ok(())
}

/// Remove a connection's signing secrets (only those in `state` when
/// given), and their values from the store.
pub async fn clear_signing_secrets(
    connection: &mut PgConnection,
    connection_id: Uuid,
    state: Option<&str>,
) -> Result<(), AppError> {
    let removed = sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM connection_signing_secrets \
         WHERE connection_id = $1 AND ($2::text IS NULL OR state = $2) RETURNING secret_id",
    )
    .bind(connection_id)
    .bind(state)
    .fetch_all(&mut *connection)
    .await?;
    for secret_id in removed {
        crate::secrets::delete(&mut *connection, secret_id).await?;
    }
    Ok(())
}

/// Point a connection's pushes at `url`, or stop them (`None`). A new
/// address gets a new signing secret, returned to be shown once; saving the
/// same address keeps the secret and turns pushes back on. The format
/// changes only when `format` says (an older receiver keeps working when
/// its address is saved again). Pushes start from now, not from history.
pub async fn set_webhook(
    connection: &mut PgConnection,
    config: &crate::config::AppConfig,
    delivery: &Connection,
    url: Option<&str>,
    format: Option<&str>,
) -> Result<Option<String>, AppError> {
    if let Some(format) = format
        && !matches!(format, "standard" | "typed_v1" | "inbox_v1")
    {
        return Err(AppError::Validation("webhookFormat"));
    }
    if let Some(url) = url {
        crate::outbound::validate_webhook_url(url, &config.webhook_trusted_origins)?;
    }
    let has_secret = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM connection_signing_secrets \
         WHERE connection_id = $1 AND state = 'active')",
    )
    .bind(delivery.id)
    .fetch_one(&mut *connection)
    .await?;
    let secret = match url {
        Some(url) if delivery.webhook_url.as_deref() != Some(url) || !has_secret => {
            Some(new_signing_secret(connection, config, delivery, false).await?)
        }
        Some(_) => None,
        None => {
            clear_signing_secrets(connection, delivery.id, None).await?;
            None
        }
    };
    sqlx::query(
        r#"
        UPDATE agent_connections
        SET push_cursor = CASE WHEN webhook_url IS NULL AND $2::text IS NOT NULL
                               THEN (SELECT next_position - 1 FROM event_position_counter)
                               ELSE push_cursor END,
            webhook_url = $2,
            webhook_format = COALESCE($3, webhook_format),
            state = CASE WHEN state = 'push_disabled' THEN 'active' ELSE state END,
            disabled_reason = NULL, consecutive_failures = 0, first_failure_at = NULL,
            last_error = NULL, version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(delivery.id)
    .bind(url)
    .bind(format)
    .execute(&mut *connection)
    .await?;
    Ok(secret)
}
