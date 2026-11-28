//! Claims and handoffs (K2.14, E F8): who is working on a task right now,
//! and the note they leave when they stop. One command for every edge: the
//! `task_claim` and `task_release` operations (agents, scripts, rules) and
//! the web's release button run the same code inside the caller's
//! transaction.

use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{enqueue_update, insert_activity};
use crate::error::AppError;

/// How long a claim lasts when the caller doesn't say.
pub const DEFAULT_CLAIM_MINUTES: i32 = 120;
/// The longest handoff note.
pub const MAX_HANDOFF_CHARS: usize = 4_000;

/// A live claim, as the tools and the web show it.
#[derive(Clone, Debug, FromRow)]
pub struct ClaimRow {
    pub work_item_id: Uuid,
    pub actor_id: Uuid,
    pub claimed_by: String,
    pub note: Option<String>,
    pub claimed_at: String,
    pub expires_at: String,
}

/// The last handoff note left on a task.
#[derive(Clone, Debug, FromRow)]
pub struct HandoffRow {
    pub by: String,
    pub via: Option<String>,
    pub note: String,
    pub at: String,
}

/// What a claim did.
#[derive(Clone, Debug)]
pub enum Claimed {
    /// The caller holds it now; `fresh` when it wasn't theirs before.
    Held { fresh: bool },
    /// The task is archived.
    Archived,
    /// Someone else holds it (`None`: they took it in the meantime).
    HeldByOther(Option<ClaimRow>),
}

/// The live claim on a task, if any.
pub async fn current(
    connection: &mut PgConnection,
    task_id: Uuid,
) -> Result<Option<ClaimRow>, AppError> {
    Ok(sqlx::query_as::<_, ClaimRow>(
        r#"
        SELECT claim.work_item_id, claim.actor_id,
               COALESCE(member.display_name, 'Someone') AS claimed_by,
               claim.note, claim.claimed_at::text AS claimed_at,
               claim.expires_at::text AS expires_at
        FROM work_item_claims claim
        LEFT JOIN project_memberships member
          ON member.project_id = claim.project_id AND member.actor_id = claim.actor_id
        WHERE claim.work_item_id = $1 AND claim.expires_at > now()
        "#,
    )
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The latest handoff notes on a task, newest first.
pub async fn handoffs(
    connection: &mut PgConnection,
    task_id: Uuid,
    limit: i64,
) -> Result<Vec<HandoffRow>, AppError> {
    Ok(sqlx::query_as::<_, HandoffRow>(
        r#"
        SELECT COALESCE(member.display_name, 'Former member') AS by,
               via_client.client_name AS via, handoff.note, handoff.created_at::text AS at
        FROM work_item_handoffs handoff
        LEFT JOIN project_memberships member
          ON member.project_id = handoff.project_id AND member.actor_id = handoff.actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = handoff.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE handoff.work_item_id = $1
        ORDER BY handoff.created_at DESC, handoff.id DESC
        LIMIT $2
        "#,
    )
    .bind(task_id)
    .bind(limit)
    .fetch_all(&mut *connection)
    .await?)
}

/// The task's version, for the update event.
async fn version(
    transaction: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> Result<i64, AppError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
            .bind(task_id)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

/// Claims a task for `actor_id` for `minutes`, or renews their claim. The
/// caller checked they may work on tasks in `project_id`.
pub async fn claim(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    note: Option<&str>,
    minutes: i32,
) -> Result<Claimed, AppError> {
    // The claim takes its turn on the project row before the task, as
    // every task write does (lock_membership) before it locks tasks and
    // the board column a status change or reorder lands in. Locked the
    // other way round, such a write waited for this task while this
    // claim's insert waited on the project row for its foreign key. NO
    // KEY UPDATE waits for those writes but never for a foreign key's
    // share lock.
    sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR NO KEY UPDATE")
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?;
    // Claimers take turns on the task row. With a shared lock here, two
    // agents claiming at once deadlocked: the winner's activity write
    // waited on the loser's lock while the loser waited on the claim.
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE id = $1 AND project_id = $2 FOR NO KEY UPDATE",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    match archived {
        None => return Err(AppError::NotFound("work_item")),
        Some(true) => return Ok(Claimed::Archived),
        Some(false) => {}
    }
    // The second claimer runs after the first commits and finds a live
    // claim it doesn't own, so the update below changes nothing.
    let claimed = sqlx::query_scalar::<_, bool>(
        r#"
        INSERT INTO work_item_claims (work_item_id, project_id, actor_id, note, expires_at)
        VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
        ON CONFLICT (work_item_id) DO UPDATE
        SET actor_id = EXCLUDED.actor_id,
            note = COALESCE(EXCLUDED.note, work_item_claims.note),
            claimed_at = CASE WHEN work_item_claims.actor_id = EXCLUDED.actor_id
                              THEN work_item_claims.claimed_at ELSE now() END,
            expires_at = EXCLUDED.expires_at
        WHERE work_item_claims.actor_id = EXCLUDED.actor_id
           OR work_item_claims.expires_at <= now()
        RETURNING (xmax = 0) AS inserted
        "#,
    )
    .bind(task_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(note)
    .bind(minutes)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(fresh) = claimed else {
        return Ok(Claimed::HeldByOther(current(transaction, task_id).await?));
    };
    if fresh {
        insert_activity(
            transaction,
            project_id,
            task_id,
            actor_id,
            "work_item_updated",
            "Started working on this",
            &serde_json::json!({ "claim": true }),
        )
        .await?;
        let version = version(transaction, task_id).await?;
        enqueue_update(transaction, project_id, task_id, version).await?;
    }
    Ok(Claimed::Held { fresh })
}

/// Of `candidates`, the tasks `actor_id` may take from the queue now: not
/// archived, held by nobody, assigned or delegated to nobody else, and
/// carrying every label in `labels` (by name, any case).
pub async fn takeable(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    actor_id: Uuid,
    candidates: &[Uuid],
    labels: &[String],
) -> Result<Vec<Uuid>, AppError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT item.id
        FROM task_rows item
        WHERE item.project_id = $1
          AND item.id = ANY($2)
          AND item.archived_at IS NULL
          AND (item.assignee_actor_id IS NULL OR item.assignee_actor_id = $3
               OR item.delegate_actor_id = $3)
          AND (item.delegate_actor_id IS NULL OR item.delegate_actor_id = $3)
          AND NOT EXISTS (
              SELECT 1 FROM work_item_claims claim
              WHERE claim.work_item_id = item.id AND claim.expires_at > now())
          AND NOT EXISTS (
              SELECT 1 FROM unnest($4::text[]) AS wanted(name)
              WHERE NOT EXISTS (
                  SELECT 1
                  FROM work_item_labels tagged
                  JOIN project_labels label
                    ON label.project_id = tagged.project_id AND label.id = tagged.label_id
                  WHERE tagged.project_id = item.project_id
                    AND tagged.work_item_id = item.id
                    AND lower(label.name) = lower(wanted.name)))
        "#,
    )
    .bind(project_id)
    .bind(candidates)
    .bind(actor_id)
    .bind(labels)
    .fetch_all(crate::db::scoped(pool))
    .await?)
}

