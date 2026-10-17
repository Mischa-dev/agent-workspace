//! W19: notifications by email, the third channel in Settings ›
//! Notifications. It works once the workspace can send mail and the person
//! has confirmed an address.
//!
//! With the digest off, each notification someone wants by email is sent
//! by a job a couple of minutes later, so one they already read in the app
//! sends nothing. With a daily or weekly digest, nothing is sent one at a
//! time; an hourly job collects what they haven't read into one email.
//! Both read the notifications as the recipient when they send (principle
//! 9), so a lost membership or a removed message sends nothing.

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    jobs::{self, JobError, JobRegistry, NewJob},
    mail::{self, Message},
    notification_settings,
    notifications::{self, NotificationView},
};

pub const SEND_KIND: &str = "notifications.email";
pub const DIGEST_KIND: &str = "notifications.email_digest";
/// How long a notification waits before its email, so reading it in the
/// app first sends nothing.
const SEND_DELAY: StdDuration = StdDuration::from_secs(2 * 60);
const DIGEST_ITEMS: usize = 30;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        SEND_KIND,
        StdDuration::from_secs(120),
        |context| async move {
            let job: SendJob = context.job.payload_as()?;
            send_one(&context.state, &job).await
        },
    );
    registry.register_with_timeout(
        DIGEST_KIND,
        StdDuration::from_secs(15 * 60),
        |context| async move { send_digests(&context.state).await },
    );
    registry.every(DIGEST_KIND, StdDuration::from_secs(60 * 60));
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendJob {
    notification_id: Uuid,
    recipient: Uuid,
}

/// Whether `recipient` can get email in `organization_id` at all: the
/// workspace sends mail and they confirmed an address.
pub async fn available(
    connection: &mut sqlx::PgConnection,
    organization_id: Uuid,
    recipient: Uuid,
) -> Result<bool, AppError> {
    Ok(mail::configured(&mut *connection, organization_id).await?
        && mail::verified_address_for_actor(&mut *connection, recipient)
            .await?
            .is_some())
}

/// Queue the email for a notification occurrence just written, unless the
/// person collects email in a digest. Runs inside the transaction that
/// wrote it.
pub(crate) async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    notification_id: Uuid,
    recipient: Uuid,
    dedupe_key: &str,
) -> Result<(), AppError> {
    if !available(transaction, organization_id, recipient).await? {
        return Ok(());
    }
    let (_, digest) =
        notification_settings::quiet_hours(transaction, organization_id, recipient).await?;
    if digest != "off" {
        return Ok(());
    }
    jobs::enqueue(
        &mut **transaction,
        NewJob::new(SEND_KIND)
            .workspace(organization_id)
            .payload(SendJob {
                notification_id,
                recipient,
            })
            .unique(format!("{SEND_KIND}:{recipient}:{dedupe_key}"))
            .delay(SEND_DELAY)
            .max_attempts(5),
    )
    .await?;
    Ok(())
}

fn absolute(origin: &str, href: &str) -> String {
    if href.starts_with('/') {
        format!("{origin}{href}")
    } else {
        href.to_owned()
    }
}

fn describe(origin: &str, notification: &NotificationView) -> String {
    let mut text = format!("{}\n{}", notification.title, notification.project_name);
    if let Some(preview) = &notification.preview {
        text.push_str("\n\n");
        text.push_str(preview);
    }
    text.push_str("\n\nOpen: ");
    text.push_str(&absolute(origin, &notification.href));
    text
}

fn footer(origin: &str) -> String {
    format!(
        "\n\n--\nYou get this because you chose email for these in Agent Workspace. Change what you get by email: {origin}/settings/account/notifications\n"
    )
}

async fn organization_of(state: &AppState, project_id: Uuid) -> Result<Uuid, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&state.pool)
            .await?,
    )
}

