//! Budgets (A2.14): the limits a workspace puts on what accounts use, kept
//! as H1's `spend_limits` rows and checked by [`crate::metering::admit`]
//! like every other limit. A person and an agent with the same budget are
//! refused alike.
//!
//! What counts: calls through the API and agent tools (`operations`, and
//! `writes` for changes), messages sent, pushes to agents (`deliveries`:
//! over a hard limit they wait for the next period instead of failing), and
//! what runs report they cost (`reported_cost_micros`, `reported_tokens`:
//! over a hard limit, new runs are refused). Near a budget, whoever set it
//! is told once a period.

pub mod ops;

use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    metering::{self, Admission, Ask, LimitNotice, Recorded},
};

/// Asks [`metering::admit`], telling the budget's owner when the ask comes
/// near it. A refusal is [`AppError::LimitReached`].
pub async fn admit(
    transaction: &mut Transaction<'_, Postgres>,
    ask: Ask<'_>,
) -> Result<(), AppError> {
    let workspace = ask.workspace_id;
    if let Admission::AllowedNearLimit { notice } = metering::admit(transaction, ask).await? {
        warn(transaction, workspace, &notice).await?;
    }
    Ok(())
}

/// One message sent by `account` in `conversation`, counted once.
/// In the send's own transaction: a send that fails counts nothing (and a
/// separate one would wait on the conversation the send holds).
pub async fn admit_message(
    transaction: &mut Transaction<'_, Postgres>,
    space: Uuid,
    conversation: Uuid,
    account: Uuid,
    message: Uuid,
) -> Result<(), AppError> {
    let workspace = workspace_of(transaction, space).await?;
    let key = format!("message:{message}");
    admit(
        transaction,
        Ask {
            account_id: Some(account),
            space_id: Some(space),
            conversation_id: Some(conversation),
            record: Some(Recorded {
                source_kind: "message",
                source_id: Some(message),
                dedupe_key: &key,
            }),
            ..Ask::new(workspace, metering::MESSAGES_METER, 1)
        },
    )
    .await
}

/// One call through the API or an agent tool, and one write when it
/// changes something: counted when asked, in its own short transaction
/// before the call's begins, so the counter rows are never held while the
/// call runs (a call it sets off, as the same account, would wait on them).
/// A call that then fails still counts; a retry with the same request id
/// counts once.
pub async fn admit_call(
    pool: &sqlx::PgPool,
    workspace: Uuid,
    space: Option<Uuid>,
    account: Uuid,
    writes: bool,
    request_id: Option<&str>,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let call = request_id.map_or_else(
        || Uuid::now_v7().to_string(),
        |key| format!("{account}:{}", key.chars().take(120).collect::<String>()),
    );
    let meters: &[&'static str] = if writes {
        &["operations", "writes"]
    } else {
        &["operations"]
    };
    for meter in meters {
        let key = format!("{meter}:{call}");
        admit(
            &mut transaction,
            Ask {
                account_id: Some(account),
                space_id: space,
                record: Some(Recorded {
                    source_kind: "operation",
                    source_id: None,
                    dedupe_key: &key,
                }),
                ..Ask::new(workspace, meter, 1)
            },
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Whether `account` may start a run: refused once a hard budget on what
/// runs report they cost, or the tokens they report, is used up.
pub async fn admit_run(
    transaction: &mut Transaction<'_, Postgres>,
    space: Uuid,
    account: Uuid,
) -> Result<(), AppError> {
    let workspace = workspace_of(transaction, space).await?;
    let space = Some(space);
    for meter in ["reported_cost_micros", "reported_tokens"] {
        // One more unit: at the limit is used up.
        admit(
            transaction,
            Ask {
                account_id: Some(account),
                space_id: space,
                ..Ask::new(workspace, meter, 1)
            },
        )
        .await?;
    }
    Ok(())
}

/// What a run reported it used: counted against its account's budgets.
/// Already spent, so never refused.
pub struct RunUsage {
    pub workspace: Uuid,
    pub space: Option<Uuid>,
    pub conversation: Option<Uuid>,
    pub account: Uuid,
    /// The usage report it came in, which keeps a retry from counting twice.
    pub report: Uuid,
    pub tokens: i64,
    pub cost: Option<f64>,
}

pub async fn record_run_usage(
    transaction: &mut Transaction<'_, Postgres>,
    usage: RunUsage,
) -> Result<(), AppError> {
    let RunUsage {
        workspace,
        space,
        conversation,
        account,
        report,
        tokens,
        cost,
    } = usage;
    let micros = cost
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .map(|cost| (cost * 1_000_000.0).round() as i64);
    let usages = [
        ("reported_tokens", Some(tokens).filter(|tokens| *tokens > 0)),
        ("reported_cost_micros", micros),
    ];
    for (meter, quantity) in usages {
        let Some(quantity) = quantity else { continue };
        let key = format!("{meter}:{report}");
        metering::record(
            transaction,
            metering::Usage {
                workspace_id: workspace,
                meter,
                quantity,
                account_id: Some(account),
                space_id: space,
                conversation_id: conversation,
                recorded: Recorded {
                    source_kind: "usage_report",
                    source_id: Some(report),
                    dedupe_key: &key,
                },
            },
        )
        .await?;
    }
    Ok(())
}

/// One push to `account`'s connection: `Ok(None)` when it may go, or when
/// a hard budget on deliveries holds it, the time it may go again.
pub async fn admit_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    account: Uuid,
    key: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    let ask = Ask {
        account_id: Some(account),
        record: Some(Recorded {
            source_kind: "delivery",
            source_id: None,
            dedupe_key: key,
        }),
        ..Ask::new(workspace, "deliveries", 1)
    };
    match admit(transaction, ask).await {
        Ok(()) => Ok(None),
        Err(AppError::LimitReached(refusal)) => {
            Ok(Some(refusal.resets_at.unwrap_or_else(|| {
                chrono::Utc::now() + chrono::Duration::hours(1)
            })))
        }
        Err(error) => Err(error),
    }
}

async fn workspace_of(
    transaction: &mut Transaction<'_, Postgres>,
    space: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(space)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("space"))
}

/// Tells whoever set a budget that it is nearly used, once a period (the
/// metering notice is given once). Numbers only, never anyone's content.
async fn warn(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    notice: &LimitNotice,
) -> Result<(), AppError> {
    let Some((budget, owner)) = notice.budget else {
        return Ok(());
    };
    let space = crate::spaces::workspace::ensure(transaction, workspace).await?;
    let day = chrono::Utc::now().format("%Y-%m-%d");
    let notification = crate::notifications::NewNotification {
        project_id: space,
        recipient_actor_id: owner,
        kind: crate::notifications::kind::BUDGET_WARNING,
        subject_kind: "budget",
        subject_id: budget,
        actor_id: None,
        conversation_id: None,
        message_id: None,
        work_item_id: None,
        agent_run_id: None,
        pull_request_id: None,
        detail: json!({
            "title": "A budget you set is nearly used",
            "meter": notice.meter,
            "limit": notice.limit,
            "used": notice.used,
            "budgetId": budget,
        }),
        group_key: format!("budget:{budget}"),
        dedupe_key: format!("budget:{budget}:{day}"),
    };
    if crate::notifications::deliver(transaction, &notification).await? {
        crate::notifications::announce(transaction, space, owner).await?;
    }
    Ok(())
}
