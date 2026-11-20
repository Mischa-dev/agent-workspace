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

/// One message sent by `account` in `conversation`, counted once, in the
/// conversation's workspace (the direct scope for a DM or group, which is in
/// no space). In the send's own transaction: a send that fails counts
/// nothing (and a separate one would wait on the conversation the send
/// holds).
pub async fn admit_message(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    space: Option<Uuid>,
    conversation: Uuid,
    account: Uuid,
    message: Uuid,
) -> Result<(), AppError> {
    let key = format!("message:{message}");
    admit(
        transaction,
        Ask {
            account_id: Some(account),
            space_id: space,
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
/// Every call counts, a retry or a call that then fails too: a request id
/// says nothing about what the call does, so reusing one must not skip the
/// budget. A call in a conversation the caller can read counts there too,
/// so a budget on that conversation holds it; one it can't read is left
/// out, so no budget there is spent or shown to it.
pub async fn admit_call(
    pool: &sqlx::PgPool,
    workspace: Uuid,
    space: Option<Uuid>,
    conversation: Option<Uuid>,
    account: Uuid,
    writes: bool,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let conversation = match conversation {
        Some(conversation) => {
            sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
                .bind(conversation)
                .bind(account)
                .fetch_one(&mut *transaction)
                .await?
                .then_some(conversation)
        }
        None => None,
    };
    // The space first: a write holding it makes this wait, as the call
    // itself would, but holding nothing yet (recording the use locks its
    // account, which a removal waiting on nothing but this would need).
    if let Some(space) = space {
        sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR KEY SHARE")
            .bind(space)
            .execute(&mut *transaction)
            .await?;
    }
    let call = Uuid::now_v7();
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
                conversation_id: conversation,
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
/// runs report they cost, or the tokens they report, is used up, on the
/// account, its space or the conversation the run is for.
pub async fn admit_run(
    transaction: &mut Transaction<'_, Postgres>,
    space: Uuid,
    conversation: Option<Uuid>,
    account: Uuid,
) -> Result<(), AppError> {
    let workspace = workspace_of(transaction, space).await?;
    let space = Some(space);
    // In the order `record_run_usage` counts them, so the two never wait on
    // each other's counters.
    for meter in ["reported_tokens", "reported_cost_micros"] {
        // One more unit: at the limit is used up.
        admit(
            transaction,
            Ask {
                account_id: Some(account),
                space_id: space,
                conversation_id: conversation,
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
        // Near or past a budget with this report: whoever set it is told
        // now, not when the next run is admitted. Asked as `admit_run`
        // asks, so the two take the counters in the same order. Already
        // spent, so a refusal only means it is past the limit.
        match admit(
            transaction,
            Ask {
                account_id: Some(account),
                space_id: space,
                conversation_id: conversation,
                ..Ask::new(workspace, meter, quantity)
            },
        )
        .await
        {
            Ok(()) | Err(AppError::LimitReached(_)) => {}
            Err(error) => return Err(error),
        }
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
///
/// In its own short transaction, committed at once: a claim admits several
/// pushes from different conversations, and holding one's counters while
/// asking for the next would wait in a circle with another claim that
/// took them in the other order. Counted once by `key`, so a claim that
/// then fails counts nothing again when it is retried.
pub async fn admit_delivery(
    pool: &sqlx::PgPool,
    event: &crate::events::LoggedEvent,
    account: Uuid,
    key: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let admitted = admit_delivery_in(&mut transaction, event, account, key).await?;
    transaction.commit().await?;
    Ok(admitted)
}

async fn admit_delivery_in(
    transaction: &mut Transaction<'_, Postgres>,
    event: &crate::events::LoggedEvent,
    account: Uuid,
    key: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    // Where the event happened, so a budget on its space or conversation
    // holds its pushes too.
    let ask = Ask {
        account_id: Some(account),
        space_id: event.space_id,
        conversation_id: event.conversation_id,
        record: Some(Recorded {
            source_kind: "delivery",
            source_id: None,
            dedupe_key: key,
        }),
        ..Ask::new(event.workspace_id, "deliveries", 1)
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