async fn send_one(state: &AppState, job: &SendJob) -> Result<(), JobError> {
    // Read as the recipient: anything they can no longer see sends nothing.
    let Some(notification) =
        notifications::get_many(&state.pool, job.recipient, &[job.notification_id])
            .await?
            .into_iter()
            .next()
    else {
        return Ok(());
    };
    if notification.read {
        return Ok(());
    }
    let organization_id = organization_of(state, notification.project_id).await?;
    {
        let mut connection = state.pool.acquire().await.map_err(AppError::from)?;
        let decision = notification_settings::decide(
            &mut connection,
            job.recipient,
            notification.project_id,
            &notification.kind,
        )
        .await?;
        if !decision.email {
            return Ok(());
        }
    }
    let Some(address) = mail::verified_address_for_actor(&state.pool, job.recipient).await? else {
        return Ok(());
    };
    let Some(transport) = mail::transport(state, organization_id).await? else {
        return Ok(());
    };
    let origin = crate::events::public_origin(state);
    let subject = format!(
        "{} · {}",
        notification.title.chars().take(140).collect::<String>(),
        notification.project_name
    );
    mail::send(
        state,
        &transport,
        &Message {
            to: address,
            subject,
            text: format!("{}{}", describe(&origin, &notification), footer(&origin)),
        },
    )
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct DigestCandidate {
    organization_id: Uuid,
    actor_id: Uuid,
    last_sent_at: Option<DateTime<Utc>>,
}

/// Send each person whose digest is due one email with what they haven't
/// read since the last one.
async fn send_digests(state: &AppState) -> Result<(), JobError> {
    let candidates = sqlx::query_as::<_, DigestCandidate>(
        r#"
        SELECT membership.organization_id, membership.actor_id, digest.last_sent_at
        FROM organization_memberships membership
        JOIN workspace_mail_settings mail
          ON mail.organization_id = membership.organization_id AND mail.enabled
        JOIN users user_account ON user_account.actor_id = membership.actor_id
         AND user_account.status = 'active'
        JOIN user_email_addresses address ON address.user_id = user_account.id
         AND address.verified_at IS NOT NULL
        LEFT JOIN notification_email_digests digest
          ON digest.organization_id = membership.organization_id
         AND digest.actor_id = membership.actor_id
        WHERE membership.enabled
          AND (digest.last_sent_at IS NULL OR digest.last_sent_at < now() - interval '23 hours')
        ORDER BY membership.organization_id, membership.actor_id
        LIMIT 5000
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    for candidate in candidates {
        // One person's failure (a bounced address) mustn't stop the rest.
        if let Err(error) = send_digest(state, &candidate).await {
            tracing::warn!(
                category = error.log_category(),
                "email digest for one person failed"
            );
        }
    }
    Ok(())
}

async fn send_digest(state: &AppState, candidate: &DigestCandidate) -> Result<(), AppError> {
    let mut connection = state.pool.acquire().await?;
    let (quiet, digest) = notification_settings::quiet_hours(
        &mut connection,
        candidate.organization_id,
        candidate.actor_id,
    )
    .await?;
    let period = match digest.as_str() {
        "daily" => Duration::days(1),
        "weekly" => Duration::days(7),
        _ => return Ok(()),
    };
    let now = Utc::now();
    // An hour of slack so a daily digest doesn't creep later each day.
    if candidate
        .last_sent_at
        .is_some_and(|at| now - at < period - Duration::hours(1))
        || quiet.ends_after(now).is_some()
    {
        return Ok(());
    }
    let since = candidate.last_sent_at.unwrap_or(now - period);
    let ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT n.id FROM notifications n
        JOIN projects project ON project.id = n.project_id
        WHERE n.recipient_actor_id = $1 AND project.organization_id = $2
          AND n.read_at IS NULL AND n.surfaced_at > $3
        ORDER BY n.surfaced_at DESC
        LIMIT 200
        "#,
    )
    .bind(candidate.actor_id)
    .bind(candidate.organization_id)
    .bind(since)
    .fetch_all(&mut *connection)
    .await?;
    let mut items = Vec::new();
    for notification in notifications::get_many(&state.pool, candidate.actor_id, &ids).await? {
        let decision = notification_settings::decide(
            &mut connection,
            candidate.actor_id,
            notification.project_id,
            &notification.kind,
        )
        .await?;
        if decision.email {
            items.push(notification);
        }
    }
    if items.is_empty() {
        record_digest(&mut connection, candidate).await?;
        return Ok(());
    }
    let Some(address) =
        mail::verified_address_for_actor(&mut *connection, candidate.actor_id).await?
    else {
        return Ok(());
    };
    let Some(transport) = mail::transport(state, candidate.organization_id).await? else {
        return Ok(());
    };
    let workspace = sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
        .bind(candidate.organization_id)
        .fetch_one(&mut *connection)
        .await?;
    let origin = crate::events::public_origin(state);
    let total = items.len();
    let mut text = format!(
        "{total} thing{} you haven't read in {workspace}:\n",
        if total == 1 { "" } else { "s" }
    );
    for notification in items.iter().take(DIGEST_ITEMS) {
        text.push_str("\n• ");
        text.push_str(&describe(&origin, notification).replace('\n', "\n  "));
        text.push('\n');
    }
    if total > DIGEST_ITEMS {
        text.push_str(&format!(
            "\n…and {} more in your inbox: {origin}/inbox\n",
            total - DIGEST_ITEMS
        ));
    }
    text.push_str(&footer(&origin));
    let subject = format!(
        "{} digest: {total} unread in {workspace}",
        if digest == "daily" { "Daily" } else { "Weekly" }
    );
    // Record first: a digest that half-sent is better skipped than doubled.
    record_digest(&mut connection, candidate).await?;
    drop(connection);
    if let Err(error) = mail::send(
        state,
        &transport,
        &Message {
            to: address,
            subject,
            text,
        },
    )
    .await
    {
        // The reason can echo the address back; keep it out of the log.
        let _ = error;
        tracing::warn!("email digest wasn't sent");
    }
    Ok(())
}

async fn record_digest(
    connection: &mut sqlx::PgConnection,
    candidate: &DigestCandidate,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO notification_email_digests (organization_id, actor_id, last_sent_at)
        VALUES ($1, $2, now())
        ON CONFLICT (organization_id, actor_id) DO UPDATE SET last_sent_at = now()
        "#,
    )
    .bind(candidate.organization_id)
    .bind(candidate.actor_id)
    .execute(connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::absolute;

    #[test]
    fn links_in_email_are_absolute() {
        assert_eq!(
            absolute("https://w.example", "/inbox"),
            "https://w.example/inbox"
        );
        assert_eq!(
            absolute("https://w.example", "https://github.com/a/b/pull/1"),
            "https://github.com/a/b/pull/1"
        );
    }
}