/// Which of `candidates` were started and given back: the last holder left
/// a handoff, or their claim ran out, and nobody holds the task now.
pub async fn given_back(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    candidates: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT item.id
        FROM task_rows item
        WHERE item.project_id = $1
          AND item.id = ANY($2)
          AND NOT EXISTS (
              SELECT 1 FROM work_item_claims claim
              WHERE claim.work_item_id = item.id AND claim.expires_at > now())
          AND (EXISTS (SELECT 1 FROM work_item_handoffs handoff
                       WHERE handoff.work_item_id = item.id)
               OR EXISTS (SELECT 1 FROM work_item_claims claim
                          WHERE claim.work_item_id = item.id))
        "#,
    )
    .bind(project_id)
    .bind(candidates)
    .fetch_all(crate::db::scoped(pool))
    .await?)
}

/// Who may end a claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Releaser {
    /// Only its holder: anyone else's release changes nothing.
    Holder,
    /// Whoever holds it (the caller may manage the space's tasks).
    Anyone,
}

/// What a release did.
#[derive(Clone, Debug)]
pub enum Released {
    /// The claim ended; `holder` held it.
    Ended { holder: Uuid },
    /// There was no claim the caller could end.
    Nothing,
    /// A handoff was given, but the caller doesn't hold the claim.
    NotHolder,
}

/// Ends a claim, leaving `handoff` for the next claimer when the caller
/// held it. `via_grant` is the connected app the caller acts through.
pub async fn release(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    via_grant: Option<Uuid>,
    handoff: Option<&str>,
    releaser: Releaser,
) -> Result<Released, AppError> {
    let holder = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM work_item_claims WHERE project_id = $1 AND work_item_id = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let may_end = holder.is_some_and(|holder| holder == actor_id || releaser == Releaser::Anyone);
    // Only whoever held the task hands it off: a note from anyone else
    // would read to the next claimer as the last holder's word, and a
    // retried release would save it twice.
    if handoff.is_some() && holder != Some(actor_id) {
        return Ok(Released::NotHolder);
    }
    let Some(holder) = holder.filter(|_| may_end) else {
        return Ok(Released::Nothing);
    };
    sqlx::query("DELETE FROM work_item_claims WHERE project_id = $1 AND work_item_id = $2")
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **transaction)
        .await?;
    match handoff {
        Some(note) => {
            sqlx::query(
                r#"
                INSERT INTO work_item_handoffs (
                    id, project_id, work_item_id, actor_id, via_oauth_grant_id, note
                ) VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(project_id)
            .bind(task_id)
            .bind(actor_id)
            .bind(via_grant)
            .bind(note)
            .execute(&mut **transaction)
            .await?;
            insert_activity(
                transaction,
                project_id,
                task_id,
                actor_id,
                "work_item_updated",
                &format!("Handed off: {}", snippet(note, 200)),
                &serde_json::json!({ "handoff": true }),
            )
            .await?;
        }
        None if holder != actor_id => {
            insert_activity(
                transaction,
                project_id,
                task_id,
                actor_id,
                "work_item_updated",
                "Released the claim",
                &serde_json::json!({ "claim": false, "claimedBy": holder }),
            )
            .await?;
        }
        None => {}
    }
    let version = version(transaction, task_id).await?;
    enqueue_update(transaction, project_id, task_id, version).await?;
    Ok(Released::Ended { holder })
}

/// `text` on one line, at most `limit` characters, with an ellipsis when
/// cut.
fn snippet(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut cut = collapsed
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    cut.push('…');
    cut
}
