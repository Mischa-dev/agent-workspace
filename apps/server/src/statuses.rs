//! Account status (C2.15): an emoji and a line next to a name, for people
//! and agents alike. One table, `account_statuses`, keyed by the account and
//! a workspace that may be empty (R19): a status set for no workspace is the
//! account's own and shows wherever it is a member; one set for a workspace
//! (an agent's line about work there) shows only in that workspace.
//!
//! `status_set` and `status_delete` (in `ops.rs`) are the only writes. Reads
//! show a status only to someone who shares the workspace the reader is in,
//! and never an expired one; the `status_clear_due` job removes those rows.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

pub mod ops;

pub const MAX_TEXT: usize = 120;
pub const MAX_EMOJI: usize = 16;
/// The most accounts one status read answers for.
pub const MAX_ACCOUNTS: usize = 200;
pub const STATES: &[&str] = &["available", "working", "asleep", "paused", "unresponsive"];

/// A status as people and agents see it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// True for a line about work in this workspace only; false for the
    /// account's own status, shown in every workspace it is in.
    pub workspace_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// available, working, asleep, paused or unresponsive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// When it clears by itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub clear_at: Option<DateTime<Utc>>,
    /// Away until then: shown as "Away until …".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub away_until: Option<DateTime<Utc>>,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
}

/// What to set. Fields left out are cleared: a status is set whole.
#[derive(Clone, Debug, Default)]
pub struct StatusInput {
    pub emoji: Option<String>,
    pub text: Option<String>,
    pub state: Option<String>,
    pub clear_at: Option<DateTime<Utc>>,
    pub away_until: Option<DateTime<Utc>>,
}

/// Why a status was refused, by field.
#[derive(Debug)]
pub struct Refused {
    pub field: &'static str,
    pub message: &'static str,
}

fn one_line(value: Option<&str>, max: usize) -> Result<Option<String>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value))
}

impl StatusInput {
    /// The input with its text tidied, or which field is wrong.
    pub fn checked(self, now: DateTime<Utc>) -> Result<Self, Refused> {
        let text = one_line(self.text.as_deref(), MAX_TEXT).map_err(|()| Refused {
            field: "text",
            message: "A status is one line of up to 120 characters.",
        })?;
        let emoji = match self.emoji.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(emoji)
                if emoji.chars().count() <= MAX_EMOJI
                    && !emoji
                        .chars()
                        .any(|character| character.is_control() || character.is_whitespace()) =>
            {
                Some(emoji.to_owned())
            }
            Some(_) => {
                return Err(Refused {
                    field: "emoji",
                    message: "Pass one emoji, such as 🌴, or a :shortcode:.",
                });
            }
        };
        let state = match self.state.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(state) if STATES.contains(&state) => Some(state.to_owned()),
            Some(_) => {
                return Err(Refused {
                    field: "state",
                    message: "Use available, working, asleep, paused or unresponsive.",
                });
            }
        };
        if self.clear_at.is_some_and(|at| at <= now) {
            return Err(Refused {
                field: "clearAt",
                message: "clearAt is when it clears by itself: pass a time in the future.",
            });
        }
        if emoji.is_none() && text.is_none() && state.is_none() && self.away_until.is_none() {
            return Err(Refused {
                field: "text",
                message: "Say something: text, emoji, state or awayUntil. To clear a status, use status_delete.",
            });
        }
        Ok(Self {
            emoji,
            text,
            state,
            clear_at: self.clear_at,
            away_until: self.away_until,
        })
    }
}

/// A paused account shows as paused whatever state it set (A2.10).
const VIEW_COLUMNS: &str = "account_id, workspace_id IS NOT NULL AS workspace_only, emoji, \
     text, CASE WHEN paused_at IS NOT NULL THEN 'paused' ELSE state END AS state, clear_at, \
     away_until, updated_at";

