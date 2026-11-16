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

const VIEW_COLUMNS: &str = "account_id, workspace_id IS NOT NULL AS workspace_only, emoji, \
     text, state, clear_at, away_until, updated_at";

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
        "DELETE FROM account_statuses WHERE account_id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(removed > 0)
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
          AND (status.clear_at IS NULL OR status.clear_at > now())
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
    Ok(shown)
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
/// job). Returns how many went.
pub async fn clear_due(pool: &sqlx::PgPool, limit: i64) -> Result<u64, AppError> {
    let cleared = crate::db::scoped_one(pool, async |connection| {
        sqlx::query(
            r#"
        DELETE FROM account_statuses
        WHERE id IN (
            SELECT id FROM account_statuses
            WHERE clear_at IS NOT NULL AND clear_at <= now()
            ORDER BY clear_at
            LIMIT $1
        )
        "#,
        )
        .bind(limit)
        .execute(&mut *connection)
        .await
    })
    .await?;
    Ok(cleared.rows_affected())
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
