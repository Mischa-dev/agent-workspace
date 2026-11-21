//! A1.24: plan windows over the host protocol.
//!
//! A host names the provider accounts it runs on in `describe` (`accounts:
//! [{ref, name}]`); each becomes a provider account bound to the host by
//! that reference, owned as the host is, with no home on this server. The
//! host then reports their windows on `POST /api/v1/host/windows`, and the
//! report is stored exactly as the built-in host's own reads are
//! (`usage::store_report`), so the capacity alerts, the inbox and push
//! notices and the suggested batch work the same whichever host reports. A
//! report for an account the host didn't describe is refused.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{HostRow, protocol::HostError};
use crate::{
    AppState,
    provider::adapter::{ResetCredit, ResetCredits, UsageReport, UsageWindow},
};

/// At most this many accounts per host.
const MAX_ACCOUNTS: usize = 50;
const MAX_WINDOWS: usize = 20;

fn invalid(field: &'static str, message: impl Into<String>) -> HostError {
    HostError::Invalid {
        field,
        message: message.into(),
    }
}

fn line(value: Option<&str>, maximum: usize) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.chars().count() <= maximum)
        .filter(|text| !text.chars().any(char::is_control))
        .map(str::to_owned)
}

/// The accounts a description names, checked: `[(ref, name)]`.
pub fn described_accounts(accounts: &[Value]) -> Result<Vec<(String, String)>, HostError> {
    if accounts.len() > MAX_ACCOUNTS {
        return Err(invalid("accounts", "Describe at most 50 accounts."));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut checked = Vec::with_capacity(accounts.len());
    for account in accounts {
        let reference = line(account["ref"].as_str(), 200).ok_or_else(|| {
            invalid(
                "accounts",
                "Give each account a ref of 1 to 200 characters.",
            )
        })?;
        let name = line(account["name"].as_str(), 120).ok_or_else(|| {
            invalid(
                "accounts",
                "Give each account a name of 1 to 120 characters.",
            )
        })?;
        if !seen.insert(reference.clone()) {
            return Err(invalid("accounts", "Each account's ref must be different."));
        }
        checked.push((reference, name));
    }
    Ok(checked)
}

/// Binds the described accounts to the host: new ones are added, renamed
/// ones renamed, and ones no longer described are hidden (their windows
/// stay with them, as for any hidden account).
pub async fn bind_described(
    transaction: &mut Transaction<'_, Postgres>,
    host: &HostRow,
    accounts: &[(String, String)],
) -> Result<(), HostError> {
    let references: Vec<&str> = accounts
        .iter()
        .map(|(reference, _)| reference.as_str())
        .collect();
    sqlx::query(
        "UPDATE provider_accounts SET hidden_at = now(), updated_at = now() \
         WHERE host_id = $1 AND host_account_ref IS NOT NULL AND hidden_at IS NULL \
           AND NOT (host_account_ref = ANY($2))",
    )
    .bind(host.id)
    .bind(&references)
    .execute(&mut **transaction)
    .await?;
    if accounts.is_empty() {
        return Ok(());
    }
    // Owned as the host is: a person's host by that person, a workspace's
    // by whoever linked it. Kept in the workspace that owns the host, or
    // for a person's host the first workspace they joined.
    let Some((owner, workspace)) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        r#"
        SELECT COALESCE(host.owner_account_id, host.enrolled_by_account_id),
               COALESCE(host.owner_workspace_id, (
                   SELECT membership.organization_id FROM organization_memberships membership
                   WHERE membership.actor_id = host.owner_account_id AND membership.enabled
                   ORDER BY membership.created_at, membership.organization_id LIMIT 1))
        FROM hosts host WHERE host.id = $1
        "#,
    )
    .bind(host.id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Err(HostError::Unauthorized);
    };
    let Some(workspace) = workspace else {
        return Err(invalid(
            "accounts",
            "The host's owner isn't in a workspace yet, so its accounts have nowhere to live.",
        ));
    };
    for (reference, name) in accounts {
        sqlx::query(
            r#"
            INSERT INTO provider_accounts (
                id, organization_id, owner_actor_id, adapter_kind, name, home_path,
                host_id, host_account_ref, status, account_state
            ) VALUES ($1, $2, $3, 'host', $4, NULL, $5, $6, 'available', 'authenticated')
            ON CONFLICT (host_id, host_account_ref) WHERE host_account_ref IS NOT NULL
            DO UPDATE SET name = EXCLUDED.name, hidden_at = NULL, updated_at = now()
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace)
        .bind(owner)
        .bind(name)
        .bind(host.id)
        .bind(reference)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowsReport {
    pub accounts: Vec<AccountReport>,
    /// Only the windows listed change; the rest, the plan and the banked
    /// resets stay as they were (what a run hears mid-turn).
    #[serde(default)]
    pub partial: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountReport {
    #[serde(rename = "ref")]
    pub reference: String,
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<WindowReport>,
    pub reset_credits: Option<CreditsReport>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowReport {
    /// Stable for the account: letters, digits and `._-`.
    pub key: String,
    /// What the host calls it, such as "5-hour session".
    pub label: Option<String>,
    pub used_percent: u16,
    pub resets_at: Option<DateTime<Utc>>,
    pub duration_minutes: Option<i64>,
    #[serde(default)]
    pub limit_reached: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreditsReport {
    pub available: u32,
    #[serde(default)]
    pub credits: Vec<CreditReport>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreditReport {
    pub id: String,
    pub title: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub granted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsStored {
    pub accounts: usize,
}

impl AccountReport {
    fn usage(&self) -> Result<UsageReport, HostError> {
        if self.windows.len() > MAX_WINDOWS {
            return Err(invalid("windows", "Report at most 20 windows an account."));
        }
        let mut windows = Vec::with_capacity(self.windows.len());
        for window in &self.windows {
            if !crate::usage::valid_window_key(&window.key) {
                return Err(invalid(
                    "windows",
                    "A window's key is letters, digits and ._- (at most 64).",
                ));
            }
            if window.used_percent > 100 {
                return Err(invalid("windows", "usedPercent is 0 to 100."));
            }
            windows.push(UsageWindow {
                key: window.key.clone(),
                limit_name: line(window.label.as_deref(), 80),
                used_percent: window.used_percent,
                resets_at: window.resets_at.map(|at| at.timestamp()),
                duration_minutes: window.duration_minutes,
                limit_reached: window.limit_reached,
            });
        }
        Ok(UsageReport {
            windows,
            reset_credits: self.reset_credits.as_ref().map(|credits| ResetCredits {
                available: credits.available,
                credits: credits
                    .credits
                    .iter()
                    .map(|credit| ResetCredit {
                        id: credit.id.clone(),
                        title: credit.title.clone(),
                        expires_at: credit.expires_at.map(|at| at.timestamp()),
                        granted_at: credit.granted_at.map(|at| at.timestamp()),
                    })
                    .collect(),
            }),
            plan: line(self.plan.as_deref(), 40),
        })
    }
}

/// Who reports windows: a host, for the accounts it described, or the
/// built-in host (A1.17), for the account homes on this machine, named by
/// their ids.
#[derive(Clone, Copy, Debug)]
pub enum Reporter<'a> {
    Host(&'a HostRow),
    Builtin,
}

/// Stores a host's report: every account in it must be one the host
/// described, or nothing is stored. Each account's alert rules run next.
pub async fn report(
    state: &AppState,
    reporter: Reporter<'_>,
    report: WindowsReport,
) -> Result<WindowsStored, HostError> {
    if report.accounts.len() > MAX_ACCOUNTS {
        return Err(invalid("accounts", "Report at most 50 accounts."));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    for account in &report.accounts {
        let usage = account.usage()?;
        let account_id = match reporter {
            Reporter::Host(host) => {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM provider_accounts \
                     WHERE host_id = $1 AND host_account_ref = $2 AND hidden_at IS NULL",
                )
                .bind(host.id)
                .bind(&account.reference)
                .fetch_optional(&mut *transaction)
                .await?
            }
            Reporter::Builtin => match Uuid::parse_str(&account.reference) {
                Ok(id) => {
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM provider_accounts \
                         WHERE id = $1 AND host_id IS NULL AND home_path IS NOT NULL",
                    )
                    .bind(id)
                    .fetch_optional(&mut *transaction)
                    .await?
                }
                Err(_) => None,
            },
        };
        let Some(account_id) = account_id else {
            return Err(invalid(
                "accounts",
                "That account isn't one this host described: describe it first.",
            ));
        };
        let Some(stored) = crate::usage::load_account(&mut *transaction, account_id).await? else {
            return Err(invalid("accounts", "That account is gone."));
        };
        crate::usage::store_report(&mut transaction, &stored, &usage, !report.partial).await?;
        crate::usage::evaluate_soon(&mut transaction, &stored).await?;
    }
    transaction.commit().await?;
    Ok(WindowsStored {
        accounts: report.accounts.len(),
    })
}