/// Sets `account_id`'s status, for every workspace (`workspace_id` None) or
/// for one, replacing what was there. `setter` is who set it: the account
/// itself, or someone managing it.
pub async fn set(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    setter: Uuid,
    workspace_id: Option<Uuid>,
    input: &StatusInput,
) -> Result<StatusView, AppError> {
    let set_by = if setter == account_id { "self" } else { "host" };
    Ok(sqlx::query_as::<_, StatusView>(&format!(
        r#"
        INSERT INTO account_statuses (
            id, account_id, workspace_id, emoji, text, state, clear_at, away_until,
            set_by, set_by_account_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (account_id, workspace_id) DO UPDATE
        SET emoji = EXCLUDED.emoji, text = EXCLUDED.text, state = EXCLUDED.state,
            clear_at = EXCLUDED.clear_at, away_until = EXCLUDED.away_until,
            set_by = EXCLUDED.set_by, set_by_account_id = EXCLUDED.set_by_account_id,
            updated_at = now()
        RETURNING {VIEW_COLUMNS}
        "#
    ))
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(workspace_id)
    .bind(&input.emoji)
    .bind(&input.text)
    .bind(&input.state)
    .bind(input.clear_at)
    .bind(input.away_until)
    .bind(set_by)
    .bind(setter)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Clears `account_id`'s status for every workspace or for one. Returns
/// whether there was one.
pub async fn clear(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    workspace_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let removed = sqlx::query(
        "DELETE FROM account_statuses WHERE account_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 \
         AND paused_at IS NULL",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    // A paused account keeps its pause; only what it said goes.
    let emptied = sqlx::query(
        "UPDATE account_statuses SET emoji = NULL, text = NULL, state = NULL, clear_at = NULL, \
         away_until = NULL, updated_at = now() \
         WHERE account_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 AND paused_at IS NOT NULL",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(removed + emptied > 0)
}

/// Tells the spaces of `workspace_id` that `account_id` belongs to that its
/// status changed (`account.status.changed`), in the change's transaction.
/// Other workspaces' screens read it again when they next load it.
pub async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    workspace_id: Uuid,
    workspace_only: bool,
) -> Result<(), AppError> {
    let spaces = sqlx::query_scalar::<_, Uuid>(
        "SELECT project.id FROM project_memberships membership \
         JOIN projects project ON project.id = membership.project_id \
         WHERE membership.actor_id = $1 AND project.organization_id = $2 \
           AND project.archived_at IS NULL \
         ORDER BY project.created_at, project.id LIMIT 200",
    )
    .bind(account_id)
    .bind(workspace_id)
    .fetch_all(&mut **transaction)
    .await?;
    for space in spaces {
        crate::outbox::enqueue_typed(
            transaction,
            space,
            crate::outbox::OutboxEventPayload::AccountStatusChanged(
                crate::outbox::AccountStatusChangedPayload {
                    account_id,
                    workspace_id: workspace_only.then_some(workspace_id),
                },
            ),
        )
        .await?;
    }
    Ok(())
}

/// Announces a change to `account_id`'s status wherever it shows: a line
/// about one workspace's work in that workspace, an account-wide one in
/// every workspace the account is in (readers in each have it on screen).
pub async fn announce_where_shown(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    workspace_only: Option<Uuid>,
) -> Result<(), AppError> {
    let workspaces = match workspace_only {
        Some(workspace_id) => vec![workspace_id],
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT organization_id FROM organization_memberships \
                 WHERE actor_id = $1 AND enabled ORDER BY organization_id LIMIT 50",
            )
            .bind(account_id)
            .fetch_all(&mut **transaction)
            .await?
        }
    };
    for workspace in workspaces {
        announce(transaction, account_id, workspace, workspace_only.is_some()).await?;
    }
    Ok(())
}

/// The statuses shown in `workspace_id` for `accounts`: each account's own
/// one and its line for this workspace, for accounts that are members of it
/// (anyone else's status is none of this workspace's business). Expired
/// ones are left out.
pub async fn shown_in(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    accounts: &[Uuid],
) -> Result<HashMap<Uuid, Vec<StatusView>>, AppError> {
    let accounts = &accounts[..accounts.len().min(MAX_ACCOUNTS)];
    let rows = sqlx::query_as::<_, StatusView>(&format!(
        r#"
        SELECT {VIEW_COLUMNS}
        FROM account_statuses status
        WHERE status.account_id = ANY($2)
          AND (status.workspace_id IS NULL OR status.workspace_id = $1)
          AND (status.clear_at IS NULL OR status.clear_at > now()
               OR status.paused_at IS NOT NULL)
          AND EXISTS (
              SELECT 1 FROM organization_memberships membership
              WHERE membership.organization_id = $1
                AND membership.actor_id = status.account_id
                AND membership.enabled
          )
        ORDER BY status.account_id, status.workspace_id NULLS FIRST
        "#
    ))
    .bind(workspace_id)
    .bind(accounts)
    .fetch_all(&mut *connection)
    .await?;
    let mut shown: HashMap<Uuid, Vec<StatusView>> = HashMap::new();
    for row in rows {
        shown.entry(row.account_id).or_default().push(row);
    }
    // A2.10: an account that hasn't picked up a request it was asked here
    // shows as not responding in this workspace, unless it is paused.
    let unresponsive = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT request.agent_actor_id
        FROM reply_requests request
        JOIN projects space ON space.id = request.project_id
        WHERE space.organization_id = $1 AND request.agent_actor_id = ANY($2)
          AND request.status = 'pending' AND request.unresponsive_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(accounts)
    .fetch_all(&mut *connection)
    .await?;
    for account in unresponsive {
        let views = shown.entry(account).or_default();
        if views
            .iter()
            .any(|view| view.state.as_deref() == Some("paused"))
        {
            continue;
        }
        match views.iter_mut().find(|view| view.workspace_only) {
            Some(view) => view.state = Some("unresponsive".to_owned()),
            None => views.push(StatusView {
                account_id: account,
                workspace_only: true,
                emoji: None,
                text: None,
                state: Some("unresponsive".to_owned()),
                clear_at: None,
                away_until: None,
                updated_at: Utc::now(),
            }),
        }
    }
    Ok(shown)
}

/// Pause `account_id` in one workspace, or everywhere (`workspace_id`
/// None): it hears only about its own pause, connections and cancelled
/// replies until resumed, and its writes do nothing (A2.10).
pub async fn pause(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    workspace_id: Option<Uuid>,
    by: Uuid,
    reason: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO account_statuses (
            id, account_id, workspace_id, set_by, set_by_account_id,
            paused_by_account_id, paused_reason, paused_at
        )
        VALUES ($1, $2, $3, 'workspace', $4, $4, $5, now())
        ON CONFLICT (account_id, workspace_id) DO UPDATE
        SET paused_by_account_id = $4, paused_reason = $5,
            paused_at = COALESCE(account_statuses.paused_at, now()), updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(workspace_id)
    .bind(by)
    .bind(reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Lift a pause. Returns whether there was one. A row that held only the
/// pause goes.
pub async fn resume(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    workspace_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let lifted = sqlx::query(
        "UPDATE account_statuses SET paused_by_account_id = NULL, paused_reason = NULL, \
         paused_at = NULL, updated_at = now() \
         WHERE account_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 AND paused_at IS NOT NULL",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    sqlx::query(
        "DELETE FROM account_statuses WHERE account_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 \
         AND paused_at IS NULL AND emoji IS NULL AND text IS NULL AND state IS NULL \
         AND away_until IS NULL",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(&mut **transaction)
    .await?;
    Ok(lifted > 0)
}

/// The workspaces `account_id` is paused in: `None` in the list means
/// everywhere.
pub async fn paused_in(
    connection: &mut PgConnection,
    account_id: Uuid,
) -> Result<Vec<Option<Uuid>>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT workspace_id FROM account_statuses WHERE account_id = $1 AND paused_at IS NOT NULL",
    )
    .bind(account_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// Whether `account_id` is paused in `workspace_id`.
pub async fn is_paused(
    connection: &mut PgConnection,
    account_id: Uuid,
    workspace_id: Uuid,
) -> Result<bool, AppError> {
    Ok(paused_in(connection, account_id)
        .await?
        .iter()
        .any(|paused| paused.is_none_or(|paused| paused == workspace_id)))
}

const CLEAR_DUE_KIND: &str = "status.clear_due";

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(CLEAR_DUE_KIND, |context| async move {
        // A due status is already hidden from every read; this only tidies.
        while clear_due(&context.state.pool, 500).await? == 500 {}
        Ok(())
    });
    registry.every(CLEAR_DUE_KIND, std::time::Duration::from_secs(5 * 60));
}

/// Removes statuses whose time to clear has come (the `status.clear_due`
/// job), telling the screens that show them in the same transaction, as an
/// explicit clear does. Returns how many went.
pub async fn clear_due(pool: &sqlx::PgPool, limit: i64) -> Result<u64, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let cleared = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        r#"
        DELETE FROM account_statuses
        WHERE id IN (
            SELECT id FROM account_statuses
            WHERE clear_at IS NOT NULL AND clear_at <= now() AND paused_at IS NULL
            ORDER BY clear_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING account_id, workspace_id
        "#,
    )
    .bind(limit)
    .fetch_all(&mut *transaction)
    .await?;
    for (account_id, workspace_id) in &cleared {
        announce_where_shown(&mut transaction, *account_id, *workspace_id).await?;
    }
    // A paused account's own words run out; the pause stays.
    sqlx::query(
        "UPDATE account_statuses SET emoji = NULL, text = NULL, state = NULL, clear_at = NULL, \
         away_until = NULL, updated_at = now() \
         WHERE clear_at IS NOT NULL AND clear_at <= now() AND paused_at IS NOT NULL",
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(cleared.len() as u64)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::StatusInput;

    #[test]
    fn a_status_is_one_tidy_line() {
        let now = Utc::now();
        let checked = StatusInput {
            text: Some("  In   a meeting ".to_owned()),
            emoji: Some("📅".to_owned()),
            ..StatusInput::default()
        }
        .checked(now)
        .expect("valid");
        assert_eq!(checked.text.as_deref(), Some("In a meeting"));
        for (input, field) in [
            (
                StatusInput {
                    text: Some("x".repeat(121)),
                    ..StatusInput::default()
                },
                "text",
            ),
            (
                StatusInput {
                    text: Some("a\u{7}b".to_owned()),
                    ..StatusInput::default()
                },
                "text",
            ),
            (
                StatusInput {
                    emoji: Some("two words".to_owned()),
                    ..StatusInput::default()
                },
                "emoji",
            ),
            (
                StatusInput {
                    state: Some("busy".to_owned()),
                    ..StatusInput::default()
                },
                "state",
            ),
            (
                StatusInput {
                    text: Some("Lunch".to_owned()),
                    clear_at: Some(now - Duration::minutes(1)),
                    ..StatusInput::default()
                },
                "clearAt",
            ),
            (StatusInput::default(), "text"),
        ] {
            let refused = input.checked(now).expect_err(field);
            assert_eq!(refused.field, field);
        }
    }
}
